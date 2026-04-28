use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{
    cache::CacheStore,
    github::{
        self, PullRequestDetail, PullRequestReviewComment, PullRequestReviewThread,
        PullRequestSummary, WorkspaceSnapshot,
    },
    platform_macos,
    state::pr_key,
};

const NOTIFICATION_STATE_CACHE_KEY: &str = "notification-state-v1";
const REVIEW_COMMENT_READ_STATE_CACHE_KEY: &str = "review-comment-read-state-v1";

#[derive(Debug, Clone)]
pub struct WorkspaceSyncOutcome {
    pub workspace: WorkspaceSnapshot,
    pub notifications: Vec<SystemNotification>,
    pub unread_review_comment_ids: BTreeSet<String>,
    pub review_detail_snapshots: Vec<github::PullRequestDetailSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemNotification {
    pub title: String,
    pub body: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
struct PersistedNotificationState {
    review_requested_pr_keys: Vec<String>,
    thread_last_comment_ids: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
struct ReviewCommentReadState {
    observed_pr_keys: BTreeSet<String>,
    known_comment_ids: BTreeSet<String>,
    unread_comment_ids: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NotificationInput {
    review_requested_prs: Vec<TrackedPullRequest>,
    tracked_threads: Vec<TrackedReviewThread>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TrackedPullRequest {
    pr_key: String,
    repository: String,
    number: i64,
    title: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TrackedReviewThread {
    id: String,
    pull_request: TrackedPullRequest,
    owner_login: String,
    comments: Vec<TrackedComment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TrackedComment {
    id: String,
    author_login: String,
    body: String,
}

struct NotificationEvaluation {
    state: PersistedNotificationState,
    notifications: Vec<SystemNotification>,
}

pub fn sync_workspace_with_notifications(
    cache: &CacheStore,
) -> Result<WorkspaceSyncOutcome, String> {
    let workspace = github::sync_workspace_snapshot(cache)?;
    let previous = cache
        .get::<PersistedNotificationState>(NOTIFICATION_STATE_CACHE_KEY)?
        .map(|document| document.value);
    let (input, review_detail_snapshots) = build_notification_input(cache, &workspace);
    let evaluation = evaluate_notifications(&input, previous.as_ref());
    cache.put(
        NOTIFICATION_STATE_CACHE_KEY,
        &evaluation.state,
        notification_timestamp_ms(),
    )?;

    Ok(WorkspaceSyncOutcome {
        workspace,
        notifications: evaluation.notifications,
        unread_review_comment_ids: load_unread_review_comment_ids(cache)?,
        review_detail_snapshots,
    })
}

pub fn load_unread_review_comment_ids(cache: &CacheStore) -> Result<BTreeSet<String>, String> {
    Ok(load_review_comment_read_state(cache)?.unread_comment_ids)
}

pub fn mark_review_comments_read<I>(
    cache: &CacheStore,
    comment_ids: I,
) -> Result<BTreeSet<String>, String>
where
    I: IntoIterator<Item = String>,
{
    let mut read_state = load_review_comment_read_state(cache)?;
    for comment_id in comment_ids {
        read_state.unread_comment_ids.remove(&comment_id);
    }
    save_review_comment_read_state(cache, &read_state)?;
    Ok(read_state.unread_comment_ids)
}

pub fn sync_pull_request_detail_with_read_state(
    cache: &CacheStore,
    repository: &str,
    number: i64,
) -> Result<(github::PullRequestDetailSnapshot, BTreeSet<String>), String> {
    let snapshot = github::sync_pull_request_detail(cache, repository, number)?;
    if let Some(detail) = snapshot.detail.as_ref() {
        let viewer_login = snapshot.auth.active_login.as_deref();
        let unread_ids = record_review_comments(cache, detail, viewer_login)?;
        return Ok((snapshot, unread_ids));
    }

    Ok((snapshot, load_unread_review_comment_ids(cache)?))
}

pub fn deliver_system_notifications(notifications: &[SystemNotification]) {
    for notification in notifications {
        if let Err(error) =
            platform_macos::deliver_system_notification(&notification.title, &notification.body)
        {
            eprintln!(
                "Failed to deliver system notification '{}': {error}",
                notification.title
            );
        }
    }
}

fn build_notification_input(
    cache: &CacheStore,
    workspace: &WorkspaceSnapshot,
) -> (NotificationInput, Vec<github::PullRequestDetailSnapshot>) {
    let review_requested_prs = review_requested_pull_requests(workspace);
    let viewer_login = workspace
        .viewer
        .as_ref()
        .map(|viewer| viewer.login.as_str())
        .or(workspace.auth.active_login.as_deref())
        .unwrap_or_default()
        .to_string();

    let mut tracked_threads = Vec::new();
    let mut review_detail_snapshots = Vec::new();

    for pull_request in &review_requested_prs {
        match github::sync_pull_request_detail(cache, &pull_request.repository, pull_request.number)
        {
            Ok(snapshot) => {
                if let Some(detail) = snapshot.detail.as_ref() {
                    if let Err(error) = record_review_comments(cache, detail, Some(&viewer_login)) {
                        eprintln!(
                            "Failed to record review comment read state for {}#{}: {error}",
                            pull_request.repository, pull_request.number
                        );
                    }
                    tracked_threads.extend(extract_tracked_threads(
                        detail,
                        pull_request,
                        &viewer_login,
                    ));
                }
                review_detail_snapshots.push(snapshot);
            }
            Err(error) => {
                eprintln!(
                    "Failed to load review threads for {}#{} notifications: {error}",
                    pull_request.repository, pull_request.number
                );
            }
        }
    }

    (
        NotificationInput {
            review_requested_prs,
            tracked_threads,
        },
        review_detail_snapshots,
    )
}

fn record_review_comments(
    cache: &CacheStore,
    detail: &PullRequestDetail,
    viewer_login: Option<&str>,
) -> Result<BTreeSet<String>, String> {
    let mut read_state = load_review_comment_read_state(cache)?;
    let detail_key = pr_key(&detail.repository, detail.number);
    let first_pr_observation = !read_state.observed_pr_keys.contains(&detail_key);
    let viewer_login = viewer_login.unwrap_or_default();

    for comment in detail
        .review_threads
        .iter()
        .flat_map(|thread| &thread.comments)
    {
        let newly_known = read_state.known_comment_ids.insert(comment.id.clone());
        if newly_known && !first_pr_observation && comment.author_login != viewer_login {
            read_state.unread_comment_ids.insert(comment.id.clone());
        }
    }

    read_state.observed_pr_keys.insert(detail_key);
    save_review_comment_read_state(cache, &read_state)?;
    Ok(read_state.unread_comment_ids)
}

fn load_review_comment_read_state(cache: &CacheStore) -> Result<ReviewCommentReadState, String> {
    Ok(cache
        .get::<ReviewCommentReadState>(REVIEW_COMMENT_READ_STATE_CACHE_KEY)?
        .map(|document| document.value)
        .unwrap_or_default())
}

fn save_review_comment_read_state(
    cache: &CacheStore,
    read_state: &ReviewCommentReadState,
) -> Result<(), String> {
    cache.put(
        REVIEW_COMMENT_READ_STATE_CACHE_KEY,
        read_state,
        notification_timestamp_ms(),
    )
}

fn review_requested_pull_requests(workspace: &WorkspaceSnapshot) -> Vec<TrackedPullRequest> {
    workspace
        .queues
        .iter()
        .find(|queue| queue.id == "reviewRequested")
        .map(|queue| queue.items.iter().map(tracked_pull_request).collect())
        .unwrap_or_default()
}

fn tracked_pull_request(summary: &PullRequestSummary) -> TrackedPullRequest {
    TrackedPullRequest {
        pr_key: pr_key(&summary.repository, summary.number),
        repository: summary.repository.clone(),
        number: summary.number,
        title: summary.title.clone(),
    }
}

fn extract_tracked_threads(
    detail: &PullRequestDetail,
    pull_request: &TrackedPullRequest,
    viewer_login: &str,
) -> Vec<TrackedReviewThread> {
    detail
        .review_threads
        .iter()
        .filter_map(|thread| tracked_review_thread(thread, pull_request, viewer_login))
        .collect()
}

fn tracked_review_thread(
    thread: &PullRequestReviewThread,
    pull_request: &TrackedPullRequest,
    viewer_login: &str,
) -> Option<TrackedReviewThread> {
    let owner_login = thread.comments.first()?.author_login.clone();
    if owner_login != viewer_login {
        return None;
    }

    Some(TrackedReviewThread {
        id: thread.id.clone(),
        pull_request: pull_request.clone(),
        owner_login,
        comments: thread.comments.iter().map(tracked_comment).collect(),
    })
}

fn tracked_comment(comment: &PullRequestReviewComment) -> TrackedComment {
    TrackedComment {
        id: comment.id.clone(),
        author_login: comment.author_login.clone(),
        body: comment.body.clone(),
    }
}

fn evaluate_notifications(
    input: &NotificationInput,
    previous: Option<&PersistedNotificationState>,
) -> NotificationEvaluation {
    let next_state = PersistedNotificationState {
        review_requested_pr_keys: input
            .review_requested_prs
            .iter()
            .map(|pull_request| pull_request.pr_key.clone())
            .collect(),
        thread_last_comment_ids: input
            .tracked_threads
            .iter()
            .filter_map(|thread| Some((thread.id.clone(), thread.comments.last()?.id.clone())))
            .collect(),
    };

    let Some(previous) = previous else {
        return NotificationEvaluation {
            state: next_state,
            notifications: Vec::new(),
        };
    };

    let previous_review_requests = previous
        .review_requested_pr_keys
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut notifications = Vec::new();

    for pull_request in &input.review_requested_prs {
        if previous_review_requests.contains(&pull_request.pr_key) {
            continue;
        }

        notifications.push(SystemNotification {
            title: format!(
                "Review requested · {}#{}",
                pull_request.repository, pull_request.number
            ),
            body: summarize_text(&pull_request.title, 160),
        });
    }

    for thread in &input.tracked_threads {
        let Some(previous_comment_id) = previous.thread_last_comment_ids.get(&thread.id) else {
            continue;
        };
        let Some(previous_index) = thread
            .comments
            .iter()
            .position(|comment| comment.id == *previous_comment_id)
        else {
            continue;
        };

        for comment in thread.comments.iter().skip(previous_index + 1) {
            if comment.author_login == thread.owner_login {
                continue;
            }

            notifications.push(SystemNotification {
                title: format!(
                    "New comment on your review · {}#{}",
                    thread.pull_request.repository, thread.pull_request.number
                ),
                body: format!(
                    "{}: {}",
                    comment.author_login,
                    summarize_text(&comment.body, 160)
                ),
            });
        }
    }

    NotificationEvaluation {
        state: next_state,
        notifications,
    }
}

fn summarize_text(value: &str, max_len: usize) -> String {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return "No content".to_string();
    }

    if normalized.chars().count() <= max_len {
        return normalized;
    }

    normalized
        .chars()
        .take(max_len.saturating_sub(1))
        .collect::<String>()
        + "…"
}

fn notification_timestamp_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_cache() -> CacheStore {
        let path = std::env::temp_dir().join(format!(
            "remiss-notification-test-{}-{}.sqlite3",
            std::process::id(),
            notification_timestamp_ms()
        ));
        CacheStore::new(path).expect("failed to create temp cache")
    }

    fn pull_request(key: &str, repository: &str, number: i64, title: &str) -> TrackedPullRequest {
        TrackedPullRequest {
            pr_key: key.to_string(),
            repository: repository.to_string(),
            number,
            title: title.to_string(),
        }
    }

    fn comment(id: &str, author_login: &str, body: &str) -> TrackedComment {
        TrackedComment {
            id: id.to_string(),
            author_login: author_login.to_string(),
            body: body.to_string(),
        }
    }

    fn thread(
        id: &str,
        pull_request: &TrackedPullRequest,
        owner_login: &str,
        comments: Vec<TrackedComment>,
    ) -> TrackedReviewThread {
        TrackedReviewThread {
            id: id.to_string(),
            pull_request: pull_request.clone(),
            owner_login: owner_login.to_string(),
            comments,
        }
    }

    fn review_comment(id: &str, author_login: &str) -> PullRequestReviewComment {
        PullRequestReviewComment {
            id: id.to_string(),
            author_login: author_login.to_string(),
            author_avatar_url: None,
            body: format!("body-{id}"),
            path: "src/main.rs".to_string(),
            line: Some(1),
            original_line: Some(1),
            start_line: None,
            original_start_line: None,
            state: "PUBLISHED".to_string(),
            created_at: "2026-04-24T10:00:00Z".to_string(),
            updated_at: "2026-04-24T10:00:00Z".to_string(),
            published_at: Some("2026-04-24T10:00:00Z".to_string()),
            reply_to_id: None,
            url: format!("https://example.com/{id}"),
        }
    }

    fn review_thread(comments: Vec<PullRequestReviewComment>) -> PullRequestReviewThread {
        PullRequestReviewThread {
            id: "thread-1".to_string(),
            path: "src/main.rs".to_string(),
            line: Some(1),
            original_line: Some(1),
            start_line: None,
            original_start_line: None,
            diff_side: "RIGHT".to_string(),
            start_diff_side: None,
            is_collapsed: false,
            is_outdated: false,
            is_resolved: false,
            subject_type: "LINE".to_string(),
            resolved_by_login: None,
            viewer_can_reply: true,
            viewer_can_resolve: true,
            viewer_can_unresolve: false,
            comments,
        }
    }

    fn detail_with_thread_comments(
        repository: &str,
        number: i64,
        comments: Vec<PullRequestReviewComment>,
    ) -> PullRequestDetail {
        PullRequestDetail {
            id: "pr-id".to_string(),
            repository: repository.to_string(),
            number,
            title: "PR".to_string(),
            body: String::new(),
            url: "https://example.com/pr".to_string(),
            author_login: "author".to_string(),
            author_avatar_url: None,
            state: "OPEN".to_string(),
            is_draft: false,
            review_decision: None,
            base_ref_name: "main".to_string(),
            head_ref_name: "branch".to_string(),
            base_ref_oid: None,
            head_ref_oid: None,
            additions: 1,
            deletions: 0,
            changed_files: 1,
            comments_count: 0,
            commits_count: 1,
            created_at: "2026-04-24T09:00:00Z".to_string(),
            updated_at: "2026-04-24T10:00:00Z".to_string(),
            labels: Vec::new(),
            reviewers: Vec::new(),
            reviewer_avatar_urls: std::collections::BTreeMap::new(),
            comments: Vec::new(),
            latest_reviews: Vec::new(),
            review_threads: vec![review_thread(comments)],
            files: Vec::new(),
            raw_diff: String::new(),
            parsed_diff: Vec::new(),
            data_completeness: crate::github::PullRequestDataCompleteness::default(),
        }
    }

    #[test]
    fn first_sync_primes_state_without_notifications() {
        let pull_request = pull_request("org/repo#42", "org/repo", 42, "Improve review UX");
        let input = NotificationInput {
            review_requested_prs: vec![pull_request.clone()],
            tracked_threads: vec![thread(
                "thread-1",
                &pull_request,
                "me",
                vec![comment("c1", "me", "Please rename this")],
            )],
        };

        let evaluation = evaluate_notifications(&input, None);

        assert!(evaluation.notifications.is_empty());
        assert_eq!(
            evaluation.state.review_requested_pr_keys,
            vec!["org/repo#42".to_string()]
        );
        assert_eq!(
            evaluation.state.thread_last_comment_ids.get("thread-1"),
            Some(&"c1".to_string())
        );
    }

    #[test]
    fn notifies_when_pull_request_enters_review_requested_queue() {
        let input = NotificationInput {
            review_requested_prs: vec![pull_request(
                "org/repo#42",
                "org/repo",
                42,
                "Improve review UX",
            )],
            tracked_threads: Vec::new(),
        };
        let previous = PersistedNotificationState::default();

        let evaluation = evaluate_notifications(&input, Some(&previous));

        assert_eq!(evaluation.notifications.len(), 1);
        assert_eq!(
            evaluation.notifications[0].title,
            "Review requested · org/repo#42"
        );
    }

    #[test]
    fn notifies_for_new_foreign_comment_after_watermark() {
        let pull_request = pull_request("org/repo#42", "org/repo", 42, "Improve review UX");
        let input = NotificationInput {
            review_requested_prs: vec![pull_request.clone()],
            tracked_threads: vec![thread(
                "thread-1",
                &pull_request,
                "me",
                vec![
                    comment("c1", "me", "Please rename this"),
                    comment("c2", "alice", "Done"),
                ],
            )],
        };
        let previous = PersistedNotificationState {
            review_requested_pr_keys: vec!["org/repo#42".to_string()],
            thread_last_comment_ids: BTreeMap::from([("thread-1".to_string(), "c1".to_string())]),
        };

        let evaluation = evaluate_notifications(&input, Some(&previous));

        assert_eq!(evaluation.notifications.len(), 1);
        assert_eq!(
            evaluation.notifications[0].title,
            "New comment on your review · org/repo#42"
        );
        assert_eq!(evaluation.notifications[0].body, "alice: Done");
    }

    #[test]
    fn ignores_new_comments_authored_by_viewer() {
        let pull_request = pull_request("org/repo#42", "org/repo", 42, "Improve review UX");
        let input = NotificationInput {
            review_requested_prs: vec![pull_request.clone()],
            tracked_threads: vec![thread(
                "thread-1",
                &pull_request,
                "me",
                vec![
                    comment("c1", "me", "Please rename this"),
                    comment("c2", "me", "Following up"),
                ],
            )],
        };
        let previous = PersistedNotificationState {
            review_requested_pr_keys: vec!["org/repo#42".to_string()],
            thread_last_comment_ids: BTreeMap::from([("thread-1".to_string(), "c1".to_string())]),
        };

        let evaluation = evaluate_notifications(&input, Some(&previous));

        assert!(evaluation.notifications.is_empty());
    }

    #[test]
    fn does_not_notify_when_thread_is_seen_for_the_first_time() {
        let pull_request = pull_request("org/repo#42", "org/repo", 42, "Improve review UX");
        let input = NotificationInput {
            review_requested_prs: vec![pull_request.clone()],
            tracked_threads: vec![thread(
                "thread-1",
                &pull_request,
                "me",
                vec![
                    comment("c1", "me", "Please rename this"),
                    comment("c2", "alice", "Done"),
                ],
            )],
        };
        let previous = PersistedNotificationState {
            review_requested_pr_keys: vec!["org/repo#42".to_string()],
            thread_last_comment_ids: BTreeMap::new(),
        };

        let evaluation = evaluate_notifications(&input, Some(&previous));

        assert!(evaluation.notifications.is_empty());
    }

    #[test]
    fn review_comment_read_state_tracks_new_foreign_comments_until_marked_read() {
        let cache = temp_cache();
        let baseline =
            detail_with_thread_comments("org/repo", 42, vec![review_comment("c1", "alice")]);

        let unread =
            record_review_comments(&cache, &baseline, Some("me")).expect("baseline record failed");

        assert!(unread.is_empty());

        let updated = detail_with_thread_comments(
            "org/repo",
            42,
            vec![
                review_comment("c1", "alice"),
                review_comment("c2", "bob"),
                review_comment("c3", "me"),
            ],
        );
        let unread =
            record_review_comments(&cache, &updated, Some("me")).expect("updated record failed");

        assert_eq!(unread, BTreeSet::from(["c2".to_string()]));

        let unread =
            mark_review_comments_read(&cache, vec!["c2".to_string()]).expect("mark read failed");

        assert!(unread.is_empty());
    }
}
