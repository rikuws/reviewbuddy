use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use sha1::{Digest, Sha1};

use crate::{
    agents::{self, jsonrepair::parse_tolerant, prompt::build_stack_planning_prompt},
    code_tour::CodeTourProvider,
    github::PullRequestDetail,
};

use super::super::{
    atoms::extract_change_atoms,
    model::{
        stack_now_ms, ChangeAtom, ChangeAtomId, ChangeAtomSource, ChangeRole, Confidence,
        LayerMetrics, LayerReviewStatus, RepoContext, ReviewStack, ReviewStackLayer,
        StackDiscoveryError, StackKind, StackProviderMetadata, StackSource, StackWarning,
        VirtualLayerRef, VirtualStackSizing, STACK_GENERATOR_VERSION,
    },
    validation::{
        requires_manual_review, validate_ai_stack_plan, AiReviewPriority, AiStackPlan,
        AiStackPlanLayer, AiStackPlanStrategy, ValidatedAiStackPlan,
    },
};

use super::virtual_commits::{self, CommitContext, CommitSuitability, CommitSummary};

const MAX_AI_STACK_ATOMS: usize = 180;
const AI_MIN_CHANGED_LINES: usize = 800;
const AI_MIN_ATOMS: usize = 10;

pub struct AiVirtualStackProvider;

pub fn discover(
    selected_pr: &PullRequestDetail,
    repo_context: &RepoContext,
    sizing: &VirtualStackSizing,
    provider: CodeTourProvider,
) -> Result<Option<ReviewStack>, StackDiscoveryError> {
    let atoms = extract_change_atoms(selected_pr);
    if atoms.is_empty() {
        return Ok(None);
    }

    let commit_context = virtual_commits::commit_context_for_pr(selected_pr, repo_context, &atoms)?
        .unwrap_or_else(|| missing_commit_context(selected_pr.commits_count));
    let total_changed_lines = atoms
        .iter()
        .map(|atom| atom.additions + atom.deletions)
        .sum::<usize>()
        .max((selected_pr.additions + selected_pr.deletions).max(0) as usize);

    if !should_attempt_ai_planning(&atoms, total_changed_lines, &commit_context.suitability) {
        return Ok(None);
    }

    if atoms.len() > MAX_AI_STACK_ATOMS {
        return deterministic_fallback_stack(
            selected_pr,
            repo_context,
            sizing,
            "AI stack planning was unavailable because the atom list exceeded the prompt budget; Remiss used deterministic semantic grouping.",
            Some(json!({
                "atomCount": atoms.len(),
                "maxAiStackAtoms": MAX_AI_STACK_ATOMS,
                "commitSuitability": commit_context.suitability,
            })),
        )
        .map(Some);
    }

    let Some(working_directory) = repo_context.local_repo_path.as_ref() else {
        return deterministic_fallback_stack(
            selected_pr,
            repo_context,
            sizing,
            "AI stack planning was unavailable because no local checkout was ready; Remiss used deterministic semantic grouping.",
            Some(json!({ "commitSuitability": commit_context.suitability })),
        )
        .map(Some);
    };

    let backend = agents::backend_for(provider);
    match backend.status() {
        Ok(status) if status.available && status.authenticated => {}
        Ok(status) => {
            return deterministic_fallback_stack(
                selected_pr,
                repo_context,
                sizing,
                "AI stack planning was unavailable; Remiss used deterministic semantic grouping.",
                Some(json!({
                    "provider": provider.slug(),
                    "status": status,
                    "commitSuitability": commit_context.suitability,
                })),
            )
            .map(Some);
        }
        Err(error) => {
            return deterministic_fallback_stack(
                selected_pr,
                repo_context,
                sizing,
                "AI stack planning was unavailable; Remiss used deterministic semantic grouping.",
                Some(json!({
                    "provider": provider.slug(),
                    "error": error,
                    "commitSuitability": commit_context.suitability,
                })),
            )
            .map(Some);
        }
    }

    let input_json = build_stack_planning_input(selected_pr, &atoms, &commit_context);
    let prompt = build_stack_planning_prompt(&input_json);
    let response = match agents::run_json_prompt(
        provider,
        working_directory.to_string_lossy().as_ref(),
        prompt,
    ) {
        Ok(response) => response,
        Err(error) => {
            return deterministic_fallback_stack(
                selected_pr,
                repo_context,
                sizing,
                "AI stack planning was unavailable; Remiss used deterministic semantic grouping.",
                Some(json!({
                    "provider": provider.slug(),
                    "error": error,
                    "commitSuitability": commit_context.suitability,
                })),
            )
            .map(Some);
        }
    };

    let plan = match parse_tolerant::<AiStackPlan>(&response.text) {
        Ok(plan) => plan,
        Err(error) => {
            return deterministic_fallback_stack(
                selected_pr,
                repo_context,
                sizing,
                "AI stack planning returned invalid output; Remiss used deterministic semantic grouping.",
                Some(json!({
                    "provider": provider.slug(),
                    "modelOrAgent": response.model,
                    "error": error.message,
                    "commitSuitability": commit_context.suitability,
                })),
            )
            .map(Some);
        }
    };

    let validated = match validate_ai_stack_plan(&plan, &atoms, total_changed_lines) {
        Ok(validated) => validated,
        Err(error) => {
            return deterministic_fallback_stack(
                selected_pr,
                repo_context,
                sizing,
                "AI stack planning returned invalid output; Remiss used deterministic semantic grouping.",
                Some(json!({
                    "provider": provider.slug(),
                    "modelOrAgent": response.model,
                    "validationError": error.message,
                    "commitSuitability": commit_context.suitability,
                })),
            )
            .map(Some);
        }
    };

    Ok(Some(build_stack_from_validated_plan(
        selected_pr,
        atoms,
        validated,
        response.model,
        provider,
        commit_context,
    )))
}

