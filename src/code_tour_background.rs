use std::collections::BTreeMap;

use crate::{
    cache::CacheStore,
    code_tour::{self, CodeTourSettings},
    github::{self, PullRequestDetail, PullRequestSummary, WorkspaceSnapshot},
    local_repo,
    state::pr_key,
};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BackgroundCodeTourSyncOutcome {
    pub enabled_repositories: usize,
    pub pull_requests_considered: usize,
    pub generated_tours: usize,
    pub reused_cached_tours: usize,
}

impl BackgroundCodeTourSyncOutcome {
    pub fn summary(&self) -> String {
        if self.enabled_repositories == 0 {
            return "Automatic background code tours are disabled for every repository."
                .to_string();
        }

        if self.pull_requests_considered == 0 {
            return format!(
                "No open pull requests matched the {} repository setting{} for automatic code tours.",
                self.enabled_repositories,
                if self.enabled_repositories == 1 { "" } else { "s" }
            );
        }

        if self.generated_tours == 0 {
            return format!(
                "Checked {} pull request{} and reused {} cached guide{}.",
                self.pull_requests_considered,
                if self.pull_requests_considered == 1 {
                    ""
                } else {
                    "s"
                },
                self.reused_cached_tours,
                if self.reused_cached_tours == 1 {
                    ""
                } else {
                    "s"
                }
            );
        }

        format!(
            "Checked {} pull request{}, generated {} guide{}, and reused {} cached guide{}.",
            self.pull_requests_considered,
            if self.pull_requests_considered == 1 {
                ""
            } else {
                "s"
            },
            self.generated_tours,
            if self.generated_tours == 1 { "" } else { "s" },
            self.reused_cached_tours,
            if self.reused_cached_tours == 1 {
                ""
            } else {
                "s"
            }
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PullRequestTourState {
    Generated,
    Cached,
}

pub fn sync_workspace_code_tours(
    cache: &CacheStore,
    workspace: &WorkspaceSnapshot,
    settings: &CodeTourSettings,
) -> Result<BackgroundCodeTourSyncOutcome, String> {
    let pull_requests = enabled_pull_requests(workspace, settings);
    let mut outcome = BackgroundCodeTourSyncOutcome {
        enabled_repositories: settings.automatic_repositories.len(),
        pull_requests_considered: pull_requests.len(),
        ..BackgroundCodeTourSyncOutcome::default()
    };

    if pull_requests.is_empty() {
        return Ok(outcome);
    }

    let provider_statuses = code_tour::load_code_tour_provider_statuses()?;
    let Some(provider_status) = provider_statuses
        .iter()
        .find(|status| status.provider == settings.provider)
    else {
        return Err(format!(
            "{} is not detected in this workspace.",
            settings.provider.label()
        ));
    };

    if !provider_status.available {
        return Err(provider_status.message.clone());
    }

    if !provider_status.authenticated {
        return Err(provider_status.message.clone());
    }

    for summary in pull_requests {
        match ensure_pull_request_code_tour(cache, &summary, settings.provider) {
            Ok(PullRequestTourState::Generated) => outcome.generated_tours += 1,
            Ok(PullRequestTourState::Cached) => outcome.reused_cached_tours += 1,
            Err(error) => {
                return Err(format!(
                    "Failed to prepare {}#{} for automatic code tours: {error}",
                    summary.repository, summary.number
                ));
            }
        }
    }

    Ok(outcome)
}

fn enabled_pull_requests(
    workspace: &WorkspaceSnapshot,
    settings: &CodeTourSettings,
) -> Vec<PullRequestSummary> {
    let mut unique_pull_requests = BTreeMap::<String, PullRequestSummary>::new();

    for queue in &workspace.queues {
        for summary in &queue.items {
            if !settings.automatically_generates_for(&summary.repository) {
                continue;
            }

            let key = pr_key(&summary.repository, summary.number);
            match unique_pull_requests.get(&key) {
                Some(existing) if existing.updated_at >= summary.updated_at => {}
                _ => {
                    unique_pull_requests.insert(key, summary.clone());
                }
            }
        }
    }

    unique_pull_requests.into_values().collect()
}

fn ensure_pull_request_code_tour(
    cache: &CacheStore,
    summary: &PullRequestSummary,
    provider: code_tour::CodeTourProvider,
) -> Result<PullRequestTourState, String> {
    let detail = load_or_sync_pull_request_detail(cache, summary)?;
    if code_tour::load_code_tour(cache, &detail, provider)?.is_some() {
        return Ok(PullRequestTourState::Cached);
    }

    let local_repository_status = local_repo::ensure_local_repository_for_pull_request(
        cache,
        &summary.repository,
        summary.number,
        detail.head_ref_oid.as_deref(),
    )?;
    let working_directory = local_repository_status
        .path
        .clone()
        .ok_or_else(|| local_repository_status.message.clone())?;
    let generation_input =
        code_tour::build_code_tour_generation_input(&detail, provider, &working_directory);

    code_tour::generate_code_tour_with_progress(cache, generation_input, |_| {})?;
    Ok(PullRequestTourState::Generated)
}

fn load_or_sync_pull_request_detail(
    cache: &CacheStore,
    summary: &PullRequestSummary,
) -> Result<PullRequestDetail, String> {
    let cached = github::load_pull_request_detail(cache, &summary.repository, summary.number)?;
    if let Some(detail) = cached.detail {
        if detail.updated_at == summary.updated_at {
            return Ok(detail);
        }
    }

    let snapshot = github::sync_pull_request_detail(cache, &summary.repository, summary.number)?;
    snapshot.detail.ok_or_else(|| {
        format!(
            "GitHub did not return detail for {}#{}.",
            summary.repository, summary.number
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::code_tour::CodeTourProvider;
    use crate::github::{AuthState, PullRequestQueue, Viewer};
    use std::collections::BTreeSet;

    fn summary(repository: &str, number: i64, updated_at: &str) -> PullRequestSummary {
        PullRequestSummary {
            repository: repository.to_string(),
            number,
            title: format!("PR {number}"),
            author_login: "octocat".to_string(),
            author_avatar_url: None,
            is_draft: false,
            comments_count: 0,
            additions: 1,
            deletions: 1,
            changed_files: 1,
            state: "OPEN".to_string(),
            review_decision: None,
            updated_at: updated_at.to_string(),
            url: format!("https://example.com/{repository}/{number}"),
        }
    }

    fn workspace(queues: Vec<PullRequestQueue>) -> WorkspaceSnapshot {
        WorkspaceSnapshot {
            auth: AuthState {
                is_authenticated: true,
                active_login: Some("octocat".to_string()),
                active_hostname: Some("github.com".to_string()),
                message: "ready".to_string(),
            },
            loaded_from_cache: false,
            fetched_at_ms: Some(1),
            viewer: Some(Viewer {
                login: "octocat".to_string(),
                name: Some("Octocat".to_string()),
            }),
            queues,
        }
    }

    #[test]
    fn enabled_pull_requests_deduplicates_matching_repositories() {
        let mut automatic_repositories = BTreeSet::new();
        automatic_repositories.insert("acme/api".to_string());
        let settings = CodeTourSettings {
            provider: CodeTourProvider::Copilot,
            automatic_repositories,
        };

        let workspace = workspace(vec![
            PullRequestQueue {
                id: "reviewRequested".to_string(),
                label: "Review requested".to_string(),
                items: vec![
                    summary("acme/api", 42, "2026-04-17T12:00:00Z"),
                    summary("acme/web", 7, "2026-04-17T12:00:00Z"),
                ],
                total_count: 2,
                is_complete: true,
                truncated_reason: None,
            },
            PullRequestQueue {
                id: "involved".to_string(),
                label: "Involved".to_string(),
                items: vec![summary("acme/api", 42, "2026-04-17T12:05:00Z")],
                total_count: 1,
                is_complete: true,
                truncated_reason: None,
            },
        ]);

        let pull_requests = enabled_pull_requests(&workspace, &settings);

        assert_eq!(pull_requests.len(), 1);
        assert_eq!(pull_requests[0].repository, "acme/api");
        assert_eq!(pull_requests[0].number, 42);
        assert_eq!(pull_requests[0].updated_at, "2026-04-17T12:05:00Z");
    }

    #[test]
    fn background_sync_summary_handles_disabled_repositories() {
        let outcome = BackgroundCodeTourSyncOutcome::default();

        assert_eq!(
            outcome.summary(),
            "Automatic background code tours are disabled for every repository."
        );
    }
}
