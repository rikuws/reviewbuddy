use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    },
    time::Duration,
};

use gpui::{App, AsyncWindowContext, Entity, Window};
use once_cell::sync::Lazy;

use crate::{
    cache::CacheStore,
    code_tour::{self, build_tour_request_key, tour_code_version_key, CodeTourProvider},
    github::PullRequestDetail,
    local_repo,
    stacks::{
        cache::{load_ai_review_stack, save_ai_review_stack},
        discover_review_stack,
        model::{Confidence, RepoContext, ReviewStack, StackDiscoveryOptions},
    },
    state::AppState,
};

static REVIEW_INTELLIGENCE_JOB_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));
static FOREGROUND_REVIEW_INTELLIGENCE_JOBS: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewIntelligenceScope {
    All,
    StackOnly,
    TourOnly,
}

impl ReviewIntelligenceScope {
    fn includes_stack(self) -> bool {
        matches!(self, Self::All | Self::StackOnly)
    }

    fn includes_tour(self) -> bool {
        matches!(self, Self::All | Self::TourOnly)
    }
}

struct ForegroundJobPermit;

impl ForegroundJobPermit {
    fn new() -> Self {
        FOREGROUND_REVIEW_INTELLIGENCE_JOBS.fetch_add(1, Ordering::SeqCst);
        Self
    }
}

impl Drop for ForegroundJobPermit {
    fn drop(&mut self) {
        FOREGROUND_REVIEW_INTELLIGENCE_JOBS.fetch_sub(1, Ordering::SeqCst);
    }
}

pub fn run_foreground_blocking<T>(task: impl FnOnce() -> T) -> T {
    let _guard = REVIEW_INTELLIGENCE_JOB_LOCK
        .lock()
        .expect("review intelligence job lock poisoned");
    task()
}

pub fn run_background_blocking<T>(task: impl FnOnce() -> T) -> T {
    loop {
        while FOREGROUND_REVIEW_INTELLIGENCE_JOBS.load(Ordering::SeqCst) > 0 {
            std::thread::sleep(Duration::from_millis(150));
        }

        let _guard = REVIEW_INTELLIGENCE_JOB_LOCK
            .lock()
            .expect("review intelligence job lock poisoned");
        if FOREGROUND_REVIEW_INTELLIGENCE_JOBS.load(Ordering::SeqCst) == 0 {
            return task();
        }
    }
}

pub fn trigger_review_intelligence(
    state: &Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
    scope: ReviewIntelligenceScope,
    force: bool,
) {
    let model = state.clone();
    window
        .spawn(cx, async move |cx: &mut AsyncWindowContext| {
            run_review_intelligence_flow(model, scope, force, false, cx).await;
        })
        .detach();
}