pub fn should_attempt_ai_planning(
    atoms: &[ChangeAtom],
    total_changed_lines: usize,
    commit_suitability: &CommitSuitability,
) -> bool {
    if atoms.is_empty() {
        return false;
    }

    if commit_suitability.suitable_for_layers && total_changed_lines < 2_000 {
        return false;
    }

    total_changed_lines >= AI_MIN_CHANGED_LINES || atoms.len() >= AI_MIN_ATOMS
}

pub fn build_stack_from_validated_plan(
    selected_pr: &PullRequestDetail,
    atoms: Vec<ChangeAtom>,
    validated: ValidatedAiStackPlan,
    model_or_agent: Option<String>,
    provider: CodeTourProvider,
    commit_context: CommitContext,
) -> ReviewStack {
    let plan = validated.plan;
    let stack_id = virtual_stack_id(selected_pr);
    let atoms_by_id = atoms
        .iter()
        .map(|atom| (atom.id.clone(), atom))
        .collect::<BTreeMap<_, _>>();
    let mut layers = Vec::<ReviewStackLayer>::new();

    for (index, plan_layer) in plan.layers.iter().enumerate() {
        let layer_atoms = atom_refs_for_ids(&plan_layer.atom_ids, &atoms_by_id);
        let role = dominant_role(&layer_atoms);
        let metrics = metrics_for_atoms(&layer_atoms);
        let layer_id = virtual_layer_id(&stack_id, index, role, &plan_layer.atom_ids);
        let warnings = layer_atoms
            .iter()
            .flat_map(|atom| atom.warnings.iter().cloned())
            .collect::<Vec<_>>();

        layers.push(ReviewStackLayer {
            id: layer_id,
            index,
            title: clean_layer_text(&plan_layer.title, "AI review layer", 90),
            summary: clean_layer_text(&plan_layer.summary, "AI grouped review layer.", 220),
            rationale: clean_layer_text(
                &plan_layer.rationale,
                "AI grouped these changes by semantic review order.",
                500,
            ),
            pr: None,
            virtual_layer: Some(VirtualLayerRef {
                source: StackSource::VirtualAi,
                role,
                source_label: review_priority_label(&plan_layer.review_priority).to_string(),
            }),
            base_oid: selected_pr.base_ref_oid.clone(),
            head_oid: selected_pr.head_ref_oid.clone(),
            atom_ids: plan_layer.atom_ids.clone(),
            depends_on_layer_ids: Vec::new(),
            metrics,
            status: LayerReviewStatus::NotReviewed,
            confidence: plan_layer.confidence,
            warnings,
        });
    }

    let layer_ids = layers
        .iter()
        .map(|layer| layer.id.clone())
        .collect::<Vec<_>>();
    for (index, plan_layer) in plan.layers.iter().enumerate() {
        layers[index].depends_on_layer_ids = plan_layer
            .depends_on_layer_indexes
            .iter()
            .filter_map(|dep_index| layer_ids.get(*dep_index).cloned())
            .collect();
    }

    if !plan.manual_review_atom_ids.is_empty() {
        let index = layers.len();
        let manual_atoms = atom_refs_for_ids(&plan.manual_review_atom_ids, &atoms_by_id);
        let metrics = metrics_for_atoms(&manual_atoms);
        let role = dominant_role(&manual_atoms);
        let layer_id = virtual_layer_id(&stack_id, index, role, &plan.manual_review_atom_ids);
        let warnings = manual_atoms
            .iter()
            .flat_map(|atom| atom.warnings.iter().cloned())
            .chain(std::iter::once(StackWarning::new(
                "manual-review",
                "AI marked these atoms for manual review.",
            )))
            .collect::<Vec<_>>();
        layers.push(ReviewStackLayer {
            id: layer_id,
            index,
            title: "Manual review / uncertain changes".to_string(),
            summary: format!(
                "{} atom{} need a whole-file or manual pass.",
                plan.manual_review_atom_ids.len(),
                if plan.manual_review_atom_ids.len() == 1 {
                    ""
                } else {
                    "s"
                }
            ),
            rationale: "AI marked these atoms as generated, binary, huge, ambiguous, or low-confidence. Remiss keeps them visible as an explicit final layer."
                .to_string(),
            pr: None,
            virtual_layer: Some(VirtualLayerRef {
                source: StackSource::VirtualAi,
                role,
                source_label: "manual_review".to_string(),
            }),
            base_oid: selected_pr.base_ref_oid.clone(),
            head_oid: selected_pr.head_ref_oid.clone(),
            atom_ids: plan.manual_review_atom_ids.clone(),
            depends_on_layer_ids: layers
                .last()
                .map(|layer| vec![layer.id.clone()])
                .unwrap_or_default(),
            metrics,
            status: LayerReviewStatus::NotReviewed,
            confidence: Confidence::Low,
            warnings,
        });
    }

    let mut warnings = plan
        .warnings
        .iter()
        .map(|warning| StackWarning::new("ai-stack-warning", warning.clone()))
        .collect::<Vec<_>>();
    warnings.extend(
        atoms
            .iter()
            .filter(|atom| requires_manual_review(atom))
            .flat_map(|atom| atom.warnings.iter().cloned()),
    );

    ReviewStack {
        id: stack_id,
        repository: selected_pr.repository.clone(),
        selected_pr_number: selected_pr.number,
        source: StackSource::VirtualAi,
        kind: StackKind::Virtual,
        confidence: plan.confidence,
        trunk_branch: Some(selected_pr.base_ref_name.clone()),
        base_oid: selected_pr.base_ref_oid.clone(),
        head_oid: selected_pr.head_ref_oid.clone(),
        layers,
        atoms,
        warnings,
        provider: Some(StackProviderMetadata {
            provider: "ai_virtual_stack".to_string(),
            raw_payload: Some(json!({
                "modelOrAgent": model_or_agent.unwrap_or_else(|| provider.label().to_string()),
                "strategy": plan.strategy,
                "rationale": plan.rationale,
                "commitSuitability": commit_context.suitability,
                "commits": commit_context.commits,
                "deterministicFallback": "virtual_semantic",
            })),
        }),
        generated_at_ms: stack_now_ms(),
        generator_version: STACK_GENERATOR_VERSION.to_string(),
    }
}

