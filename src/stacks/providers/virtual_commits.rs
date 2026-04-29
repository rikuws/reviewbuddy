use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    time::Duration,
};

use crate::{command_runner::CommandRunner, github::PullRequestDetail};
use serde::{Deserialize, Serialize};

use super::super::{
    atoms::{classify_change_role, extract_change_atoms},
    model::{
        stack_now_ms, ChangeAtom, ChangeRole, Confidence, LayerMetrics, LayerReviewStatus,
        RepoContext, ReviewStack, ReviewStackLayer, StackDiscoveryError, StackKind,
        StackProviderMetadata, StackSource, StackWarning, VirtualLayerRef, STACK_GENERATOR_VERSION,
    },
};

pub fn discover(
    selected_pr: &PullRequestDetail,
    repo_context: &RepoContext,
) -> Result<Option<ReviewStack>, StackDiscoveryError> {
    if selected_pr.commits_count <= 0 {
        return Ok(None);
    }

    let Some(base_oid) = selected_pr.base_ref_oid.as_deref() else {
        return Ok(None);
    };
    let Some(head_oid) = selected_pr.head_ref_oid.as_deref() else {
        return Ok(None);
    };

    let atoms = extract_change_atoms(selected_pr);
    if atoms.is_empty() {
        return Ok(None);
    }

    let Some(context) = commit_context_for_pr(selected_pr, repo_context, &atoms)? else {
        return Ok(None);
    };
    if !context.suitability.suitable_for_layers {
        log_commit_suitability(selected_pr, &context.suitability);
        return Ok(None);
    }

    log_commit_suitability(selected_pr, &context.suitability);

    let commits = context.commits;

    let mut atoms_by_path = BTreeMap::<String, Vec<&ChangeAtom>>::new();
    for atom in &atoms {
        atoms_by_path
            .entry(atom.path.clone())
            .or_default()
            .push(atom);
    }

    let stack_id = format!(
        "commit-stack:{}:{}:{}",
        selected_pr.repository, base_oid, head_oid
    );
    let mut assigned_atom_ids = std::collections::BTreeSet::<String>::new();
    let mut layers = Vec::<ReviewStackLayer>::new();
    let mut warnings = Vec::<StackWarning>::new();

    for commit in &commits {
        let mut layer_atoms = Vec::<&ChangeAtom>::new();
        for path in &commit.paths {
            for atom in atoms_by_path.get(path).into_iter().flatten() {
                if assigned_atom_ids.insert(atom.id.clone()) {
                    layer_atoms.push(*atom);
                }
            }
        }

        if layer_atoms.is_empty() {
            warnings.push(StackWarning::new(
                "empty-commit-layer",
                format!(
                    "Commit {} did not map cleanly to parsed diff atoms and was skipped.",
                    short_oid(commit.oid.as_str())
                ),
            ));
            continue;
        }

        let index = layers.len();
        let metrics = metrics_for_atoms(&layer_atoms);
        let dominant_role = dominant_role(&layer_atoms);
        let layer_id = format!("commit-layer-{index}-{}", short_oid(commit.oid.as_str()));
        layers.push(ReviewStackLayer {
            id: layer_id,
            index,
            title: commit.subject.clone(),
            summary: format!(
                "Commit {} touches {} file{} with {} changed line{}.",
                short_oid(commit.oid.as_str()),
                metrics.file_count,
                if metrics.file_count == 1 { "" } else { "s" },
                commit.changed_lines,
                if commit.changed_lines == 1 { "" } else { "s" }
            ),
            rationale: "This layer follows the author's commit boundary because the PR has a small, meaningful commit sequence.".to_string(),
            pr: None,
            virtual_layer: Some(VirtualLayerRef {
                source: StackSource::VirtualCommits,
                role: dominant_role,
                source_label: short_oid(commit.oid.as_str()),
            }),
            base_oid: Some(base_oid.to_string()),
            head_oid: Some(commit.oid.clone()),
            atom_ids: layer_atoms.iter().map(|atom| atom.id.clone()).collect(),
            depends_on_layer_ids: if index > 0 {
                vec![layers[index - 1].id.clone()]
            } else {
                Vec::new()
            },
            metrics,
            status: LayerReviewStatus::NotReviewed,
            confidence: Confidence::Medium,
            warnings: Vec::new(),
        });
    }

    let unassigned = atoms
        .iter()
        .filter(|atom| !assigned_atom_ids.contains(&atom.id))
        .collect::<Vec<_>>();
    if !unassigned.is_empty() {
        let index = layers.len();
        let metrics = metrics_for_atoms(&unassigned);
        layers.push(ReviewStackLayer {
            id: format!("commit-layer-{index}-manual"),
            index,
            title: "Unassigned / manual review".to_string(),
            summary: format!(
                "{} atom{} could not be mapped to a commit file list.",
                unassigned.len(),
                if unassigned.len() == 1 { "" } else { "s" }
            ),
            rationale: "These atoms remain visible because commit metadata did not account for every parsed diff atom.".to_string(),
            pr: None,
            virtual_layer: Some(VirtualLayerRef {
                source: StackSource::VirtualCommits,
                role: ChangeRole::Unknown,
                source_label: "manual".to_string(),
            }),
            base_oid: Some(base_oid.to_string()),
            head_oid: Some(head_oid.to_string()),
            atom_ids: unassigned.iter().map(|atom| atom.id.clone()).collect(),
            depends_on_layer_ids: layers
                .last()
                .map(|layer| vec![layer.id.clone()])
                .unwrap_or_default(),
            metrics,
            status: LayerReviewStatus::NotReviewed,
            confidence: Confidence::Low,
            warnings: vec![StackWarning::new(
                "unassigned-atoms",
                "Some atoms could not be mapped to commit metadata.",
            )],
        });
        warnings.push(StackWarning::new(
            "unassigned-atoms",
            "Some atoms could not be mapped to commit metadata.",
        ));
    }

    if layers.len() < 2 {
        return Ok(None);
    }

    Ok(Some(ReviewStack {
        id: stack_id,
        repository: selected_pr.repository.clone(),
        selected_pr_number: selected_pr.number,
        source: StackSource::VirtualCommits,
        kind: StackKind::Virtual,
        confidence: if warnings.is_empty() {
            Confidence::Medium
        } else {
            Confidence::Low
        },
        trunk_branch: Some(selected_pr.base_ref_name.clone()),
        base_oid: selected_pr.base_ref_oid.clone(),
        head_oid: selected_pr.head_ref_oid.clone(),
        layers,
        atoms,
        warnings,
        provider: Some(StackProviderMetadata {
            provider: "virtual_commits".to_string(),
            raw_payload: Some(serde_json::json!({
                "commitSuitability": context.suitability,
                "commits": commits,
            })),
        }),
        generated_at_ms: stack_now_ms(),
        generator_version: STACK_GENERATOR_VERSION.to_string(),
    }))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitContext {
    pub commits: Vec<CommitSummary>,
    pub suitability: CommitSuitability,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitSuitability {
    pub score: f32,
    pub suitable_for_layers: bool,
    pub reasons: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitSummary {
    pub oid: String,
    pub subject: String,
    pub paths: Vec<String>,
    pub changed_lines: usize,
    #[serde(default)]
    pub roles: Vec<ChangeRole>,
}

pub fn commit_context_for_pr(
    selected_pr: &PullRequestDetail,
    repo_context: &RepoContext,
    atoms: &[ChangeAtom],
) -> Result<Option<CommitContext>, StackDiscoveryError> {
    let Some(repo_path) = repo_context.local_repo_path.as_deref() else {
        return Ok(None);
    };
    let Some(base_oid) = selected_pr.base_ref_oid.as_deref() else {
        return Ok(None);
    };
    let Some(head_oid) = selected_pr.head_ref_oid.as_deref() else {
        return Ok(None);
    };

    let mut commits = commit_summaries(repo_path, base_oid, head_oid)?;
    if commits.is_empty() {
        return Ok(None);
    }

    attach_commit_roles(&mut commits, atoms);
    let total_changed_lines = atoms
        .iter()
        .map(|atom| atom.additions + atom.deletions)
        .sum::<usize>()
        .max((selected_pr.additions + selected_pr.deletions).max(0) as usize);
    let suitability = score_commit_suitability(&commits, total_changed_lines);

    Ok(Some(CommitContext {
        commits,
        suitability,
    }))
}

pub fn score_commit_suitability(
    commits: &[CommitSummary],
    total_changed_lines: usize,
) -> CommitSuitability {
    let commit_count = commits.len();
    let commit_changed_lines = commits
        .iter()
        .map(|commit| commit.changed_lines)
        .sum::<usize>();
    let total_changed_lines = total_changed_lines.max(commit_changed_lines).max(1);
    let largest_commit_changed_lines = commits
        .iter()
        .map(|commit| commit.changed_lines)
        .max()
        .unwrap_or_default();
    let largest_ratio = largest_commit_changed_lines as f32 / total_changed_lines as f32;
    let weak_message_count = commits
        .iter()
        .filter(|commit| weak_commit_subject(&commit.subject))
        .count();
    let mixed_large_commit = commits
        .iter()
        .any(|commit| commit.changed_lines > 1_200 && non_trivial_role_count(&commit.roles) >= 3);

    let mut score = 0.0f32;
    let mut reasons = Vec::<String>::new();
    let mut hard_reject = false;

    if commit_count <= 2 && total_changed_lines > 800 {
        hard_reject = true;
        reasons.push(format!(
            "Only {commit_count} commit{} for a {total_changed_lines}-line PR.",
            if commit_count == 1 { "" } else { "s" }
        ));
    }

    if (3..=12).contains(&commit_count) {
        score += 0.18;
    } else if !(commit_count <= 2 && total_changed_lines > 800) {
        reasons.push(if commit_count == 1 {
            "1 commit is outside the preferred 3-12 layer range.".to_string()
        } else {
            format!("{commit_count} commits are outside the preferred 3-12 layer range.")
        });
    }

    if largest_ratio > 0.65 {
        hard_reject = true;
        reasons.push(format!(
            "Largest commit contains {:.0}% of the changed lines.",
            largest_ratio * 100.0
        ));
    } else if largest_ratio <= 0.45 {
        score += 0.18;
    } else {
        score += 0.08;
        reasons.push(format!(
            "Largest commit contains {:.0}% of the changed lines.",
            largest_ratio * 100.0
        ));
    }

    if mixed_large_commit {
        hard_reject = true;
        reasons.push(
            "At least one commit is over 1200 changed lines and touches 3+ review roles."
                .to_string(),
        );
    } else {
        score += 0.12;
    }

    if weak_message_count > 0 && total_changed_lines > 800 {
        hard_reject = true;
        reasons.push(format!(
            "{} commit message{} look weak for a large PR.",
            weak_message_count,
            if weak_message_count == 1 { "" } else { "s" }
        ));
    } else if weak_message_count == 0 {
        score += 0.12;
    } else {
        score += 0.05;
        reasons.push(format!(
            "{} commit message{} are weak.",
            weak_message_count,
            if weak_message_count == 1 { "" } else { "s" }
        ));
    }

    let coherent_commits = commits
        .iter()
        .filter(|commit| non_trivial_role_count(&commit.roles) <= 2)
        .count();
    if coherent_commits * 4 >= commit_count.saturating_mul(3).max(1) {
        score += 0.18;
    } else {
        reasons.push("Commits touch mixed review roles.".to_string());
    }

    let balanced_commit_count = commits
        .iter()
        .filter(|commit| {
            let ratio = commit.changed_lines as f32 / total_changed_lines as f32;
            (0.05..=0.45).contains(&ratio)
        })
        .count();
    if balanced_commit_count * 3 >= commit_count.saturating_mul(2).max(1) {
        score += 0.14;
    } else {
        reasons.push("Commit sizes are not well balanced.".to_string());
    }

    let individually_small = commits
        .iter()
        .filter(|commit| commit.changed_lines <= 1_200)
        .count();
    if individually_small == commit_count {
        score += 0.10;
    } else {
        reasons.push("One or more commits are individually too large.".to_string());
    }

    let heavily_mixed_support = commits.iter().any(|commit| {
        let has_support = commit
            .roles
            .iter()
            .any(|role| matches!(role, ChangeRole::Tests | ChangeRole::Docs));
        let has_behavior = commit.roles.iter().any(|role| {
            matches!(
                role,
                ChangeRole::Foundation
                    | ChangeRole::CoreLogic
                    | ChangeRole::Integration
                    | ChangeRole::Presentation
                    | ChangeRole::Config
            )
        });
        has_support && has_behavior && commit.changed_lines > 300
    });
    if heavily_mixed_support {
        reasons.push("Tests or docs are mixed heavily with implementation commits.".to_string());
    } else {
        score += 0.08;
    }

    let score = score.min(1.0);
    let suitable_for_layers = !hard_reject && (3..=12).contains(&commit_count) && score >= 0.68;
    if suitable_for_layers && reasons.is_empty() {
        reasons.push("Commits are granular, coherent, and balanced.".to_string());
    }

    CommitSuitability {
        score,
        suitable_for_layers,
        reasons,
    }
}

fn log_commit_suitability(selected_pr: &PullRequestDetail, suitability: &CommitSuitability) {
    let primary_reason = suitability
        .reasons
        .first()
        .map(String::as_str)
        .unwrap_or("Commits are granular, coherent, and balanced.");
    let additional_reason_count = suitability.reasons.len().saturating_sub(1);

    eprintln!(
        "Commit virtual stack suitability: pr={}#{} score={:.2} suitable={} decision={} reason=\"{}\" additional_reasons={}",
        selected_pr.repository,
        selected_pr.number,
        suitability.score,
        suitability.suitable_for_layers,
        if suitability.suitable_for_layers {
            "use_commit_layers"
        } else {
            "skip_commit_layers"
        },
        primary_reason,
        additional_reason_count
    );
}

fn commit_summaries(
    repo_path: &Path,
    base_oid: &str,
    head_oid: &str,
) -> Result<Vec<CommitSummary>, StackDiscoveryError> {
    let output = CommandRunner::new("git")
        .args([
            "log",
            "--reverse",
            "--numstat",
            "--format=%H%x00%s",
            &format!("{base_oid}..{head_oid}"),
        ])
        .current_dir(repo_path)
        .timeout(Duration::from_secs(20))
        .run()
        .map_err(StackDiscoveryError::new)?;

    if output.exit_code != Some(0) {
        return Err(StackDiscoveryError::new(if output.stderr.is_empty() {
            "Failed to read commit stack from local git.".to_string()
        } else {
            output.stderr
        }));
    }

    let mut commits = Vec::<CommitSummary>::new();
    let mut current: Option<CommitSummary> = None;

    for line in output.stdout.lines() {
        if let Some((oid, subject)) = line.split_once('\0') {
            if let Some(commit) = current.take() {
                commits.push(commit);
            }
            current = Some(CommitSummary {
                oid: oid.to_string(),
                subject: clean_subject(subject),
                paths: Vec::new(),
                changed_lines: 0,
                roles: Vec::new(),
            });
            continue;
        }

        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let (Some(commit), Some(stat)) = (current.as_mut(), parse_numstat_line(line)) {
            commit.changed_lines += stat.changed_lines;
            if !commit.paths.contains(&stat.path) {
                commit.paths.push(stat.path);
            }
        }
    }

    if let Some(commit) = current {
        commits.push(commit);
    }

    Ok(commits)
}

#[derive(Clone, Debug)]
struct NumstatLine {
    path: String,
    changed_lines: usize,
}

fn parse_numstat_line(line: &str) -> Option<NumstatLine> {
    let mut parts = line.split('\t');
    let additions = parse_numstat_count(parts.next()?);
    let deletions = parse_numstat_count(parts.next()?);
    let path = parts.next()?.trim();
    if path.is_empty() {
        return None;
    }
    Some(NumstatLine {
        path: normalize_numstat_path(path),
        changed_lines: additions.saturating_add(deletions),
    })
}

fn parse_numstat_count(value: &str) -> usize {
    value.trim().parse::<usize>().unwrap_or(0)
}

fn normalize_numstat_path(path: &str) -> String {
    let trimmed = path.trim();
    if let (Some(open), Some(close)) = (trimmed.find('{'), trimmed.rfind('}')) {
        if open < close {
            let prefix = &trimmed[..open];
            let inner = &trimmed[open + 1..close];
            let suffix = &trimmed[close + 1..];
            if let Some((_, to)) = inner.rsplit_once(" => ") {
                return format!("{prefix}{}{suffix}", to.trim());
            }
        }
    }
    if let Some((_, to)) = trimmed.rsplit_once(" => ") {
        return to
            .trim()
            .trim_start_matches('{')
            .trim_end_matches('}')
            .to_string();
    }
    trimmed.to_string()
}

fn attach_commit_roles(commits: &mut [CommitSummary], atoms: &[ChangeAtom]) {
    let mut roles_by_path = BTreeMap::<String, BTreeSet<ChangeRole>>::new();
    for atom in atoms {
        roles_by_path
            .entry(atom.path.clone())
            .or_default()
            .insert(atom.role);
    }

    for commit in commits {
        let mut roles = BTreeSet::<ChangeRole>::new();
        for path in &commit.paths {
            if let Some(path_roles) = roles_by_path.get(path) {
                roles.extend(path_roles.iter().copied());
            } else {
                roles.insert(classify_change_role(path, None));
            }
        }
        commit.roles = roles.into_iter().collect();
    }
}

fn non_trivial_role_count(roles: &[ChangeRole]) -> usize {
    roles
        .iter()
        .filter(|role| !matches!(role, ChangeRole::Unknown | ChangeRole::Generated))
        .collect::<BTreeSet<_>>()
        .len()
}

fn weak_commit_subject(subject: &str) -> bool {
    let lower = subject.trim().to_ascii_lowercase();
    if lower.len() < 8 {
        return true;
    }

    let normalized = lower
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || character.is_ascii_whitespace())
        .collect::<String>();
    let words = normalized.split_whitespace().collect::<Vec<_>>();
    let weak_exact = [
        "wip", "fixup", "misc", "cleanup", "final", "changes", "updates", "stuff", "work",
    ];
    if words.len() <= 2 && words.iter().any(|word| weak_exact.contains(word)) {
        return true;
    }

    lower.starts_with("fixup!")
        || lower.starts_with("squash!")
        || lower.starts_with("wip")
        || lower.contains("final changes")
        || lower.contains("misc changes")
}

fn clean_subject(subject: &str) -> String {
    let trimmed = subject.trim();
    if trimmed.is_empty() {
        "Commit layer".to_string()
    } else {
        trimmed.chars().take(80).collect()
    }
}

fn short_oid(oid: &str) -> String {
    oid.chars().take(8).collect()
}

fn metrics_for_atoms(atoms: &[&ChangeAtom]) -> LayerMetrics {
    let file_count = atoms
        .iter()
        .map(|atom| atom.path.as_str())
        .collect::<std::collections::BTreeSet<_>>()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn huge_two_commit_pr_is_not_suitable_for_commit_layers() {
        let commits = vec![
            commit(
                "a",
                "Initial implementation",
                3_300,
                vec![
                    ChangeRole::Foundation,
                    ChangeRole::CoreLogic,
                    ChangeRole::Integration,
                    ChangeRole::Tests,
                ],
            ),
            commit("b", "Final changes", 700, vec![ChangeRole::Tests]),
        ];

        let suitability = score_commit_suitability(&commits, 4_000);

        assert!(!suitability.suitable_for_layers);
        assert!(suitability.score < 0.68);
        assert!(suitability
            .reasons
            .iter()
            .any(|reason| reason.contains("Only 2 commits")));
        assert!(suitability
            .reasons
            .iter()
            .any(|reason| reason.contains("Largest commit")));
    }

    #[test]
    fn good_commit_sequence_is_suitable_for_layers() {
        let commits = vec![
            commit(
                "a",
                "Add shared widget model",
                180,
                vec![ChangeRole::Foundation],
            ),
            commit(
                "b",
                "Implement widget service",
                220,
                vec![ChangeRole::CoreLogic],
            ),
            commit("c", "Wire widget API", 160, vec![ChangeRole::Integration]),
            commit(
                "d",
                "Render widget panel",
                190,
                vec![ChangeRole::Presentation],
            ),
            commit("e", "Cover widget behavior", 140, vec![ChangeRole::Tests]),
        ];

        let suitability = score_commit_suitability(&commits, 890);

        assert!(suitability.suitable_for_layers);
        assert!(suitability.score >= 0.68);
    }

    #[test]
    fn one_giant_commit_is_not_suitable_for_layers() {
        let commits = vec![commit(
            "a",
            "Implement feature",
            2_500,
            vec![
                ChangeRole::Foundation,
                ChangeRole::CoreLogic,
                ChangeRole::Integration,
                ChangeRole::Tests,
            ],
        )];

        let suitability = score_commit_suitability(&commits, 2_500);

        assert!(!suitability.suitable_for_layers);
        assert!(suitability
            .reasons
            .iter()
            .any(|reason| reason.contains("Only 1 commit")));
    }

    fn commit(
        oid: &str,
        subject: &str,
        changed_lines: usize,
        roles: Vec<ChangeRole>,
    ) -> CommitSummary {
        CommitSummary {
            oid: oid.to_string(),
            subject: subject.to_string(),
            paths: roles
                .iter()
                .enumerate()
                .map(|(index, role)| format!("src/{index}-{}.rs", role.label()))
                .collect(),
            changed_lines,
            roles,
        }
    }
}