pub(crate) async fn run_review_intelligence_flow(
    model: Entity<AppState>,
    scope: ReviewIntelligenceScope,
    force: bool,
    automatic: bool,
    cx: &mut AsyncWindowContext,
) {
    let Some(initial) = model
        .read_with(cx, |state, _| {
            let detail = state.active_detail()?.clone();
            let detail_key = state.active_pr_key.clone()?;
            let provider = state.selected_tour_provider();
            let open_pull_requests = state
                .active_detail_state()
                .and_then(|detail_state| detail_state.stack_open_pull_requests.clone())
                .unwrap_or_default();
            Some((
                state.cache.clone(),
                detail_key,
                detail,
                provider,
                state.code_tour_provider_statuses_loaded,
                open_pull_requests,
            ))
        })
        .ok()
        .flatten()
    else {
        return;
    };

    let (cache, detail_key, detail, provider, statuses_loaded, open_pull_requests) = initial;
    let request_key = review_intelligence_request_key(&detail, provider);
    let code_version_key = tour_code_version_key(&detail);
    let tour_request_key = build_tour_request_key(&detail, provider);

    let should_start = model
        .update(cx, |state, cx| {
            let Some(detail_state) = state.detail_states.get_mut(&detail_key) else {
                return false;
            };
            if detail_state.review_intelligence_loading
                && detail_state.review_intelligence_request_key.as_deref() == Some(&request_key)
            {
                return false;
            }

            detail_state.review_intelligence_request_key = Some(request_key.clone());
            detail_state.review_intelligence_loading = true;
            detail_state.local_repository_loading = true;
            detail_state.local_repository_error = None;

            if !statuses_loaded {
                state.code_tour_provider_loading = true;
                state.code_tour_provider_error = None;
            }

            if scope.includes_stack() {
                let stack_request_changed =
                    detail_state.ai_stack_state.request_key.as_deref() != Some(&request_key);
                detail_state.ai_stack_state.request_key = Some(request_key.clone());
                detail_state.ai_stack_state.loading = true;
                detail_state.ai_stack_state.generating = false;
                if force || stack_request_changed {
                    detail_state.ai_stack_state.stack = None;
                }
                detail_state.ai_stack_state.error = None;
                detail_state.ai_stack_state.message =
                    Some("Preparing local checkout for AI stack review.".to_string());
                detail_state.ai_stack_state.success = false;
            }

            if scope.includes_tour() {
                let tour_state = detail_state.tour_states.entry(provider).or_default();
                let tour_request_changed =
                    tour_state.request_key.as_deref() != Some(&tour_request_key);
                tour_state.request_key = Some(tour_request_key.clone());
                if force || tour_request_changed {
                    tour_state.document = None;
                }
                tour_state.loading = true;
                tour_state.generating = false;
                tour_state.progress_summary = Some(if scope.includes_stack() {
                    "Preparing AI tour and stack".to_string()
                } else {
                    "Preparing AI tour".to_string()
                });
                tour_state.progress_detail = Some(
                    "Preparing the local checkout and checking cached intelligence for this pull request."
                        .to_string(),
                );
                tour_state.progress_log.clear();
                tour_state.progress_log_file_path = None;
                tour_state.error = None;
                tour_state.message = None;
                tour_state.success = false;
            }

            cx.notify();
            true
        })
        .ok()
        .unwrap_or(false);

    if !should_start {
        return;
    }

    let _permit = ForegroundJobPermit::new();

    if !statuses_loaded {
        let statuses_result = cx
            .background_executor()
            .spawn(async { code_tour::load_code_tour_provider_statuses() })
            .await;
        model
            .update(cx, |state, cx| {
                state.code_tour_provider_loading = false;
                state.code_tour_provider_statuses_loaded = true;
                match statuses_result {
                    Ok(statuses) => {
                        state.code_tour_provider_statuses = statuses;
                        state.code_tour_provider_error = None;
                    }
                    Err(error) => {
                        state.code_tour_provider_error = Some(error);
                    }
                }
                cx.notify();
            })
            .ok();
    }

    let local_repo_result = cx
        .background_executor()
        .spawn({
            let cache = cache.clone();
            let repository = detail.repository.clone();
            let pull_request_number = detail.number;
            let head_ref_oid = detail.head_ref_oid.clone();
            async move {
                run_foreground_blocking(|| {
                    local_repo::ensure_local_repository_for_pull_request(
                        &cache,
                        &repository,
                        pull_request_number,
                        head_ref_oid.as_deref(),
                    )
                })
            }
        })
        .await;

    let local_repo_status = match local_repo_result {
        Ok(status) => {
            model
                .update(cx, |state, cx| {
                    if let Some(detail_state) = state.detail_states.get_mut(&detail_key) {
                        detail_state.local_repository_loading = false;
                        detail_state.local_repository_status = Some(status.clone());
                        detail_state.local_repository_error = None;
                    }
                    cx.notify();
                })
                .ok();
            status
        }
        Err(error) => {
            fail_checkout(
                &model,
                &detail_key,
                scope,
                provider,
                &request_key,
                &error,
                cx,
            )
            .await;
            finish_request(&model, &detail_key, &request_key, cx).await;
            return;
        }
    };

    if scope.includes_stack() {
        generate_or_load_stack(
            &model,
            cache.as_ref(),
            &detail_key,
            &detail,
            provider,
            &request_key,
            &code_version_key,
            &local_repo_status,
            open_pull_requests,
            force,
            cx,
        )
        .await;
    }

    if scope.includes_tour() {
        generate_or_load_tour(
            &model,
            cache.as_ref(),
            &detail_key,
            detail.clone(),
            provider,
            tour_request_key,
            &local_repo_status,
            force,
            automatic,
            cx,
        )
        .await;
    }

    finish_request(&model, &detail_key, &request_key, cx).await;
}

