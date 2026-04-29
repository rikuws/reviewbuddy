use serde::Serialize;
use serde_json::{json, Value};

use crate::code_tour::{
    CodeTourCandidateGroup, CodeTourFileContext, CodeTourReviewCommentContext,
    CodeTourReviewContext, CodeTourReviewThreadContext, DiffAnchor, GenerateCodeTourInput,
    TourSectionCategory, TourSectionPriority, TourStep,
};

use super::schema::TOUR_OUTPUT_SCHEMA_JSON;

pub const MAX_BODY_CHARS: usize = 2_500;
pub const MAX_REVIEW_BODY_CHARS: usize = 900;
pub const MAX_COMMENT_BODY_CHARS: usize = 500;
pub const MAX_FILES: usize = 80;
pub const MAX_REVIEWS: usize = 5;
pub const MAX_THREADS: usize = 12;
pub const MAX_COMMENTS_PER_THREAD: usize = 3;
pub const MAX_SNIPPET_CHARS: usize = 500;

pub const BASE_INSTRUCTIONS: &[&str] = &[
    "You are generating a guided code tour for a GitHub pull request.",
    "Act like a senior pair programmer walking a reviewer through the change.",
    "Assume the reviewer already knows the codebase well. Be direct, useful, and never condescending.",
    "Stay grounded in the provided pull-request data and the provided local checkout.",
    "Do not edit files, propose patches, or imply that you changed the code.",
    "Finish the whole task in this turn. Do not wait for more instructions.",
    "Be fast and selective. Do not exhaustively explore the repository.",
    "Use only read-only tools (view/read, grep/rg, glob). Never use shell, write, or git commands in this session.",
    "Start from the provided candidate groups and candidate steps before opening more files.",
    "Inspect only the changed files plus direct supporting callsites.",
    "Inspect at most 24 files total and do not reopen the same supporting file more than twice.",
    "Do not spawn sub-agents or background agents, and never try shell or git recovery commands.",
    "Inspect at most one targeted supporting callsite per section beyond the changed files. Once the story is clear, stop using tools and return the final JSON immediately.",
    "If a candidate file is missing from the checkout, treat it as deleted, renamed, or out-of-sync and continue with the provided pull-request context, snippets, and remaining files.",
    "If a supporting callsite cannot be verified quickly, omit it instead of continuing to search.",
    "If a search returns no direct hit, do not keep widening it. Continue with the verified pull-request context you already have.",
    "A complete best-effort tour is better than an exhaustive investigation.",
    "Return JSON only with no markdown fences or extra commentary.",
    "Always use the provided candidate step ids. Never invent ids.",
    "Explain the whole pull request first, then organize the changed files into related sections.",
    "Use the section stepIds to cover the whole changeset. Reuse each candidate file step at most once across sections.",
    "Each section is an AI-authored semantic change group: title it as a reviewer-facing story, not as a path bucket or generic diff kind.",
    "Each section must choose exactly one category from sectionCategoryCatalog and one priority from sectionPriorityCatalog.",
    "Set priority to high only when a reviewer should inspect that group early, medium for normal review attention, and low for supporting or low-risk changes.",
    "Each step summary should be one sentence. Each detail should be 1 to 3 sentences focused on what changed, why it matters, and what to verify in review.",
    "Each section should explain why those files belong together and how the change moves across them.",
    "Adapt the Explain Code style for a native GPUI review view.",
    "Do not return Markdown headings, fenced code blocks, emoji, or prose-only filler sections.",
    "Treat each JSON section as one visible GPUI group: section.title is the plain title, section.summary is the short gist, section.detail is the brief explanation, and section.stepIds identifies the diff blocks rendered underneath.",
    "Keep section prose short, scannable, and grounded in the provided diff. Prefer simple words and one main idea per sentence.",
    "Do not invent intent that is not supported by the pull request context, changed files, review threads, or local checkout.",
    "For new or materially changed APIs, helpers, components, types, or commands, include concrete verified callsites when they help teach the change.",
    "Only include callsites you can support from the provided checkout. Keep callsite snippets compact.",
    "Surface unresolved review concerns in openQuestions when appropriate.",
];

