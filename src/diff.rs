use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::github::{PullRequestDetail, PullRequestReviewThread};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedDiffFile {
    pub path: String,
    pub previous_path: Option<String>,
    pub hunks: Vec<ParsedDiffHunk>,
    pub is_binary: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedDiffHunk {
    pub header: String,
    pub lines: Vec<ParsedDiffLine>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedDiffLine {
    pub kind: DiffLineKind,
    #[serde(default)]
    pub prefix: String,
    pub left_line_number: Option<i64>,
    pub right_line_number: Option<i64>,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum DiffLineKind {
    Context,
    Addition,
    Deletion,
    Meta,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffRenderRow {
    FileCommentsHeader {
        count: usize,
    },
    FileCommentThread {
        thread_index: usize,
    },
    HunkHeader {
        hunk_index: usize,
    },
    Line {
        hunk_index: usize,
        line_index: usize,
    },
    InlineThread {
        thread_index: usize,
    },
    OutdatedCommentsHeader {
        count: usize,
    },
    OutdatedThread {
        thread_index: usize,
    },
    NoTextHunks,
    RawDiffFallback,
    NoParsedDiff,
}

pub fn parse_unified_diff(diff: &str) -> Vec<ParsedDiffFile> {
    let mut files = Vec::new();
    let mut current_file: Option<ParsedDiffFileBuilder> = None;
    let mut current_hunk: Option<ParsedDiffHunkBuilder> = None;

    for line in diff.lines() {
        if let Some(stripped) = line.strip_prefix("diff --git ") {
            finalize_hunk(&mut current_file, &mut current_hunk);
            finalize_file(&mut files, &mut current_file);

            let mut parts = stripped.split_whitespace();
            let previous = parts.next().map(normalize_diff_path);
            let path = parts.next().map(normalize_diff_path).unwrap_or_default();

            current_file = Some(ParsedDiffFileBuilder {
                path,
                previous_path: previous,
                hunks: Vec::new(),
                is_binary: false,
            });
            continue;
        }

        let Some(file) = current_file.as_mut() else {
            continue;
        };

        if line.starts_with("Binary files ") {
            file.is_binary = true;
            continue;
        }

        if let Some(path) = line.strip_prefix("+++ ") {
            let normalized = normalize_diff_path(path);
            if !normalized.is_empty() && normalized != "/dev/null" {
                file.path = normalized;
            }
            continue;
        }

        if let Some(path) = line.strip_prefix("--- ") {
            let normalized = normalize_diff_path(path);
            if !normalized.is_empty() && normalized != "/dev/null" {
                file.previous_path = Some(normalized);
            }
            continue;
        }

        if line.starts_with("@@") {
            finalize_hunk(&mut current_file, &mut current_hunk);
            let (left_line, right_line) = parse_hunk_header(line);
            current_hunk = Some(ParsedDiffHunkBuilder {
                header: line.to_string(),
                lines: Vec::new(),
                next_left_line: left_line,
                next_right_line: right_line,
            });
            continue;
        }

        if let Some(hunk) = current_hunk.as_mut() {
            hunk.push_line(line);
        }
    }

    finalize_hunk(&mut current_file, &mut current_hunk);
    finalize_file(&mut files, &mut current_file);

    files
}

pub fn find_parsed_diff_file<'a>(
    parsed: &'a [ParsedDiffFile],
    path: &str,
) -> Option<&'a ParsedDiffFile> {
    find_parsed_diff_file_with_index(parsed, path).map(|(_, file)| file)
}

pub fn find_parsed_diff_file_with_index<'a>(
    parsed: &'a [ParsedDiffFile],
    path: &str,
) -> Option<(usize, &'a ParsedDiffFile)> {
    parsed
        .iter()
        .enumerate()
        .find(|(_, file)| file.path == path)
        .or_else(|| {
            parsed
                .iter()
                .enumerate()
                .find(|(_, file)| file.previous_path.as_deref() == Some(path))
        })
}

pub fn build_diff_render_rows(detail: &PullRequestDetail, file_path: &str) -> Vec<DiffRenderRow> {
    let parsed = find_parsed_diff_file(&detail.parsed_diff, file_path);
    let (file_comment_threads, inline_thread_map, outdated_threads) =
        index_review_threads(&detail.review_threads, file_path);

    let mut rows = Vec::new();

    if !file_comment_threads.is_empty() {
        rows.push(DiffRenderRow::FileCommentsHeader {
            count: file_comment_threads.len(),
        });
        rows.extend(
            file_comment_threads
                .into_iter()
                .map(|thread_index| DiffRenderRow::FileCommentThread { thread_index }),
        );
    }

    if let Some(parsed) = parsed {
        if parsed.hunks.is_empty() {
            rows.push(DiffRenderRow::NoTextHunks);
        } else {
            for (hunk_index, hunk) in parsed.hunks.iter().enumerate() {
                rows.push(DiffRenderRow::HunkHeader { hunk_index });

                for (line_index, line) in hunk.lines.iter().enumerate() {
                    rows.push(DiffRenderRow::Line {
                        hunk_index,
                        line_index,
                    });

                    if let Some((side, line_number)) = diff_thread_key_for_line(line) {
                        if let Some(thread_indices) = inline_thread_map.get(&(side, line_number)) {
                            rows.extend(
                                thread_indices.iter().copied().map(|thread_index| {
                                    DiffRenderRow::InlineThread { thread_index }
                                }),
                            );
                        }
                    }
                }
            }
        }
    } else if detail.parsed_diff.is_empty() {
        rows.push(DiffRenderRow::RawDiffFallback);
    } else {
        rows.push(DiffRenderRow::NoParsedDiff);
    }

    if !outdated_threads.is_empty() {
        rows.push(DiffRenderRow::OutdatedCommentsHeader {
            count: outdated_threads.len(),
        });
        rows.extend(
            outdated_threads
                .into_iter()
                .map(|thread_index| DiffRenderRow::OutdatedThread { thread_index }),
        );
    }

    rows
}

#[derive(Debug)]
struct ParsedDiffFileBuilder {
    path: String,
    previous_path: Option<String>,
    hunks: Vec<ParsedDiffHunk>,
    is_binary: bool,
}

#[derive(Debug)]
struct ParsedDiffHunkBuilder {
    header: String,
    lines: Vec<ParsedDiffLine>,
    next_left_line: i64,
    next_right_line: i64,
}

impl ParsedDiffHunkBuilder {
    fn push_line(&mut self, line: &str) {
        if let Some(content) = line.strip_prefix('+') {
            self.lines.push(ParsedDiffLine {
                kind: DiffLineKind::Addition,
                prefix: "+".to_string(),
                left_line_number: None,
                right_line_number: Some(self.next_right_line),
                content: content.to_string(),
            });
            self.next_right_line += 1;
            return;
        }

        if let Some(content) = line.strip_prefix('-') {
            self.lines.push(ParsedDiffLine {
                kind: DiffLineKind::Deletion,
                prefix: "-".to_string(),
                left_line_number: Some(self.next_left_line),
                right_line_number: None,
                content: content.to_string(),
            });
            self.next_left_line += 1;
            return;
        }

        if let Some(content) = line.strip_prefix(' ') {
            self.lines.push(ParsedDiffLine {
                kind: DiffLineKind::Context,
                prefix: " ".to_string(),
                left_line_number: Some(self.next_left_line),
                right_line_number: Some(self.next_right_line),
                content: content.to_string(),
            });
            self.next_left_line += 1;
            self.next_right_line += 1;
            return;
        }

        self.lines.push(ParsedDiffLine {
            kind: DiffLineKind::Meta,
            prefix: String::new(),
            left_line_number: None,
            right_line_number: None,
            content: line.to_string(),
        });
    }

    fn build(self) -> ParsedDiffHunk {
        ParsedDiffHunk {
            header: self.header,
            lines: self.lines,
        }
    }
}

fn normalize_diff_path(path: &str) -> String {
    path.trim()
        .trim_start_matches("a/")
        .trim_start_matches("b/")
        .to_string()
}

fn parse_hunk_header(header: &str) -> (i64, i64) {
    let mut parts = header.split_whitespace();
    let left = parts.nth(1).and_then(parse_hunk_range).unwrap_or(0);
    let right = parts.next().and_then(parse_hunk_range).unwrap_or(0);
    (left, right)
}

fn parse_hunk_range(value: &str) -> Option<i64> {
    let trimmed = value.trim_start_matches(['@', '-', '+']);
    let start = trimmed.split(',').next()?;
    start.parse::<i64>().ok()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
enum DiffThreadSide {
    Left,
    Right,
}

type InlineThreadIndex = HashMap<(DiffThreadSide, i64), Vec<usize>>;
type ReviewThreadIndex = (Vec<usize>, InlineThreadIndex, Vec<usize>);

fn index_review_threads(
    review_threads: &[PullRequestReviewThread],
    file_path: &str,
) -> ReviewThreadIndex {
    let mut file_comment_threads = Vec::new();
    let mut inline_threads = HashMap::<(DiffThreadSide, i64), Vec<usize>>::new();
    let mut outdated_threads = Vec::new();

    for (thread_index, thread) in review_threads.iter().enumerate() {
        if thread.path != file_path {
            continue;
        }

        if thread.is_outdated {
            outdated_threads.push(thread_index);
            continue;
        }

        let has_line = thread.line.is_some() || thread.original_line.is_some();
        if !has_line {
            file_comment_threads.push(thread_index);
            continue;
        }

        let side = match thread.diff_side.as_str() {
            "LEFT" => Some(DiffThreadSide::Left),
            "RIGHT" => Some(DiffThreadSide::Right),
            _ => None,
        };

        if let Some(side) = side {
            push_thread_anchor(
                &mut inline_threads,
                side,
                thread.line,
                thread.original_line,
                thread_index,
            );
        }
    }

    (file_comment_threads, inline_threads, outdated_threads)
}

fn push_thread_anchor(
    inline_threads: &mut HashMap<(DiffThreadSide, i64), Vec<usize>>,
    side: DiffThreadSide,
    line: Option<i64>,
    original_line: Option<i64>,
    thread_index: usize,
) {
    if let Some(line) = line {
        inline_threads
            .entry((side, line))
            .or_default()
            .push(thread_index);
    }

    if let Some(original_line) = original_line {
        if Some(original_line) != line {
            inline_threads
                .entry((side, original_line))
                .or_default()
                .push(thread_index);
        }
    }
}

fn diff_thread_key_for_line(line: &ParsedDiffLine) -> Option<(DiffThreadSide, i64)> {
    match line.kind {
        DiffLineKind::Addition | DiffLineKind::Context => line
            .right_line_number
            .map(|line_number| (DiffThreadSide::Right, line_number)),
        DiffLineKind::Deletion => line
            .left_line_number
            .map(|line_number| (DiffThreadSide::Left, line_number)),
        DiffLineKind::Meta => None,
    }
}

fn finalize_hunk(
    current_file: &mut Option<ParsedDiffFileBuilder>,
    current_hunk: &mut Option<ParsedDiffHunkBuilder>,
) {
    if let (Some(file), Some(hunk)) = (current_file.as_mut(), current_hunk.take()) {
        file.hunks.push(hunk.build());
    }
}

fn finalize_file(
    files: &mut Vec<ParsedDiffFile>,
    current_file: &mut Option<ParsedDiffFileBuilder>,
) {
    if let Some(file) = current_file.take() {
        files.push(ParsedDiffFile {
            path: file.path,
            previous_path: file.previous_path,
            hunks: file.hunks,
            is_binary: file.is_binary,
        });
    }
}