async fn generate_or_load_stack(
    model: &Entity<AppState>,
    cache: &CacheStore,
    detail_key: &str,
    detail: &PullRequestDetail,
    provider: CodeTourProvider,
    request_key: &str,
    code_version_key: &str,
    local_repo_status: &local_repo::LocalRepositoryStatus,
    open_pull_requests: Vec<crate::stacks::model::StackPullRequestRef>,
    force: bool,
    cx: &mut AsyncWindowContext,
) {
    if !force {
        let cached = cx
            .background_executor()
            .spawn({
                let cache = CacheStore::clone(cache);
                let repository = detail.repository.clone();
                let pr_number = detail.number;
                let code_version_key = code_version_key.to_string();
                async move {
                    load_ai_review_stack(
                        &cache,
                        &repository,
                        pr_number,
                        provider,
                        &code_version_key,
                    )
                }
            })
            .await;

        if let Ok(Some(stack)) = cached {
            set_stack_success(
                model,
                detail_key,
                request_key,
                stack,
                Some("Loaded cached AI stack review.".to_string()),
                cx,
            )
            .await;
            return;
        }
    }

    let Some(working_directory) = local_repo_status.path.as_ref() else {
        set_stack_error(
            model,
            detail_key,
            request_key,
            detail,
            local_repo_status.message.clone(),
            cx,
        )
        .await;
        return;
    };

    model
        .update(cx, |state, cx| {
            if let Some(detail_state) = state.detail_states.get_mut(detail_key) {
                if detail_state.ai_stack_state.request_key.as_deref() == Some(request_key) {
                    detail_state.ai_stack_state.loading = false;
                    detail_state.ai_stack_state.generating = true;
                    detail_state.ai_stack_state.message =
                        Some("Generating AI stack review.".to_string());
                }
            }
            cx.notify();
        })
        .ok();

    let stack_result = cx
        .background_executor()
        .spawn({
            let detail = detail.clone();
            let working_directory = PathBuf::from(working_directory);
            async move {
                run_foreground_blocking(|| {
                    let options = StackDiscoveryOptions {
                        enable_github_native: false,
                        enable_branch_topology: false,
                        enable_local_metadata: false,
                        enable_ai_virtual: true,
                        enable_virtual_commits: false,
                        enable_virtual_semantic: false,
                        ai_provider: Some(provider),
                        ..StackDiscoveryOptions::default()
                    };

                    let repo_context = RepoContext {
                        open_pull_requests,
                        local_repo_path: Some(working_directory),
                        trunk_branch: None,
                    };

                    discover_review_stack(&detail, &repo_context, options)
                        .map_err(|error| error.message)
                })
            }
        })
        .await;

    match stack_result {
        Ok(stack) if !stack_is_ai_unavailable(&stack) => {
            let _ = save_ai_review_stack(cache, &stack, provider, code_version_key);
            set_stack_success(
                model,
                detail_key,
                request_key,
                stack,
                Some("Generated AI stack review.".to_string()),
                cx,
            )
            .await;
        }
        Ok(stack) => {
            let message = stack
                .warnings
                .first()
                .map(|warning| warning.message.clone())
                .unwrap_or_else(|| {
                    "AI stack planning was unavailable. Retry after checkout and provider issues are resolved."
                        .to_string()
                });
            set_stack_transient_failure(model, detail_key, request_key, stack, message, cx).await;
        }
        Err(error) => {
            set_stack_error(model, detail_key, request_key, detail, error, cx).await;
        }
    }
}

async fn generate_or_load_tour(
    model: &Entity<AppState>,
    cache: &CacheStore,
    detail_key: &str,
    detail: PullRequestDetail,
    provider: CodeTourProvider,
    tour_request_key: String,
    local_repo_status: &local_repo::LocalRepositoryStatus,
    force: bool,
    automatic: bool,
    cx: &mut AsyncWindowContext,
) {
    if !force {
        let cached = cx
            .background_executor()
            .spawn({
                let cache = CacheStore::clone(cache);
                let detail = detail.clone();
                async move { code_tour::load_code_tour(&cache, &detail, provider) }
            })
            .await;

        if let Ok(Some(tour)) = cached {
            model
                .update(cx, |state, cx| {
                    if let Some(detail_state) = state.detail_states.get_mut(detail_key) {
                        let tour_state = detail_state.tour_states.entry(provider).or_default();
                        if tour_state.request_key.as_deref() == Some(&tour_request_key) {
                            tour_state.loading = false;
                            tour_state.generating = false;
                            tour_state.document = Some(tour);
                            tour_state.error = None;
                            tour_state.message = Some("Loaded cached AI tour.".to_string());
                            tour_state.success = true;
                        }
                    }
                    cx.notify();
                })
                .ok();
            return;
        }
    }

    crate::views::ai_tour::generate_tour_flow(
        model.clone(),
        Some((detail_key.to_string(), detail, provider, tour_request_key)),
        Some(local_repo_status.clone()),
        automatic,
        cx,
    )
    .await;
}

