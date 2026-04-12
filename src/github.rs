use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::process::Command;

use crate::{
    cache::{CacheStore, CachedDocument},
    diff::ParsedDiffFile,
    gh::{self, CommandOutput},
};

const WORKSPACE_CACHE_KEY: &str = "workspace-snapshot-v1";
const AUTH_STATE_CACHE_KEY: &str = "auth-state-v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthState {
    pub is_authenticated: bool,
    pub active_login: Option<String>,
    pub active_hostname: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Viewer {
    pub login: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequestSummary {
    pub repository: String,
    pub number: i64,
    pub title: String,
    pub author_login: String,
    pub comments_count: i64,
    pub additions: i64,
    pub deletions: i64,
    pub changed_files: i64,
    pub state: String,
    pub review_decision: Option<String>,
    pub updated_at: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequestQueue {
    pub id: String,
    pub label: String,
    pub items: Vec<PullRequestSummary>,
    pub total_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceCachePayload {
    pub viewer: Option<Viewer>,
    pub queues: Vec<PullRequestQueue>,
}

#[derive(Debug, Clone)]
pub struct WorkspaceSnapshot {
    pub auth: AuthState,
    pub loaded_from_cache: bool,
    pub fetched_at_ms: Option<i64>,
    pub viewer: Option<Viewer>,
    pub queues: Vec<PullRequestQueue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequestFile {
    pub path: String,
    pub additions: i64,
    pub deletions: i64,
    #[serde(default = "default_change_type")]
    pub change_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequestReview {
    pub author_login: String,
    pub state: String,
    pub body: String,
    pub submitted_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequestReviewComment {
    pub id: String,
    pub author_login: String,
    pub body: String,
    pub path: String,
    pub line: Option<i64>,
    pub original_line: Option<i64>,
    pub start_line: Option<i64>,
    pub original_start_line: Option<i64>,
    pub state: String,
    pub created_at: String,
    pub updated_at: String,
    pub published_at: Option<String>,
    pub reply_to_id: Option<String>,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequestReviewThread {
    pub id: String,
    pub path: String,
    pub line: Option<i64>,
    pub original_line: Option<i64>,
    pub start_line: Option<i64>,
    pub original_start_line: Option<i64>,
    pub diff_side: String,
    pub start_diff_side: Option<String>,
    pub is_collapsed: bool,
    pub is_outdated: bool,
    pub is_resolved: bool,
    pub subject_type: String,
    pub resolved_by_login: Option<String>,
    pub viewer_can_reply: bool,
    pub viewer_can_resolve: bool,
    pub viewer_can_unresolve: bool,
    pub comments: Vec<PullRequestReviewComment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequestDetail {
    pub id: String,
    pub repository: String,
    pub number: i64,
    pub title: String,
    pub body: String,
    pub url: String,
    pub author_login: String,
    pub state: String,
    pub is_draft: bool,
    pub review_decision: Option<String>,
    pub base_ref_name: String,
    pub head_ref_name: String,
    pub base_ref_oid: Option<String>,
    pub head_ref_oid: Option<String>,
    pub additions: i64,
    pub deletions: i64,
    pub changed_files: i64,
    pub comments_count: i64,
    pub commits_count: i64,
    pub created_at: String,
    pub updated_at: String,
    pub labels: Vec<String>,
    pub reviewers: Vec<String>,
    pub latest_reviews: Vec<PullRequestReview>,
    pub review_threads: Vec<PullRequestReviewThread>,
    pub files: Vec<PullRequestFile>,
    pub raw_diff: String,
    pub parsed_diff: Vec<ParsedDiffFile>,
}

#[derive(Debug, Clone)]
pub struct PullRequestDetailSnapshot {
    pub auth: AuthState,
    pub loaded_from_cache: bool,
    pub fetched_at_ms: Option<i64>,
    pub detail: Option<PullRequestDetail>,
}

#[derive(Debug, Clone)]
pub struct ActionResult {
    pub success: bool,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ReviewAction {
    Approve,
    Comment,
    RequestChanges,
}

pub fn load_workspace_snapshot(cache: &CacheStore) -> Result<WorkspaceSnapshot, String> {
    let auth = cached_auth_state(cache)?;
    let cached = cache.get::<WorkspaceCachePayload>(WORKSPACE_CACHE_KEY)?;
    Ok(workspace_snapshot_from_cache(auth, cached))
}

pub fn sync_workspace_snapshot(cache: &CacheStore) -> Result<WorkspaceSnapshot, String> {
    let auth = refresh_auth_state(cache)?;

    if !auth.is_authenticated {
        return load_workspace_snapshot(cache);
    }

    let payload = fetch_workspace_payload()?;
    let fetched_at_ms = now_ms();
    cache.put(WORKSPACE_CACHE_KEY, &payload, fetched_at_ms)?;

    Ok(WorkspaceSnapshot {
        auth,
        loaded_from_cache: false,
        fetched_at_ms: Some(fetched_at_ms),
        viewer: payload.viewer,
        queues: payload.queues,
    })
}

pub fn load_pull_request_detail(
    cache: &CacheStore,
    repository: &str,
    number: i64,
) -> Result<PullRequestDetailSnapshot, String> {
    let auth = cached_auth_state(cache)?;
    let key = pull_request_detail_cache_key(repository, number);
    let cached = cache.get::<PullRequestDetail>(&key)?;

    Ok(PullRequestDetailSnapshot {
        auth,
        loaded_from_cache: cached.is_some(),
        fetched_at_ms: cached.as_ref().map(|d| d.fetched_at_ms),
        detail: cached.map(|d| d.value),
    })
}

pub fn sync_pull_request_detail(
    cache: &CacheStore,
    repository: &str,
    number: i64,
) -> Result<PullRequestDetailSnapshot, String> {
    let auth = refresh_auth_state(cache)?;

    if !auth.is_authenticated {
        return load_pull_request_detail(cache, repository, number);
    }

    let detail = fetch_pull_request_detail(repository, number)?;
    let fetched_at_ms = now_ms();
    let key = pull_request_detail_cache_key(repository, number);
    cache.put(&key, &detail, fetched_at_ms)?;

    Ok(PullRequestDetailSnapshot {
        auth,
        loaded_from_cache: false,
        fetched_at_ms: Some(fetched_at_ms),
        detail: Some(detail),
    })
}

pub fn submit_pull_request_review(
    repository: &str,
    number: i64,
    action: ReviewAction,
    body: &str,
) -> Result<ActionResult, String> {
    let auth = live_auth_state()?;

    if !auth.is_authenticated {
        return Ok(ActionResult {
            success: false,
            message: auth.message,
        });
    }

    let mut args = vec![
        "pr".to_string(),
        "review".to_string(),
        number.to_string(),
        "--repo".to_string(),
        repository.to_string(),
    ];

    match action {
        ReviewAction::Approve => args.push("--approve".to_string()),
        ReviewAction::Comment => args.push("--comment".to_string()),
        ReviewAction::RequestChanges => args.push("--request-changes".to_string()),
    }

    if !body.trim().is_empty() {
        args.push("--body".to_string());
        args.push(body.to_string());
    }

    let output = gh::run_owned(args)?;

    if output.exit_code == Some(0) {
        Ok(ActionResult {
            success: true,
            message: "Review submitted through gh.".to_string(),
        })
    } else {
        Ok(ActionResult {
            success: false,
            message: combine_process_error(output, "Failed to submit review"),
        })
    }
}

pub fn add_pull_request_review_thread(
    pull_request_id: &str,
    path: &str,
    body: &str,
    line: Option<i64>,
    side: Option<&str>,
    subject_type: Option<&str>,
) -> Result<ActionResult, String> {
    let auth = live_auth_state()?;

    if !auth.is_authenticated {
        return Ok(ActionResult {
            success: false,
            message: auth.message,
        });
    }

    if body.trim().is_empty() {
        return Ok(ActionResult {
            success: false,
            message: "Comment body cannot be empty.".to_string(),
        });
    }

    let subject = subject_type.unwrap_or(if line.is_some() { "LINE" } else { "FILE" });

    if subject == "LINE" && (line.is_none() || side.is_none()) {
        return Ok(ActionResult {
            success: false,
            message: "Line comments require both a line number and diff side.".to_string(),
        });
    }

    let mutation = r#"
        mutation(
          $pullRequestId: ID!,
          $path: String!,
          $body: String!,
          $line: Int,
          $side: DiffSide,
          $subjectType: PullRequestReviewThreadSubjectType
        ) {
          addPullRequestReviewThread(
            input: {
              pullRequestId: $pullRequestId
              path: $path
              body: $body
              line: $line
              side: $side
              subjectType: $subjectType
            }
          ) {
            thread { id }
          }
        }
    "#;

    let response = gh::graphql(
        mutation,
        json!({
            "pullRequestId": pull_request_id,
            "path": path,
            "body": body,
            "line": line,
            "side": side,
            "subjectType": subject,
        }),
    )?;

    if let Some(error_message) = graphql_error_message(&response) {
        return Ok(ActionResult {
            success: false,
            message: error_message,
        });
    }

    let success = response
        .get("data")
        .and_then(|v| v.get("addPullRequestReviewThread"))
        .and_then(|v| v.get("thread"))
        .and_then(|v| v.get("id"))
        .and_then(Value::as_str)
        .is_some();

    Ok(ActionResult {
        success,
        message: if success {
            "Review thread added to the diff.".to_string()
        } else {
            "GitHub did not return the new review thread.".to_string()
        },
    })
}

pub fn reply_to_review_thread(thread_id: &str, body: &str) -> Result<ActionResult, String> {
    let auth = live_auth_state()?;

    if !auth.is_authenticated {
        return Ok(ActionResult {
            success: false,
            message: auth.message,
        });
    }

    if body.trim().is_empty() {
        return Ok(ActionResult {
            success: false,
            message: "Reply body cannot be empty.".to_string(),
        });
    }

    let mutation = r#"
        mutation($threadId: ID!, $body: String!) {
          addPullRequestReviewThreadReply(
            input: { pullRequestReviewThreadId: $threadId, body: $body }
          ) { thread { id } }
        }
    "#;

    let response = gh::graphql(mutation, json!({ "threadId": thread_id, "body": body }))?;

    if let Some(error_message) = graphql_error_message(&response) {
        return Ok(ActionResult {
            success: false,
            message: error_message,
        });
    }

    let success = response
        .get("data")
        .and_then(|v| v.get("addPullRequestReviewThreadReply"))
        .and_then(|v| v.get("thread"))
        .and_then(|v| v.get("id"))
        .and_then(Value::as_str)
        .is_some();

    Ok(ActionResult {
        success,
        message: if success {
            "Reply added to the thread.".to_string()
        } else {
            "GitHub did not return the updated review thread.".to_string()
        },
    })
}

pub fn set_review_thread_resolution(
    thread_id: &str,
    resolved: bool,
) -> Result<ActionResult, String> {
    let auth = live_auth_state()?;

    if !auth.is_authenticated {
        return Ok(ActionResult {
            success: false,
            message: auth.message,
        });
    }

    let mutation = if resolved {
        r#"mutation($threadId: ID!) {
          resolveReviewThread(input: { threadId: $threadId }) {
            thread { id isResolved }
          }
        }"#
    } else {
        r#"mutation($threadId: ID!) {
          unresolveReviewThread(input: { threadId: $threadId }) {
            thread { id isResolved }
          }
        }"#
    };

    let response = gh::graphql(mutation, json!({ "threadId": thread_id }))?;

    if let Some(error_message) = graphql_error_message(&response) {
        return Ok(ActionResult {
            success: false,
            message: error_message,
        });
    }

    let mutation_name = if resolved {
        "resolveReviewThread"
    } else {
        "unresolveReviewThread"
    };

    let success = response
        .get("data")
        .and_then(|v| v.get(mutation_name))
        .and_then(|v| v.get("thread"))
        .and_then(|v| v.get("id"))
        .and_then(Value::as_str)
        .is_some();

    Ok(ActionResult {
        success,
        message: if success {
            if resolved {
                "Thread resolved.".to_string()
            } else {
                "Thread reopened.".to_string()
            }
        } else if resolved {
            "GitHub did not confirm that the thread was resolved.".to_string()
        } else {
            "GitHub did not confirm that the thread was reopened.".to_string()
        },
    })
}

pub fn load_pull_request_file_content(
    repository: &str,
    reference: &str,
    path: &str,
) -> Result<RepositoryFileContent, String> {
    let auth = live_auth_state()?;

    if !auth.is_authenticated {
        return Err(auth.message);
    }

    fetch_repository_file_content(repository, reference, path)
}

#[derive(Debug, Clone)]
pub struct RepositoryFileContent {
    pub repository: String,
    pub reference: String,
    pub path: String,
    pub content: Option<String>,
    pub is_binary: bool,
    pub size_bytes: usize,
}

// --- Private implementation ---

fn workspace_snapshot_from_cache(
    auth: AuthState,
    cached: Option<CachedDocument<WorkspaceCachePayload>>,
) -> WorkspaceSnapshot {
    match cached {
        Some(document) => WorkspaceSnapshot {
            auth,
            loaded_from_cache: true,
            fetched_at_ms: Some(document.fetched_at_ms),
            viewer: document.value.viewer,
            queues: document.value.queues,
        },
        None => WorkspaceSnapshot {
            auth,
            loaded_from_cache: false,
            fetched_at_ms: None,
            viewer: None,
            queues: default_queues(),
        },
    }
}

fn fetch_workspace_payload() -> Result<WorkspaceCachePayload, String> {
    let viewer = fetch_viewer()?;
    let queue_specs = [
        (
            "reviewRequested",
            "Review requested",
            "is:open is:pr archived:false review-requested:@me",
        ),
        (
            "assigned",
            "Assigned",
            "is:open is:pr archived:false assignee:@me",
        ),
        (
            "authored",
            "Authored",
            "is:open is:pr archived:false author:@me",
        ),
        (
            "mentioned",
            "Mentioned",
            "is:open is:pr archived:false mentions:@me",
        ),
        (
            "involved",
            "Involved",
            "is:open is:pr archived:false involves:@me",
        ),
    ];

    let mut queues = Vec::with_capacity(queue_specs.len());
    for (id, label, query) in queue_specs {
        queues.push(fetch_queue(id, label, query)?);
    }

    Ok(WorkspaceCachePayload {
        viewer: Some(viewer),
        queues,
    })
}

fn fetch_viewer() -> Result<Viewer, String> {
    let response = gh::run_json_owned(vec!["api".to_string(), "user".to_string()])?;
    let login = response
        .get("login")
        .and_then(Value::as_str)
        .ok_or_else(|| "gh api user did not return a login.".to_string())?;

    Ok(Viewer {
        login: login.to_string(),
        name: response
            .get("name")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

fn fetch_queue(id: &str, label: &str, search_query: &str) -> Result<PullRequestQueue, String> {
    let query = r#"
        query($searchQuery: String!, $count: Int!) {
          search(query: $searchQuery, type: ISSUE, first: $count) {
            issueCount
            nodes {
              ... on PullRequest {
                number
                title
                url
                state
                updatedAt
                additions
                deletions
                changedFiles
                reviewDecision
                author { login }
                comments { totalCount }
                repository { nameWithOwner }
              }
            }
          }
        }
    "#;

    let response = gh::graphql(query, json!({ "searchQuery": search_query, "count": 50 }))?;
    let search = response
        .get("data")
        .and_then(|v| v.get("search"))
        .ok_or_else(|| "Missing search data in GraphQL response.".to_string())?;

    let total_count = search
        .get("issueCount")
        .and_then(Value::as_i64)
        .unwrap_or(0);

    let items = search
        .get("nodes")
        .and_then(Value::as_array)
        .map(|nodes| {
            nodes
                .iter()
                .filter_map(map_pull_request_summary)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Ok(PullRequestQueue {
        id: id.to_string(),
        label: label.to_string(),
        items,
        total_count,
    })
}

fn fetch_pull_request_detail(repository: &str, number: i64) -> Result<PullRequestDetail, String> {
    let (owner, name) = split_repository(repository)?;
    let query = r#"
        query($owner: String!, $name: String!, $number: Int!) {
          repository(owner: $owner, name: $name) {
            pullRequest(number: $number) {
              id number title body url state isDraft reviewDecision
              baseRefName headRefName baseRefOid headRefOid
              additions deletions changedFiles createdAt updatedAt
              author { login }
              comments { totalCount }
              commits { totalCount }
              labels(first: 20) { nodes { name } }
              reviewRequests(first: 20) {
                nodes { requestedReviewer { ... on User { login } ... on Team { slug } } }
              }
              latestReviews(first: 20) {
                nodes { state body submittedAt author { login } }
              }
              reviewThreads(first: 100) {
                nodes {
                  id path line originalLine startLine originalStartLine
                  diffSide startDiffSide isCollapsed isOutdated isResolved
                  subjectType viewerCanReply viewerCanResolve viewerCanUnresolve
                  resolvedBy { login }
                  comments(first: 20) {
                    nodes {
                      id body path line originalLine startLine originalStartLine
                      state createdAt updatedAt publishedAt url
                      replyTo { id }
                      author { login }
                    }
                  }
                }
              }
              files(first: 100) { nodes { path additions deletions changeType } }
            }
          }
        }
    "#;

    let response = gh::graphql(
        query,
        json!({ "owner": owner, "name": name, "number": number }),
    )?;

    if let Some(error_message) = graphql_error_message(&response) {
        return Err(error_message);
    }

    let pr = response
        .get("data")
        .and_then(|v| v.get("repository"))
        .and_then(|v| v.get("pullRequest"))
        .ok_or_else(|| format!("Pull request {repository}#{number} was not found."))?;

    let diff_output = gh::run_owned(vec![
        "pr".to_string(),
        "diff".to_string(),
        number.to_string(),
        "--repo".to_string(),
        repository.to_string(),
    ])?;

    if diff_output.exit_code != Some(0) {
        return Err(combine_process_error(
            diff_output,
            &format!("Failed to fetch diff for {repository}#{number}"),
        ));
    }

    let parsed_diff = crate::diff::parse_unified_diff(&diff_output.stdout);
    let raw_diff = diff_output.stdout;

    Ok(PullRequestDetail {
        id: str_field(pr, "id"),
        repository: repository.to_string(),
        number,
        title: str_field(pr, "title"),
        body: str_field(pr, "body"),
        url: str_field(pr, "url"),
        author_login: pr
            .get("author")
            .and_then(|v| v.get("login"))
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        state: str_field_or(pr, "state", "OPEN"),
        is_draft: pr.get("isDraft").and_then(Value::as_bool).unwrap_or(false),
        review_decision: pr
            .get("reviewDecision")
            .and_then(Value::as_str)
            .map(str::to_string),
        base_ref_name: str_field(pr, "baseRefName"),
        head_ref_name: str_field(pr, "headRefName"),
        base_ref_oid: pr
            .get("baseRefOid")
            .and_then(Value::as_str)
            .map(str::to_string),
        head_ref_oid: pr
            .get("headRefOid")
            .and_then(Value::as_str)
            .map(str::to_string),
        additions: i64_field(pr, "additions"),
        deletions: i64_field(pr, "deletions"),
        changed_files: i64_field(pr, "changedFiles"),
        comments_count: pr
            .get("comments")
            .and_then(|v| v.get("totalCount"))
            .and_then(Value::as_i64)
            .unwrap_or(0),
        commits_count: pr
            .get("commits")
            .and_then(|v| v.get("totalCount"))
            .and_then(Value::as_i64)
            .unwrap_or(0),
        created_at: str_field(pr, "createdAt"),
        updated_at: str_field(pr, "updatedAt"),
        labels: pr
            .get("labels")
            .and_then(|v| v.get("nodes"))
            .and_then(Value::as_array)
            .map(|nodes| {
                nodes
                    .iter()
                    .filter_map(|n| n.get("name").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        reviewers: pr
            .get("reviewRequests")
            .and_then(|v| v.get("nodes"))
            .and_then(Value::as_array)
            .map(|nodes| {
                nodes
                    .iter()
                    .filter_map(|n| n.get("requestedReviewer"))
                    .filter_map(|r| {
                        r.get("login")
                            .or_else(|| r.get("slug"))
                            .and_then(Value::as_str)
                    })
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        latest_reviews: pr
            .get("latestReviews")
            .and_then(|v| v.get("nodes"))
            .and_then(Value::as_array)
            .map(|nodes| {
                nodes
                    .iter()
                    .map(|n| PullRequestReview {
                        author_login: n
                            .get("author")
                            .and_then(|v| v.get("login"))
                            .and_then(Value::as_str)
                            .unwrap_or("unknown")
                            .to_string(),
                        state: str_field_or(n, "state", "COMMENTED"),
                        body: str_field(n, "body"),
                        submitted_at: n
                            .get("submittedAt")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        review_threads: pr
            .get("reviewThreads")
            .and_then(|v| v.get("nodes"))
            .and_then(Value::as_array)
            .map(|nodes| nodes.iter().filter_map(map_review_thread).collect())
            .unwrap_or_default(),
        files: pr
            .get("files")
            .and_then(|v| v.get("nodes"))
            .and_then(Value::as_array)
            .map(|nodes| {
                nodes
                    .iter()
                    .filter_map(|n| {
                        Some(PullRequestFile {
                            path: n.get("path")?.as_str()?.to_string(),
                            additions: i64_field(n, "additions"),
                            deletions: i64_field(n, "deletions"),
                            change_type: str_field_or(n, "changeType", "MODIFIED"),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default(),
        raw_diff,
        parsed_diff,
    })
}

fn cached_auth_state(cache: &CacheStore) -> Result<AuthState, String> {
    if let Some(cached) = cache.get::<AuthState>(AUTH_STATE_CACHE_KEY)? {
        return Ok(cached.value);
    }

    Ok(AuthState {
        is_authenticated: false,
        active_login: None,
        active_hostname: None,
        message: "Auth state not loaded yet. Sync the workspace to refresh GitHub auth."
            .to_string(),
    })
}

fn refresh_auth_state(cache: &CacheStore) -> Result<AuthState, String> {
    let auth = live_auth_state()?;
    cache.put(AUTH_STATE_CACHE_KEY, &auth, now_ms())?;
    Ok(auth)
}

fn live_auth_state() -> Result<AuthState, String> {
    let hostname = std::env::var("GH_HOST")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| "github.com".to_string());

    match gh::run_owned(vec![
        "api".to_string(),
        "user".to_string(),
        "--jq".to_string(),
        ".login".to_string(),
    ]) {
        Ok(output) if output.exit_code == Some(0) && !output.stdout.is_empty() => Ok(AuthState {
            is_authenticated: true,
            active_login: Some(output.stdout),
            active_hostname: Some(hostname.clone()),
            message: format!("Using gh auth on {}.", hostname),
        }),
        Ok(output) => Ok(AuthState {
            is_authenticated: false,
            active_login: None,
            active_hostname: None,
            message: combine_process_error(
                output,
                "gh is installed but not authenticated. Run `gh auth login` to load live GitHub data.",
            ),
        }),
        Err(error) => Ok(AuthState {
            is_authenticated: false,
            active_login: None,
            active_hostname: None,
            message: error,
        }),
    }
}

fn fetch_repository_file_content(
    repository: &str,
    reference: &str,
    path: &str,
) -> Result<RepositoryFileContent, String> {
    let (owner, name) = split_repository(repository)?;
    let encoded_path = path
        .split('/')
        .map(encode_uri_component)
        .collect::<Vec<_>>()
        .join("/");
    let endpoint = format!(
        "repos/{}/{}/contents/{}",
        encode_uri_component(owner),
        encode_uri_component(name),
        encoded_path
    );

    let output = Command::new("gh")
        .args([
            "api",
            &endpoint,
            "--method",
            "GET",
            "--header",
            "Accept: application/vnd.github.raw",
            "--field",
            &format!("ref={reference}"),
        ])
        .output()
        .map_err(|e| format!("Failed to launch gh: {e}"))?;

    if !output.status.success() {
        return Err(format!(
            "Failed to fetch file contents for {repository}@{reference}:{path}: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let size_bytes = output.stdout.len();
    match String::from_utf8(output.stdout) {
        Ok(content) => Ok(RepositoryFileContent {
            repository: repository.to_string(),
            reference: reference.to_string(),
            path: path.to_string(),
            content: Some(content),
            is_binary: false,
            size_bytes,
        }),
        Err(_) => Ok(RepositoryFileContent {
            repository: repository.to_string(),
            reference: reference.to_string(),
            path: path.to_string(),
            content: None,
            is_binary: true,
            size_bytes,
        }),
    }
}

fn map_pull_request_summary(node: &Value) -> Option<PullRequestSummary> {
    Some(PullRequestSummary {
        repository: node
            .get("repository")?
            .get("nameWithOwner")?
            .as_str()?
            .to_string(),
        number: node.get("number")?.as_i64()?,
        title: node.get("title")?.as_str()?.to_string(),
        author_login: node
            .get("author")
            .and_then(|v| v.get("login"))
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        comments_count: node
            .get("comments")
            .and_then(|v| v.get("totalCount"))
            .and_then(Value::as_i64)
            .unwrap_or(0),
        additions: i64_field(node, "additions"),
        deletions: i64_field(node, "deletions"),
        changed_files: i64_field(node, "changedFiles"),
        state: str_field_or(node, "state", "OPEN"),
        review_decision: node
            .get("reviewDecision")
            .and_then(Value::as_str)
            .map(str::to_string),
        updated_at: str_field(node, "updatedAt"),
        url: node.get("url")?.as_str()?.to_string(),
    })
}

fn map_review_thread(node: &Value) -> Option<PullRequestReviewThread> {
    Some(PullRequestReviewThread {
        id: node.get("id")?.as_str()?.to_string(),
        path: node.get("path")?.as_str()?.to_string(),
        line: node.get("line").and_then(Value::as_i64),
        original_line: node.get("originalLine").and_then(Value::as_i64),
        start_line: node.get("startLine").and_then(Value::as_i64),
        original_start_line: node.get("originalStartLine").and_then(Value::as_i64),
        diff_side: str_field_or(node, "diffSide", "RIGHT"),
        start_diff_side: node
            .get("startDiffSide")
            .and_then(Value::as_str)
            .map(str::to_string),
        is_collapsed: node
            .get("isCollapsed")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        is_outdated: node
            .get("isOutdated")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        is_resolved: node
            .get("isResolved")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        subject_type: str_field_or(node, "subjectType", "LINE"),
        resolved_by_login: node
            .get("resolvedBy")
            .and_then(|v| v.get("login"))
            .and_then(Value::as_str)
            .map(str::to_string),
        viewer_can_reply: node
            .get("viewerCanReply")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        viewer_can_resolve: node
            .get("viewerCanResolve")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        viewer_can_unresolve: node
            .get("viewerCanUnresolve")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        comments: node
            .get("comments")
            .and_then(|v| v.get("nodes"))
            .and_then(Value::as_array)
            .map(|nodes| {
                nodes
                    .iter()
                    .filter_map(|c| {
                        Some(PullRequestReviewComment {
                            id: c.get("id")?.as_str()?.to_string(),
                            author_login: c
                                .get("author")
                                .and_then(|v| v.get("login"))
                                .and_then(Value::as_str)
                                .unwrap_or("unknown")
                                .to_string(),
                            body: str_field(c, "body"),
                            path: c.get("path")?.as_str()?.to_string(),
                            line: c.get("line").and_then(Value::as_i64),
                            original_line: c.get("originalLine").and_then(Value::as_i64),
                            start_line: c.get("startLine").and_then(Value::as_i64),
                            original_start_line: c.get("originalStartLine").and_then(Value::as_i64),
                            state: str_field_or(c, "state", "PUBLISHED"),
                            created_at: str_field(c, "createdAt"),
                            updated_at: str_field(c, "updatedAt"),
                            published_at: c
                                .get("publishedAt")
                                .and_then(Value::as_str)
                                .map(str::to_string),
                            reply_to_id: c
                                .get("replyTo")
                                .and_then(|v| v.get("id"))
                                .and_then(Value::as_str)
                                .map(str::to_string),
                            url: str_field(c, "url"),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default(),
    })
}

fn graphql_error_message(response: &Value) -> Option<String> {
    response
        .get("errors")
        .and_then(Value::as_array)
        .filter(|errors| !errors.is_empty())
        .map(|errors| {
            errors
                .iter()
                .filter_map(|e| e.get("message").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .filter(|m| !m.trim().is_empty())
}

fn split_repository(repository: &str) -> Result<(&str, &str), String> {
    repository
        .split_once('/')
        .ok_or_else(|| format!("Invalid repository name '{repository}'. Expected owner/name."))
}

fn default_queues() -> Vec<PullRequestQueue> {
    [
        "reviewRequested",
        "assigned",
        "authored",
        "mentioned",
        "involved",
    ]
    .iter()
    .zip([
        "Review requested",
        "Assigned",
        "Authored",
        "Mentioned",
        "Involved",
    ])
    .map(|(id, label)| PullRequestQueue {
        id: id.to_string(),
        label: label.to_string(),
        items: Vec::new(),
        total_count: 0,
    })
    .collect()
}

fn pull_request_detail_cache_key(repository: &str, number: i64) -> String {
    format!("pr-detail-v2:{}#{}", repository, number)
}

fn default_change_type() -> String {
    "MODIFIED".to_string()
}

fn encode_uri_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(byte as char);
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

fn combine_process_error(output: CommandOutput, prefix: &str) -> String {
    if !output.stderr.is_empty() {
        format!("{prefix}: {}", output.stderr)
    } else if !output.stdout.is_empty() {
        format!("{prefix}: {}", output.stdout)
    } else {
        prefix.to_string()
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn str_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn str_field_or(value: &Value, key: &str, default: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or(default)
        .to_string()
}

fn i64_field(value: &Value, key: &str) -> i64 {
    value.get(key).and_then(Value::as_i64).unwrap_or(0)
}