pub fn build_tour_prompt(input: &GenerateCodeTourInput) -> String {
    let context = build_prompt_context(input);
    let schema_pretty = serde_json::to_string_pretty(
        &serde_json::from_str::<Value>(TOUR_OUTPUT_SCHEMA_JSON).expect("schema must parse"),
    )
    .expect("schema must serialize");
    let context_pretty = serde_json::to_string_pretty(&context).expect("context must serialize");

    let mut lines: Vec<String> = BASE_INSTRUCTIONS.iter().map(|s| (*s).to_string()).collect();
    lines.push(String::new());
    lines.push("JSON schema:".to_string());
    lines.push(schema_pretty);
    lines.push(String::new());
    lines.push("Pull-request context:".to_string());
    lines.push(context_pretty);
    lines.join("\n")
}

pub fn trim_text(value: &str, max_length: usize) -> String {
    let normalized = value.trim();
    if normalized.chars().count() <= max_length {
        return normalized.to_string();
    }

    let truncated = normalized
        .chars()
        .take(max_length.saturating_sub(1))
        .collect::<String>();
    format!("{}…", truncated.trim_end())
}

fn build_prompt_context(input: &GenerateCodeTourInput) -> Value {
    let overview_step = input.candidate_steps.first();
    let file_steps: Vec<&TourStep> = input.candidate_steps.iter().skip(1).collect();

    let mut prioritized = input.review_threads.clone();
    prioritized.sort_by_key(|thread| thread.is_resolved);

    json!({
        "repository": input.repository,
        "workingDirectory": input.working_directory,
        "pullRequest": {
            "number": input.number,
            "title": input.title,
            "url": input.url,
            "authorLogin": input.author_login,
            "reviewDecision": input.review_decision,
            "baseRefName": input.base_ref_name,
            "headRefName": input.head_ref_name,
            "updatedAt": input.updated_at,
            "stats": {
                "commits": input.commits_count,
                "changedFiles": input.changed_files,
                "additions": input.additions,
                "deletions": input.deletions,
            },
            "body": trim_text(&input.body, MAX_BODY_CHARS),
        },
        "files": input
            .files
            .iter()
            .take(MAX_FILES)
            .map(|file| json!({
                "path": file.path,
                "changeType": file.change_type,
                "additions": file.additions,
                "deletions": file.deletions,
            }))
            .collect::<Vec<_>>(),
        "latestReviews": input
            .latest_reviews
            .iter()
            .take(MAX_REVIEWS)
            .map(|review| json!({
                "authorLogin": review.author_login,
                "state": review.state,
                "submittedAt": review.submitted_at,
                "body": trim_text(&review.body, MAX_REVIEW_BODY_CHARS),
            }))
            .collect::<Vec<_>>(),
        "reviewThreads": prioritized
            .iter()
            .take(MAX_THREADS)
            .map(|thread| json!({
                "path": thread.path,
                "line": thread.line,
                "diffSide": thread.diff_side,
                "subjectType": thread.subject_type,
                "isResolved": thread.is_resolved,
                "comments": thread
                    .comments
                    .iter()
                    .take(MAX_COMMENTS_PER_THREAD)
                    .map(|comment| json!({
                        "authorLogin": comment.author_login,
                        "body": trim_text(&comment.body, MAX_COMMENT_BODY_CHARS),
                    }))
                    .collect::<Vec<_>>(),
            }))
            .collect::<Vec<_>>(),
        "overviewStep": overview_step.map(summarize_step),
        "sectionCategoryCatalog": TourSectionCategory::all()
            .iter()
            .map(|category| json!({
                "value": category.slug(),
                "label": category.label(),
            }))
            .collect::<Vec<_>>(),
        "sectionPriorityCatalog": TourSectionPriority::all()
            .iter()
            .map(|priority| json!({
                "value": priority.slug(),
                "label": priority.label(),
            }))
            .collect::<Vec<_>>(),
        "candidateGroups": input
            .candidate_groups
            .iter()
            .map(summarize_group)
            .collect::<Vec<_>>(),
        "candidateSteps": file_steps.into_iter().map(summarize_step).collect::<Vec<_>>(),
    })
}

