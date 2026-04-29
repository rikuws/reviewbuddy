use crate::{
    code_tour::DiffAnchor,
    github::{PullRequestDetail, PullRequestFile},
    semantic_diff::{build_semantic_diff_file, SemanticChangeKind},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewQueueBucket {
    StartHere,
    NeedsScrutiny,
    QuickPass,
}

impl ReviewQueueBucket {
    pub fn label(&self) -> &'static str {
        match self {
            Self::StartHere => "Start here",
            Self::NeedsScrutiny => "Needs scrutiny",
            Self::QuickPass => "Quick pass",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ReviewQueueItem {
    pub file_path: String,
    pub change_type: String,
    pub score: i64,
    pub bucket: ReviewQueueBucket,
    pub risk_label: String,
    pub reasons: Vec<String>,
    pub additions: i64,
    pub deletions: i64,
    pub thread_count: usize,
    pub anchor: Option<DiffAnchor>,
}

#[derive(Clone, Debug, Default)]
pub struct ReviewQueue {
    pub start_here: Vec<ReviewQueueItem>,
    pub needs_scrutiny: Vec<ReviewQueueItem>,
    pub quick_pass: Vec<ReviewQueueItem>,
}

impl ReviewQueue {
    pub fn all_items(&self) -> impl Iterator<Item = &ReviewQueueItem> {
        self.start_here
            .iter()
            .chain(self.needs_scrutiny.iter())
            .chain(self.quick_pass.iter())
    }

    pub fn default_item(&self) -> Option<&ReviewQueueItem> {
        self.start_here
            .first()
            .or_else(|| self.needs_scrutiny.first())
            .or_else(|| self.quick_pass.first())
    }
}

pub fn build_review_queue(detail: &PullRequestDetail) -> ReviewQueue {
    let mut queue = ReviewQueue::default();
    for item in detail
        .files
        .iter()
        .map(|file| build_review_queue_item(detail, file))
    {
        match item.bucket {
            ReviewQueueBucket::StartHere => queue.start_here.push(item),
            ReviewQueueBucket::NeedsScrutiny => queue.needs_scrutiny.push(item),
            ReviewQueueBucket::QuickPass => queue.quick_pass.push(item),
        }
    }

    queue
}

pub fn default_review_file(detail: &PullRequestDetail) -> Option<String> {
    build_review_queue(detail)
        .default_item()
        .map(|item| item.file_path.clone())
}

fn build_review_queue_item(detail: &PullRequestDetail, file: &PullRequestFile) -> ReviewQueueItem {
    let parsed = crate::diff::find_parsed_diff_file(&detail.parsed_diff, &file.path);
    let semantic = build_semantic_diff_file(file, parsed, &detail.review_threads);
    let thread_count = detail
        .review_threads
        .iter()
        .filter(|thread| thread.path == file.path && !thread.is_resolved)
        .count();

    let mut score = file.additions + file.deletions;
    score += (thread_count as i64) * 35;
    score += match semantic.file_kind {
        SemanticChangeKind::Rename => -10,
        SemanticChangeKind::Docs => -18,
        SemanticChangeKind::Formatting => -20,
        SemanticChangeKind::Imports | SemanticChangeKind::Comments => -16,
        SemanticChangeKind::Tests => 8,
        SemanticChangeKind::Config => 18,
        SemanticChangeKind::Type => 24,
        SemanticChangeKind::Refactor => 16,
        SemanticChangeKind::Extract | SemanticChangeKind::Inline => 18,
        SemanticChangeKind::DataFlow => 22,
        _ => 10,
    };

    if file.path.contains("/src/") || file.path.starts_with("src/") {
        score += 8;
    }
    if is_root_file(&file.path) {
        score += 6;
    }
    if file.change_type == "RENAMED" && file.additions + file.deletions <= 12 {
        score -= 20;
    }

    let bucket = if score >= 90 || thread_count >= 2 {
        ReviewQueueBucket::StartHere
    } else if score <= 28 {
        ReviewQueueBucket::QuickPass
    } else {
        ReviewQueueBucket::NeedsScrutiny
    };

    let risk_label = match bucket {
        ReviewQueueBucket::StartHere => "Hotspot",
        ReviewQueueBucket::NeedsScrutiny => "Review",
        ReviewQueueBucket::QuickPass => "Quick",
    }
    .to_string();

    let mut reasons = Vec::<String>::new();
    if thread_count > 0 {
        reasons.push(format!(
            "{thread_count} unresolved thread{}",
            if thread_count == 1 { "" } else { "s" }
        ));
    }
    if file.additions + file.deletions >= 80 {
        reasons.push("large delta".to_string());
    }
    if matches!(
        semantic.file_kind,
        SemanticChangeKind::Type | SemanticChangeKind::Refactor | SemanticChangeKind::DataFlow
    ) {
        reasons.push(format!("{} change", semantic.file_kind.label()));
    }
    if matches!(
        semantic.file_kind,
        SemanticChangeKind::Tests | SemanticChangeKind::Docs
    ) {
        reasons.push(format!("{}-leaning", semantic.file_kind.label()));
    }
    if reasons.is_empty() {
        reasons.push(semantic.file_summary.clone());
    }

    ReviewQueueItem {
        file_path: file.path.clone(),
        change_type: file.change_type.clone(),
        score,
        bucket,
        risk_label,
        reasons,
        additions: file.additions,
        deletions: file.deletions,
        thread_count,
        anchor: semantic
            .sections
            .first()
            .and_then(|section| section.anchor.clone()),
    }
}

fn is_root_file(path: &str) -> bool {
    !path.contains('/')
}

#[cfg(test)]
mod tests {
    use crate::github::{PullRequestDetail, PullRequestFile};

    use super::{build_review_queue, ReviewQueueBucket};

    #[test]
    fn large_code_change_bubbles_to_start_here() {
        let detail = PullRequestDetail {
            id: "pr".to_string(),
            repository: "acme/api".to_string(),
            number: 1,
            title: "PR".to_string(),
            body: String::new(),
            url: String::new(),
            author_login: "octocat".to_string(),
            author_avatar_url: None,
            state: "OPEN".to_string(),
            is_draft: false,
            review_decision: None,
            base_ref_name: "main".to_string(),
            head_ref_name: "feature".to_string(),
            base_ref_oid: None,
            head_ref_oid: None,
            additions: 120,
            deletions: 10,
            changed_files: 2,
            comments_count: 0,
            commits_count: 1,
            created_at: String::new(),
            updated_at: String::new(),
            labels: Vec::new(),
            reviewers: Vec::new(),
            reviewer_avatar_urls: std::collections::BTreeMap::new(),
            comments: Vec::new(),
            latest_reviews: Vec::new(),
            review_threads: Vec::new(),
            files: vec![
                PullRequestFile {
                    path: "src/main.rs".to_string(),
                    additions: 100,
                    deletions: 10,
                    change_type: "MODIFIED".to_string(),
                },
                PullRequestFile {
                    path: "README.md".to_string(),
                    additions: 3,
                    deletions: 1,
                    change_type: "MODIFIED".to_string(),
                },
            ],
            raw_diff: String::new(),
            parsed_diff: Vec::new(),
            data_completeness: crate::github::PullRequestDataCompleteness::default(),
        };

        let queue = build_review_queue(&detail);
        assert_eq!(queue.start_here[0].file_path, "src/main.rs");
        assert_eq!(queue.quick_pass[0].bucket, ReviewQueueBucket::QuickPass);
    }
}
