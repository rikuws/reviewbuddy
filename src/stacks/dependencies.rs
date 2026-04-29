use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::model::{ChangeAtom, ChangeAtomId, ChangeRole, Confidence};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AtomDependency {
    pub from_atom_id: ChangeAtomId,
    pub to_atom_id: ChangeAtomId,
    pub kind: DependencyKind,
    pub confidence: Confidence,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub enum DependencyKind {
    SymbolReference,
    RoleOrdering,
    TestTarget,
    PathLocality,
}

pub fn build_atom_dependencies(atoms: &[ChangeAtom]) -> Vec<AtomDependency> {
    let mut dependencies =
        BTreeMap::<(ChangeAtomId, ChangeAtomId, DependencyKind), Confidence>::new();
    let mut definitions = BTreeMap::<String, Vec<&ChangeAtom>>::new();

    for atom in atoms {
        for symbol in &atom.defined_symbols {
            definitions
                .entry(normalize_symbol(symbol))
                .or_default()
                .push(atom);
        }
    }

    for atom in atoms {
        for referenced in &atom.referenced_symbols {
            let referenced = normalize_symbol(referenced);
            let Some(definers) = definitions.get(&referenced) else {
                continue;
            };
            for definer in definers {
                if definer.id == atom.id {
                    continue;
                }
                dependencies.insert(
                    (
                        definer.id.clone(),
                        atom.id.clone(),
                        DependencyKind::SymbolReference,
                    ),
                    Confidence::Medium,
                );
            }
        }
    }

    for left in atoms {
        for right in atoms {
            if left.id == right.id {
                continue;
            }

            if role_should_precede(left.role, right.role) {
                let confidence = if left.path == right.path {
                    Confidence::Medium
                } else {
                    Confidence::Low
                };
                dependencies
                    .entry((
                        left.id.clone(),
                        right.id.clone(),
                        DependencyKind::RoleOrdering,
                    ))
                    .or_insert(confidence);
            }

            if right.role == ChangeRole::Tests
                && left.role != ChangeRole::Tests
                && atoms_share_module(left.path.as_str(), right.path.as_str())
            {
                dependencies.insert(
                    (
                        left.id.clone(),
                        right.id.clone(),
                        DependencyKind::TestTarget,
                    ),
                    Confidence::Medium,
                );
            }
        }
    }

    dependencies
        .into_iter()
        .map(
            |((from_atom_id, to_atom_id, kind), confidence)| AtomDependency {
                from_atom_id,
                to_atom_id,
                kind,
                confidence,
            },
        )
        .collect()
}

pub fn strongly_connected_components(
    atom_ids: &[ChangeAtomId],
    dependencies: &[AtomDependency],
) -> Vec<Vec<ChangeAtomId>> {
    let mut graph = BTreeMap::<ChangeAtomId, Vec<ChangeAtomId>>::new();
    for atom_id in atom_ids {
        graph.entry(atom_id.clone()).or_default();
    }
    for dependency in dependencies {
        graph
            .entry(dependency.from_atom_id.clone())
            .or_default()
            .push(dependency.to_atom_id.clone());
    }

    let mut state = TarjanState::default();
    for atom_id in atom_ids {
        if !state.indices.contains_key(atom_id) {
            strong_connect(atom_id, &graph, &mut state);
        }
    }

    state.components
}

pub fn dependency_depths(
    atom_ids: &[ChangeAtomId],
    dependencies: &[AtomDependency],
) -> BTreeMap<ChangeAtomId, usize> {
    let mut incoming = BTreeMap::<ChangeAtomId, usize>::new();
    let mut outgoing = BTreeMap::<ChangeAtomId, Vec<ChangeAtomId>>::new();

    for atom_id in atom_ids {
        incoming.entry(atom_id.clone()).or_insert(0);
        outgoing.entry(atom_id.clone()).or_default();
    }
    for dependency in dependencies {
        outgoing
            .entry(dependency.from_atom_id.clone())
            .or_default()
            .push(dependency.to_atom_id.clone());
        *incoming.entry(dependency.to_atom_id.clone()).or_insert(0) += 1;
    }

    let mut ready = incoming
        .iter()
        .filter_map(|(atom_id, count)| (*count == 0).then_some(atom_id.clone()))
        .collect::<Vec<_>>();
    let mut depths = atom_ids
        .iter()
        .map(|atom_id| (atom_id.clone(), 0usize))
        .collect::<BTreeMap<_, _>>();

    while let Some(atom_id) = ready.pop() {
        let current_depth = depths.get(&atom_id).copied().unwrap_or_default();
        for child in outgoing.get(&atom_id).into_iter().flatten() {
            let child_depth = depths.entry(child.clone()).or_default();
            *child_depth = (*child_depth).max(current_depth + 1);
            if let Some(count) = incoming.get_mut(child) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    ready.push(child.clone());
                }
            }
        }
    }

    depths
}

