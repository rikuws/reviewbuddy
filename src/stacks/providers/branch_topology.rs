use std::{collections::BTreeMap, path::Path, time::Duration};

use crate::{command_runner::CommandRunner, github::PullRequestDetail};

use super::super::model::{
    stack_now_ms, Confidence, LayerMetrics, LayerReviewStatus, RepoContext, ReviewStack,
    ReviewStackLayer, StackDiscoveryError, StackKind, StackProviderMetadata, StackPullRequestRef,
    StackSource, StackWarning, STACK_GENERATOR_VERSION,
};

pub fn discover(
    selected_pr: &PullRequestDetail,
    repo_context: &RepoContext,
) -> Result<Option<ReviewStack>, StackDiscoveryError> {
    let selected = selected_pr_ref(selected_pr);
    let mut open_prs = repo_context.open_pull_requests.clone();
    if !open_prs
        .iter()
        .any(|pr| pr.repository == selected.repository && pr.number == selected.number)
    {
        open_prs.push(selected.clone());
    }

    let mut heads = BTreeMap::<String, Vec<StackPullRequestRef>>::new();
    let mut bases = BTreeMap::<String, Vec<StackPullRequestRef>>::new();
    for pr in open_prs
        .into_iter()
        .filter(|pr| pr.repository == selected.repository)
    {
        heads
            .entry(pr.head_ref_name.clone())
            .or_default()
            .push(pr.clone());
        bases.entry(pr.base_ref_name.clone()).or_default().push(pr);
    }

    let mut warnings = Vec::<StackWarning>::new();
    let mut downstack = Vec::<StackPullRequestRef>::new();
    let mut current = selected.clone();
    let mut visited = vec![current.number];
    let trunk = repo_context.trunk_branch.clone();

    loop {
        if trunk.as_deref() == Some(current.base_ref_name.as_str()) {
            break;
        }

        let candidates = heads
            .get(&current.base_ref_name)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|candidate| candidate.number != current.number)
            .collect::<Vec<_>>();

        let Some(parent) = choose_unique_candidate(
            candidates,
            "ambiguous-parent",
            &format!(
                "Multiple open PRs have head branch '{}'. Review stack confidence was reduced.",
                current.base_ref_name
            ),
            &mut warnings,
        ) else {
            break;
        };

        if visited.contains(&parent.number) {
            warnings.push(StackWarning::new(
                "branch-cycle",
                "Branch topology contains a cycle; only the acyclic path is shown.",
            ));
            break;
        }

        visited.push(parent.number);
        downstack.push(parent.clone());
        current = parent;
    }

    downstack.reverse();

    let mut upstack = Vec::<StackPullRequestRef>::new();
    current = selected.clone();
    loop {
        let candidates = bases
            .get(&current.head_ref_name)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|candidate| candidate.number != current.number)
            .collect::<Vec<_>>();

        let Some(child) = choose_unique_candidate(
            candidates,
            "ambiguous-child",
            &format!(
                "Multiple open PRs target branch '{}'. The selected route uses one child and marks the stack as ambiguous.",
                current.head_ref_name
            ),
            &mut warnings,
        ) else {
            break;
        };

        if visited.contains(&child.number) {
            warnings.push(StackWarning::new(
                "branch-cycle",
                "Branch topology contains a cycle; only the acyclic path is shown.",
            ));
            break;
        }

        visited.push(child.number);
        upstack.push(child.clone());
        current = child;
    }

    if downstack.is_empty() && upstack.is_empty() {
        return Ok(None);
    }

    let mut prs = downstack;
    prs.push(selected);
    prs.extend(upstack);
    validate_ancestry(repo_context.local_repo_path.as_deref(), &prs, &mut warnings);

    let confidence = if warnings
        .iter()
        .any(|warning| warning.code == "branch-cycle" || warning.code == "ancestry-mismatch")
    {
        Confidence::Low
    } else if warnings.is_empty() {
        Confidence::High
    } else {
        Confidence::Medium
    };

    let stack_id = format!(
        "real:{}:{}:{}:{}",
        selected_pr.repository,
        selected_pr.number,
        selected_pr.base_ref_oid.as_deref().unwrap_or("base"),
        selected_pr.head_ref_oid.as_deref().unwrap_or("head")
    );
    let layers = prs
        .iter()
        .enumerate()
        .map(|(index, pr)| ReviewStackLayer {
            id: format!("layer-pr-{}", pr.number),
            index,
            title: format!("#{} {}", pr.number, pr.title),
            summary: format!(
                "{} -> {}",
                pr.base_ref_name.as_str(),
                pr.head_ref_name.as_str()
            ),
            rationale: "This layer is backed by a pull request in the branch chain.".to_string(),
            pr: Some(pr.clone()),
            virtual_layer: None,
            base_oid: pr.base_ref_oid.clone(),
            head_oid: pr.head_ref_oid.clone(),
            atom_ids: Vec::new(),
            depends_on_layer_ids: if index > 0 {
                vec![format!("layer-pr-{}", prs[index - 1].number)]
            } else {
                Vec::new()
            },
            metrics: LayerMetrics::default(),
            status: LayerReviewStatus::NotReviewed,
            confidence,
            warnings: Vec::new(),
        })
        .collect::<Vec<_>>();

    Ok(Some(ReviewStack {
        id: stack_id,
        repository: selected_pr.repository.clone(),
        selected_pr_number: selected_pr.number,
        source: StackSource::BranchTopology,
        kind: StackKind::Real,
        confidence,
        trunk_branch: trunk.or_else(|| prs.first().map(|pr| pr.base_ref_name.clone())),
        base_oid: selected_pr.base_ref_oid.clone(),
        head_oid: selected_pr.head_ref_oid.clone(),
        layers,
        atoms: Vec::new(),
        warnings,
        provider: Some(StackProviderMetadata {
            provider: "branch_topology".to_string(),
            raw_payload: None,
        }),
        generated_at_ms: stack_now_ms(),
        generator_version: STACK_GENERATOR_VERSION.to_string(),
    }))
}

