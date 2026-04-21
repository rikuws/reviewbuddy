use std::collections::HashMap;

use crate::{
    code_tour::DiffAnchor,
    diff::{DiffLineKind, ParsedDiffFile, ParsedDiffHunk},
    github::{PullRequestFile, PullRequestReviewThread},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SemanticChangeKind {
    Logic,
    Type,
    Refactor,
    Extract,
    Inline,
    Rename,
    Formatting,
    Tests,
    Docs,
    Config,
    DataFlow,
    Unknown,
}

impl SemanticChangeKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Logic => "logic",
            Self::Type => "type",
            Self::Refactor => "refactor",
            Self::Extract => "extract",
            Self::Inline => "inline",
            Self::Rename => "rename",
            Self::Formatting => "formatting",
            Self::Tests => "tests",
            Self::Docs => "docs",
            Self::Config => "config",
            Self::DataFlow => "data-flow",
            Self::Unknown => "change",
        }
    }
}

#[derive(Clone, Debug)]
pub struct SemanticDiffSection {
    pub id: String,
    pub title: String,
    pub kind: SemanticChangeKind,
    pub summary: String,
    pub additions: usize,
    pub deletions: usize,
    pub thread_count: usize,
    pub line_count: usize,
    pub anchor: Option<DiffAnchor>,
    pub hunk_indices: Vec<usize>,
}

#[derive(Clone, Debug)]
pub struct SemanticDiffFile {
    pub file_kind: SemanticChangeKind,
    pub file_summary: String,
    pub sections: Vec<SemanticDiffSection>,
    pub section_by_hunk: HashMap<usize, usize>,
}

impl SemanticDiffFile {
    pub fn section_index_for_hunk(&self, hunk_index: usize) -> Option<usize> {
        self.section_by_hunk.get(&hunk_index).copied()
    }

    pub fn section_for_anchor(&self, anchor: Option<&DiffAnchor>) -> Option<&SemanticDiffSection> {
        let anchor = anchor?;
        self.sections.iter().find(|section| {
            let Some(candidate) = section.anchor.as_ref() else {
                return false;
            };

            if let Some(line) = anchor.line {
                if candidate.line == Some(line) {
                    return true;
                }
            }

            candidate.hunk_header == anchor.hunk_header
        })
    }
}

pub fn build_semantic_diff_file(
    file: &PullRequestFile,
    parsed_file: Option<&ParsedDiffFile>,
    review_threads: &[PullRequestReviewThread],
) -> SemanticDiffFile {
    let file_kind = classify_file_kind(file.path.as_str(), file.change_type.as_str());
    let file_thread_count = review_threads
        .iter()
        .filter(|thread| thread.path == file.path && !thread.is_resolved)
        .count();

    let Some(parsed_file) = parsed_file else {
        return SemanticDiffFile {
            file_kind,
            file_summary: file_summary(file_kind, file, file_thread_count),
            sections: vec![SemanticDiffSection {
                id: format!("{}::full-file", file.path),
                title: file.path.clone(),
                kind: file_kind,
                summary: file_summary(file_kind, file, file_thread_count),
                additions: file.additions.max(0) as usize,
                deletions: file.deletions.max(0) as usize,
                thread_count: file_thread_count,
                line_count: (file.additions + file.deletions).max(0) as usize,
                anchor: Some(DiffAnchor {
                    file_path: file.path.clone(),
                    hunk_header: None,
                    line: None,
                    side: None,
                    thread_id: None,
                }),
                hunk_indices: Vec::new(),
            }],
            section_by_hunk: HashMap::new(),
        };
    };

    let mut sections = Vec::<SemanticDiffSection>::new();
    let mut section_by_hunk = HashMap::<usize, usize>::new();

    for (hunk_index, hunk) in parsed_file.hunks.iter().enumerate() {
        let title = semantic_title_for_hunk(hunk, file.path.as_str(), hunk_index);
        let kind = classify_hunk_kind(file, hunk, file_kind);
        let additions = hunk
            .lines
            .iter()
            .filter(|line| line.kind == DiffLineKind::Addition)
            .count();
        let deletions = hunk
            .lines
            .iter()
            .filter(|line| line.kind == DiffLineKind::Deletion)
            .count();
        let line_count = additions + deletions;
        let anchor = first_anchor_for_hunk(file.path.as_str(), hunk);
        let thread_count = review_threads
            .iter()
            .filter(|thread| {
                thread.path == file.path
                    && !thread.is_resolved
                    && hunk_contains_thread(hunk, thread.line.or(thread.original_line))
            })
            .count();

        let summary = format!(
            "{} in {} with +{} / -{}{}.",
            kind.label(),
            title,
            additions,
            deletions,
            if thread_count > 0 {
                format!(
                    " and {thread_count} open thread{}",
                    if thread_count == 1 { "" } else { "s" }
                )
            } else {
                String::new()
            }
        );

        if let Some(previous) = sections.last_mut() {
            if previous.title == title && previous.kind == kind {
                previous.additions += additions;
                previous.deletions += deletions;
                previous.line_count += line_count;
                previous.thread_count += thread_count;
                previous.hunk_indices.push(hunk_index);
                section_by_hunk.insert(hunk_index, sections.len() - 1);
                continue;
            }
        }

        let section_index = sections.len();
        sections.push(SemanticDiffSection {
            id: format!("{}::{section_index}", file.path),
            title,
            kind,
            summary,
            additions,
            deletions,
            thread_count,
            line_count,
            anchor,
            hunk_indices: vec![hunk_index],
        });
        section_by_hunk.insert(hunk_index, section_index);
    }

    SemanticDiffFile {
        file_kind,
        file_summary: file_summary(file_kind, file, file_thread_count),
        sections,
        section_by_hunk,
    }
}

