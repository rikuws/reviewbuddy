use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::model::{ChangeAtom, ChangeAtomId, ChangeAtomSource, ChangeRole, Confidence};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AiStackPlanStrategy {
    CommitVirtualStack,
    SemanticVirtualStack,
    HybridVirtualStack,
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
    pub summary: String,
    pub rationale: String,
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
        return Err(AiStackPlanValidationError::new(
            "AI stack plan did not return any layers.",
        ));
    }

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
    for (layer_index, layer) in plan.layers.iter().enumerate() {
        if layer.atom_ids.is_empty() {
            return Err(AiStackPlanValidationError::new(format!(
                "AI stack plan returned empty layer {}.",
                layer_index + 1
            )));
        }

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

        for atom_id in &layer.atom_ids {
            validate_atom_id(atom_id, &known_ids)?;
            if !seen.insert(atom_id.clone()) {
                return Err(AiStackPlanValidationError::new(format!(
                    "AI stack plan assigned atom '{atom_id}' more than once."
                )));
            }
        }
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
        return Err(AiStackPlanValidationError::new(format!(
            "AI stack plan omitted {} atom{}.",
            missing.len(),
            if missing.len() == 1 { "" } else { "s" }
        )));
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

    Ok(ValidatedAiStackPlan { plan: plan.clone() })
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

fn atom_weight(atom: &ChangeAtom) -> usize {
    atom.additions.saturating_add(atom.deletions).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stacks::model::{LineRange, StackWarning};

    #[test]
    fn rejects_omitted_atoms() {
        let atoms = vec![
            atom("atom_1", ChangeRole::CoreLogic),
            atom("atom_2", ChangeRole::Tests),
        ];
        let plan = plan_with_layers(vec![vec!["atom_1"]], vec![]);

        let error =
            validate_ai_stack_plan(&plan, &atoms, 120).expect_err("missing atom should reject");
        assert!(error.message.contains("omitted"));
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

    fn plan_with_layers(layer_atom_ids: Vec<Vec<&str>>, manual_atom_ids: Vec<&str>) -> AiStackPlan {
        AiStackPlan {
            strategy: AiStackPlanStrategy::SemanticVirtualStack,
            confidence: Confidence::Medium,
            rationale: "Grouped by semantic review order.".to_string(),
            layers: layer_atom_ids
                .into_iter()
                .map(|atom_ids| AiStackPlanLayer {
                    title: "Layer".to_string(),
                    summary: "Summary".to_string(),
                    rationale: "Rationale".to_string(),
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
}
