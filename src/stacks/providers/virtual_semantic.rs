use std::collections::{BTreeMap, BTreeSet};

use sha1::{Digest, Sha1};

use crate::github::PullRequestDetail;

use super::super::{
    atoms::extract_change_atoms,
    dependencies::{build_atom_dependencies, dependency_depths, strongly_connected_components},
    model::{
        stack_now_ms, ChangeAtom, ChangeAtomId, ChangeRole, Confidence, LayerMetrics,
        LayerReviewStatus, RepoContext, ReviewStack, ReviewStackLayer, StackDiscoveryError,
        StackKind, StackProviderMetadata, StackSource, StackWarning, VirtualLayerRef,
        VirtualStackSizing, STACK_GENERATOR_VERSION,
    },
};

pub fn discover(
    selected_pr: &PullRequestDetail,
    _repo_context: &RepoContext,
    sizing: &VirtualStackSizing,
) -> Result<Option<ReviewStack>, StackDiscoveryError> {
    let atoms = extract_change_atoms(selected_pr);
    if atoms.is_empty() {
        return Ok(Some(empty_virtual_stack(selected_pr)));
    }

    Ok(Some(build_stack_from_atoms(selected_pr, atoms, sizing)))
}

fn build_stack_from_atoms(
    selected_pr: &PullRequestDetail,
    atoms: Vec<ChangeAtom>,
    sizing: &VirtualStackSizing,
) -> ReviewStack {
    let dependencies = build_atom_dependencies(&atoms);
    let atom_ids = atoms.iter().map(|atom| atom.id.clone()).collect::<Vec<_>>();
    let components = strongly_connected_components(&atom_ids, &dependencies);
    let depths = dependency_depths(&atom_ids, &dependencies);
    let mut warnings = Vec::<StackWarning>::new();

    let cycle_count = components
        .iter()
        .filter(|component| component.len() > 1)
        .count();
    if cycle_count > 0 {
        warnings.push(StackWarning::new(
            "dependency-cycles",
            format!("{cycle_count} dependency cycle(s) were collapsed before layer ordering."),
        ));
    }

    warnings.extend(
        atoms
            .iter()
            .flat_map(|atom| atom.warnings.iter().cloned())
            .collect::<Vec<_>>(),
    );

    let mut atoms_by_role = BTreeMap::<ChangeRole, Vec<&ChangeAtom>>::new();
    for atom in &atoms {
        atoms_by_role.entry(atom.role).or_default().push(atom);
    }

    for role_atoms in atoms_by_role.values_mut() {
        role_atoms.sort_by(|left, right| {
            depths
                .get(&left.id)
                .copied()
                .unwrap_or_default()
                .cmp(&depths.get(&right.id).copied().unwrap_or_default())
                .then_with(|| right.risk_score.cmp(&left.risk_score))
                .then_with(|| left.path.cmp(&right.path))
                .then_with(|| left.id.cmp(&right.id))
        });
    }

    let mut layer_groups = initial_layer_groups(atoms_by_role, sizing);
    if layer_groups.len() > sizing.target_max_layers {
        layer_groups = merge_excess_layers(layer_groups, sizing.target_max_layers);
    }

    let confidence = semantic_confidence(&atoms, &warnings, cycle_count);
    let stack_id = virtual_stack_id(selected_pr, StackSource::VirtualSemantic);
    let mut layers = layer_groups
        .into_iter()
        .enumerate()
        .map(|(index, group)| {
            let role = group.role;
            let atom_ids = group
                .atoms
                .iter()
                .map(|atom| atom.id.clone())
                .collect::<Vec<_>>();
            let metrics = metrics_for_atoms(&group.atoms);
            let layer_id = virtual_layer_id(&stack_id, index, role, &atom_ids);
            let title = layer_title(role, index, group.split_label.as_deref());
            let summary = layer_summary(role, &metrics);
            let rationale = layer_rationale(role, confidence, &group.atoms);
            let layer_confidence =
                if group.atoms.iter().any(|atom| {
                    atom.role == ChangeRole::Generated || atom.role == ChangeRole::Unknown
                }) {
                    Confidence::Low
                } else {
                    confidence
                };
            let warnings = group
                .atoms
                .iter()
                .flat_map(|atom| atom.warnings.iter().cloned())
                .collect::<Vec<_>>();

            ReviewStackLayer {
                id: layer_id,
                index,
                title,
                summary,
                rationale,
                pr: None,
                virtual_layer: Some(VirtualLayerRef {
                    source: StackSource::VirtualSemantic,
                    role,
                    source_label: "semantic diff".to_string(),
                }),
                base_oid: selected_pr.base_ref_oid.clone(),
                head_oid: selected_pr.head_ref_oid.clone(),
                atom_ids,
                depends_on_layer_ids: Vec::new(),
                metrics,
                status: LayerReviewStatus::NotReviewed,
                confidence: layer_confidence,
                warnings,
            }
        })
        .collect::<Vec<_>>();

    for index in 1..layers.len() {
        let role = layers[index]
            .virtual_layer
            .as_ref()
            .map(|virtual_layer| virtual_layer.role);
        if !matches!(role, Some(ChangeRole::Foundation | ChangeRole::Config)) {
            let previous_id = layers[index - 1].id.clone();
            layers[index].depends_on_layer_ids = vec![previous_id];
        }
    }

    ReviewStack {
        id: stack_id,
        repository: selected_pr.repository.clone(),
        selected_pr_number: selected_pr.number,
        source: StackSource::VirtualSemantic,
        kind: StackKind::Virtual,
        confidence,
        trunk_branch: Some(selected_pr.base_ref_name.clone()),
        base_oid: selected_pr.base_ref_oid.clone(),
        head_oid: selected_pr.head_ref_oid.clone(),
        layers,
        atoms,
        warnings,
        provider: Some(StackProviderMetadata {
            provider: "virtual_semantic".to_string(),
            raw_payload: None,
        }),
        generated_at_ms: stack_now_ms(),
        generator_version: STACK_GENERATOR_VERSION.to_string(),
    }
}