fn file_summary(kind: SemanticChangeKind, file: &PullRequestFile, thread_count: usize) -> String {
    let base = format!(
        "{} file with +{} / -{}",
        kind.label(),
        file.additions,
        file.deletions
    );
    if thread_count > 0 {
        format!(
            "{base} and {thread_count} unresolved review thread{}.",
            if thread_count == 1 { "" } else { "s" }
        )
    } else {
        format!("{base}.")
    }
}

fn classify_file_kind(path: &str, change_type: &str) -> SemanticChangeKind {
    if change_type == "RENAMED" || change_type == "COPIED" {
        return SemanticChangeKind::Rename;
    }

    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".md") || lower.ends_with(".rst") || lower.contains("/docs/") {
        return SemanticChangeKind::Docs;
    }

    if lower.contains("/test")
        || lower.contains("/tests/")
        || lower.contains("_test.")
        || lower.contains(".spec.")
        || lower.contains(".test.")
    {
        return SemanticChangeKind::Tests;
    }

    if lower.ends_with(".json")
        || lower.ends_with(".toml")
        || lower.ends_with(".yaml")
        || lower.ends_with(".yml")
        || lower.ends_with(".lock")
        || lower.ends_with(".ini")
    {
        return SemanticChangeKind::Config;
    }

    SemanticChangeKind::Logic
}

fn classify_hunk_kind(
    file: &PullRequestFile,
    hunk: &ParsedDiffHunk,
    file_kind: SemanticChangeKind,
) -> SemanticChangeKind {
    if matches!(
        file_kind,
        SemanticChangeKind::Docs
            | SemanticChangeKind::Tests
            | SemanticChangeKind::Config
            | SemanticChangeKind::Rename
    ) {
        return file_kind;
    }

    if is_whitespace_only_hunk(hunk) {
        return SemanticChangeKind::Formatting;
    }

    let additions = changed_lines(hunk, DiffLineKind::Addition);
    let deletions = changed_lines(hunk, DiffLineKind::Deletion);
    let adds_symbol = additions
        .iter()
        .any(|line| looks_like_symbol_definition(line));
    let deletes_symbol = deletions
        .iter()
        .any(|line| looks_like_symbol_definition(line));

    if adds_symbol && !deletions.is_empty() && similarity_score(&additions, &deletions) > 0.58 {
        return SemanticChangeKind::Extract;
    }

    if deletes_symbol && !additions.is_empty() && similarity_score(&additions, &deletions) > 0.58 {
        return SemanticChangeKind::Inline;
    }

    if !additions.is_empty()
        && !deletions.is_empty()
        && similarity_score(&additions, &deletions) > 0.62
    {
        return SemanticChangeKind::Refactor;
    }

    let title = semantic_title_for_hunk(hunk, file.path.as_str(), 0).to_ascii_lowercase();
    if title.starts_with("struct ")
        || title.starts_with("enum ")
        || title.starts_with("trait ")
        || title.starts_with("class ")
        || title.starts_with("interface ")
        || title.starts_with("type ")
    {
        return SemanticChangeKind::Type;
    }

    if title.starts_with("match ")
        || title.starts_with("switch ")
        || title.starts_with("if ")
        || title.starts_with("guard ")
    {
        return SemanticChangeKind::DataFlow;
    }

    SemanticChangeKind::Logic
}

