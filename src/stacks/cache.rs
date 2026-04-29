use crate::{cache::CacheStore, code_tour::CodeTourProvider};

use super::model::{stack_now_ms, ChangeAtom, ReviewStack, StackReviewProgress};

const AI_REVIEW_STACK_CACHE_PREFIX: &str = "ai-review-stack-v1";
const STACK_PROGRESS_CACHE_PREFIX: &str = "stack-review-progress-v1";

pub fn ai_review_stack_cache_key(
    repository: &str,
    pr_number: i64,
    provider: CodeTourProvider,
    code_version_key: &str,
) -> String {
    format!(
        "{AI_REVIEW_STACK_CACHE_PREFIX}:{}#{}:{}:{}",
        repository,
        pr_number,
        provider.slug(),
        code_version_key
    )
}

pub fn load_ai_review_stack(
    cache: &CacheStore,
    repository: &str,
    pr_number: i64,
    provider: CodeTourProvider,
    code_version_key: &str,
) -> Result<Option<ReviewStack>, String> {
    let key = ai_review_stack_cache_key(repository, pr_number, provider, code_version_key);
    Ok(cache
        .get::<ReviewStack>(&key)?
        .map(|document| document.value))
}

pub fn save_ai_review_stack(
    cache: &CacheStore,
    stack: &ReviewStack,
    provider: CodeTourProvider,
    code_version_key: &str,
) -> Result<(), String> {
    let key = ai_review_stack_cache_key(
        &stack.repository,
        stack.selected_pr_number,
        provider,
        code_version_key,
    );
    cache.put(&key, stack, stack_now_ms())
}

pub fn stack_progress_cache_key(repository: &str, pr_number: i64, stack_id: &str) -> String {
    format!("{STACK_PROGRESS_CACHE_PREFIX}:{repository}#{pr_number}:{stack_id}")
}

pub fn load_stack_progress(
    cache: &CacheStore,
    repository: &str,
    pr_number: i64,
    stack_id: &str,
) -> Result<Option<StackReviewProgress>, String> {
    let key = stack_progress_cache_key(repository, pr_number, stack_id);
    Ok(cache
        .get::<StackReviewProgress>(&key)?
        .map(|document| document.value))
}

pub fn save_stack_progress(
    cache: &CacheStore,
    progress: &StackReviewProgress,
) -> Result<(), String> {
    let key =
        stack_progress_cache_key(&progress.repository, progress.pr_number, &progress.stack_id);
    cache.put(&key, progress, stack_now_ms())
}

pub fn remap_reviewed_atoms(
    previous: &StackReviewProgress,
    previous_stack: &ReviewStack,
    next_stack: &ReviewStack,
) -> StackReviewProgress {
    let previous_hashes = previous
        .reviewed_atom_ids
        .iter()
        .filter_map(|atom_id| previous_stack.atom(atom_id))
        .map(atom_identity)
        .collect::<std::collections::BTreeSet<_>>();
    let reviewed_atom_ids = next_stack
        .atoms
        .iter()
        .filter(|atom| previous_hashes.contains(&atom_identity(atom)))
        .map(|atom| atom.id.clone())
        .collect::<Vec<_>>();
    let reviewed_atom_set = reviewed_atom_ids
        .iter()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    let reviewed_layer_ids = next_stack
        .layers
        .iter()
        .filter(|layer| {
            !layer.atom_ids.is_empty()
                && layer
                    .atom_ids
                    .iter()
                    .all(|atom_id| reviewed_atom_set.contains(atom_id))
        })
        .map(|layer| layer.id.clone())
        .collect::<Vec<_>>();

    StackReviewProgress {
        stack_id: next_stack.id.clone(),
        repository: next_stack.repository.clone(),
        pr_number: next_stack.selected_pr_number,
        reviewed_layer_ids,
        reviewed_atom_ids,
        current_layer_id: next_stack
            .selected_layer(previous.current_layer_id.as_deref())
            .map(|layer| layer.id.clone()),
        last_location: previous.last_location.clone(),
        updated_at_ms: stack_now_ms(),
    }
}

fn atom_identity(atom: &ChangeAtom) -> (String, String, String) {
    (
        atom.path.clone(),
        atom.symbol_name.clone().unwrap_or_default(),
        atom.patch_hash.clone(),
    )
}

#[cfg(test)]
mod tests {
    use super::{
        ai_review_stack_cache_key, load_ai_review_stack, remap_reviewed_atoms, save_ai_review_stack,
    };
    use crate::cache::CacheStore;
    use crate::code_tour::CodeTourProvider;
    use crate::stacks::model::{
        ChangeAtom, ChangeAtomSource, ChangeRole, Confidence, LayerMetrics, LayerReviewStatus,
        ReviewStack, ReviewStackLayer, StackKind, StackReviewProgress, StackSource,
    };