#[derive(Clone)]
struct LayerGroup<'a> {
    role: ChangeRole,
    atoms: Vec<&'a ChangeAtom>,
    split_label: Option<String>,
}

fn initial_layer_groups<'a>(
    atoms_by_role: BTreeMap<ChangeRole, Vec<&'a ChangeAtom>>,
    sizing: &VirtualStackSizing,
) -> Vec<LayerGroup<'a>> {
    let mut groups = Vec::new();
    for role in [
        ChangeRole::Foundation,
        ChangeRole::Config,
        ChangeRole::CoreLogic,
        ChangeRole::Integration,
        ChangeRole::Presentation,
        ChangeRole::Tests,
        ChangeRole::Docs,
        ChangeRole::Generated,
        ChangeRole::Unknown,
    ] {
        let Some(atoms) = atoms_by_role.get(&role).filter(|atoms| !atoms.is_empty()) else {
            continue;
        };
        groups.extend(split_large_group(role, atoms.clone(), sizing));
    }
    groups
}

fn split_large_group<'a>(
    role: ChangeRole,
    atoms: Vec<&'a ChangeAtom>,
    sizing: &VirtualStackSizing,
) -> Vec<LayerGroup<'a>> {
    let metrics = metrics_for_atoms(&atoms);
    if metrics.changed_lines <= sizing.max_layer_changed_lines
        && metrics.file_count <= sizing.max_layer_files
    {
        return vec![LayerGroup {
            role,
            atoms,
            split_label: None,
        }];
    }

    let mut by_dir = BTreeMap::<String, Vec<&ChangeAtom>>::new();
    for atom in atoms {
        by_dir
            .entry(directory_label(atom.path.as_str()))
            .or_default()
            .push(atom);
    }

    by_dir
        .into_iter()
        .flat_map(|(dir, atoms)| {
            let metrics = metrics_for_atoms(&atoms);
            if metrics.changed_lines <= sizing.max_layer_changed_lines
                && metrics.file_count <= sizing.max_layer_files
            {
                vec![LayerGroup {
                    role,
                    atoms,
                    split_label: Some(dir),
                }]
            } else {
                split_by_file(role, atoms, dir, sizing.max_layer_changed_lines)
            }
        })
        .collect()
}

fn split_by_file<'a>(
    role: ChangeRole,
    atoms: Vec<&'a ChangeAtom>,
    directory: String,
    max_changed_lines: usize,
) -> Vec<LayerGroup<'a>> {
    let mut groups = Vec::new();
    let mut current = Vec::<&ChangeAtom>::new();
    let mut current_lines = 0usize;

    for atom in atoms {
        let atom_lines = atom.additions + atom.deletions;
        if !current.is_empty() && current_lines + atom_lines > max_changed_lines {
            groups.push(LayerGroup {
                role,
                atoms: std::mem::take(&mut current),
                split_label: Some(directory.clone()),
            });
            current_lines = 0;
        }
        current_lines += atom_lines;
        current.push(atom);
    }

    if !current.is_empty() {
        groups.push(LayerGroup {
            role,
            atoms: current,
            split_label: Some(directory),
        });
    }

    groups
}

