use crate::{
    code_tour::DiffAnchor,
    github::{PullRequestDetail, PullRequestReviewThread},
    review_queue::ReviewQueueItem,
    semantic_diff::{SemanticDiffFile, SemanticDiffSection},
};

#[derive(Clone, Debug)]
pub struct ReviewStatusCounts {
    pub approved: usize,
    pub changes_requested: usize,
    pub commented: usize,
    pub waiting: usize,
}

#[derive(Clone, Debug)]
pub struct RelatedReviewFile {
    pub path: String,
    pub reason: String,
    pub changed: bool,
}

#[derive(Clone, Debug)]
pub struct FileThreadPreview {
    pub author_login: String,
    pub preview: String,
    pub updated_at: String,
    pub is_resolved: bool,
    pub location_label: String,
}

#[derive(Clone, Debug)]
pub struct ReviewContextData {
    pub queue_item: Option<ReviewQueueItem>,
    pub selected_section: Option<SemanticDiffSection>,
    pub ownership_label: String,
    pub review_status: ReviewStatusCounts,
    pub related_files: Vec<RelatedReviewFile>,
    pub docs_and_tests: Vec<RelatedReviewFile>,
    pub file_threads: Vec<FileThreadPreview>,
}

pub fn build_review_context(
    detail: &PullRequestDetail,
    queue_item: Option<ReviewQueueItem>,
    semantic: &SemanticDiffFile,
    selected_file_path: &str,
    selected_anchor: Option<&DiffAnchor>,
) -> ReviewContextData {
    let file_threads = detail
        .review_threads
        .iter()
        .filter(|thread| thread.path == selected_file_path)
        .map(thread_preview)
        .collect::<Vec<_>>();

    ReviewContextData {
        queue_item,
        selected_section: semantic.section_for_anchor(selected_anchor).cloned(),
        ownership_label: ownership_label(selected_file_path),
        review_status: summarize_review_status(detail),
        related_files: related_changed_files(detail, selected_file_path),
        docs_and_tests: related_docs_and_tests(detail, selected_file_path),
        file_threads,
    }
}

fn summarize_review_status(detail: &PullRequestDetail) -> ReviewStatusCounts {
    let mut approved = 0usize;
    let mut changes_requested = 0usize;
    let mut commented = 0usize;

    let mut latest_by_author = std::collections::BTreeMap::<&str, &str>::new();
    for review in &detail.latest_reviews {
        latest_by_author.insert(review.author_login.as_str(), review.state.as_str());
    }

    for state in latest_by_author.values() {
        match *state {
            "APPROVED" => approved += 1,
            "CHANGES_REQUESTED" => changes_requested += 1,
            _ => commented += 1,
        }
    }

    let responded = approved + changes_requested + commented;
    let waiting = detail.reviewers.len().saturating_sub(responded);

    ReviewStatusCounts {
        approved,
        changes_requested,
        commented,
        waiting,
    }
}

fn related_changed_files(
    detail: &PullRequestDetail,
    selected_file_path: &str,
) -> Vec<RelatedReviewFile> {
    let selected_prefix = path_area(selected_file_path);
    detail
        .files
        .iter()
        .filter(|file| file.path != selected_file_path)
        .filter(|file| path_area(&file.path) == selected_prefix)
        .take(6)
        .map(|file| RelatedReviewFile {
            path: file.path.clone(),
            reason: "same review area".to_string(),
            changed: true,
        })
        .collect()
}

fn related_docs_and_tests(
    detail: &PullRequestDetail,
    selected_file_path: &str,
) -> Vec<RelatedReviewFile> {
    let stem = file_stem(selected_file_path);
    detail
        .files
        .iter()
        .filter(|file| file.path != selected_file_path)
        .filter(|file| {
            let lower = file.path.to_ascii_lowercase();
            lower.contains(&stem)
                && (lower.ends_with(".md")
                    || lower.contains("/docs/")
                    || lower.contains("/test")
                    || lower.contains("/tests/")
                    || lower.contains("_test.")
                    || lower.contains(".spec.")
                    || lower.contains(".test."))
        })
        .take(4)
        .map(|file| RelatedReviewFile {
            path: file.path.clone(),
            reason: if file.path.ends_with(".md") || file.path.contains("/docs/") {
                "adjacent docs".to_string()
            } else {
                "adjacent test".to_string()
            },
            changed: true,
        })
        .collect()
}

fn ownership_label(path: &str) -> String {
    let area = path_area(path);
    if area.is_empty() {
        "Repository root".to_string()
    } else {
        format!("Area: {area}")
    }
}

fn path_area(path: &str) -> String {
    let mut segments = path.split('/').filter(|segment| !segment.is_empty());
    match (segments.next(), segments.next()) {
        (Some(first), Some(second))
            if matches!(
                first,
                "src" | "app" | "lib" | "packages" | "server" | "client" | "internal" | "tests"
            ) =>
        {
            format!("{first}/{second}")
        }
        (Some(first), _) => first.to_string(),
        _ => String::new(),
    }
}

fn thread_preview(thread: &PullRequestReviewThread) -> FileThreadPreview {
    let latest_comment = thread.comments.last();
    let preview = latest_comment
        .map(|comment| truncate(&comment.body, 160))
        .unwrap_or_else(|| "No comment body.".to_string());
    let updated_at = latest_comment
        .and_then(|comment| comment.published_at.clone())
        .unwrap_or_else(|| {
            latest_comment
                .map(|comment| comment.updated_at.clone())
                .unwrap_or_default()
        });
    let location_label = thread
        .line
        .or(thread.original_line)
        .map(|line| format!("{}:{line}", thread.path))
        .unwrap_or_else(|| thread.path.clone());

    FileThreadPreview {
        author_login: latest_comment
            .map(|comment| comment.author_login.clone())
            .unwrap_or_else(|| "review".to_string()),
        preview,
        updated_at,
        is_resolved: thread.is_resolved,
        location_label,
    }
}

fn truncate(value: &str, limit: usize) -> String {
    let trimmed = value.trim();
    if trimmed.chars().count() <= limit {
        return trimmed.to_string();
    }

    let mut out = trimmed.chars().take(limit).collect::<String>();
    out.push('…');
    out
}

fn file_stem(path: &str) -> String {
    path.rsplit('/')
        .next()
        .unwrap_or(path)
        .split('.')
        .next()
        .unwrap_or(path)
        .to_ascii_lowercase()
}