fn semantic_title_for_hunk(hunk: &ParsedDiffHunk, file_path: &str, hunk_index: usize) -> String {
    let header_context = hunk
        .header
        .split("@@")
        .last()
        .map(str::trim)
        .filter(|context| !context.is_empty())
        .and_then(clean_symbol_title);

    if let Some(title) = header_context {
        return title;
    }

    for line in changed_lines(hunk, DiffLineKind::Addition)
        .into_iter()
        .chain(changed_lines(hunk, DiffLineKind::Deletion))
    {
        if let Some(title) = clean_symbol_title(&line) {
            return title;
        }
    }

    format!("{} hunk {}", file_stem(file_path), hunk_index + 1)
}

fn clean_symbol_title(value: &str) -> Option<String> {
    let trimmed = value
        .trim()
        .trim_start_matches("pub ")
        .trim_start_matches("async ")
        .trim_start_matches("export ")
        .trim_start_matches("default ");

    if trimmed.is_empty() {
        return None;
    }

    let candidate = [
        "fn ",
        "struct ",
        "enum ",
        "trait ",
        "impl ",
        "mod ",
        "type ",
        "class ",
        "interface ",
        "function ",
        "def ",
        "func ",
    ]
    .iter()
    .find_map(|pattern| extract_after_pattern(trimmed, pattern))
    .or_else(|| trimmed.contains(" => ").then(|| trim_title(trimmed)))
    .unwrap_or_else(|| trim_title(trimmed));

    (!candidate.is_empty()).then_some(candidate)
}

fn extract_after_pattern(value: &str, pattern: &str) -> Option<String> {
    let (_, rest) = value.split_once(pattern)?;
    let rest = rest.trim();
    if rest.is_empty() {
        return None;
    }

    let name = if pattern == "impl " {
        rest.split('{').next().unwrap_or(rest).trim()
    } else {
        rest.split(['(', '{', '<', ':', '=', ' '])
            .next()
            .unwrap_or(rest)
            .trim()
    };

    if name.is_empty() {
        None
    } else if pattern == "impl " {
        Some(trim_title(&format!("impl {name}")))
    } else {
        Some(trim_title(&format!("{pattern}{name}")))
    }
}

fn trim_title(value: &str) -> String {
    let mut out = value
        .trim()
        .trim_end_matches('{')
        .trim_end_matches(':')
        .trim()
        .chars()
        .take(64)
        .collect::<String>();
    if value.chars().count() > 64 {
        out.push('…');
    }
    out
}

fn changed_lines(hunk: &ParsedDiffHunk, kind: DiffLineKind) -> Vec<String> {
    hunk.lines
        .iter()
        .filter(|line| line.kind == kind)
        .map(|line| line.content.clone())
        .collect()
}

fn looks_like_symbol_definition(line: &str) -> bool {
    let trimmed = line.trim();
    [
        "fn ",
        "struct ",
        "enum ",
        "trait ",
        "impl ",
        "class ",
        "interface ",
        "type ",
        "function ",
        "def ",
        "func ",
    ]
    .iter()
    .any(|pattern| trimmed.contains(pattern))
}

fn is_whitespace_only_hunk(hunk: &ParsedDiffHunk) -> bool {
    let additions = changed_lines(hunk, DiffLineKind::Addition)
        .into_iter()
        .map(normalize_whitespace_only)
        .collect::<Vec<_>>();
    let deletions = changed_lines(hunk, DiffLineKind::Deletion)
        .into_iter()
        .map(normalize_whitespace_only)
        .collect::<Vec<_>>();

    !additions.is_empty() && additions == deletions
}

fn similarity_score(additions: &[String], deletions: &[String]) -> f32 {
    let add_tokens = additions
        .iter()
        .flat_map(|line| tokenize(line))
        .collect::<Vec<_>>();
    let del_tokens = deletions
        .iter()
        .flat_map(|line| tokenize(line))
        .collect::<Vec<_>>();

    if add_tokens.is_empty() || del_tokens.is_empty() {
        return 0.0;
    }

    let add_set = add_tokens.iter().collect::<std::collections::HashSet<_>>();
    let del_set = del_tokens.iter().collect::<std::collections::HashSet<_>>();
    let union = add_set.union(&del_set).count();
    let intersection = add_set.intersection(&del_set).count();

    if union == 0 {
        0.0
    } else {
        intersection as f32 / union as f32
    }
}

fn tokenize(value: &str) -> Vec<String> {
    value
        .split(|ch: char| !ch.is_alphanumeric() && ch != '_')
        .map(|segment| segment.trim().to_ascii_lowercase())
        .filter(|segment| segment.len() > 1)
        .collect()
}

