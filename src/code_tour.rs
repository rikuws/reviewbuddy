use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

use crate::{
    agents,
    cache::CacheStore,
    diff::{DiffLineKind, ParsedDiffFile, ParsedDiffHunk, ParsedDiffLine},
    github::{PullRequestDetail, PullRequestFile, PullRequestReviewThread},
};

const CODE_TOUR_CACHE_KEY_PREFIX: &str = "code-tour-v5";
const CODE_TOUR_SETTINGS_CACHE_KEY: &str = "code-tour-settings-v1";

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CodeTourProvider {
    #[default]
    Codex,
    Copilot,
}

impl CodeTourProvider {
    pub fn slug(&self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Copilot => "copilot",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Copilot => "Copilot",
        }
    }

    pub fn all() -> &'static [CodeTourProvider] {
        &[CodeTourProvider::Codex, CodeTourProvider::Copilot]
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CodeTourSettings {
    #[serde(default)]
    pub provider: CodeTourProvider,
    #[serde(default)]
    pub automatic_repositories: BTreeSet<String>,
}

impl CodeTourSettings {
    pub fn automatically_generates_for(&self, repository: &str) -> bool {
        self.automatic_repositories.contains(repository)
    }

    pub fn set_automatic_generation_for(&mut self, repository: &str, enabled: bool) {
        if enabled {
            self.automatic_repositories.insert(repository.to_string());
        } else {
            self.automatic_repositories.remove(repository);
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeTourProviderStatus {
    pub provider: CodeTourProvider,
    pub label: String,
    pub available: bool,
    pub authenticated: bool,
    pub message: String,
    pub detail: String,
    pub default_model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiffAnchor {
    pub file_path: String,
    pub hunk_header: Option<String>,
    pub line: Option<i64>,
    pub side: Option<String>,
    pub thread_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TourStep {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub summary: String,
    pub detail: String,
    pub file_path: Option<String>,
    pub anchor: Option<DiffAnchor>,
    pub additions: i64,
    pub deletions: i64,
    pub unresolved_thread_count: i64,
    pub snippet: Option<String>,
    pub badge: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TourCallsite {
    pub title: String,
    pub path: String,
    pub line: Option<i64>,
    pub summary: String,
    pub snippet: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TourSectionCategory {
    AuthSecurity,
    DataState,
    ApiIo,
    UiUx,
    Tests,
    Docs,
    Config,
    Infra,
    Refactor,
    Performance,
    Reliability,
    #[default]
    Other,
}

impl TourSectionCategory {
    pub fn all() -> &'static [TourSectionCategory] {
        &[
            TourSectionCategory::AuthSecurity,
            TourSectionCategory::DataState,
            TourSectionCategory::ApiIo,
            TourSectionCategory::UiUx,
            TourSectionCategory::Tests,
            TourSectionCategory::Docs,
            TourSectionCategory::Config,
            TourSectionCategory::Infra,
            TourSectionCategory::Refactor,
            TourSectionCategory::Performance,
            TourSectionCategory::Reliability,
            TourSectionCategory::Other,
        ]
    }

    pub fn slug(&self) -> &'static str {
        match self {
            Self::AuthSecurity => "auth-security",
            Self::DataState => "data-state",
            Self::ApiIo => "api-io",
            Self::UiUx => "ui-ux",
            Self::Tests => "tests",
            Self::Docs => "docs",
            Self::Config => "config",
            Self::Infra => "infra",
            Self::Refactor => "refactor",
            Self::Performance => "performance",
            Self::Reliability => "reliability",
            Self::Other => "other",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::AuthSecurity => "Auth / Security",
            Self::DataState => "Data / State",
            Self::ApiIo => "API / I/O",
            Self::UiUx => "UI / UX",
            Self::Tests => "Tests",
            Self::Docs => "Docs",
            Self::Config => "Config",
            Self::Infra => "Infra",
            Self::Refactor => "Refactor",
            Self::Performance => "Performance",
            Self::Reliability => "Reliability",
            Self::Other => "Other",
        }
    }

    pub fn from_slug(value: &str) -> Option<Self> {
        match value.trim() {
            "auth-security" => Some(Self::AuthSecurity),
            "data-state" => Some(Self::DataState),
            "api-io" => Some(Self::ApiIo),
            "ui-ux" => Some(Self::UiUx),
            "tests" => Some(Self::Tests),
            "docs" => Some(Self::Docs),
            "config" => Some(Self::Config),
            "infra" => Some(Self::Infra),
            "refactor" => Some(Self::Refactor),
            "performance" => Some(Self::Performance),
            "reliability" => Some(Self::Reliability),
            "other" => Some(Self::Other),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TourSectionPriority {
    Low,
    #[default]
    Medium,
    High,
}

impl TourSectionPriority {
    pub fn all() -> &'static [TourSectionPriority] {
        &[
            TourSectionPriority::Low,
            TourSectionPriority::Medium,
            TourSectionPriority::High,
        ]
    }

    pub fn slug(&self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
        }
    }

    pub fn from_slug(value: &str) -> Option<Self> {
        match value.trim() {
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TourSection {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub detail: String,
    pub badge: String,
    pub category: TourSectionCategory,
    pub priority: TourSectionPriority,
    pub step_ids: Vec<String>,
    pub review_points: Vec<String>,
    pub callsites: Vec<TourCallsite>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneratedCodeTour {
    pub provider: CodeTourProvider,
    pub model: Option<String>,
    pub generated_at: String,
    pub summary: String,
    pub review_focus: String,
    pub open_questions: Vec<String>,
    pub warnings: Vec<String>,
    pub sections: Vec<TourSection>,
    pub steps: Vec<TourStep>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeTourProgressUpdate {
    pub stage: String,
    pub summary: String,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub log: Option<String>,
    #[serde(default)]
    pub log_file_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeTourFileContext {
    pub path: String,
    pub additions: i64,
    pub deletions: i64,
    pub change_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeTourReviewContext {
    pub author_login: String,
    pub state: String,
    pub body: String,
    pub submitted_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeTourReviewCommentContext {
    pub author_login: String,
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeTourReviewThreadContext {
    pub path: String,
    pub line: Option<i64>,
    pub diff_side: Option<String>,
    pub is_resolved: bool,
    pub subject_type: String,
    pub comments: Vec<CodeTourReviewCommentContext>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeTourCandidateGroup {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub step_ids: Vec<String>,
    pub file_paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateCodeTourInput {
    pub provider: CodeTourProvider,
    pub working_directory: String,
    pub repository: String,
    pub number: i64,
    pub code_version_key: String,
    pub title: String,
    pub body: String,
    pub url: String,
    pub author_login: String,
    pub review_decision: Option<String>,
    pub base_ref_name: String,
    pub head_ref_name: String,
    pub head_ref_oid: Option<String>,
    pub updated_at: String,
    pub additions: i64,
    pub deletions: i64,
    pub changed_files: i64,
    pub commits_count: i64,
    pub files: Vec<CodeTourFileContext>,
    pub latest_reviews: Vec<CodeTourReviewContext>,
    pub review_threads: Vec<CodeTourReviewThreadContext>,
    pub candidate_steps: Vec<TourStep>,
    pub candidate_groups: Vec<CodeTourCandidateGroup>,
}

pub fn load_code_tour_provider_statuses() -> Result<Vec<CodeTourProviderStatus>, String> {
    Ok(agents::load_all_statuses())
}

pub fn load_code_tour_settings(cache: &CacheStore) -> Result<CodeTourSettings, String> {
    Ok(cache
        .get::<CodeTourSettings>(CODE_TOUR_SETTINGS_CACHE_KEY)?
        .map(|document| document.value)
        .unwrap_or_default())
}

pub fn save_code_tour_settings(
    cache: &CacheStore,
    settings: &CodeTourSettings,
) -> Result<(), String> {
    cache.put(CODE_TOUR_SETTINGS_CACHE_KEY, settings, now_ms())
}

pub fn load_code_tour(
    cache: &CacheStore,
    detail: &PullRequestDetail,
    provider: CodeTourProvider,
) -> Result<Option<GeneratedCodeTour>, String> {
    let cache_key = code_tour_cache_key(detail, provider);

    Ok(cache
        .get::<GeneratedCodeTour>(&cache_key)?
        .map(|document| document.value))
}

pub fn generate_code_tour_with_progress<F>(
    cache: &CacheStore,
    input: GenerateCodeTourInput,
    on_progress: F,
) -> Result<GeneratedCodeTour, String>
where
    F: FnMut(CodeTourProgressUpdate),
{
    if input.working_directory.trim().is_empty() {
        return Err("Code tours require a local checkout path.".to_string());
    }

    if !Path::new(&input.working_directory).exists() {
        return Err(format!(
            "The local checkout path '{}' does not exist.",
            input.working_directory
        ));
    }

    if input.candidate_steps.is_empty() {
        return Err("Code tour generation needs at least one candidate step.".to_string());
    }

    let backend = agents::backend_for(input.provider);
    let mut progress_sink: Box<dyn FnMut(CodeTourProgressUpdate)> = Box::new(on_progress);
    let tour = backend.generate(&input, progress_sink.as_mut())?;

    let cache_key = code_tour_cache_key_from_parts(
        &input.repository,
        input.number,
        input.provider,
        &input.code_version_key,
    );

    cache.put(&cache_key, &tour, now_ms())?;

    Ok(tour)
}

pub fn build_code_tour_generation_input(
    detail: &PullRequestDetail,
    provider: CodeTourProvider,
    working_directory: &str,
) -> GenerateCodeTourInput {
    let candidate_steps = build_tour_steps(detail);
    let overview_step = candidate_steps.first().cloned();
    let file_steps = candidate_steps.iter().skip(1).cloned().collect::<Vec<_>>();
    let candidate_groups = build_candidate_groups(&file_steps);

    GenerateCodeTourInput {
        provider,
        working_directory: working_directory.to_string(),
        repository: detail.repository.clone(),
        number: detail.number,
        code_version_key: tour_code_version_key(detail),
        title: detail.title.clone(),
        body: trim_text(&detail.body, 2_500),
        url: detail.url.clone(),
        author_login: detail.author_login.clone(),
        review_decision: detail.review_decision.clone(),
        base_ref_name: detail.base_ref_name.clone(),
        head_ref_name: detail.head_ref_name.clone(),
        head_ref_oid: detail.head_ref_oid.clone(),
        updated_at: detail.updated_at.clone(),
        additions: detail.additions,
        deletions: detail.deletions,
        changed_files: detail.changed_files,
        commits_count: detail.commits_count,
        files: detail
            .files
            .iter()
            .map(map_code_tour_file_context)
            .collect(),
        latest_reviews: detail
            .latest_reviews
            .iter()
            .take(5)
            .map(map_code_tour_review_context)
            .collect(),
        review_threads: prioritize_review_threads(&detail.review_threads)
            .into_iter()
            .take(12)
            .map(|thread| map_code_tour_review_thread_context(&thread))
            .collect(),
        candidate_steps: if let Some(overview) = overview_step {
            let mut steps = vec![overview];
            steps.extend(file_steps);
            steps
        } else {
            file_steps
        },
        candidate_groups,
    }
}

pub fn build_tour_request_key(detail: &PullRequestDetail, provider: CodeTourProvider) -> String {
    let code_version = tour_code_version_key(detail);
    format!(
        "{}:{}:{}:{code_version}",
        provider.slug(),
        detail.repository,
        detail.number,
    )
}

pub fn tour_code_version_key(detail: &PullRequestDetail) -> String {
    detail
        .head_ref_oid
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| format!("head-{value}"))
        .unwrap_or_else(|| format!("diff-{}", hash_text(&detail.raw_diff)))
}

pub fn line_matches_diff_anchor(line: &ParsedDiffLine, anchor: Option<&DiffAnchor>) -> bool {
    let Some(anchor) = anchor else {
        return false;
    };
    let Some(side) = anchor.side.as_deref() else {
        return false;
    };
    let Some(line_number) = anchor.line else {
        return false;
    };

    match side {
        "LEFT" => line.left_line_number == Some(line_number),
        "RIGHT" => line.right_line_number == Some(line_number),
        _ => false,
    }
}

pub fn thread_matches_diff_anchor(
    thread: &PullRequestReviewThread,
    anchor: Option<&DiffAnchor>,
) -> bool {
    anchor
        .and_then(|anchor| anchor.thread_id.as_deref())
        .map(|thread_id| thread.id == thread_id)
        .unwrap_or(false)
}

pub fn find_parsed_diff_file<'a>(
    parsed_diff: &'a [ParsedDiffFile],
    path: &str,
) -> Option<&'a ParsedDiffFile> {
    parsed_diff
        .iter()
        .find(|file| file.path == path)
        .or_else(|| {
            parsed_diff
                .iter()
                .find(|file| file.previous_path.as_deref() == Some(path))
        })
}

fn build_tour_steps(detail: &PullRequestDetail) -> Vec<TourStep> {
    let unresolved_thread_count = detail
        .review_threads
        .iter()
        .filter(|thread| !thread.is_resolved)
        .count() as i64;

    let mut steps = vec![TourStep {
        id: "overview".to_string(),
        kind: "overview".to_string(),
        title: format!(
            "{} files, {} commits",
            detail.changed_files, detail.commits_count
        ),
        summary: build_overview_summary(detail, unresolved_thread_count),
        detail: format!(
            "{} is targeting {} from {}.",
            detail.author_login, detail.base_ref_name, detail.head_ref_name
        ),
        file_path: None,
        anchor: None,
        additions: detail.additions,
        deletions: detail.deletions,
        unresolved_thread_count,
        snippet: None,
        badge: detail
            .review_decision
            .clone()
            .unwrap_or_else(|| if detail.is_draft { "draft" } else { "ready" }.to_string()),
    }];

    let mut ranked_files = detail
        .files
        .iter()
        .filter_map(|file| build_file_step(detail, file))
        .collect::<Vec<_>>();

    ranked_files.sort_by_key(|step| -file_step_score(step));
    steps.extend(ranked_files);
    steps
}

fn build_file_step(detail: &PullRequestDetail, file: &PullRequestFile) -> Option<TourStep> {
    let parsed_file = find_parsed_diff_file(&detail.parsed_diff, &file.path);
    let file_threads = detail
        .review_threads
        .iter()
        .filter(|thread| thread.path == file.path)
        .collect::<Vec<_>>();
    let unresolved_thread_count = file_threads
        .iter()
        .filter(|thread| !thread.is_resolved)
        .count() as i64;

    let anchor = file_threads
        .iter()
        .find(|thread| !thread.is_resolved)
        .and_then(|thread| review_thread_anchor(thread))
        .or_else(|| {
            file_threads
                .first()
                .and_then(|thread| review_thread_anchor(thread))
        })
        .or_else(|| parsed_file.and_then(first_anchor_for_parsed_file))
        .or_else(|| {
            Some(DiffAnchor {
                file_path: file.path.clone(),
                hunk_header: None,
                line: None,
                side: None,
                thread_id: None,
            })
        });

    Some(TourStep {
        id: format!("file:{}", file.path),
        kind: "file".to_string(),
        title: file.path.clone(),
        summary: build_file_summary(file, unresolved_thread_count),
        detail: build_file_detail(file, unresolved_thread_count),
        file_path: Some(file.path.clone()),
        anchor,
        additions: file.additions,
        deletions: file.deletions,
        unresolved_thread_count,
        snippet: parsed_file.and_then(snippet_for_parsed_file),
        badge: badge_for_change_type(&file.change_type).to_string(),
    })
}

fn build_overview_summary(detail: &PullRequestDetail, unresolved_thread_count: i64) -> String {
    let review_decision = detail
        .review_decision
        .as_deref()
        .map(|decision| format!("{} decision", decision.to_ascii_lowercase()))
        .unwrap_or_else(|| "no review decision yet".to_string());
    let thread_summary = if unresolved_thread_count > 0 {
        format!(
            "{unresolved_thread_count} unresolved thread{}",
            if unresolved_thread_count == 1 {
                ""
            } else {
                "s"
            }
        )
    } else {
        "no unresolved threads".to_string()
    };

    format!("{review_decision}; {thread_summary}.")
}

fn build_file_summary(file: &PullRequestFile, unresolved_thread_count: i64) -> String {
    let delta = format!("+{} / -{}", file.additions, file.deletions);

    if unresolved_thread_count > 0 {
        format!(
            "{delta} with {unresolved_thread_count} unresolved thread{}.",
            if unresolved_thread_count == 1 {
                ""
            } else {
                "s"
            }
        )
    } else {
        format!("{delta} and no open discussion threads.")
    }
}

fn build_file_detail(file: &PullRequestFile, unresolved_thread_count: i64) -> String {
    let change_label = badge_for_change_type(&file.change_type);

    if unresolved_thread_count > 0 {
        format!("{change_label} file with active review discussion.")
    } else {
        format!("{change_label} file ready for a raw diff pass.")
    }
}

fn build_candidate_groups(file_steps: &[TourStep]) -> Vec<CodeTourCandidateGroup> {
    #[derive(Default)]
    struct Bucket {
        order: usize,
        step_ids: Vec<String>,
        file_paths: Vec<String>,
        additions: i64,
        deletions: i64,
        unresolved_thread_count: i64,
    }

    let mut buckets: HashMap<String, Bucket> = HashMap::new();

    for (index, step) in file_steps.iter().enumerate() {
        let file_path = step.file_path.as_deref().unwrap_or(&step.title);
        let key = group_key_for_file_path(file_path);
        let bucket = buckets.entry(key).or_insert_with(|| Bucket {
            order: index,
            ..Default::default()
        });

        bucket.step_ids.push(step.id.clone());
        bucket.file_paths.push(file_path.to_string());
        bucket.additions += step.additions;
        bucket.deletions += step.deletions;
        bucket.unresolved_thread_count += step.unresolved_thread_count;
    }

    let mut grouped = buckets.into_iter().collect::<Vec<_>>();
    grouped.sort_by_key(|(_, bucket)| bucket.order);

    grouped
        .into_iter()
        .enumerate()
        .map(|(index, (key, bucket))| CodeTourCandidateGroup {
            id: format!("group:{}", index + 1),
            title: title_for_group_key(&key),
            summary: build_candidate_group_summary(
                bucket.step_ids.len() as i64,
                bucket.additions,
                bucket.deletions,
                bucket.unresolved_thread_count,
            ),
            step_ids: bucket.step_ids,
            file_paths: bucket.file_paths,
        })
        .collect()
}

fn group_key_for_file_path(file_path: &str) -> String {
    let segments = file_path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();

    if segments.len() <= 1 {
        return "__root__".to_string();
    }

    let first = segments[0];
    let second = segments[1];
    let structured_roots = HashSet::from([
        ".github",
        "app",
        "client",
        "cmd",
        "docs",
        "internal",
        "lib",
        "packages",
        "pkg",
        "scripts",
        "server",
        "spec",
        "src",
        "src-tauri",
        "test",
        "tests",
        "ui",
        "web",
    ]);

    if structured_roots.contains(first) {
        if segments.len() >= 3 {
            return format!("{first}/{second}");
        }
        return first.to_string();
    }

    first.to_string()
}

fn title_for_group_key(key: &str) -> String {
    if key == "__root__" {
        "Repository root changes".to_string()
    } else {
        format!("Changes in {key}")
    }
}

fn build_candidate_group_summary(
    file_count: i64,
    additions: i64,
    deletions: i64,
    unresolved_thread_count: i64,
) -> String {
    let delta = format!("+{additions} / -{deletions}");

    if unresolved_thread_count > 0 {
        format!(
            "{file_count} related file{} with {delta} and {unresolved_thread_count} unresolved thread{}.",
            if file_count == 1 { "" } else { "s" },
            if unresolved_thread_count == 1 { "" } else { "s" }
        )
    } else {
        format!(
            "{file_count} related file{} with {delta}.",
            if file_count == 1 { "" } else { "s" }
        )
    }
}

fn badge_for_change_type(change_type: &str) -> &'static str {
    match change_type {
        "ADDED" => "added",
        "DELETED" => "deleted",
        "RENAMED" => "renamed",
        "COPIED" => "copied",
        _ => "modified",
    }
}

fn first_anchor_for_parsed_file(parsed_file: &ParsedDiffFile) -> Option<DiffAnchor> {
    if let Some(location) = representative_diff_location(parsed_file) {
        let hunk = &parsed_file.hunks[location.hunk_index];
        let line = &hunk.lines[location.line_index];
        let anchor = resolve_diff_line_anchor(&parsed_file.path, line, Some(&hunk.header));
        if let Some(anchor) = anchor {
            return Some(anchor);
        }
    }

    parsed_file
        .hunks
        .first()
        .map(|hunk| DiffAnchor {
            file_path: parsed_file.path.clone(),
            hunk_header: Some(hunk.header.clone()),
            line: None,
            side: None,
            thread_id: None,
        })
        .or_else(|| {
            Some(DiffAnchor {
                file_path: parsed_file.path.clone(),
                hunk_header: None,
                line: None,
                side: None,
                thread_id: None,
            })
        })
}

fn snippet_for_parsed_file(parsed_file: &ParsedDiffFile) -> Option<String> {
    let location = representative_diff_location(parsed_file)?;
    let hunk = parsed_file.hunks.get(location.hunk_index)?;
    let snippet_lines = snippet_lines_for_hunk(hunk, location.line_index, 6);
    let lines = snippet_lines
        .iter()
        .map(|line| {
            format!(
                "{}{}",
                if line.prefix.is_empty() {
                    " "
                } else {
                    &line.prefix
                },
                line.content
            )
        })
        .collect::<Vec<_>>();

    if lines.is_empty() {
        Some(hunk.header.clone())
    } else {
        Some(format!("{}\n{}", hunk.header, lines.join("\n")))
    }
}

#[derive(Clone, Copy)]
struct RepresentativeDiffLocation {
    hunk_index: usize,
    line_index: usize,
}

fn representative_diff_location(
    parsed_file: &ParsedDiffFile,
) -> Option<RepresentativeDiffLocation> {
    let mut best: Option<(i64, RepresentativeDiffLocation)> = None;

    for (hunk_index, hunk) in parsed_file.hunks.iter().enumerate() {
        let hunk_score = hunk_relevance_score(hunk);
        let import_block = hunk_looks_like_import_block(hunk);

        for (line_index, line) in hunk.lines.iter().enumerate() {
            if !matches!(line.kind, DiffLineKind::Addition | DiffLineKind::Deletion) {
                continue;
            }

            let score = hunk_score + diff_line_relevance_score(line, import_block);
            let location = RepresentativeDiffLocation {
                hunk_index,
                line_index,
            };

            if best
                .as_ref()
                .map(|(best_score, _)| score > *best_score)
                .unwrap_or(true)
            {
                best = Some((score, location));
            }
        }
    }

    best.map(|(_, location)| location)
        .or_else(|| first_anchorable_diff_location(parsed_file))
}

fn first_anchorable_diff_location(
    parsed_file: &ParsedDiffFile,
) -> Option<RepresentativeDiffLocation> {
    for (hunk_index, hunk) in parsed_file.hunks.iter().enumerate() {
        for (line_index, line) in hunk.lines.iter().enumerate() {
            if resolve_diff_line_anchor(&parsed_file.path, line, Some(&hunk.header)).is_some() {
                return Some(RepresentativeDiffLocation {
                    hunk_index,
                    line_index,
                });
            }
        }
    }

    None
}

fn hunk_relevance_score(hunk: &ParsedDiffHunk) -> i64 {
    let import_block = hunk_looks_like_import_block(hunk);
    let mut changed_count = 0;
    let mut meaningful_count = 0;
    let mut declaration_count = 0;
    let mut behavior_count = 0;

    for line in &hunk.lines {
        if !matches!(line.kind, DiffLineKind::Addition | DiffLineKind::Deletion) {
            continue;
        }

        changed_count += 1;
        let content = line.content.trim();
        if is_low_signal_diff_line(content)
            || (import_block && is_import_block_member_line(content))
        {
            continue;
        }

        meaningful_count += 1;
        if looks_like_declaration_line(content) {
            declaration_count += 1;
        }
        if looks_like_behavior_line(content) {
            behavior_count += 1;
        }
    }

    if changed_count == 0 {
        return -200;
    }

    if meaningful_count == 0 {
        return -100 + changed_count.min(12) as i64;
    }

    meaningful_count * 12
        + declaration_count * 36
        + behavior_count * 8
        + changed_count.min(18) as i64
}

fn diff_line_relevance_score(line: &ParsedDiffLine, import_block: bool) -> i64 {
    let content = line.content.trim();
    let mut score = match line.kind {
        DiffLineKind::Addition => 24,
        DiffLineKind::Deletion => 20,
        DiffLineKind::Context => 4,
        DiffLineKind::Meta => -80,
    };

    if content.is_empty() {
        return score - 50;
    }

    if is_low_signal_diff_line(content) || (import_block && is_import_block_member_line(content)) {
        score -= 60;
    } else {
        score += 18;
    }

    if looks_like_declaration_line(content) {
        score += 42;
    }

    if looks_like_behavior_line(content) {
        score += 14;
    }

    if content.len() > 80 {
        score += 4;
    }

    score
}

fn snippet_lines_for_hunk(
    hunk: &ParsedDiffHunk,
    focus_line_index: usize,
    max_lines: usize,
) -> Vec<&ParsedDiffLine> {
    let renderable_indices = hunk
        .lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| (line.kind != DiffLineKind::Meta).then_some(index))
        .collect::<Vec<_>>();

    if renderable_indices.is_empty() || max_lines == 0 {
        return Vec::new();
    }

    let focus_position = renderable_indices
        .iter()
        .position(|index| *index == focus_line_index)
        .unwrap_or(0);
    let mut start = focus_position.saturating_sub(2);
    let end = (start + max_lines).min(renderable_indices.len());
    if end.saturating_sub(start) < max_lines {
        start = end.saturating_sub(max_lines);
    }

    renderable_indices[start..end]
        .iter()
        .filter_map(|index| hunk.lines.get(*index))
        .collect()
}

fn hunk_looks_like_import_block(hunk: &ParsedDiffHunk) -> bool {
    hunk.lines.iter().any(|line| {
        let content = line.content.trim();
        is_import_statement_line(content)
            || content == "import ("
            || content.starts_with("import {")
            || content.starts_with("use {")
    })
}

fn is_low_signal_diff_line(content: &str) -> bool {
    if content.is_empty() {
        return true;
    }

    let trimmed = content.trim();
    if matches!(
        trimmed,
        "{" | "}" | ");" | ")," | "};" | "}," | ")" | "]" | "];" | "],"
    ) {
        return true;
    }

    is_comment_only_line(trimmed)
        || is_import_statement_line(trimmed)
        || is_reexport_statement_line(trimmed)
}

fn is_comment_only_line(content: &str) -> bool {
    content.starts_with("//")
        || content.starts_with("/*")
        || content.starts_with('*')
        || (content.starts_with('#') && !content.starts_with("#["))
}

fn is_import_statement_line(content: &str) -> bool {
    content.starts_with("import ")
        || content.starts_with("import(")
        || content.starts_with("from ")
        || content.starts_with("use ")
        || content.starts_with("pub use ")
        || content.starts_with("mod ")
        || content.starts_with("package ")
        || content.starts_with("@import ")
}

fn is_reexport_statement_line(content: &str) -> bool {
    content.starts_with("export {")
        || content.starts_with("export *")
        || content.starts_with("export type {")
}

fn is_import_block_member_line(content: &str) -> bool {
    let normalized = content
        .trim()
        .trim_matches('{')
        .trim_matches('}')
        .trim_end_matches(',')
        .trim_end_matches(';')
        .trim();

    !normalized.is_empty()
        && normalized.len() <= 80
        && normalized.chars().all(|ch| {
            ch.is_ascii_alphanumeric()
                || matches!(ch, '_' | '$' | ':' | '.' | '/' | '-' | '"' | '\'' | ' ')
        })
        && !normalized.contains('(')
        && !normalized.contains('=')
        && !looks_like_declaration_line(normalized)
}

fn looks_like_declaration_line(content: &str) -> bool {
    let trimmed = content
        .trim_start_matches("pub ")
        .trim_start_matches("async ")
        .trim_start_matches("export ")
        .trim_start_matches("default ");

    trimmed.starts_with("fn ")
        || trimmed.starts_with("function ")
        || trimmed.starts_with("class ")
        || trimmed.starts_with("struct ")
        || trimmed.starts_with("enum ")
        || trimmed.starts_with("trait ")
        || trimmed.starts_with("interface ")
        || trimmed.starts_with("type ")
        || trimmed.starts_with("impl ")
        || trimmed.starts_with("const ")
        || trimmed.starts_with("let ")
        || trimmed.starts_with("var ")
        || trimmed.starts_with("def ")
}

fn looks_like_behavior_line(content: &str) -> bool {
    let trimmed = content.trim();
    trimmed.contains("=>")
        || trimmed.contains('=')
        || trimmed.contains('(')
        || trimmed.starts_with("return ")
        || trimmed.starts_with("if ")
        || trimmed.starts_with("match ")
        || trimmed.starts_with("for ")
        || trimmed.starts_with("while ")
        || trimmed.starts_with("await ")
        || trimmed.starts_with("try ")
        || trimmed.starts_with("throw ")
}

fn file_step_score(step: &TourStep) -> i64 {
    step.additions + step.deletions + step.unresolved_thread_count * 25
}

fn map_code_tour_file_context(file: &PullRequestFile) -> CodeTourFileContext {
    CodeTourFileContext {
        path: file.path.clone(),
        additions: file.additions,
        deletions: file.deletions,
        change_type: file.change_type.clone(),
    }
}

fn map_code_tour_review_context(
    review: &crate::github::PullRequestReview,
) -> CodeTourReviewContext {
    CodeTourReviewContext {
        author_login: review.author_login.clone(),
        state: review.state.clone(),
        body: trim_text(&review.body, 900),
        submitted_at: review.submitted_at.clone(),
    }
}

fn map_code_tour_review_thread_context(
    thread: &PullRequestReviewThread,
) -> CodeTourReviewThreadContext {
    let diff_side = if !thread.diff_side.trim().is_empty() {
        Some(thread.diff_side.clone())
    } else {
        thread.start_diff_side.clone()
    };

    CodeTourReviewThreadContext {
        path: thread.path.clone(),
        line: thread.line.or(thread.original_line),
        diff_side,
        is_resolved: thread.is_resolved,
        subject_type: thread.subject_type.clone(),
        comments: thread
            .comments
            .iter()
            .take(3)
            .map(|comment| CodeTourReviewCommentContext {
                author_login: comment.author_login.clone(),
                body: trim_text(&comment.body, 500),
            })
            .collect(),
    }
}

fn prioritize_review_threads(threads: &[PullRequestReviewThread]) -> Vec<PullRequestReviewThread> {
    let mut prioritized = threads.to_vec();
    prioritized.sort_by_key(|thread| thread.is_resolved);
    prioritized
}

pub fn review_thread_anchor(thread: &PullRequestReviewThread) -> Option<DiffAnchor> {
    if thread.subject_type == "FILE" {
        return Some(DiffAnchor {
            file_path: thread.path.clone(),
            hunk_header: None,
            line: None,
            side: None,
            thread_id: Some(thread.id.clone()),
        });
    }

    let side = if !thread.diff_side.trim().is_empty() {
        thread.diff_side.clone()
    } else {
        thread
            .start_diff_side
            .clone()
            .unwrap_or_else(|| "RIGHT".to_string())
    };

    let line = if side == "LEFT" {
        thread
            .original_line
            .or(thread.line)
            .or(thread.original_start_line)
            .or(thread.start_line)
    } else {
        thread
            .line
            .or(thread.original_line)
            .or(thread.start_line)
            .or(thread.original_start_line)
    };

    Some(DiffAnchor {
        file_path: thread.path.clone(),
        hunk_header: None,
        line,
        side: line.map(|_| side),
        thread_id: Some(thread.id.clone()),
    })
}

fn resolve_diff_line_anchor(
    file_path: &str,
    line: &ParsedDiffLine,
    hunk_header: Option<&str>,
) -> Option<DiffAnchor> {
    let side = preferred_diff_side_for_line(line)?;
    let line_number = if side == "LEFT" {
        line.left_line_number
    } else {
        line.right_line_number
    }?;

    Some(DiffAnchor {
        file_path: file_path.to_string(),
        hunk_header: hunk_header.map(str::to_string),
        line: Some(line_number),
        side: Some(side.to_string()),
        thread_id: None,
    })
}

fn preferred_diff_side_for_line(line: &ParsedDiffLine) -> Option<&'static str> {
    if line.kind == DiffLineKind::Deletion && line.left_line_number.is_some() {
        return Some("LEFT");
    }

    if matches!(line.kind, DiffLineKind::Addition | DiffLineKind::Context)
        && line.right_line_number.is_some()
    {
        return Some("RIGHT");
    }

    if line.left_line_number.is_some() && line.right_line_number.is_none() {
        return Some("LEFT");
    }

    if line.right_line_number.is_some() {
        return Some("RIGHT");
    }

    None
}

fn trim_text(value: &str, max_length: usize) -> String {
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

fn code_tour_cache_key(detail: &PullRequestDetail, provider: CodeTourProvider) -> String {
    code_tour_cache_key_from_parts(
        &detail.repository,
        detail.number,
        provider,
        &tour_code_version_key(detail),
    )
}

fn code_tour_cache_key_from_parts(
    repository: &str,
    number: i64,
    provider: CodeTourProvider,
    code_version: &str,
) -> String {
    format!(
        "{}:{}:{}:{}:{}",
        CODE_TOUR_CACHE_KEY_PREFIX,
        provider.slug(),
        repository,
        number,
        code_version,
    )
}

fn hash_text(value: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        diff::parse_unified_diff,
        github::{PullRequestDetail, PullRequestFile},
    };

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

    #[test]
    fn build_tour_request_key_ignores_metadata_only_updates_when_head_matches() {
        let first = detail(
            "2026-04-17T10:00:00Z",
            Some("head123"),
            "diff --git a/a b/a\n+one\n",
        );
        let second = detail(
            "2026-04-17T11:00:00Z",
            Some("head123"),
            "diff --git a/a b/a\n+one\n",
        );

        assert_eq!(
            build_tour_request_key(&first, CodeTourProvider::Copilot),
            build_tour_request_key(&second, CodeTourProvider::Copilot),
        );
    }

    #[test]
    fn build_tour_request_key_falls_back_to_diff_hash_without_head_oid() {
        let first = detail("2026-04-17T10:00:00Z", None, "diff --git a/a b/a\n+one\n");
        let second = detail("2026-04-17T11:00:00Z", None, "diff --git a/a b/a\n+one\n");
        let changed = detail("2026-04-17T11:00:00Z", None, "diff --git a/a b/a\n+two\n");

        assert_eq!(
            build_tour_request_key(&first, CodeTourProvider::Codex),
            build_tour_request_key(&second, CodeTourProvider::Codex),
        );
        assert_ne!(
            build_tour_request_key(&first, CodeTourProvider::Codex),
            build_tour_request_key(&changed, CodeTourProvider::Codex),
        );
    }

    #[test]
    fn code_tour_settings_default_to_disabled_repositories() {
        let settings = CodeTourSettings::default();

        assert_eq!(settings.provider, CodeTourProvider::Codex);
        assert!(settings.automatic_repositories.is_empty());
        assert!(!settings.automatically_generates_for("acme/api"));
    }

    #[test]
    fn tour_step_anchor_prefers_code_hunk_over_import_hunk() {
        let raw_diff = r#"diff --git a/src/service.rs b/src/service.rs
--- a/src/service.rs
+++ b/src/service.rs
@@ -1,5 +1,6 @@
 use crate::config::Config;
+use crate::moved::MovedThing;
 use crate::old::OldThing;
 
 pub struct Service;
@@ -40,7 +41,9 @@ fn run_flow(input: Input) -> Output {
     let prepared = prepare(input);
-    old_flow(prepared)
+    let checked = validate(prepared);
+    moved_functionality(checked)
 }
"#;
        let parsed = parse_unified_diff(raw_diff);
        let parsed_file = parsed.first().expect("diff should contain a file");

        let anchor = first_anchor_for_parsed_file(parsed_file).expect("file should have an anchor");
        assert_eq!(
            anchor.hunk_header.as_deref(),
            Some("@@ -40,7 +41,9 @@ fn run_flow(input: Input) -> Output {")
        );
        assert_eq!(anchor.side.as_deref(), Some("RIGHT"));
        assert!(anchor.line.unwrap_or_default() >= 42);

        let snippet = snippet_for_parsed_file(parsed_file).expect("file should have a snippet");
        assert!(snippet.contains("moved_functionality"));
        assert!(!snippet.contains("use crate::moved::MovedThing"));
    }

    #[test]
    fn build_code_tour_generation_input_uses_representative_file_snippet() {
        let raw_diff = r#"diff --git a/src/service.rs b/src/service.rs
--- a/src/service.rs
+++ b/src/service.rs
@@ -1,5 +1,6 @@
 use crate::config::Config;
+use crate::moved::MovedThing;
 use crate::old::OldThing;
 
 pub struct Service;
@@ -40,7 +41,9 @@ fn run_flow(input: Input) -> Output {
     let prepared = prepare(input);
-    old_flow(prepared)
+    let checked = validate(prepared);
+    moved_functionality(checked)
 }
"#;
        let mut detail = detail("2026-04-17T10:00:00Z", Some("head123"), raw_diff);
        detail.files = vec![PullRequestFile {
            path: "src/service.rs".to_string(),
            additions: 3,
            deletions: 1,
            change_type: "MODIFIED".to_string(),
        }];
        detail.parsed_diff = parse_unified_diff(raw_diff);

        let input = build_code_tour_generation_input(&detail, CodeTourProvider::Codex, "/tmp/repo");
        let step = input
            .candidate_steps
            .iter()
            .find(|step| step.id == "file:src/service.rs")
            .expect("file step should exist");

        assert_eq!(
            step.anchor
                .as_ref()
                .and_then(|anchor| anchor.hunk_header.as_deref()),
            Some("@@ -40,7 +41,9 @@ fn run_flow(input: Input) -> Output {")
        );
        assert!(step
            .snippet
            .as_deref()
            .unwrap_or_default()
            .contains("moved_functionality"));
    }
}
