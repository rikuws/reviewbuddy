use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::{
    dependencies::{build_atom_dependencies, DependencyKind},
    model::{ChangeAtom, ChangeAtomId, ChangeAtomSource, ChangeRole, Confidence},
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AiStackPlanStrategy {
    CommitVirtualStack,
    SemanticVirtualStack,
    HybridVirtualStack,
    DependencyChain,
    RefactorThenChange,
    MechanicalThenUse,
    VerticalFeatureSlices,
    RiskIsolation,
    ReviewerBoundary,
    FlatManualReview,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AiReviewPriority {
    StartHere,
    Normal,
    QuickPass,
    ManualReview,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct AiStackPlan {
    pub strategy: AiStackPlanStrategy,
    pub confidence: Confidence,
    pub rationale: String,
    pub layers: Vec<AiStackPlanLayer>,
    #[serde(default)]
    pub manual_review_atom_ids: Vec<ChangeAtomId>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct AiStackPlanLayer {
    pub title: String,
    #[serde(default)]
    pub review_question: String,
    pub summary: String,
    pub rationale: String,
    #[serde(default)]
    pub substantive_atom_ids: Vec<ChangeAtomId>,
    #[serde(default)]
    pub attached_noise_atom_ids: Vec<ChangeAtomId>,
    #[serde(default)]
    pub atom_ids: Vec<ChangeAtomId>,
    #[serde(default)]
    pub depends_on_layer_indexes: Vec<usize>,
    pub confidence: Confidence,
    pub review_priority: AiReviewPriority,
}

#[derive(Clone, Debug)]
pub struct ValidatedAiStackPlan {
    pub plan: AiStackPlan,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AiStackPlanValidationError {
    pub message: String,
}

impl AiStackPlanValidationError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for AiStackPlanValidationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for AiStackPlanValidationError {}

pub fn validate_ai_stack_plan(
    plan: &AiStackPlan,
    atoms: &[ChangeAtom],
    total_changed_lines: usize,
) -> Result<ValidatedAiStackPlan, AiStackPlanValidationError> {
    let atoms_by_id = atoms
        .iter()
        .map(|atom| (atom.id.clone(), atom))
        .collect::<BTreeMap<_, _>>();
    let known_ids = atoms
        .iter()
        .map(|atom| atom.id.clone())
        .collect::<BTreeSet<_>>();
    if known_ids.is_empty() {
        return Err(AiStackPlanValidationError::new(
            "AI stack plan cannot be validated without input atoms.",
        ));
    }

    if plan.layers.is_empty() {
        let known_count = known_ids.len();
        let manual_count = plan
            .manual_review_atom_ids
            .iter()
            .collect::<BTreeSet<_>>()
            .len();
        let all_in_manual_review = manual_count >= known_count
            && known_ids
                .iter()
                .all(|id| plan.manual_review_atom_ids.iter().any(|m| m == id));
        if !(matches!(plan.strategy, AiStackPlanStrategy::FlatManualReview) && all_in_manual_review)
        {
            return Err(AiStackPlanValidationError::new(
                "AI stack plan did not return any layers.",
            ));
        }
    }

    let mut plan = plan.clone();
    normalize_plan_layer_atom_ids(&mut plan, &atoms_by_id);

    if total_changed_lines > 800
        && plan.layers.len() <= 2
        && plan.strategy != AiStackPlanStrategy::FlatManualReview
        && !is_homogeneous_change(atoms)
    {
        return Err(AiStackPlanValidationError::new(
            "AI stack plan returned too few layers for a large mixed-purpose PR.",
        ));
    }

    let mut seen = BTreeSet::<ChangeAtomId>::new();
    let mut layer_index_by_atom = BTreeMap::<ChangeAtomId, usize>::new();
    let mut substantive_counts_by_layer = Vec::<usize>::new();
    for (layer_index, layer) in plan.layers.iter().enumerate() {
        if layer.atom_ids.is_empty() {
            return Err(AiStackPlanValidationError::new(format!(
                "AI stack plan returned empty layer {}.",
                layer_index + 1
            )));
        }

        validate_layer_text(layer, layer_index)?;

        for dep_index in &layer.depends_on_layer_indexes {
            if *dep_index >= plan.layers.len() {
                return Err(AiStackPlanValidationError::new(format!(
                    "AI stack plan layer {} depends on missing layer index {}.",
                    layer_index + 1,
                    dep_index
                )));
            }
            if *dep_index >= layer_index {
                return Err(AiStackPlanValidationError::new(format!(
                    "AI stack plan layer {} depends on a later layer index {}.",
                    layer_index + 1,
                    dep_index
                )));
            }
        }

        let mut layer_seen = BTreeSet::<ChangeAtomId>::new();
        for atom_id in &layer.atom_ids {
            validate_atom_id(atom_id, &known_ids)?;
            if !layer_seen.insert(atom_id.clone()) {
                return Err(AiStackPlanValidationError::new(format!(
                    "AI stack plan assigned atom '{atom_id}' more than once in layer {}.",
                    layer_index + 1
                )));
            }
            if !seen.insert(atom_id.clone()) {
                return Err(AiStackPlanValidationError::new(format!(
                    "AI stack plan assigned atom '{atom_id}' more than once."
                )));
            }
            layer_index_by_atom.insert(atom_id.clone(), layer_index);
        }

        let layer_atoms = layer
            .atom_ids
            .iter()
            .filter_map(|atom_id| atoms_by_id.get(atom_id).copied())
            .collect::<Vec<_>>();
        validate_layer_has_substantive_center(layer, layer_index, &layer_atoms)?;
        validate_import_and_noise_ratio(layer, layer_index, &layer_atoms)?;
        validate_orphan_tests_layer(layer, layer_index, &layer_atoms, atoms)?;
        substantive_counts_by_layer.push(
            layer_atoms
                .iter()
                .filter(|atom| atom_is_substantive(atom))
                .count(),
        );
    }

    for atom_id in &plan.manual_review_atom_ids {
        validate_atom_id(atom_id, &known_ids)?;
        if !seen.insert(atom_id.clone()) {
            return Err(AiStackPlanValidationError::new(format!(
                "AI stack plan assigned atom '{atom_id}' more than once."
            )));
        }
    }

    let missing = known_ids
        .difference(&seen)
        .cloned()
        .collect::<Vec<ChangeAtomId>>();
    if !missing.is_empty() {
        plan.warnings.push(format!(
            "AI stack plan omitted {} atom{}; auto-routed to manual_review_atom_ids.",
            missing.len(),
            if missing.len() == 1 { "" } else { "s" }
        ));
        for atom_id in &missing {
            plan.manual_review_atom_ids.push(atom_id.clone());
            seen.insert(atom_id.clone());
        }
    }

    let manual_ids = plan
        .manual_review_atom_ids
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    if let Some(atom) = atoms
        .iter()
        .find(|atom| requires_manual_review(atom) && !manual_ids.contains(&atom.id))
    {
        return Err(AiStackPlanValidationError::new(format!(
            "AI stack plan did not put manual-review atom '{}' in manual_review_atom_ids.",
            atom.id
        )));
    }

    validate_dependency_order(&layer_index_by_atom, atoms)?;
    validate_no_tail_dump(&plan, atoms, &substantive_counts_by_layer, &atoms_by_id)?;

    Ok(ValidatedAiStackPlan { plan })
}

fn normalize_plan_layer_atom_ids(
    plan: &mut AiStackPlan,
    atoms_by_id: &BTreeMap<ChangeAtomId, &ChangeAtom>,
) {
    for layer in &mut plan.layers {
        let mut ids = Vec::<ChangeAtomId>::new();
        if layer.atom_ids.is_empty() {
            ids.extend(layer.substantive_atom_ids.iter().cloned());
            ids.extend(layer.attached_noise_atom_ids.iter().cloned());
        } else {
            ids.extend(layer.atom_ids.iter().cloned());
            ids.extend(layer.substantive_atom_ids.iter().cloned());
            ids.extend(layer.attached_noise_atom_ids.iter().cloned());
        }

        let mut seen = BTreeSet::<ChangeAtomId>::new();
        layer.atom_ids = ids
            .into_iter()
            .filter(|atom_id| seen.insert(atom_id.clone()))
            .collect();

        if layer.substantive_atom_ids.is_empty() {
            layer.substantive_atom_ids = layer
                .atom_ids
                .iter()
                .filter_map(|atom_id| atoms_by_id.get(atom_id).copied())
                .filter(|atom| atom_is_substantive(atom))
                .map(|atom| atom.id.clone())
                .collect();
        }

        if layer.attached_noise_atom_ids.is_empty() {
            layer.attached_noise_atom_ids = layer
                .atom_ids
                .iter()
                .filter_map(|atom_id| atoms_by_id.get(atom_id).copied())
                .filter(|atom| atom_noise_kind(atom).is_some())
                .map(|atom| atom.id.clone())
                .collect();
        }
    }
}

fn validate_layer_text(
    layer: &AiStackPlanLayer,
    layer_index: usize,
) -> Result<(), AiStackPlanValidationError> {
    if is_forbidden_generic_title(&layer.title) {
        return Err(AiStackPlanValidationError::new(format!(
            "AI stack plan layer {} has a generic title '{}'.",
            layer_index + 1,
            layer.title
        )));
    }

    if !layer.review_question.trim().is_empty()
        && is_generic_review_question(&layer.review_question)
    {
        return Err(AiStackPlanValidationError::new(format!(
            "AI stack plan layer {} has a generic review question.",
            layer_index + 1
        )));
    }

    Ok(())
}

fn validate_layer_has_substantive_center(
    layer: &AiStackPlanLayer,
    layer_index: usize,
    layer_atoms: &[&ChangeAtom],
) -> Result<(), AiStackPlanValidationError> {
    if layer_atoms.iter().any(|atom| atom_is_substantive(atom)) {
        return Ok(());
    }

    if is_allowed_mechanical_layer(layer, layer_atoms) {
        return Ok(());
    }

    Err(AiStackPlanValidationError::new(format!(
        "AI stack plan layer {} has no substantive center.",
        layer_index + 1
    )))
}

fn validate_import_and_noise_ratio(
    layer: &AiStackPlanLayer,
    layer_index: usize,
    layer_atoms: &[&ChangeAtom],
) -> Result<(), AiStackPlanValidationError> {
    let changed_lines = layer_atoms
        .iter()
        .map(|atom| atom_weight(atom))
        .sum::<usize>()
        .max(1);
    let noise_lines = layer_atoms
        .iter()
        .filter(|atom| atom_noise_kind(atom).is_some())
        .map(|atom| atom_weight(atom))
        .sum::<usize>();
    let import_lines = layer_atoms
        .iter()
        .filter(|atom| atom_noise_kind(atom) == Some("imports"))
        .map(|atom| atom_weight(atom))
        .sum::<usize>();

    if is_allowed_mechanical_layer(layer, layer_atoms) {
        return Ok(());
    }

    if import_lines * 100 > changed_lines * 70 {
        return Err(AiStackPlanValidationError::new(format!(
            "AI stack plan layer {} is mostly import noise.",
            layer_index + 1
        )));
    }

    if noise_lines * 100 > changed_lines * 70
        && !layer_atoms.iter().any(|atom| atom_is_substantive(atom))
    {
        return Err(AiStackPlanValidationError::new(format!(
            "AI stack plan layer {} is mostly non-substantive noise.",
            layer_index + 1
        )));
    }

    Ok(())
}

fn validate_orphan_tests_layer(
    layer: &AiStackPlanLayer,
    layer_index: usize,
    layer_atoms: &[&ChangeAtom],
    all_atoms: &[ChangeAtom],
) -> Result<(), AiStackPlanValidationError> {
    if layer_atoms.is_empty()
        || !layer_atoms
            .iter()
            .all(|atom| atom.role == ChangeRole::Tests)
        || all_atoms.iter().all(|atom| atom.role == ChangeRole::Tests)
    {
        return Ok(());
    }

    if title_or_question_mentions_any(
        layer,
        &[
            "integration",
            "e2e",
            "end-to-end",
            "acceptance",
            "test infrastructure",
            "fixture",
            "harness",
            "regression coverage",
        ],
    ) {
        return Ok(());
    }

    let direct_test_count = layer_atoms
        .iter()
        .filter(|test_atom| test_atom_targets_changed_code(test_atom, all_atoms))
        .count();
    if direct_test_count * 2 >= layer_atoms.len().max(1) {
        return Err(AiStackPlanValidationError::new(format!(
            "AI stack plan layer {} separates direct tests from the behavior they validate.",
            layer_index + 1
        )));
    }

    if is_generic_test_layer_title(&layer.title) || layer.review_question.trim().is_empty() {
        return Err(AiStackPlanValidationError::new(format!(
            "AI stack plan layer {} separates generic tests from the behavior they validate.",
            layer_index + 1
        )));
    }

    Ok(())
}

fn validate_dependency_order(
    layer_index_by_atom: &BTreeMap<ChangeAtomId, usize>,
    atoms: &[ChangeAtom],
) -> Result<(), AiStackPlanValidationError> {
    for dependency in build_atom_dependencies(atoms) {
        if matches!(dependency.kind, DependencyKind::PathLocality) {
            continue;
        }

        let Some(from_index) = layer_index_by_atom.get(&dependency.from_atom_id) else {
            continue;
        };
        let Some(to_index) = layer_index_by_atom.get(&dependency.to_atom_id) else {
            continue;
        };

        if from_index > to_index {
            if dependency_is_hard_ordering_constraint(&dependency) {
                return Err(AiStackPlanValidationError::new(format!(
                    "AI stack plan orders atom '{}' after dependent atom '{}'.",
                    dependency.from_atom_id, dependency.to_atom_id
                )));
            }
        }
    }

    Ok(())
}

fn dependency_is_hard_ordering_constraint(
    dependency: &super::dependencies::AtomDependency,
) -> bool {
    match dependency.kind {
        DependencyKind::PathLocality => false,
        DependencyKind::TestTarget => true,
        DependencyKind::RoleOrdering => dependency.confidence != Confidence::Low,
        DependencyKind::SymbolReference => false,
    }
}

fn validate_no_tail_dump(
    plan: &AiStackPlan,
    atoms: &[ChangeAtom],
    substantive_counts_by_layer: &[usize],
    atoms_by_id: &BTreeMap<ChangeAtomId, &ChangeAtom>,
) -> Result<(), AiStackPlanValidationError> {
    if plan.layers.len() <= 1 || is_homogeneous_change(atoms) {
        return Ok(());
    }

    let total_substantive = substantive_counts_by_layer.iter().sum::<usize>();
    let Some(final_count) = substantive_counts_by_layer.last().copied() else {
        return Ok(());
    };

    if total_substantive == 0 || final_count < 3 {
        return Ok(());
    }

    let final_layer = plan.layers.last().expect("non-empty layers");
    let final_atoms = final_layer
        .atom_ids
        .iter()
        .filter_map(|atom_id| atoms_by_id.get(atom_id).copied())
        .filter(|atom| atom_is_substantive(atom))
        .collect::<Vec<_>>();
    let final_concern_count = concern_count(&final_atoms);

    if final_count * 100 > total_substantive * 40 || final_concern_count > 2 {
        return Err(AiStackPlanValidationError::new(
            "AI stack plan final layer looks like a tail dump.",
        ));
    }

    Ok(())
}

fn validate_atom_id(
    atom_id: &str,
    known_ids: &BTreeSet<ChangeAtomId>,
) -> Result<(), AiStackPlanValidationError> {
    if known_ids.contains(atom_id) {
        Ok(())
    } else {
        Err(AiStackPlanValidationError::new(format!(
            "AI stack plan referenced unknown atom id '{atom_id}'."
        )))
    }
}

pub fn is_homogeneous_change(atoms: &[ChangeAtom]) -> bool {
    if atoms.is_empty() {
        return false;
    }

    if atoms.iter().all(requires_manual_review) {
        return true;
    }

    let total_weight = atoms
        .iter()
        .map(atom_weight)
        .sum::<usize>()
        .max(atoms.len());
    let mut by_role = BTreeMap::<ChangeRole, usize>::new();
    let mut by_semantic_kind = BTreeMap::<String, usize>::new();

    for atom in atoms {
        let weight = atom_weight(atom);
        *by_role.entry(atom.role).or_default() += weight;
        if let Some(kind) = atom.semantic_kind.as_deref() {
            *by_semantic_kind.entry(kind.to_string()).or_default() += weight;
        }
    }

    let dominant_role = by_role.values().copied().max().unwrap_or_default();
    if dominant_role * 100 < total_weight * 85 {
        return false;
    }

    let dominant_role_kind = by_role
        .into_iter()
        .max_by_key(|(_, weight)| *weight)
        .map(|(role, _)| role)
        .unwrap_or(ChangeRole::Unknown);
    if matches!(
        dominant_role_kind,
        ChangeRole::Docs | ChangeRole::Tests | ChangeRole::Generated
    ) {
        return true;
    }

    by_semantic_kind
        .values()
        .copied()
        .max()
        .map(|dominant_kind| dominant_kind * 100 >= total_weight * 75)
        .unwrap_or(false)
}

pub fn requires_manual_review(atom: &ChangeAtom) -> bool {
    atom.role == ChangeRole::Generated
        || matches!(
            atom.source,
            ChangeAtomSource::GeneratedPlaceholder | ChangeAtomSource::BinaryPlaceholder
        )
        || atom
            .warnings
            .iter()
            .any(|warning| warning.code == "manual-review")
}

pub fn atom_is_substantive(atom: &ChangeAtom) -> bool {
    !requires_manual_review(atom) && atom_noise_kind(atom).is_none()
}

pub fn atom_noise_kind(atom: &ChangeAtom) -> Option<&'static str> {
    let semantic_kind = atom.semantic_kind.as_deref()?.to_ascii_lowercase();
    match semantic_kind.as_str() {
        "imports" => Some("imports"),
        "formatting" => Some("formatting"),
        "comments" => Some("comments"),
        _ => None,
    }
}

fn atom_weight(atom: &ChangeAtom) -> usize {
    atom.additions.saturating_add(atom.deletions).max(1)
}

fn is_allowed_mechanical_layer(layer: &AiStackPlanLayer, atoms: &[&ChangeAtom]) -> bool {
    if atoms.is_empty() {
        return false;
    }

    let all_noise = atoms.iter().all(|atom| atom_noise_kind(atom).is_some());
    if !all_noise {
        return false;
    }

    title_or_question_mentions_any(
        layer,
        &[
            "mechanical",
            "generated",
            "format",
            "formatting",
            "comment",
            "comments",
        ],
    ) && !atoms
        .iter()
        .any(|atom| atom_noise_kind(atom) == Some("imports"))
}

fn concern_count(atoms: &[&ChangeAtom]) -> usize {
    atoms
        .iter()
        .map(|atom| {
            (
                atom.role,
                atom.semantic_kind.clone().unwrap_or_default(),
                directory_label(atom.path.as_str()),
            )
        })
        .collect::<BTreeSet<_>>()
        .len()
}

fn test_atom_targets_changed_code(test_atom: &ChangeAtom, all_atoms: &[ChangeAtom]) -> bool {
    all_atoms
        .iter()
        .filter(|atom| atom.role != ChangeRole::Tests && atom_is_substantive(atom))
        .any(|atom| {
            symbols_overlap(test_atom, atom)
                || normalized_module_stem(test_atom.path.as_str())
                    == normalized_module_stem(atom.path.as_str())
        })
}

fn symbols_overlap(test_atom: &ChangeAtom, changed_atom: &ChangeAtom) -> bool {
    if test_atom.referenced_symbols.is_empty() || changed_atom.defined_symbols.is_empty() {
        return false;
    }

    let referenced = test_atom
        .referenced_symbols
        .iter()
        .map(|symbol| normalize_symbol(symbol))
        .collect::<BTreeSet<_>>();
    changed_atom
        .defined_symbols
        .iter()
        .map(|symbol| normalize_symbol(symbol))
        .any(|symbol| referenced.contains(&symbol))
}

fn normalize_symbol(symbol: &str) -> String {
    symbol
        .trim()
        .trim_start_matches("crate::")
        .trim_start_matches("self::")
        .to_ascii_lowercase()
}

fn normalized_module_stem(path: &str) -> String {
    let file_name = path.rsplit('/').next().unwrap_or(path);
    file_name
        .split('.')
        .next()
        .unwrap_or(file_name)
        .trim_end_matches("_test")
        .trim_end_matches("_tests")
        .trim_end_matches("_spec")
        .trim_end_matches("_specs")
        .trim_end_matches(".test")
        .trim_end_matches(".spec")
        .to_ascii_lowercase()
}

fn directory_label(path: &str) -> String {
    path.rsplit_once('/')
        .map(|(dir, _)| dir.to_string())
        .unwrap_or_default()
}

fn title_or_question_mentions_any(layer: &AiStackPlanLayer, needles: &[&str]) -> bool {
    let haystack = format!(
        "{} {} {} {}",
        layer.title, layer.review_question, layer.summary, layer.rationale
    )
    .to_ascii_lowercase();
    needles.iter().any(|needle| haystack.contains(needle))
}

fn is_forbidden_generic_title(title: &str) -> bool {
    let normalized = normalize_layer_text(title);
    matches!(
        normalized.as_str(),
        "misc"
            | "misc changes"
            | "remaining"
            | "remaining changes"
            | "other"
            | "other files"
            | "cleanup"
            | "clean up"
            | "imports"
            | "update imports"
            | "updates"
            | "update files"
            | "add changes"
            | "implementation"
            | "final changes"
            | "everything else"
    )
}

fn is_generic_test_layer_title(title: &str) -> bool {
    matches!(
        normalize_layer_text(title).as_str(),
        "tests" | "add tests" | "update tests" | "test coverage" | "coverage"
    )
}

fn is_generic_review_question(question: &str) -> bool {
    matches!(
        normalize_layer_text(question).as_str(),
        "is this okay"
            | "does this look good"
            | "review this"
            | "are these changes correct"
            | "does this layer look correct"
    )
}

fn normalize_layer_text(value: &str) -> String {
    value
        .trim()
        .trim_matches(|ch: char| !ch.is_ascii_alphanumeric())
        .to_ascii_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stacks::model::{LineRange, StackWarning};

    #[test]
    fn auto_routes_omitted_atoms_to_manual_review() {
        let atoms = vec![
            atom("atom_1", ChangeRole::CoreLogic),
            atom("atom_2", ChangeRole::Tests),
        ];
        let plan = plan_with_layers(vec![vec!["atom_1"]], vec![]);

        let validated = validate_ai_stack_plan(&plan, &atoms, 120)
            .expect("missing atoms should be auto-routed, not rejected");
        assert!(validated
            .plan
            .manual_review_atom_ids
            .iter()
            .any(|id| id == "atom_2"));
        assert!(validated
            .plan
            .warnings
            .iter()
            .any(|warning| warning.contains("omitted")
                && warning.contains("manual_review_atom_ids")));
    }

    #[test]
    fn rejects_invented_atom_ids() {
        let atoms = vec![atom("atom_1", ChangeRole::CoreLogic)];
        let plan = plan_with_layers(vec![vec!["atom_1", "atom_fake"]], vec![]);

        let error =
            validate_ai_stack_plan(&plan, &atoms, 120).expect_err("unknown atom should reject");
        assert!(error.message.contains("unknown atom id"));
    }

    #[test]
    fn rejects_large_mixed_pr_with_two_layers() {
        let atoms = vec![
            atom("atom_1", ChangeRole::Foundation),
            atom("atom_2", ChangeRole::CoreLogic),
            atom("atom_3", ChangeRole::Integration),
            atom("atom_4", ChangeRole::Tests),
        ];
        let plan = plan_with_layers(
            vec![vec!["atom_1", "atom_2"], vec!["atom_3", "atom_4"]],
            vec![],
        );

        let error = validate_ai_stack_plan(&plan, &atoms, 2_500)
            .expect_err("large mixed low-layer plan should reject");
        assert!(error.message.contains("too few layers"));
    }

    #[test]
    fn accepts_homogeneous_large_low_layer_plan() {
        let mut atoms = vec![
            atom("atom_1", ChangeRole::Docs),
            atom("atom_2", ChangeRole::Docs),
        ];
        atoms[0].additions = 700;
        atoms[1].additions = 600;
        let plan = plan_with_layers(vec![vec!["atom_1", "atom_2"]], vec![]);

        validate_ai_stack_plan(&plan, &atoms, 1_300).expect("docs-only plan should pass");
    }

    #[test]
    fn generated_atoms_must_be_manual_review() {
        let mut generated = atom("atom_generated", ChangeRole::Generated);
        generated.source = ChangeAtomSource::GeneratedPlaceholder;
        generated.warnings = vec![StackWarning::new("manual-review", "Generated file.")];
        let atoms = vec![atom("atom_1", ChangeRole::CoreLogic), generated];
        let plan = plan_with_layers(vec![vec!["atom_1", "atom_generated"]], vec![]);

        let error = validate_ai_stack_plan(&plan, &atoms, 200)
            .expect_err("generated atom should require manual review");
        assert!(error.message.contains("manual-review atom"));
    }

    #[test]
    fn rejects_import_only_layers() {
        let atoms = vec![
            noise_atom("atom_imports", "imports"),
            atom("atom_core", ChangeRole::CoreLogic),
        ];
        let mut plan = plan_with_layers(vec![vec!["atom_imports"], vec!["atom_core"]], vec![]);
        plan.layers[0].title = "Refresh module imports".to_string();

        let error = validate_ai_stack_plan(&plan, &atoms, 140)
            .expect_err("import-only layer should reject");
        assert!(error.message.contains("no substantive center"));
    }

    #[test]
    fn accepts_noise_attached_to_substantive_layer() {
        let atoms = vec![
            noise_atom("atom_imports", "imports"),
            atom("atom_core", ChangeRole::CoreLogic),
        ];
        let mut plan = plan_with_layers(vec![vec!["atom_core", "atom_imports"]], vec![]);
        plan.layers[0].title = "Build core behavior with required imports".to_string();

        validate_ai_stack_plan(&plan, &atoms, 140).expect("attached import noise should pass");
    }

    #[test]
    fn accepts_substantive_and_attached_noise_schema_without_atom_ids() {
        let atoms = vec![
            noise_atom("atom_imports", "imports"),
            atom("atom_core", ChangeRole::CoreLogic),
        ];
        let plan = AiStackPlan {
            strategy: AiStackPlanStrategy::DependencyChain,
            confidence: Confidence::Medium,
            rationale: "Repair candidate layers.".to_string(),
            layers: vec![AiStackPlanLayer {
                title: "Build core behavior with required imports".to_string(),
                review_question: "Does the core behavior work with its required imports?"
                    .to_string(),
                summary: "Core behavior plus attached import noise.".to_string(),
                rationale: "The import atom supports the substantive core atom.".to_string(),
                substantive_atom_ids: vec!["atom_core".to_string()],
                attached_noise_atom_ids: vec!["atom_imports".to_string()],
                atom_ids: Vec::new(),
                depends_on_layer_indexes: Vec::new(),
                confidence: Confidence::Medium,
                review_priority: AiReviewPriority::Normal,
            }],
            manual_review_atom_ids: Vec::new(),
            warnings: Vec::new(),
        };

        let validated = validate_ai_stack_plan(&plan, &atoms, 140)
            .expect("new stack schema should normalize atom ids");
        assert_eq!(
            validated.plan.layers[0].atom_ids,
            vec!["atom_core".to_string(), "atom_imports".to_string()]
        );
    }

    #[test]
    fn rejects_dependency_inversions() {
        let mut foundation = atom("atom_model", ChangeRole::Foundation);
        foundation.path = "src/user.rs".to_string();
        foundation.defined_symbols = vec!["User".to_string()];
        let mut core = atom("atom_service", ChangeRole::CoreLogic);
        core.path = "src/user.rs".to_string();
        core.referenced_symbols = vec!["User".to_string()];
        let atoms = vec![foundation, core];
        let mut plan = plan_with_layers(vec![vec!["atom_service"], vec!["atom_model"]], vec![]);
        plan.layers[0].title = "Build service behavior".to_string();
        plan.layers[1].title = "Introduce user model".to_string();

        let error = validate_ai_stack_plan(&plan, &atoms, 220)
            .expect_err("consumer before provider should reject");
        assert!(error.message.contains("after dependent atom"));
    }

    #[test]
    fn rejects_tail_dump_final_layers() {
        let atoms = vec![
            atom("atom_foundation", ChangeRole::Foundation),
            atom("atom_core_1", ChangeRole::CoreLogic),
            atom("atom_core_2", ChangeRole::Integration),
            atom("atom_core_3", ChangeRole::Presentation),
        ];
        let mut plan = plan_with_layers(
            vec![
                vec!["atom_foundation"],
                vec!["atom_core_1", "atom_core_2", "atom_core_3"],
            ],
            vec![],
        );
        plan.layers[0].title = "Introduce feature model".to_string();
        plan.layers[1].title = "Implement remaining feature behavior".to_string();

        let error =
            validate_ai_stack_plan(&plan, &atoms, 500).expect_err("tail dump should reject");
        assert!(error.message.contains("tail dump"));
    }

    #[test]
    fn rejects_generic_tests_layers() {
        let atoms = vec![
            atom("atom_core", ChangeRole::CoreLogic),
            atom("atom_tests", ChangeRole::Tests),
        ];
        let mut plan = plan_with_layers(vec![vec!["atom_core"], vec!["atom_tests"]], vec![]);
        plan.layers[0].title = "Build core behavior".to_string();
        plan.layers[1].title = "Tests".to_string();

        let error = validate_ai_stack_plan(&plan, &atoms, 220)
            .expect_err("generic tests layer should reject");
        assert!(error.message.contains("generic title") || error.message.contains("tests"));
    }

    #[test]
    fn rejects_direct_unit_tests_split_from_behavior() {
        let mut core = atom("atom_service", ChangeRole::CoreLogic);
        core.path = "src/service.rs".to_string();
        core.defined_symbols = vec!["load_user".to_string()];
        let mut tests = atom("atom_service_tests", ChangeRole::Tests);
        tests.path = "tests/service_test.rs".to_string();
        tests.referenced_symbols = vec!["load_user".to_string()];
        let atoms = vec![core, tests];
        let mut plan = plan_with_layers(
            vec![vec!["atom_service"], vec!["atom_service_tests"]],
            vec![],
        );
        plan.layers[0].title = "Build user loading behavior".to_string();
        plan.layers[1].title = "Validate service unit coverage".to_string();

        let error = validate_ai_stack_plan(&plan, &atoms, 220)
            .expect_err("direct unit tests should travel with behavior");
        assert!(error.message.contains("direct tests"));
    }

    #[test]
    fn rejects_misc_layer_titles() {
        let atoms = vec![atom("atom_core", ChangeRole::CoreLogic)];
        let mut plan = plan_with_layers(vec![vec!["atom_core"]], vec![]);
        plan.layers[0].title = "Remaining changes".to_string();

        let error =
            validate_ai_stack_plan(&plan, &atoms, 120).expect_err("misc title should reject");
        assert!(error.message.contains("generic title"));
    }

    fn plan_with_layers(layer_atom_ids: Vec<Vec<&str>>, manual_atom_ids: Vec<&str>) -> AiStackPlan {
        AiStackPlan {
            strategy: AiStackPlanStrategy::SemanticVirtualStack,
            confidence: Confidence::Medium,
            rationale: "Grouped by semantic review order.".to_string(),
            layers: layer_atom_ids
                .into_iter()
                .map(|atom_ids| AiStackPlanLayer {
                    title: "Layer".to_string(),
                    review_question: "Does this layer answer one review question?".to_string(),
                    summary: "Summary".to_string(),
                    rationale: "Rationale".to_string(),
                    substantive_atom_ids: Vec::new(),
                    attached_noise_atom_ids: Vec::new(),
                    atom_ids: atom_ids.into_iter().map(str::to_string).collect(),
                    depends_on_layer_indexes: Vec::new(),
                    confidence: Confidence::Medium,
                    review_priority: AiReviewPriority::Normal,
                })
                .collect(),
            manual_review_atom_ids: manual_atom_ids.into_iter().map(str::to_string).collect(),
            warnings: Vec::new(),
        }
    }

    fn atom(id: &str, role: ChangeRole) -> ChangeAtom {
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
            additions: 50,
            deletions: 10,
            patch_hash: format!("hash-{id}"),
            risk_score: 20,
            review_thread_ids: Vec::new(),
            warnings: Vec::new(),
        }
    }

    fn noise_atom(id: &str, semantic_kind: &str) -> ChangeAtom {
        let mut atom = atom(id, ChangeRole::CoreLogic);
        atom.semantic_kind = Some(semantic_kind.to_string());
        atom.defined_symbols = Vec::new();
        atom.referenced_symbols = Vec::new();
        atom.additions = 6;
        atom.deletions = 2;
        atom
    }
}