fn merge_excess_layers<'a>(
    mut groups: Vec<LayerGroup<'a>>,
    target_max_layers: usize,
) -> Vec<LayerGroup<'a>> {
    while groups.len() > target_max_layers {
        let Some(last) = groups.pop() else {
            break;
        };
        if let Some(previous) = groups.last_mut() {
            previous.atoms.extend(last.atoms);
            previous.split_label = None;
        } else {
            groups.push(last);
            break;
        }
    }
    groups
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

fn semantic_confidence(
    atoms: &[ChangeAtom],
    warnings: &[StackWarning],
    cycle_count: usize,
) -> Confidence {
    let total_lines = atoms
        .iter()
        .map(|atom| atom.additions + atom.deletions)
        .sum::<usize>()
        .max(1);
    let generated_lines = atoms
        .iter()
        .filter(|atom| atom.role == ChangeRole::Generated)
        .map(|atom| atom.additions + atom.deletions)
        .sum::<usize>();
    let unknown_atoms = atoms
        .iter()
        .filter(|atom| atom.role == ChangeRole::Unknown)
        .count();

    if generated_lines * 2 >= total_lines || unknown_atoms > atoms.len() / 2 || cycle_count > 4 {
        Confidence::Low
    } else if warnings.is_empty() && cycle_count == 0 {
        Confidence::Medium
    } else {
        Confidence::Low
    }
}

fn layer_title(role: ChangeRole, index: usize, split_label: Option<&str>) -> String {
    let base = match role {
        ChangeRole::Foundation | ChangeRole::Config => "Foundation",
        ChangeRole::CoreLogic => "Core behavior",
        ChangeRole::Integration => "Integration / wiring",
        ChangeRole::Presentation => "Presentation / UI",
        ChangeRole::Tests => "Tests",
        ChangeRole::Docs => "Docs / cleanup",
        ChangeRole::Generated => "Generated / manual review",
        ChangeRole::Unknown => "Unassigned / manual review",
    };

    split_label
        .map(|label| format!("{base}: {label}"))
        .unwrap_or_else(|| {
            if role == ChangeRole::Unknown {
                base.to_string()
            } else {
                format!("{} {base}", index + 1)
            }
        })
}

fn layer_summary(role: ChangeRole, metrics: &LayerMetrics) -> String {
    format!(
        "{} across {} file{} with {} changed line{}.",
        role.label(),
        metrics.file_count,
        if metrics.file_count == 1 { "" } else { "s" },
        metrics.changed_lines,
        if metrics.changed_lines == 1 { "" } else { "s" }
    )
}

fn layer_rationale(role: ChangeRole, confidence: Confidence, atoms: &[&ChangeAtom]) -> String {
    match role {
        ChangeRole::Foundation | ChangeRole::Config => {
            "These changes establish data shapes, schemas, configuration, or project foundations used by later layers.".to_string()
        }
        ChangeRole::CoreLogic => {
            "These changes modify core behavior and should be reviewed before integration, UI, and tests that depend on it.".to_string()
        }
        ChangeRole::Integration => {
            "These changes wire core behavior into APIs, adapters, persistence, or external boundaries.".to_string()
        }
        ChangeRole::Presentation => {
            "These changes expose behavior through views, components, or formatting surfaces after the underlying logic is in place.".to_string()
        }
        ChangeRole::Tests => {
            "These tests depend on earlier implementation layers and should be checked after reviewing the code they exercise.".to_string()
        }
        ChangeRole::Docs => {
            "These documentation and cleanup changes are separated so the functional review can stay focused.".to_string()
        }
        ChangeRole::Generated | ChangeRole::Unknown => format!(
            "{}. {} atom{} require manual attention.",
            confidence.label(),
            atoms.len(),
            if atoms.len() == 1 { "" } else { "s" }
        ),
    }
}