    #[test]
    fn ai_review_stack_cache_key_varies_by_provider_and_code_version() {
        let codex_head =
            ai_review_stack_cache_key("acme/repo", 42, CodeTourProvider::Codex, "head-abc");
        let copilot_head =
            ai_review_stack_cache_key("acme/repo", 42, CodeTourProvider::Copilot, "head-abc");
        let codex_next =
            ai_review_stack_cache_key("acme/repo", 42, CodeTourProvider::Codex, "head-def");

        assert_eq!(
            codex_head,
            ai_review_stack_cache_key("acme/repo", 42, CodeTourProvider::Codex, "head-abc",)
        );
        assert_ne!(codex_head, copilot_head);
        assert_ne!(codex_head, codex_next);
    }

    #[test]
    fn save_and_load_ai_review_stack_persists_success_by_provider_and_head() {
        let cache = CacheStore::new(std::env::temp_dir().join(format!(
                "remiss-stack-cache-test-{}.sqlite3",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            )))
        .expect("cache");
        let stack = stack("atom", "hash");

        save_ai_review_stack(&cache, &stack, CodeTourProvider::Codex, "head-abc")
            .expect("save stack");

        let loaded =
            load_ai_review_stack(&cache, "acme/repo", 1, CodeTourProvider::Codex, "head-abc")
                .expect("load stack")
                .expect("stored stack");
        let missing = load_ai_review_stack(
            &cache,
            "acme/repo",
            1,
            CodeTourProvider::Copilot,
            "head-abc",
        )
        .expect("load missing");

        assert_eq!(loaded.id, stack.id);
        assert!(missing.is_none());
        let _ = std::fs::remove_file(cache.path());
    }

    #[test]
    fn remaps_reviewed_atoms_by_patch_hash() {
        let previous_stack = stack("old-atom", "same-hash");
        let next_stack = stack("new-atom", "same-hash");
        let progress = StackReviewProgress {
            stack_id: previous_stack.id.clone(),
            repository: previous_stack.repository.clone(),
            pr_number: previous_stack.selected_pr_number,
            reviewed_layer_ids: Vec::new(),
            reviewed_atom_ids: vec!["old-atom".to_string()],
            current_layer_id: None,
            last_location: None,
            updated_at_ms: 1,
        };

        let remapped = remap_reviewed_atoms(&progress, &previous_stack, &next_stack);

        assert_eq!(remapped.reviewed_atom_ids, vec!["new-atom".to_string()]);
        assert_eq!(remapped.reviewed_layer_ids, vec!["layer".to_string()]);
    }

    fn stack(atom_id: &str, patch_hash: &str) -> ReviewStack {
        ReviewStack {
            id: "stack".to_string(),
            repository: "acme/repo".to_string(),
            selected_pr_number: 1,
            source: StackSource::VirtualSemantic,
            kind: StackKind::Virtual,
            confidence: Confidence::Medium,
            trunk_branch: Some("main".to_string()),
            base_oid: None,
            head_oid: None,
            layers: vec![ReviewStackLayer {
                id: "layer".to_string(),
                index: 0,
                title: "Layer".to_string(),
                summary: String::new(),
                rationale: String::new(),
                pr: None,
                virtual_layer: None,
                base_oid: None,
                head_oid: None,
                atom_ids: vec![atom_id.to_string()],
                depends_on_layer_ids: Vec::new(),
                metrics: LayerMetrics::default(),
                status: LayerReviewStatus::NotReviewed,
                confidence: Confidence::Medium,
                warnings: Vec::new(),
            }],
            atoms: vec![ChangeAtom {
                id: atom_id.to_string(),
                source: ChangeAtomSource::File,
                path: "src/main.rs".to_string(),
                previous_path: None,
                role: ChangeRole::CoreLogic,
                semantic_kind: None,
                symbol_name: Some("run".to_string()),
                defined_symbols: Vec::new(),
                referenced_symbols: Vec::new(),
                old_range: None,
                new_range: None,
                hunk_headers: Vec::new(),
                hunk_indices: Vec::new(),
                additions: 1,
                deletions: 0,
                patch_hash: patch_hash.to_string(),
                risk_score: 1,
                review_thread_ids: Vec::new(),
                warnings: Vec::new(),
            }],
            warnings: Vec::new(),
            provider: None,
            generated_at_ms: 1,
            generator_version: "test".to_string(),
        }
    }
}