fn deterministic_fallback_stack(
    selected_pr: &PullRequestDetail,
    repo_context: &RepoContext,
    sizing: &VirtualStackSizing,
    warning: &str,
    raw_payload: Option<Value>,
) -> Result<ReviewStack, StackDiscoveryError> {
    let mut stack = super::virtual_semantic::discover(selected_pr, repo_context, sizing)?
        .ok_or_else(|| StackDiscoveryError::new("Deterministic semantic fallback failed."))?;
    stack.confidence = stack.confidence.min(Confidence::Low);
    stack.warnings.push(StackWarning::new(
        "ai-virtual-stack-fallback",
        warning.to_string(),
    ));
    stack.provider = Some(StackProviderMetadata {
        provider: "virtual_semantic".to_string(),
        raw_payload: Some(json!({
            "aiVirtualStack": raw_payload.unwrap_or(Value::Null),
            "deterministicFallback": "virtual_semantic",
        })),
    });
    Ok(stack)
}

fn build_stack_planning_input(
    selected_pr: &PullRequestDetail,
    atoms: &[ChangeAtom],
    commit_context: &CommitContext,
) -> Value {
    let commits_by_path = commits_by_path(&commit_context.commits);
    let total_changed_lines = atoms
        .iter()
        .map(|atom| atom.additions + atom.deletions)
        .sum::<usize>();

    json!({
        "repository": &selected_pr.repository,
        "pr_number": selected_pr.number,
        "title": &selected_pr.title,
        "body_summary": crate::agents::prompt::trim_text(&selected_pr.body, 1_200),
        "total_files": selected_pr.changed_files,
        "total_changed_lines": total_changed_lines,
        "commit_suitability": &commit_context.suitability,
        "commits": commit_context
            .commits
            .iter()
            .map(commit_summary_json)
            .collect::<Vec<_>>(),
        "atoms": atoms
            .iter()
            .map(|atom| atom_summary_json(atom, &commits_by_path))
            .collect::<Vec<_>>(),
    })
}

