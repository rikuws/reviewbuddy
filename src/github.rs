use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

use crate::{
    cache::{CacheStore, CachedDocument},
    diff::ParsedDiffFile,
    gh::{self, CommandOutput},
    stacks::model::StackPullRequestRef,
};

const WORKSPACE_CACHE_KEY: &str = "workspace-snapshot-v3";
const AUTH_STATE_CACHE_KEY: &str = "auth-state-v1";
const GITHUB_GRAPHQL_PAGE_SIZE: i64 = 100;
const GITHUB_SEARCH_RESULT_LIMIT: usize = 1_000;

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
    #[serde(default)]
    pub author_avatar_url: Option<String>,
    #[serde(default)]
    pub is_draft: bool,
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
    #[serde(default = "default_true")]
    pub is_complete: bool,
    #[serde(default)]
    pub truncated_reason: Option<String>,
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
    #[serde(default)]
    pub author_avatar_url: Option<String>,
    pub state: String,
    pub body: String,
    pub submitted_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequestReviewComment {
    pub id: String,
    pub author_login: String,
    #[serde(default)]
    pub author_avatar_url: Option<String>,
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
pub struct PullRequestComment {
    pub id: String,
    pub author_login: String,
    #[serde(default)]
    pub author_avatar_url: Option<String>,
    pub body: String,
    pub created_at: String,
    pub updated_at: String,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConnectionCompleteness {
    pub loaded_count: usize,
    pub total_count: i64,
    pub is_complete: bool,
    #[serde(default)]
    pub truncated_reason: Option<String>,
}

impl ConnectionCompleteness {
    fn from_counts(
        loaded_count: usize,
        total_count: i64,
        truncated_reason: Option<String>,
    ) -> Self {
        Self {
            loaded_count,
            total_count,
            is_complete: truncated_reason.is_none() && loaded_count as i64 >= total_count,
            truncated_reason,
        }
    }
}

impl Default for ConnectionCompleteness {
    fn default() -> Self {
        Self {
            loaded_count: 0,
            total_count: 0,
            is_complete: true,
            truncated_reason: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PullRequestDataCompleteness {
    pub comments: ConnectionCompleteness,
    pub labels: ConnectionCompleteness,
    pub reviewers: ConnectionCompleteness,
    pub latest_reviews: ConnectionCompleteness,
    pub review_threads: ConnectionCompleteness,
    pub review_thread_comments: ConnectionCompleteness,
    pub files: ConnectionCompleteness,
}

impl PullRequestDataCompleteness {
    pub fn is_complete(&self) -> bool {
        self.comments.is_complete
            && self.labels.is_complete
            && self.reviewers.is_complete
            && self.latest_reviews.is_complete
            && self.review_threads.is_complete
            && self.review_thread_comments.is_complete
            && self.files.is_complete
    }

    pub fn warnings(&self) -> Vec<String> {
        [
            ("comments", &self.comments),
            ("labels", &self.labels),
            ("reviewers", &self.reviewers),
            ("reviews", &self.latest_reviews),
            ("review threads", &self.review_threads),
            ("thread comments", &self.review_thread_comments),
            ("files", &self.files),
        ]
        .into_iter()
        .filter_map(|(label, completeness)| {
            if completeness.is_complete {
                return None;
            }

            Some(match completeness.truncated_reason.as_deref() {
                Some(reason) if !reason.is_empty() => format!(
                    "Loaded {} of {} {label}: {reason}",
                    completeness.loaded_count, completeness.total_count
                ),
                _ => format!(
                    "Loaded {} of {} {label}.",
                    completeness.loaded_count, completeness.total_count
                ),
            })
        })
        .collect()
    }
}

impl Default for PullRequestDataCompleteness {
    fn default() -> Self {
        Self {
            comments: ConnectionCompleteness::default(),
            labels: ConnectionCompleteness::default(),
            reviewers: ConnectionCompleteness::default(),
            latest_reviews: ConnectionCompleteness::default(),
            review_threads: ConnectionCompleteness::default(),
            review_thread_comments: ConnectionCompleteness::default(),
            files: ConnectionCompleteness::default(),
        }
    }
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
    #[serde(default)]
    pub author_avatar_url: Option<String>,
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
    #[serde(default)]
    pub reviewer_avatar_urls: BTreeMap<String, String>,
    #[serde(default)]
    pub comments: Vec<PullRequestComment>,
    pub latest_reviews: Vec<PullRequestReview>,
    pub review_threads: Vec<PullRequestReviewThread>,
    pub files: Vec<PullRequestFile>,
    pub raw_diff: String,
    pub parsed_diff: Vec<ParsedDiffFile>,
    #[serde(default)]
    pub data_completeness: PullRequestDataCompleteness,
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

pub fn fetch_open_pull_request_stack_refs(
    repository: &str,
) -> Result<Vec<StackPullRequestRef>, String> {
    let (owner, name) = split_repository(repository)?;
    let query = r#"
        query($owner: String!, $name: String!, $count: Int!, $cursor: String) {
          repository(owner: $owner, name: $name) {
            pullRequests(states: OPEN, first: $count, after: $cursor, orderBy: {field: UPDATED_AT, direction: DESC}) {
              pageInfo { hasNextPage endCursor }
              nodes {
                number
                title
                url
                state
                isDraft
                reviewDecision
                baseRefName
                headRefName
                baseRefOid
                headRefOid
                repository { nameWithOwner }
              }
            }
          }
        }
    "#;

    let mut refs = Vec::new();
    let mut cursor: Option<String> = None;

    loop {
        let response = gh::graphql(
            query,
            json!({
                "owner": owner,
                "name": name,
                "count": GITHUB_GRAPHQL_PAGE_SIZE,
                "cursor": cursor,
            }),
        )?;
        if let Some(error_message) = graphql_error_message(&response) {
            return Err(error_message);
        }

        let connection = response
            .get("data")
            .and_then(|data| data.get("repository"))
            .and_then(|repo| repo.get("pullRequests"))
            .ok_or_else(|| "Missing pullRequests data in GraphQL response.".to_string())?;

        if let Some(nodes) = connection_nodes(connection) {
            refs.extend(nodes.iter().filter_map(map_stack_pull_request_ref));
        }

        let page_info = page_info(connection);
        if !page_info.has_next_page {
            break;
        }
        let Some(next_cursor) = page_info.end_cursor else {
            break;
        };
        cursor = Some(next_cursor);
    }

    Ok(refs)
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
    cache: &CacheStore,
    repository: &str,
    reference: &str,
    path: &str,
) -> Result<RepositoryFileContent, String> {
    let key = pull_request_file_content_cache_key(repository, reference, path);
    if let Some(cached) = cache.get::<RepositoryFileContent>(&key)? {
        return Ok(cached.value);
    }

    let auth = live_auth_state()?;

    if !auth.is_authenticated {
        return Err(auth.message);
    }

    let document = fetch_repository_file_content(repository, reference, path)?;
    cache.put(&key, &document, now_ms())?;
    Ok(document)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepositoryFileContent {
    pub repository: String,
    pub reference: String,
    pub path: String,
    pub content: Option<String>,
    pub is_binary: bool,
    pub size_bytes: usize,
    #[serde(default = "default_repository_file_source")]
    pub source: String,
}

pub const REPOSITORY_FILE_SOURCE_GITHUB: &str = "github";
pub const REPOSITORY_FILE_SOURCE_LOCAL_CHECKOUT: &str = "local-checkout";

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
        query($searchQuery: String!, $count: Int!, $cursor: String) {
          search(query: $searchQuery, type: ISSUE, first: $count, after: $cursor) {
            issueCount
            pageInfo { hasNextPage endCursor }
            nodes {
              ... on PullRequest {
                number
                title
                url
                state
                isDraft
                updatedAt
                additions
                deletions
                changedFiles
                reviewDecision
                author { login avatarUrl }
                comments { totalCount }
                repository { nameWithOwner }
              }
            }
          }
        }
    "#;

    let mut items = Vec::new();
    let mut total_count = 0;
    let mut cursor: Option<String> = None;
    let mut truncated_reason = None;

    loop {
        let response = gh::graphql(
            query,
            json!({
                "searchQuery": search_query,
                "count": GITHUB_GRAPHQL_PAGE_SIZE,
                "cursor": cursor,
            }),
        )?;
        if let Some(error_message) = graphql_error_message(&response) {
            return Err(error_message);
        }
        let search = response
            .get("data")
            .and_then(|v| v.get("search"))
            .ok_or_else(|| "Missing search data in GraphQL response.".to_string())?;

        total_count = search
            .get("issueCount")
            .and_then(Value::as_i64)
            .unwrap_or(total_count);

        if let Some(nodes) = connection_nodes(search) {
            items.extend(nodes.iter().filter_map(map_pull_request_summary));
        }

        let page_info = page_info(search);
        if !page_info.has_next_page {
            break;
        }

        if items.len() >= GITHUB_SEARCH_RESULT_LIMIT {
            truncated_reason = Some(format!(
                "GitHub search results are capped at {GITHUB_SEARCH_RESULT_LIMIT} items."
            ));
            break;
        }

        let Some(next_cursor) = page_info.end_cursor else {
            truncated_reason =
                Some("GitHub did not return a cursor for the next search page.".to_string());
            break;
        };
        cursor = Some(next_cursor);
    }

    if truncated_reason.is_none() && (items.len() as i64) < total_count {
        truncated_reason = Some(
            "GitHub reported more matching pull requests than the search connection returned."
                .to_string(),
        );
    }

    Ok(PullRequestQueue {
        id: id.to_string(),
        label: label.to_string(),
        items,
        total_count,
        is_complete: truncated_reason.is_none(),
        truncated_reason,
    })
}

fn fetch_pull_request_detail(repository: &str, number: i64) -> Result<PullRequestDetail, String> {
    let (owner, name) = split_repository(repository)?;
    let query = r#"
        query($owner: String!, $name: String!, $number: Int!, $count: Int!) {
          repository(owner: $owner, name: $name) {
            pullRequest(number: $number) {
              id number title body url state isDraft reviewDecision
              baseRefName headRefName baseRefOid headRefOid
              additions deletions changedFiles createdAt updatedAt
              author { login avatarUrl }
              comments(first: $count) {
                totalCount
                pageInfo { hasNextPage endCursor }
                nodes {
                  id
                  body
                  createdAt
                  updatedAt
                  url
                  author { login avatarUrl }
                }
              }
              commits { totalCount }
              labels(first: $count) {
                totalCount
                pageInfo { hasNextPage endCursor }
                nodes { name }
              }
              reviewRequests(first: $count) {
                totalCount
                pageInfo { hasNextPage endCursor }
                nodes { requestedReviewer { ... on User { login avatarUrl } ... on Team { slug avatarUrl } } }
              }
              latestReviews(first: $count) {
                totalCount
                pageInfo { hasNextPage endCursor }
                nodes { state body submittedAt author { login avatarUrl } }
              }
              reviewThreads(first: $count) {
                totalCount
                pageInfo { hasNextPage endCursor }
                nodes {
                  id path line originalLine startLine originalStartLine
                  diffSide startDiffSide isCollapsed isOutdated isResolved
                  subjectType viewerCanReply viewerCanResolve viewerCanUnresolve
                  resolvedBy { login avatarUrl }
                  comments(first: $count) {
                    totalCount
                    pageInfo { hasNextPage endCursor }
                    nodes {
                      id body path line originalLine startLine originalStartLine
                      state createdAt updatedAt publishedAt url
                      replyTo { id }
                      author { login avatarUrl }
                    }
                  }
                }
              }
              files(first: $count) {
                totalCount
                pageInfo { hasNextPage endCursor }
                nodes { path additions deletions changeType }
              }
            }
          }
        }
    "#;

    let response = gh::graphql(
        query,
        json!({ "owner": owner, "name": name, "number": number, "count": GITHUB_GRAPHQL_PAGE_SIZE }),
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

    let author = pr.get("author");
    let null = Value::Null;
    let comments_connection = pr.get("comments").unwrap_or(&null);
    let labels_connection = pr.get("labels").unwrap_or(&null);
    let review_requests_connection = pr.get("reviewRequests").unwrap_or(&null);
    let latest_reviews_connection = pr.get("latestReviews").unwrap_or(&null);
    let review_threads_connection = pr.get("reviewThreads").unwrap_or(&null);
    let files_connection = pr.get("files").unwrap_or(&null);

    let mut comments = map_connection_items(comments_connection, map_pull_request_comment);
    let comments_completeness = append_pull_request_connection_pages(
        owner,
        name,
        number,
        "comments",
        pull_request_comments_selection(),
        comments_connection,
        &mut comments,
        map_pull_request_comment,
    )?;

    let mut labels = map_connection_items(labels_connection, map_label_name);
    let labels_completeness = append_pull_request_connection_pages(
        owner,
        name,
        number,
        "labels",
        pull_request_labels_selection(),
        labels_connection,
        &mut labels,
        map_label_name,
    )?;

    let mut reviewer_items = map_connection_items(review_requests_connection, map_review_request);
    let reviewers_completeness = append_pull_request_connection_pages(
        owner,
        name,
        number,
        "reviewRequests",
        pull_request_review_requests_selection(),
        review_requests_connection,
        &mut reviewer_items,
        map_review_request,
    )?;
    let (reviewers, reviewer_avatar_urls) = split_reviewer_items(reviewer_items);

    let mut latest_reviews =
        map_connection_items(latest_reviews_connection, map_pull_request_review);
    let latest_reviews_completeness = append_pull_request_connection_pages(
        owner,
        name,
        number,
        "latestReviews",
        pull_request_latest_reviews_selection(),
        latest_reviews_connection,
        &mut latest_reviews,
        map_pull_request_review,
    )?;

    let mut review_thread_pages =
        map_connection_items(review_threads_connection, map_review_thread_page);
    let review_threads_completeness = append_pull_request_connection_pages(
        owner,
        name,
        number,
        "reviewThreads",
        pull_request_review_threads_selection(),
        review_threads_connection,
        &mut review_thread_pages,
        map_review_thread_page,
    )?;
    let review_thread_comments_completeness =
        append_review_thread_comment_pages(&mut review_thread_pages)?;
    let review_threads = review_thread_pages
        .into_iter()
        .map(|page| page.thread)
        .collect::<Vec<_>>();

    let mut files = map_connection_items(files_connection, map_pull_request_file);
    let files_completeness = append_pull_request_connection_pages(
        owner,
        name,
        number,
        "files",
        pull_request_files_selection(),
        files_connection,
        &mut files,
        map_pull_request_file,
    )?;

    let data_completeness = PullRequestDataCompleteness {
        comments: comments_completeness,
        labels: labels_completeness,
        reviewers: reviewers_completeness,
        latest_reviews: latest_reviews_completeness,
        review_threads: review_threads_completeness,
        review_thread_comments: review_thread_comments_completeness,
        files: files_completeness,
    };

    Ok(PullRequestDetail {
        id: str_field(pr, "id"),
        repository: repository.to_string(),
        number,
        title: str_field(pr, "title"),
        body: str_field(pr, "body"),
        url: str_field(pr, "url"),
        author_login: actor_login(author).unwrap_or("unknown").to_string(),
        author_avatar_url: actor_avatar_url(author).map(str::to_string),
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
        labels,
        comments,
        reviewers,
        reviewer_avatar_urls,
        latest_reviews,
        review_threads,
        files,
        raw_diff,
        parsed_diff,
        data_completeness,
    })
}

#[derive(Debug, Clone, Default)]
struct PageInfo {
    has_next_page: bool,
    end_cursor: Option<String>,
}

#[derive(Debug, Clone)]
struct ReviewThreadPage {
    thread: PullRequestReviewThread,
    comments_total_count: i64,
    comments_page_info: PageInfo,
}

fn connection_nodes(connection: &Value) -> Option<&Vec<Value>> {
    connection.get("nodes").and_then(Value::as_array)
}

fn connection_total_count(connection: &Value) -> i64 {
    connection
        .get("totalCount")
        .or_else(|| connection.get("issueCount"))
        .and_then(Value::as_i64)
        .unwrap_or(0)
}

fn page_info(connection: &Value) -> PageInfo {
    let page_info = connection.get("pageInfo");
    PageInfo {
        has_next_page: page_info
            .and_then(|value| value.get("hasNextPage"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        end_cursor: page_info
            .and_then(|value| value.get("endCursor"))
            .and_then(Value::as_str)
            .map(str::to_string),
    }
}

fn map_connection_items<T>(
    connection: &Value,
    mut map_node: impl FnMut(&Value) -> Option<T>,
) -> Vec<T> {
    connection_nodes(connection)
        .map(|nodes| {
            nodes
                .iter()
                .filter_map(|node| map_node(node))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn append_pull_request_connection_pages<T>(
    owner: &str,
    name: &str,
    number: i64,
    connection_name: &str,
    selection: &str,
    initial_connection: &Value,
    items: &mut Vec<T>,
    mut map_node: impl FnMut(&Value) -> Option<T>,
) -> Result<ConnectionCompleteness, String> {
    let total_count = connection_total_count(initial_connection);
    let mut page = page_info(initial_connection);
    let mut truncated_reason = None;

    while page.has_next_page {
        let Some(cursor) = page.end_cursor.clone() else {
            truncated_reason = Some(format!(
                "GitHub did not return a cursor for the next {connection_name} page."
            ));
            break;
        };
        let connection = fetch_pull_request_connection(
            owner,
            name,
            number,
            connection_name,
            selection,
            Some(cursor),
        )?;
        if let Some(nodes) = connection_nodes(&connection) {
            items.extend(nodes.iter().filter_map(|node| map_node(node)));
        }
        page = page_info(&connection);
    }

    if truncated_reason.is_none() && (items.len() as i64) < total_count {
        truncated_reason = Some(format!(
            "GitHub reported more {connection_name} than the connection returned."
        ));
    }

    Ok(ConnectionCompleteness::from_counts(
        items.len(),
        total_count,
        truncated_reason,
    ))
}

fn fetch_pull_request_connection(
    owner: &str,
    name: &str,
    number: i64,
    connection_name: &str,
    selection: &str,
    cursor: Option<String>,
) -> Result<Value, String> {
    let query = format!(
        r#"
        query($owner: String!, $name: String!, $number: Int!, $count: Int!, $cursor: String) {{
          repository(owner: $owner, name: $name) {{
            pullRequest(number: $number) {{
              {selection}
            }}
          }}
        }}
    "#
    );
    let response = gh::graphql(
        &query,
        json!({
            "owner": owner,
            "name": name,
            "number": number,
            "count": GITHUB_GRAPHQL_PAGE_SIZE,
            "cursor": cursor,
        }),
    )?;
    if let Some(error_message) = graphql_error_message(&response) {
        return Err(error_message);
    }

    response
        .get("data")
        .and_then(|value| value.get("repository"))
        .and_then(|value| value.get("pullRequest"))
        .and_then(|value| value.get(connection_name))
        .cloned()
        .ok_or_else(|| format!("Missing {connection_name} data in GraphQL response."))
}

fn append_review_thread_comment_pages(
    thread_pages: &mut [ReviewThreadPage],
) -> Result<ConnectionCompleteness, String> {
    let mut total_count = 0;
    let mut truncated_reason = None;

    for thread_page in thread_pages.iter_mut() {
        total_count += thread_page.comments_total_count;
        let mut page = thread_page.comments_page_info.clone();

        while page.has_next_page {
            let Some(cursor) = page.end_cursor.clone() else {
                truncated_reason = Some(format!(
                    "GitHub did not return a cursor for comments in thread {}.",
                    thread_page.thread.id
                ));
                break;
            };
            let connection =
                fetch_review_thread_comments_connection(&thread_page.thread.id, cursor)?;
            if let Some(nodes) = connection_nodes(&connection) {
                thread_page
                    .thread
                    .comments
                    .extend(nodes.iter().filter_map(map_review_comment));
            }
            page = page_info(&connection);
        }

        if (thread_page.thread.comments.len() as i64) < thread_page.comments_total_count
            && truncated_reason.is_none()
        {
            truncated_reason = Some(format!(
                "GitHub reported more comments than returned for thread {}.",
                thread_page.thread.id
            ));
        }
    }

    let loaded_count = thread_pages
        .iter()
        .map(|thread_page| thread_page.thread.comments.len())
        .sum();
    Ok(ConnectionCompleteness::from_counts(
        loaded_count,
        total_count,
        truncated_reason,
    ))
}

fn fetch_review_thread_comments_connection(
    thread_id: &str,
    cursor: String,
) -> Result<Value, String> {
    let query = r#"
        query($threadId: ID!, $count: Int!, $cursor: String) {
          node(id: $threadId) {
            ... on PullRequestReviewThread {
              comments(first: $count, after: $cursor) {
                totalCount
                pageInfo { hasNextPage endCursor }
                nodes {
                  id body path line originalLine startLine originalStartLine
                  state createdAt updatedAt publishedAt url
                  replyTo { id }
                  author { login avatarUrl }
                }
              }
            }
          }
        }
    "#;
    let response = gh::graphql(
        query,
        json!({
            "threadId": thread_id,
            "count": GITHUB_GRAPHQL_PAGE_SIZE,
            "cursor": cursor,
        }),
    )?;
    if let Some(error_message) = graphql_error_message(&response) {
        return Err(error_message);
    }

    response
        .get("data")
        .and_then(|value| value.get("node"))
        .and_then(|value| value.get("comments"))
        .cloned()
        .ok_or_else(|| format!("Missing comments data for review thread {thread_id}."))
}

fn pull_request_comments_selection() -> &'static str {
    r#"
      comments(first: $count, after: $cursor) {
        totalCount
        pageInfo { hasNextPage endCursor }
        nodes {
          id body createdAt updatedAt url
          author { login avatarUrl }
        }
      }
    "#
}

fn pull_request_labels_selection() -> &'static str {
    r#"
      labels(first: $count, after: $cursor) {
        totalCount
        pageInfo { hasNextPage endCursor }
        nodes { name }
      }
    "#
}

fn pull_request_review_requests_selection() -> &'static str {
    r#"
      reviewRequests(first: $count, after: $cursor) {
        totalCount
        pageInfo { hasNextPage endCursor }
        nodes {
          requestedReviewer {
            ... on User { login avatarUrl }
            ... on Team { slug avatarUrl }
          }
        }
      }
    "#
}

fn pull_request_latest_reviews_selection() -> &'static str {
    r#"
      latestReviews(first: $count, after: $cursor) {
        totalCount
        pageInfo { hasNextPage endCursor }
        nodes { state body submittedAt author { login avatarUrl } }
      }
    "#
}

fn pull_request_review_threads_selection() -> &'static str {
    r#"
      reviewThreads(first: $count, after: $cursor) {
        totalCount
        pageInfo { hasNextPage endCursor }
        nodes {
          id path line originalLine startLine originalStartLine
          diffSide startDiffSide isCollapsed isOutdated isResolved
          subjectType viewerCanReply viewerCanResolve viewerCanUnresolve
          resolvedBy { login avatarUrl }
          comments(first: $count) {
            totalCount
            pageInfo { hasNextPage endCursor }
            nodes {
              id body path line originalLine startLine originalStartLine
              state createdAt updatedAt publishedAt url
              replyTo { id }
              author { login avatarUrl }
            }
          }
        }
      }
    "#
}

fn pull_request_files_selection() -> &'static str {
    r#"
      files(first: $count, after: $cursor) {
        totalCount
        pageInfo { hasNextPage endCursor }
        nodes { path additions deletions changeType }
      }
    "#
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

    let output = gh::run_owned(vec![
        "api".to_string(),
        endpoint,
        "--method".to_string(),
        "GET".to_string(),
        "--header".to_string(),
        "Accept: application/vnd.github.raw".to_string(),
        "--field".to_string(),
        format!("ref={reference}"),
    ])?;

    if output.exit_code != Some(0) {
        return Err(format!(
            "Failed to fetch file contents for {repository}@{reference}:{path}: {}",
            output.stderr
        ));
    }

    let size_bytes = output.stdout_bytes.len();
    match String::from_utf8(output.stdout_bytes) {
        Ok(content) => Ok(RepositoryFileContent {
            repository: repository.to_string(),
            reference: reference.to_string(),
            path: path.to_string(),
            content: Some(content),
            is_binary: false,
            size_bytes,
            source: REPOSITORY_FILE_SOURCE_GITHUB.to_string(),
        }),
        Err(_) => Ok(RepositoryFileContent {
            repository: repository.to_string(),
            reference: reference.to_string(),
            path: path.to_string(),
            content: None,
            is_binary: true,
            size_bytes,
            source: REPOSITORY_FILE_SOURCE_GITHUB.to_string(),
        }),
    }
}

fn default_repository_file_source() -> String {
    REPOSITORY_FILE_SOURCE_GITHUB.to_string()
}

fn map_pull_request_summary(node: &Value) -> Option<PullRequestSummary> {
    let author = node.get("author");
    Some(PullRequestSummary {
        repository: node
            .get("repository")?
            .get("nameWithOwner")?
            .as_str()?
            .to_string(),
        number: node.get("number")?.as_i64()?,
        title: node.get("title")?.as_str()?.to_string(),
        author_login: actor_login(author).unwrap_or("unknown").to_string(),
        author_avatar_url: actor_avatar_url(author).map(str::to_string),
        is_draft: node
            .get("isDraft")
            .and_then(Value::as_bool)
            .unwrap_or(false),
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

fn map_stack_pull_request_ref(node: &Value) -> Option<StackPullRequestRef> {
    Some(StackPullRequestRef {
        repository: node
            .get("repository")
            .and_then(|repo| repo.get("nameWithOwner"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        number: i64_field(node, "number"),
        title: str_field(node, "title"),
        url: str_field(node, "url"),
        base_ref_name: str_field(node, "baseRefName"),
        head_ref_name: str_field(node, "headRefName"),
        base_ref_oid: node
            .get("baseRefOid")
            .and_then(Value::as_str)
            .map(str::to_string),
        head_ref_oid: node
            .get("headRefOid")
            .and_then(Value::as_str)
            .map(str::to_string),
        review_decision: node
            .get("reviewDecision")
            .and_then(Value::as_str)
            .map(str::to_string),
        state: str_field_or(node, "state", "OPEN"),
        is_draft: node
            .get("isDraft")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

fn map_pull_request_comment(node: &Value) -> Option<PullRequestComment> {
    Some(PullRequestComment {
        id: node.get("id")?.as_str()?.to_string(),
        author_login: actor_login(node.get("author"))
            .unwrap_or("unknown")
            .to_string(),
        author_avatar_url: actor_avatar_url(node.get("author")).map(str::to_string),
        body: str_field(node, "body"),
        created_at: str_field(node, "createdAt"),
        updated_at: str_field(node, "updatedAt"),
        url: node.get("url")?.as_str()?.to_string(),
    })
}

fn map_label_name(node: &Value) -> Option<String> {
    node.get("name").and_then(Value::as_str).map(str::to_string)
}

fn map_review_request(node: &Value) -> Option<(String, Option<String>)> {
    let reviewer = node.get("requestedReviewer")?;
    let login = actor_login(Some(reviewer))?.to_string();
    let avatar_url = actor_avatar_url(Some(reviewer)).map(str::to_string);
    Some((login, avatar_url))
}

fn split_reviewer_items(
    reviewer_items: Vec<(String, Option<String>)>,
) -> (Vec<String>, BTreeMap<String, String>) {
    let mut reviewers = Vec::new();
    let mut avatar_urls = BTreeMap::new();
    for (reviewer, avatar_url) in reviewer_items {
        if !reviewers.iter().any(|existing| existing == &reviewer) {
            reviewers.push(reviewer.clone());
        }
        if let Some(avatar_url) = avatar_url {
            avatar_urls.insert(reviewer, avatar_url);
        }
    }
    (reviewers, avatar_urls)
}

fn map_pull_request_review(node: &Value) -> Option<PullRequestReview> {
    Some(PullRequestReview {
        author_login: actor_login(node.get("author"))
            .unwrap_or("unknown")
            .to_string(),
        author_avatar_url: actor_avatar_url(node.get("author")).map(str::to_string),
        state: str_field_or(node, "state", "COMMENTED"),
        body: str_field(node, "body"),
        submitted_at: node
            .get("submittedAt")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

fn map_pull_request_file(node: &Value) -> Option<PullRequestFile> {
    Some(PullRequestFile {
        path: node.get("path")?.as_str()?.to_string(),
        additions: i64_field(node, "additions"),
        deletions: i64_field(node, "deletions"),
        change_type: str_field_or(node, "changeType", "MODIFIED"),
    })
}

fn actor_login(actor: Option<&Value>) -> Option<&str> {
    actor.and_then(|actor| {
        actor
            .get("login")
            .or_else(|| actor.get("slug"))
            .and_then(Value::as_str)
    })
}

fn actor_avatar_url(actor: Option<&Value>) -> Option<&str> {
    actor
        .and_then(|actor| actor.get("avatarUrl"))
        .and_then(Value::as_str)
        .filter(|url| !url.trim().is_empty())
}

fn map_review_thread_page(node: &Value) -> Option<ReviewThreadPage> {
    let null = Value::Null;
    let comments_connection = node.get("comments").unwrap_or(&null);
    Some(ReviewThreadPage {
        thread: map_review_thread(node)?,
        comments_total_count: connection_total_count(comments_connection),
        comments_page_info: page_info(comments_connection),
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
            .map(|connection| map_connection_items(connection, map_review_comment))
            .unwrap_or_default(),
    })
}

fn map_review_comment(node: &Value) -> Option<PullRequestReviewComment> {
    Some(PullRequestReviewComment {
        id: node.get("id")?.as_str()?.to_string(),
        author_login: actor_login(node.get("author"))
            .unwrap_or("unknown")
            .to_string(),
        author_avatar_url: actor_avatar_url(node.get("author")).map(str::to_string),
        body: str_field(node, "body"),
        path: node.get("path")?.as_str()?.to_string(),
        line: node.get("line").and_then(Value::as_i64),
        original_line: node.get("originalLine").and_then(Value::as_i64),
        start_line: node.get("startLine").and_then(Value::as_i64),
        original_start_line: node.get("originalStartLine").and_then(Value::as_i64),
        state: str_field_or(node, "state", "PUBLISHED"),
        created_at: str_field(node, "createdAt"),
        updated_at: str_field(node, "updatedAt"),
        published_at: node
            .get("publishedAt")
            .and_then(Value::as_str)
            .map(str::to_string),
        reply_to_id: node
            .get("replyTo")
            .and_then(|value| value.get("id"))
            .and_then(Value::as_str)
            .map(str::to_string),
        url: str_field(node, "url"),
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
        is_complete: true,
        truncated_reason: None,
    })
    .collect()
}

fn pull_request_detail_cache_key(repository: &str, number: i64) -> String {
    format!("pr-detail-v4:{}#{}", repository, number)
}

fn pull_request_file_content_cache_key(repository: &str, reference: &str, path: &str) -> String {
    format!(
        "pr-file-v1:{}:{}:{}",
        encode_uri_component(repository),
        encode_uri_component(reference),
        path.split('/')
            .map(encode_uri_component)
            .collect::<Vec<_>>()
            .join("/")
    )
}

fn default_change_type() -> String {
    "MODIFIED".to_string()
}

fn default_true() -> bool {
    true
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