#[derive(Default)]
struct TarjanState {
    index: usize,
    stack: Vec<ChangeAtomId>,
    on_stack: BTreeSet<ChangeAtomId>,
    indices: BTreeMap<ChangeAtomId, usize>,
    lowlinks: BTreeMap<ChangeAtomId, usize>,
    components: Vec<Vec<ChangeAtomId>>,
}

fn strong_connect(
    atom_id: &ChangeAtomId,
    graph: &BTreeMap<ChangeAtomId, Vec<ChangeAtomId>>,
    state: &mut TarjanState,
) {
    state.indices.insert(atom_id.clone(), state.index);
    state.lowlinks.insert(atom_id.clone(), state.index);
    state.index += 1;
    state.stack.push(atom_id.clone());
    state.on_stack.insert(atom_id.clone());

    for next in graph.get(atom_id).into_iter().flatten() {
        if !state.indices.contains_key(next) {
            strong_connect(next, graph, state);
            let low = state.lowlinks[atom_id].min(state.lowlinks[next]);
            state.lowlinks.insert(atom_id.clone(), low);
        } else if state.on_stack.contains(next) {
            let low = state.lowlinks[atom_id].min(state.indices[next]);
            state.lowlinks.insert(atom_id.clone(), low);
        }
    }

    if state.lowlinks[atom_id] == state.indices[atom_id] {
        let mut component = Vec::new();
        while let Some(member) = state.stack.pop() {
            state.on_stack.remove(&member);
            component.push(member.clone());
            if member == *atom_id {
                break;
            }
        }
        component.sort();
        state.components.push(component);
    }
}

fn role_should_precede(left: ChangeRole, right: ChangeRole) -> bool {
    use ChangeRole::*;
    matches!(
        (left, right),
        (Foundation, CoreLogic)
            | (Foundation, Integration)
            | (Foundation, Presentation)
            | (Foundation, Tests)
            | (Config, CoreLogic)
            | (Config, Integration)
            | (CoreLogic, Integration)
            | (CoreLogic, Presentation)
            | (CoreLogic, Tests)
            | (Integration, Presentation)
            | (Integration, Tests)
            | (Presentation, Tests)
    )
}

fn atoms_share_module(left: &str, right: &str) -> bool {
    let left = module_prefix(left);
    let right = module_prefix(right);
    !left.is_empty() && left == right
}

fn module_prefix(path: &str) -> String {
    path.rsplit_once('/')
        .map(|(prefix, _)| prefix.to_string())
        .unwrap_or_default()
}

fn normalize_symbol(symbol: &str) -> String {
    symbol
        .trim()
        .trim_start_matches("crate::")
        .trim_start_matches("self::")
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::{strongly_connected_components, AtomDependency, DependencyKind};
    use crate::stacks::model::Confidence;

    #[test]
    fn collapses_dependency_cycles_into_one_component() {
        let atom_ids = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let dependencies = vec![
            AtomDependency {
                from_atom_id: "a".to_string(),
                to_atom_id: "b".to_string(),
                kind: DependencyKind::SymbolReference,
                confidence: Confidence::Medium,
            },
            AtomDependency {
                from_atom_id: "b".to_string(),
                to_atom_id: "a".to_string(),
                kind: DependencyKind::SymbolReference,
                confidence: Confidence::Medium,
            },
        ];

        let components = strongly_connected_components(&atom_ids, &dependencies);

        assert!(components
            .iter()
            .any(|component| component == &vec!["a".to_string(), "b".to_string()]));
        assert!(components
            .iter()
            .any(|component| component == &vec!["c".to_string()]));
    }
}