fn commit_summary_json(commit: &CommitSummary) -> Value {
    json!({
        "oid": &commit.oid,
        "message": &commit.subject,
        "changed_lines": commit.changed_lines,
        "roles": commit.roles.iter().map(ChangeRole::label).collect::<Vec<_>>(),
    })
}

fn atom_summary_json(
    atom: &ChangeAtom,
    commits_by_path: &BTreeMap<String, Vec<&CommitSummary>>,
) -> Value {
    let commits = commits_by_path.get(&atom.path).cloned().unwrap_or_default();
    let commit_oids = commits
        .iter()
        .map(|commit| commit.oid.clone())
        .collect::<Vec<_>>();
    let commit_messages = commits
        .iter()
        .map(|commit| commit.subject.clone())
        .collect::<Vec<_>>();

    json!({
        "id": &atom.id,
        "path": &atom.path,
        "previous_path": &atom.previous_path,
        "source_kind": atom.source.stable_kind(),
        "role": atom.role.label(),
        "semantic_kind": &atom.semantic_kind,
        "title": atom_title(atom),
        "summary": atom_summary(atom),
        "symbol_name": &atom.symbol_name,
        "defined_symbols": &atom.defined_symbols,
        "referenced_symbols": atom.referenced_symbols.iter().take(24).collect::<Vec<_>>(),
        "hunk_headers": atom.hunk_headers.iter().take(4).collect::<Vec<_>>(),
        "old_range": &atom.old_range,
        "new_range": &atom.new_range,
        "additions": atom.additions,
        "deletions": atom.deletions,
        "changed_line_count": atom.additions + atom.deletions,
        "commit_oids": commit_oids,
        "commit_messages": commit_messages,
        "review_thread_count": atom.review_thread_ids.len(),
        "risk_score": atom.risk_score,
        "is_generated": atom.role == ChangeRole::Generated
            || matches!(atom.source, ChangeAtomSource::GeneratedPlaceholder),
        "is_binary": matches!(atom.source, ChangeAtomSource::BinaryPlaceholder),
        "confidence": if requires_manual_review(atom) { "low" } else { "medium" },
    })
}