fn selected_pr_ref(selected_pr: &PullRequestDetail) -> StackPullRequestRef {
    StackPullRequestRef {
        repository: selected_pr.repository.clone(),
        number: selected_pr.number,
        title: selected_pr.title.clone(),
        url: selected_pr.url.clone(),
        base_ref_name: selected_pr.base_ref_name.clone(),
        head_ref_name: selected_pr.head_ref_name.clone(),
        base_ref_oid: selected_pr.base_ref_oid.clone(),
        head_ref_oid: selected_pr.head_ref_oid.clone(),
        review_decision: selected_pr.review_decision.clone(),
        state: selected_pr.state.clone(),
        is_draft: selected_pr.is_draft,
    }
}

fn choose_unique_candidate(
    mut candidates: Vec<StackPullRequestRef>,
    warning_code: &str,
    warning_message: &str,
    warnings: &mut Vec<StackWarning>,
) -> Option<StackPullRequestRef> {
    if candidates.is_empty() {
        return None;
    }

    candidates.sort_by_key(|candidate| candidate.number);
    if candidates.len() > 1 {
        warnings.push(StackWarning::new(warning_code, warning_message));
    }
    candidates.into_iter().next()
}

fn validate_ancestry(
    repo_path: Option<&Path>,
    prs: &[StackPullRequestRef],
    warnings: &mut Vec<StackWarning>,
) {
    let Some(repo_path) = repo_path else {
        return;
    };

    for pair in prs.windows(2) {
        let parent = &pair[0];
        let child = &pair[1];
        let Some(parent_head) = parent.head_ref_oid.as_deref() else {
            continue;
        };
        let Some(child_head) = child.head_ref_oid.as_deref() else {
            continue;
        };

        let output = CommandRunner::new("git")
            .args(["merge-base", "--is-ancestor", parent_head, child_head])
            .current_dir(repo_path)
            .timeout(Duration::from_secs(10))
            .run();

        match output {
            Ok(output) if output.exit_code == Some(0) => {}
            Ok(_) | Err(_) => warnings.push(StackWarning::new(
                "ancestry-mismatch",
                format!(
                    "#{} head was not verified as an ancestor of #{} head.",
                    parent.number, child.number
                ),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        github::{PullRequestDataCompleteness, PullRequestDetail},
        stacks::model::{RepoContext, StackPullRequestRef, StackSource},
    };

    use super::discover;

    #[test]
    fn detects_linear_branch_stack() {
        let detail = detail(42, "feature-api", "feature-ui");
        let context = RepoContext {
            open_pull_requests: vec![
                pr(41, "main", "feature-api"),
                pr(42, "feature-api", "feature-ui"),
                pr(43, "feature-ui", "feature-tests"),
            ],
            local_repo_path: None,
            trunk_branch: Some("main".to_string()),
        };

        let stack = discover(&detail, &context).unwrap().unwrap();

        assert_eq!(stack.source, StackSource::BranchTopology);
        assert_eq!(stack.layers.len(), 3);
        assert_eq!(stack.layers[0].pr.as_ref().unwrap().number, 41);
        assert_eq!(stack.layers[1].pr.as_ref().unwrap().number, 42);
        assert_eq!(stack.layers[2].pr.as_ref().unwrap().number, 43);
    }

    #[test]
    fn reduces_confidence_for_ambiguous_children() {
        let detail = detail(41, "main", "feature-api");
        let context = RepoContext {
            open_pull_requests: vec![
                pr(41, "main", "feature-api"),
                pr(42, "feature-api", "feature-ui"),
                pr(43, "feature-api", "feature-cli"),
            ],
            local_repo_path: None,
            trunk_branch: Some("main".to_string()),
        };

        let stack = discover(&detail, &context).unwrap().unwrap();

        assert!(!stack.warnings.is_empty());
        assert_eq!(stack.layers.len(), 2);
    }

    fn pr(number: i64, base: &str, head: &str) -> StackPullRequestRef {
        StackPullRequestRef {
            repository: "acme/repo".to_string(),
            number,
            title: format!("PR {number}"),
            url: String::new(),
            base_ref_name: base.to_string(),
            head_ref_name: head.to_string(),
            base_ref_oid: Some(format!("{base}-oid")),
            head_ref_oid: Some(format!("{head}-oid")),
            review_decision: None,
            state: "OPEN".to_string(),
            is_draft: false,
        }
    }

    fn detail(number: i64, base: &str, head: &str) -> PullRequestDetail {
        PullRequestDetail {
            id: "pr".to_string(),
            repository: "acme/repo".to_string(),
            number,
            title: format!("PR {number}"),
            body: String::new(),
            url: String::new(),
            author_login: "octo".to_string(),
            author_avatar_url: None,
            state: "OPEN".to_string(),
            is_draft: false,
            review_decision: None,
            base_ref_name: base.to_string(),
            head_ref_name: head.to_string(),
            base_ref_oid: Some(format!("{base}-oid")),
            head_ref_oid: Some(format!("{head}-oid")),
            additions: 0,
            deletions: 0,
            changed_files: 0,
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
            files: Vec::new(),
            raw_diff: String::new(),
            parsed_diff: Vec::new(),
            data_completeness: PullRequestDataCompleteness::default(),
        }
    }
}