async fn fail_checkout(
    model: &Entity<AppState>,
    detail_key: &str,
    scope: ReviewIntelligenceScope,
    provider: CodeTourProvider,
    request_key: &str,
    error: &str,
    cx: &mut AsyncWindowContext,
) {
    model
        .update(cx, |state, cx| {
            if let Some(detail_state) = state.detail_states.get_mut(detail_key) {
                detail_state.local_repository_loading = false;
                detail_state.local_repository_error = Some(error.to_string());

                if scope.includes_stack()
                    && detail_state.ai_stack_state.request_key.as_deref() == Some(request_key)
                {
                    detail_state.ai_stack_state.loading = false;
                    detail_state.ai_stack_state.generating = false;
                    detail_state.ai_stack_state.error = Some(error.to_string());
                    detail_state.ai_stack_state.message = None;
                    detail_state.ai_stack_state.success = false;
                }

                if scope.includes_tour() {
                    let tour_state = detail_state.tour_states.entry(provider).or_default();
                    tour_state.loading = false;
                    tour_state.generating = false;
                    tour_state.error = Some(error.to_string());
                    tour_state.message = None;
                    tour_state.success = false;
                }
            }
            cx.notify();
        })
        .ok();
}

async fn set_stack_success(
    model: &Entity<AppState>,
    detail_key: &str,
    request_key: &str,
    stack: ReviewStack,
    message: Option<String>,
    cx: &mut AsyncWindowContext,
) {
    model
        .update(cx, |state, cx| {
            if let Some(detail_state) = state.detail_states.get_mut(detail_key) {
                if detail_state.ai_stack_state.request_key.as_deref() == Some(request_key) {
                    detail_state.ai_stack_state.stack = Some(std::sync::Arc::new(stack));
                    detail_state.ai_stack_state.loading = false;
                    detail_state.ai_stack_state.generating = false;
                    detail_state.ai_stack_state.error = None;
                    detail_state.ai_stack_state.message = message;
                    detail_state.ai_stack_state.success = true;
                    state.review_stack_cache.borrow_mut().clear();
                }
            }
            cx.notify();
        })
        .ok();
}

async fn set_stack_transient_failure(
    model: &Entity<AppState>,
    detail_key: &str,
    request_key: &str,
    stack: ReviewStack,
    error: String,
    cx: &mut AsyncWindowContext,
) {
    model
        .update(cx, |state, cx| {
            if let Some(detail_state) = state.detail_states.get_mut(detail_key) {
                if detail_state.ai_stack_state.request_key.as_deref() == Some(request_key) {
                    detail_state.ai_stack_state.stack = Some(std::sync::Arc::new(stack));
                    detail_state.ai_stack_state.loading = false;
                    detail_state.ai_stack_state.generating = false;
                    detail_state.ai_stack_state.error = Some(error);
                    detail_state.ai_stack_state.message = None;
                    detail_state.ai_stack_state.success = false;
                    state.review_stack_cache.borrow_mut().clear();
                }
            }
            cx.notify();
        })
        .ok();
}

async fn set_stack_error(
    model: &Entity<AppState>,
    detail_key: &str,
    request_key: &str,
    detail: &PullRequestDetail,
    error: String,
    cx: &mut AsyncWindowContext,
) {
    let stack = ai_stack_for_error(detail, &error);
    set_stack_transient_failure(model, detail_key, request_key, stack, error, cx).await;
}

async fn finish_request(
    model: &Entity<AppState>,
    detail_key: &str,
    request_key: &str,
    cx: &mut AsyncWindowContext,
) {
    model
        .update(cx, |state, cx| {
            if let Some(detail_state) = state.detail_states.get_mut(detail_key) {
                if detail_state.review_intelligence_request_key.as_deref() == Some(request_key) {
                    detail_state.review_intelligence_loading = false;
                    detail_state.review_intelligence_request_key = None;
                }
            }
            cx.notify();
        })
        .ok();
}