fn commits_by_path(commits: &[CommitSummary]) -> BTreeMap<String, Vec<&CommitSummary>> {
    let mut by_path = BTreeMap::<String, Vec<&CommitSummary>>::new();
    for commit in commits {
        for path in &commit.paths {
            by_path.entry(path.clone()).or_default().push(commit);
        }
    }
    by_path
}

fn atom_title(atom: &ChangeAtom) -> String {
    atom.symbol_name
        .clone()
        .or_else(|| atom.hunk_headers.first().cloned())
        .unwrap_or_else(|| atom.path.clone())
}

fn atom_summary(atom: &ChangeAtom) -> String {
    let source = match &atom.source {
        ChangeAtomSource::File => "file-level change",
        ChangeAtomSource::Hunk { .. } => "diff hunk",
        ChangeAtomSource::SemanticSection { .. } => "semantic section",
        ChangeAtomSource::Commit { .. } => "commit atom",
        ChangeAtomSource::GeneratedPlaceholder => "generated or huge file placeholder",
        ChangeAtomSource::BinaryPlaceholder => "binary file placeholder",
    };
    format!(
        "{} in {} with {} changed line{}.",
        source,
        atom.path,
        atom.additions + atom.deletions,
        if atom.additions + atom.deletions == 1 {
            ""
        } else {
            "s"
        }
    )
}

fn missing_commit_context(commits_count: i64) -> CommitContext {
    CommitContext {
        commits: Vec::new(),
        suitability: CommitSuitability {
            score: 0.0,
            suitable_for_layers: false,
            reasons: vec![format!(
                "Commit metadata was unavailable; GitHub reported {commits_count} commit{}.",
                if commits_count == 1 { "" } else { "s" }
            )],
        },
    }
}

fn atom_refs_for_ids<'a>(
    atom_ids: &[ChangeAtomId],
    atoms_by_id: &'a BTreeMap<ChangeAtomId, &'a ChangeAtom>,
) -> Vec<&'a ChangeAtom> {
    atom_ids
        .iter()
        .filter_map(|atom_id| atoms_by_id.get(atom_id).copied())
        .collect()
}

fn metrics_for_atoms(atoms: &[&ChangeAtom]) -> LayerMetrics {
    let file_count = atoms
        .iter()
        .map(|atom| atom.path.as_str())
        .collect::<BTreeSet<_>>()
        .len();

    LayerMetrics {
        file_count,
        atom_count: atoms.len(),
        additions: atoms.iter().map(|atom| atom.additions).sum(),
        deletions: atoms.iter().map(|atom| atom.deletions).sum(),
        changed_lines: atoms
            .iter()
            .map(|atom| atom.additions + atom.deletions)
            .sum(),
        unresolved_thread_count: atoms.iter().map(|atom| atom.review_thread_ids.len()).sum(),
        risk_score: atoms.iter().map(|atom| atom.risk_score).sum(),
    }
}

fn dominant_role(atoms: &[&ChangeAtom]) -> ChangeRole {
    let mut counts = BTreeMap::<ChangeRole, usize>::new();
    for atom in atoms {
        *counts.entry(atom.role).or_default() += atom.additions + atom.deletions + 1;
    }
    counts
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(role, _)| role)
        .unwrap_or(ChangeRole::Unknown)
}

fn review_priority_label(priority: &AiReviewPriority) -> &'static str {
    match priority {
        AiReviewPriority::StartHere => "start_here",
        AiReviewPriority::Normal => "normal",
        AiReviewPriority::QuickPass => "quick_pass",
        AiReviewPriority::ManualReview => "manual_review",
    }
}

fn clean_layer_text(value: &str, fallback: &str, limit: usize) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return fallback.to_string();
    }
    if trimmed.chars().count() <= limit {
        return trimmed.to_string();
    }
    let truncated = trimmed
        .chars()
        .take(limit.saturating_sub(1))
        .collect::<String>();
    format!("{}...", truncated.trim_end())
}