#[derive(Serialize)]
struct CandidateStepSummary<'a> {
    id: &'a str,
    kind: &'a str,
    title: &'a str,
    summary: &'a str,
    detail: &'a str,
    badge: &'a str,
    #[serde(rename = "filePath", skip_serializing_if = "Option::is_none")]
    file_path: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    anchor: Option<&'a DiffAnchor>,
    additions: i64,
    deletions: i64,
    #[serde(rename = "unresolvedThreadCount")]
    unresolved_thread_count: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    snippet: Option<String>,
}

fn summarize_step(step: &TourStep) -> Value {
    let summary = CandidateStepSummary {
        id: &step.id,
        kind: &step.kind,
        title: &step.title,
        summary: &step.summary,
        detail: &step.detail,
        badge: &step.badge,
        file_path: step.file_path.as_deref(),
        anchor: step.anchor.as_ref(),
        additions: step.additions,
        deletions: step.deletions,
        unresolved_thread_count: step.unresolved_thread_count,
        snippet: step
            .snippet
            .as_ref()
            .map(|snippet| trim_text(snippet, MAX_SNIPPET_CHARS)),
    };
    serde_json::to_value(summary).expect("summary must serialize")
}

fn summarize_group(group: &CodeTourCandidateGroup) -> Value {
    json!({
        "id": group.id,
        "title": group.title,
        "summary": group.summary,
        "stepIds": group.step_ids,
        "filePaths": group.file_paths,
    })
}

// Keep unused imports used
#[allow(dead_code)]
fn _references_types(
    _: &CodeTourFileContext,
    _: &CodeTourReviewContext,
    _: &CodeTourReviewCommentContext,
    _: &CodeTourReviewThreadContext,
) {
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::code_tour::{CodeTourProvider, GenerateCodeTourInput, TourStep};

    fn sample_input() -> GenerateCodeTourInput {
        GenerateCodeTourInput {
            provider: CodeTourProvider::Codex,
            working_directory: "/tmp/repo".to_string(),
            repository: "owner/name".to_string(),
            number: 42,
            code_version_key: "head-abc".to_string(),
            title: "Add widget".to_string(),
            body: "Implements the widget feature.".to_string(),
            url: "https://example.com".to_string(),
            author_login: "rikuws".to_string(),
            review_decision: Some("APPROVED".to_string()),
            base_ref_name: "main".to_string(),
            head_ref_name: "feature/widget".to_string(),
            head_ref_oid: Some("abc".to_string()),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            additions: 10,
            deletions: 2,
            changed_files: 3,
            commits_count: 2,
            files: vec![],
            latest_reviews: vec![],
            review_threads: vec![],
            candidate_steps: vec![TourStep {
                id: "overview".to_string(),
                kind: "overview".to_string(),
                title: "3 files, 2 commits".to_string(),
                summary: "approved; no unresolved threads.".to_string(),
                detail: "rikuws is targeting main from feature/widget.".to_string(),
                file_path: None,
                anchor: None,
                additions: 10,
                deletions: 2,
                unresolved_thread_count: 0,
                snippet: None,
                badge: "APPROVED".to_string(),
            }],
            candidate_groups: vec![],
        }
    }

    #[test]
    fn builds_prompt_with_schema_and_context() {
        let prompt = build_tour_prompt(&sample_input());
        assert!(prompt.contains("JSON schema:"));
        assert!(prompt.contains("Pull-request context:"));
        assert!(prompt.contains("\"repository\": \"owner/name\""));
        assert!(prompt.contains("You are generating a guided code tour"));
        assert!(prompt.contains("sectionCategoryCatalog"));
        assert!(prompt.contains("\"value\": \"auth-security\""));
        assert!(prompt.contains("sectionPriorityCatalog"));
        assert!(prompt.contains("\"value\": \"high\""));
    }

    #[test]
    fn trim_text_respects_character_limit() {
        let long = "あいうえお".repeat(50);
        let trimmed = trim_text(&long, 10);
        assert!(trimmed.chars().count() <= 10);
        assert!(trimmed.ends_with('…'));
    }
}