fn virtual_stack_id(selected_pr: &PullRequestDetail, source: StackSource) -> String {
    let mut hasher = Sha1::new();
    for part in [
        selected_pr.repository.as_str(),
        &selected_pr.number.to_string(),
        selected_pr.base_ref_oid.as_deref().unwrap_or_default(),
        selected_pr.head_ref_oid.as_deref().unwrap_or_default(),
        source.label(),
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
    format!("virtual-layer-{}-{:x}", index, hasher.finalize())
}

fn directory_label(path: &str) -> String {
    path.rsplit_once('/')
        .map(|(dir, _)| dir.to_string())
        .filter(|dir| !dir.is_empty())
        .unwrap_or_else(|| "root".to_string())
}

fn empty_virtual_stack(selected_pr: &PullRequestDetail) -> ReviewStack {
    let stack_id = virtual_stack_id(selected_pr, StackSource::VirtualSemantic);
    ReviewStack {
        id: stack_id.clone(),
        repository: selected_pr.repository.clone(),
        selected_pr_number: selected_pr.number,
        source: StackSource::VirtualSemantic,
        kind: StackKind::Virtual,
        confidence: Confidence::Low,
        trunk_branch: Some(selected_pr.base_ref_name.clone()),
        base_oid: selected_pr.base_ref_oid.clone(),
        head_oid: selected_pr.head_ref_oid.clone(),
        layers: vec![ReviewStackLayer {
            id: format!("{stack_id}-empty"),
            index: 0,
            title: "Unassigned / manual review".to_string(),
            summary: "No parsed diff atoms were available.".to_string(),
            rationale: "Remiss could not extract reviewable diff atoms, so the whole PR remains available in flat diff mode.".to_string(),
            pr: None,
            virtual_layer: Some(VirtualLayerRef {
                source: StackSource::VirtualSemantic,
                role: ChangeRole::Unknown,
                source_label: "empty diff".to_string(),
            }),
            base_oid: selected_pr.base_ref_oid.clone(),
            head_oid: selected_pr.head_ref_oid.clone(),
            atom_ids: Vec::new(),
            depends_on_layer_ids: Vec::new(),
            metrics: LayerMetrics::default(),
            status: LayerReviewStatus::NotReviewed,
            confidence: Confidence::Low,
            warnings: vec![StackWarning::new(
                "no-atoms",
                "No parsed hunks or file placeholders were available.",
            )],
        }],
        atoms: Vec::new(),
        warnings: vec![StackWarning::new(
            "no-atoms",
            "No parsed hunks or file placeholders were available.",
        )],
        provider: Some(StackProviderMetadata {
            provider: "virtual_semantic".to_string(),
            raw_payload: None,
        }),
        generated_at_ms: stack_now_ms(),
        generator_version: STACK_GENERATOR_VERSION.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        diff::parse_unified_diff,
        github::{PullRequestDataCompleteness, PullRequestDetail, PullRequestFile},
        stacks::{
            model::{RepoContext, VirtualStackSizing},
            providers::virtual_semantic::discover,
        },
    };

    #[test]
    fn one_commit_pr_is_split_into_ordered_semantic_layers() {
        let raw_diff = r#"diff --git a/src/model.rs b/src/model.rs
--- a/src/model.rs
+++ b/src/model.rs
@@ -1,1 +1,4 @@
+pub struct User {
+    id: String,
+}
diff --git a/src/service.rs b/src/service.rs
--- a/src/service.rs
+++ b/src/service.rs
@@ -1,1 +1,4 @@ fn load()
+fn load_user() -> User {
+    User { id: "1".into() }
+}
diff --git a/tests/service_test.rs b/tests/service_test.rs
--- a/tests/service_test.rs
+++ b/tests/service_test.rs
@@ -1,1 +1,3 @@
+#[test]
+fn loads_user() {}
"#;
        let detail = detail(raw_diff);
        let stack = discover(
            &detail,
            &RepoContext::empty(),
            &VirtualStackSizing::default(),
        )
        .unwrap()
        .unwrap();

        assert!(stack.layers.len() >= 2);
        assert!(stack.layers.last().unwrap().title.contains("Tests"));
        let assigned = stack
            .layers
            .iter()
            .flat_map(|layer| layer.atom_ids.iter())
            .collect::<Vec<_>>();
        assert_eq!(assigned.len(), stack.atoms.len());
    }

    fn detail(raw_diff: &str) -> PullRequestDetail {
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
            additions: 9,
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
            files: vec![
                PullRequestFile {
                    path: "src/model.rs".to_string(),
                    additions: 3,
                    deletions: 0,
                    change_type: "MODIFIED".to_string(),
                },
                PullRequestFile {
                    path: "src/service.rs".to_string(),
                    additions: 3,
                    deletions: 0,
                    change_type: "MODIFIED".to_string(),
                },
                PullRequestFile {
                    path: "tests/service_test.rs".to_string(),
                    additions: 2,
                    deletions: 0,
                    change_type: "MODIFIED".to_string(),
                },
            ],
            raw_diff: raw_diff.to_string(),
            parsed_diff: parse_unified_diff(raw_diff),
            data_completeness: PullRequestDataCompleteness::default(),
        }
    }
}