fn virtual_stack_id(selected_pr: &PullRequestDetail) -> String {
    let mut hasher = Sha1::new();
    for part in [
        selected_pr.repository.as_str(),
        &selected_pr.number.to_string(),
        selected_pr.base_ref_oid.as_deref().unwrap_or_default(),
        selected_pr.head_ref_oid.as_deref().unwrap_or_default(),
        StackSource::VirtualAi.label(),
        STACK_GENERATOR_VERSION,
    ] {
        hasher.update(part.as_bytes());
        hasher.update([0]);
    }
    format!("stack-{:x}", hasher.finalize())
}

fn virtual_layer_id(
    stack_id: &str,
    index: usize,
    role: ChangeRole,
    atom_ids: &[ChangeAtomId],
) -> String {
    let mut hasher = Sha1::new();
    hasher.update(stack_id.as_bytes());
    hasher.update(index.to_string().as_bytes());
    hasher.update(role.label().as_bytes());
    for atom_id in atom_ids {
        hasher.update(atom_id.as_bytes());
    }
    format!("ai-virtual-layer-{}-{:x}", index, hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        github::{PullRequestDataCompleteness, PullRequestFile},
        stacks::{
            model::{LineRange, StackWarning},
            validation::AiStackPlan,
        },
    };

    #[test]
    fn one_giant_commit_should_attempt_ai_planning() {
        let atoms = vec![
            atom("atom_1", ChangeRole::Foundation, 700),
            atom("atom_2", ChangeRole::CoreLogic, 900),
            atom("atom_3", ChangeRole::Integration, 600),
            atom("atom_4", ChangeRole::Tests, 300),
        ];
        let suitability = CommitSuitability {
            score: 0.1,
            suitable_for_layers: false,
            reasons: vec!["Only 1 commit for a large PR.".to_string()],
        };

        assert!(should_attempt_ai_planning(&atoms, 2_500, &suitability));
    }

    #[test]
    fn huge_two_commit_pr_should_attempt_ai_planning_before_commit_layers() {
        let atoms = vec![
            atom("atom_1", ChangeRole::Foundation, 900),
            atom("atom_2", ChangeRole::CoreLogic, 1_200),
            atom("atom_3", ChangeRole::Integration, 1_100),
            atom("atom_4", ChangeRole::Tests, 800),
        ];
        let suitability = CommitSuitability {
            score: 0.22,
            suitable_for_layers: false,
            reasons: vec![
                "Only 2 commits for a 4000-line PR.".to_string(),
                "Largest commit contains 82% of the changed lines.".to_string(),
            ],
        };

        assert!(should_attempt_ai_planning(&atoms, 4_000, &suitability));
    }

    #[test]
    fn builds_ai_stack_with_manual_review_final_layer() {
        let atoms = vec![
            atom("atom_1", ChangeRole::Foundation, 120),
            atom("atom_2", ChangeRole::CoreLogic, 180),
            manual_atom("atom_generated", 1_800),
        ];
        let plan = AiStackPlan {
            strategy: AiStackPlanStrategy::SemanticVirtualStack,
            confidence: Confidence::Medium,
            rationale: "Commits are too coarse.".to_string(),
            layers: vec![
                plan_layer("Foundation", vec!["atom_1"], vec![]),
                plan_layer("Core behavior", vec!["atom_2"], vec![0]),
            ],
            manual_review_atom_ids: vec!["atom_generated".to_string()],
            warnings: vec!["Generated file needs manual pass.".to_string()],
        };
        let validated = validate_ai_stack_plan(&plan, &atoms, 2_100).unwrap();
        let stack = build_stack_from_validated_plan(
            &detail(),
            atoms,
            validated,
            Some("test-model".to_string()),
            CodeTourProvider::Codex,
            CommitContext {
                commits: Vec::new(),
                suitability: CommitSuitability {
                    score: 0.0,
                    suitable_for_layers: false,
                    reasons: vec!["No commits.".to_string()],
                },
            },
        );

        assert_eq!(stack.source, StackSource::VirtualAi);
        assert_eq!(stack.layers.len(), 3);
        assert_eq!(
            stack.layers.last().unwrap().title,
            "Manual review / uncertain changes"
        );
        assert_eq!(
            stack
                .layers
                .iter()
                .flat_map(|layer| layer.atom_ids.iter())
                .collect::<BTreeSet<_>>()
                .len(),
            stack.atoms.len()
        );
    }

    #[test]
    fn invalid_ai_output_fallback_records_warning() {
        let stack = deterministic_fallback_stack(
            &detail(),
            &RepoContext::empty(),
            &VirtualStackSizing::default(),
            "AI stack planning returned invalid output; Remiss used deterministic semantic grouping.",
            Some(json!({ "validationError": "omitted atom" })),
        )
        .expect("fallback stack");

        assert_eq!(stack.source, StackSource::VirtualSemantic);
        assert!(stack.warnings.iter().any(|warning| {
            warning.code == "ai-virtual-stack-fallback"
                && warning.message.contains("invalid output")
        }));
    }

    fn plan_layer(title: &str, atom_ids: Vec<&str>, deps: Vec<usize>) -> AiStackPlanLayer {
        AiStackPlanLayer {
            title: title.to_string(),
            summary: format!("{title} summary"),
            rationale: format!("{title} rationale"),
            atom_ids: atom_ids.into_iter().map(str::to_string).collect(),
            depends_on_layer_indexes: deps,
            confidence: Confidence::Medium,
            review_priority: AiReviewPriority::Normal,
        }
    }

    fn atom(id: &str, role: ChangeRole, changed_lines: usize) -> ChangeAtom {
        ChangeAtom {
            id: id.to_string(),
            source: ChangeAtomSource::Hunk { hunk_index: 0 },
            path: format!("src/{id}.rs"),
            previous_path: None,
            role,
            semantic_kind: Some("function".to_string()),
            symbol_name: Some(id.to_string()),
            defined_symbols: vec![id.to_string()],
            referenced_symbols: Vec::new(),
            old_range: Some(LineRange { start: 1, end: 2 }),
            new_range: Some(LineRange { start: 1, end: 3 }),
            hunk_headers: Vec::new(),
            hunk_indices: vec![0],
            additions: changed_lines,
            deletions: 0,
            patch_hash: format!("hash-{id}"),
            risk_score: changed_lines as i64,
            review_thread_ids: Vec::new(),
            warnings: Vec::new(),
        }
    }

    fn manual_atom(id: &str, changed_lines: usize) -> ChangeAtom {
        let mut atom = atom(id, ChangeRole::Generated, changed_lines);
        atom.source = ChangeAtomSource::GeneratedPlaceholder;
        atom.warnings = vec![StackWarning::new("manual-review", "Generated file.")];
        atom
    }

    fn detail() -> PullRequestDetail {
        PullRequestDetail {
            id: "pr".to_string(),
            repository: "acme/repo".to_string(),
            number: 1,
            title: "PR".to_string(),
            body: String::new(),
            url: String::new(),
            author_login: "octo".to_string(),
            author_avatar_url: None,
            state: "OPEN".to_string(),
            is_draft: false,
            review_decision: None,
            base_ref_name: "main".to_string(),
            head_ref_name: "feature".to_string(),
            base_ref_oid: Some("base".to_string()),
            head_ref_oid: Some("head".to_string()),
            additions: 2_100,
            deletions: 0,
            changed_files: 3,
            comments_count: 0,
            commits_count: 1,
            created_at: String::new(),
            updated_at: "now".to_string(),
            labels: Vec::new(),
            reviewers: Vec::new(),
            reviewer_avatar_urls: Default::default(),
            comments: Vec::new(),
            latest_reviews: Vec::new(),
            review_threads: Vec::new(),
            files: vec![PullRequestFile {
                path: "src/atom_1.rs".to_string(),
                additions: 120,
                deletions: 0,
                change_type: "MODIFIED".to_string(),
            }],
            raw_diff: String::new(),
            parsed_diff: Vec::new(),
            data_completeness: PullRequestDataCompleteness::default(),
        }
    }
}