pub fn review_intelligence_request_key(
    detail: &PullRequestDetail,
    provider: CodeTourProvider,
) -> String {
    format!(
        "{}:{}#{}:{}",
        provider.slug(),
        detail.repository,
        detail.number,
        tour_code_version_key(detail)
    )
}

fn stack_is_ai_unavailable(stack: &ReviewStack) -> bool {
    stack
        .warnings
        .iter()
        .any(|warning| warning.code == "ai-virtual-stack-unavailable")
}

fn ai_stack_for_error(detail: &PullRequestDetail, message: &str) -> ReviewStack {
    crate::stacks::providers::ai_virtual::ai_unavailable_stack(
        detail,
        &format!("AI stack planning failed. {message}"),
        Some(serde_json::json!({ "error": message })),
    )
    .unwrap_or_else(|_| ReviewStack {
        id: format!("stack-error:{}#{}", detail.repository, detail.number),
        repository: detail.repository.clone(),
        selected_pr_number: detail.number,
        source: crate::stacks::model::StackSource::VirtualAi,
        kind: crate::stacks::model::StackKind::Virtual,
        confidence: Confidence::Low,
        trunk_branch: Some(detail.base_ref_name.clone()),
        base_oid: detail.base_ref_oid.clone(),
        head_oid: detail.head_ref_oid.clone(),
        layers: Vec::new(),
        atoms: Vec::new(),
        warnings: vec![crate::stacks::model::StackWarning::new(
            "ai-virtual-stack-unavailable",
            "AI stack planning failed and Remiss did not generate a non-AI stack.",
        )],
        provider: None,
        generated_at_ms: crate::stacks::model::stack_now_ms(),
        generator_version: crate::stacks::model::STACK_GENERATOR_VERSION.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::review_intelligence_request_key;
    use crate::{code_tour::CodeTourProvider, github::PullRequestDetail};

    #[test]
    fn review_intelligence_request_key_ignores_metadata_updates_when_head_matches() {
        let first = detail("2026-04-17T10:00:00Z", Some("head123"), "diff-one");
        let second = detail("2026-04-17T11:00:00Z", Some("head123"), "diff-two");

        assert_eq!(
            review_intelligence_request_key(&first, CodeTourProvider::Codex),
            review_intelligence_request_key(&second, CodeTourProvider::Codex)
        );
    }

    #[test]
    fn review_intelligence_request_key_varies_by_provider() {
        let detail = detail("2026-04-17T10:00:00Z", Some("head123"), "diff-one");

        assert_ne!(
            review_intelligence_request_key(&detail, CodeTourProvider::Codex),
            review_intelligence_request_key(&detail, CodeTourProvider::Copilot)
        );
    }

    fn detail(updated_at: &str, head_ref_oid: Option<&str>, raw_diff: &str) -> PullRequestDetail {
        PullRequestDetail {
            id: "pr1".to_string(),
            repository: "acme/api".to_string(),
            number: 42,
            title: "Test PR".to_string(),
            body: String::new(),
            url: "https://example.com/pr/42".to_string(),
            author_login: "octocat".to_string(),
            author_avatar_url: None,
            state: "OPEN".to_string(),
            is_draft: false,
            review_decision: None,
            base_ref_name: "main".to_string(),
            head_ref_name: "feature/test".to_string(),
            base_ref_oid: Some("base123".to_string()),
            head_ref_oid: head_ref_oid.map(str::to_string),
            additions: 1,
            deletions: 1,
            changed_files: 1,
            comments_count: 0,
            commits_count: 1,
            created_at: "2026-04-17T00:00:00Z".to_string(),
            updated_at: updated_at.to_string(),
            labels: Vec::new(),
            reviewers: Vec::new(),
            reviewer_avatar_urls: std::collections::BTreeMap::new(),
            comments: Vec::new(),
            latest_reviews: Vec::new(),
            review_threads: Vec::new(),
            files: Vec::new(),
            raw_diff: raw_diff.to_string(),
            parsed_diff: Vec::new(),
            data_completeness: crate::github::PullRequestDataCompleteness::default(),
        }
    }
}