fn normalize_whitespace_only(value: String) -> String {
    value.chars().filter(|ch| !ch.is_whitespace()).collect()
}

fn first_anchor_for_hunk(file_path: &str, hunk: &ParsedDiffHunk) -> Option<DiffAnchor> {
    for line in &hunk.lines {
        if line.kind == DiffLineKind::Deletion {
            if let Some(line_number) = line.left_line_number {
                return Some(DiffAnchor {
                    file_path: file_path.to_string(),
                    hunk_header: Some(hunk.header.clone()),
                    line: Some(line_number),
                    side: Some("LEFT".to_string()),
                    thread_id: None,
                });
            }
        }

        if matches!(line.kind, DiffLineKind::Addition | DiffLineKind::Context) {
            if let Some(line_number) = line.right_line_number {
                return Some(DiffAnchor {
                    file_path: file_path.to_string(),
                    hunk_header: Some(hunk.header.clone()),
                    line: Some(line_number),
                    side: Some("RIGHT".to_string()),
                    thread_id: None,
                });
            }
        }
    }

    Some(DiffAnchor {
        file_path: file_path.to_string(),
        hunk_header: Some(hunk.header.clone()),
        line: None,
        side: None,
        thread_id: None,
    })
}

fn hunk_contains_thread(hunk: &ParsedDiffHunk, thread_line: Option<i64>) -> bool {
    let Some(thread_line) = thread_line else {
        return false;
    };

    hunk.lines.iter().any(|line| {
        line.left_line_number == Some(thread_line) || line.right_line_number == Some(thread_line)
    })
}

fn file_stem(file_path: &str) -> String {
    file_path
        .rsplit('/')
        .next()
        .unwrap_or(file_path)
        .to_string()
}

#[cfg(test)]
mod tests {
    use crate::{
        diff::{DiffLineKind, ParsedDiffHunk, ParsedDiffLine},
        github::PullRequestFile,
    };

    use super::{build_semantic_diff_file, SemanticChangeKind};

    #[test]
    fn classifies_docs_files_from_path() {
        let file = PullRequestFile {
            path: "docs/review.md".to_string(),
            additions: 4,
            deletions: 1,
            change_type: "MODIFIED".to_string(),
        };

        let semantic = build_semantic_diff_file(&file, None, &[]);
        assert_eq!(semantic.file_kind, SemanticChangeKind::Docs);
    }

    #[test]
    fn extracts_symbol_name_from_hunk_header() {
        let file = PullRequestFile {
            path: "src/views/diff_view.rs".to_string(),
            additions: 10,
            deletions: 2,
            change_type: "MODIFIED".to_string(),
        };
        let parsed = crate::diff::ParsedDiffFile {
            path: file.path.clone(),
            previous_path: None,
            is_binary: false,
            hunks: vec![ParsedDiffHunk {
                header: "@@ -10,6 +10,8 @@ fn render_diff_panel(".to_string(),
                lines: vec![ParsedDiffLine {
                    kind: DiffLineKind::Addition,
                    prefix: "+".to_string(),
                    left_line_number: None,
                    right_line_number: Some(10),
                    content: "    let queue = build_review_queue(detail);".to_string(),
                }],
            }],
        };

        let semantic = build_semantic_diff_file(&file, Some(&parsed), &[]);
        assert_eq!(semantic.sections[0].title, "fn render_diff_panel");
    }

    #[test]
    fn marks_whitespace_only_hunks_as_formatting() {
        let file = PullRequestFile {
            path: "src/main.rs".to_string(),
            additions: 1,
            deletions: 1,
            change_type: "MODIFIED".to_string(),
        };
        let parsed = crate::diff::ParsedDiffFile {
            path: file.path.clone(),
            previous_path: None,
            is_binary: false,
            hunks: vec![ParsedDiffHunk {
                header: "@@ -1,2 +1,2 @@ fn main(".to_string(),
                lines: vec![
                    ParsedDiffLine {
                        kind: DiffLineKind::Deletion,
                        prefix: "-".to_string(),
                        left_line_number: Some(1),
                        right_line_number: None,
                        content: "fn main() {".to_string(),
                    },
                    ParsedDiffLine {
                        kind: DiffLineKind::Addition,
                        prefix: "+".to_string(),
                        left_line_number: None,
                        right_line_number: Some(1),
                        content: "fn  main() {".to_string(),
                    },
                ],
            }],
        };

        let semantic = build_semantic_diff_file(&file, Some(&parsed), &[]);
        assert_eq!(semantic.sections[0].kind, SemanticChangeKind::Formatting);
    }
}
