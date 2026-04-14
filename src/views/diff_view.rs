use std::sync::Arc;

use gpui::prelude::*;
use gpui::*;

use crate::code_tour::{line_matches_diff_anchor, thread_matches_diff_anchor, DiffAnchor};
use crate::diff::{
    build_diff_render_rows, find_parsed_diff_file, find_parsed_diff_file_with_index, DiffLineKind,
    DiffRenderRow, ParsedDiffFile, ParsedDiffHunk, ParsedDiffLine,
};
use crate::github;
use crate::github::{
    PullRequestDetail, PullRequestFile, PullRequestReviewComment, PullRequestReviewThread,
    RepositoryFileContent,
};
use crate::markdown::render_markdown;
use crate::state::*;
use crate::syntax::{self, SyntaxSpan};
use crate::theme::*;

use super::sections::{badge, badge_success, nested_panel, panel_state_text};

const MAX_FILE_HIGHLIGHT_BYTES: usize = 512 * 1024;

pub fn enter_files_surface(state: &Entity<AppState>, window: &mut Window, cx: &mut App) {
    state.update(cx, |s, cx| {
        s.active_surface = PullRequestSurface::Files;

        if s.selected_file_path.is_none() {
            s.selected_file_path = s.active_detail().and_then(|detail| {
                detail
                    .files
                    .first()
                    .map(|file| file.path.clone())
                    .or_else(|| detail.parsed_diff.first().map(|file| file.path.clone()))
            });
        }

        cx.notify();
    });

    ensure_selected_file_content_loaded(state, window, cx);
}

pub fn render_files_view(state: &Entity<AppState>, cx: &App) -> impl IntoElement {
    let s = state.read(cx);
    let detail = s.active_detail();

    let Some(detail) = detail else {
        return div()
            .child(panel_state_text("No detail data available."))
            .into_any_element();
    };

    let files = &detail.files;
    let parsed_diff = &detail.parsed_diff;
    let additions = detail.additions;
    let deletions = detail.deletions;
    let selected_anchor = s.selected_diff_anchor.clone();

    let selected_path = s
        .selected_file_path
        .as_deref()
        .and_then(|path| files.iter().find(|file| file.path == path))
        .map(|file| file.path.as_str())
        .or_else(|| {
            files
                .first()
                .map(|f| f.path.as_str())
                .or_else(|| parsed_diff.first().map(|f| f.path.as_str()))
        });

    let state_for_tree = state.clone();

    div()
        .flex()
        .flex_grow()
        .min_h_0()
        // File tree sidebar
        .child(render_file_tree(
            &files,
            additions,
            deletions,
            selected_path,
            state_for_tree,
        ))
        // Diff panel
        .child(render_diff_panel(
            state,
            &s,
            detail,
            selected_path,
            selected_anchor.as_ref(),
            cx,
        ))
        .into_any_element()
}

fn render_file_tree(
    files: &[PullRequestFile],
    additions: i64,
    deletions: i64,
    selected_path: Option<&str>,
    state: Entity<AppState>,
) -> impl IntoElement {
    div()
        .w(file_tree_width())
        .flex_shrink_0()
        .min_h_0()
        .bg(bg_surface())
        .border_r(px(1.0))
        .border_color(border_default())
        .id("file-tree-scroll")
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .child(
            div()
                .px(px(16.0))
                .py(px(14.0))
                .border_b(px(1.0))
                .border_color(border_default())
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(fg_emphasis())
                        .child("Changed files"),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_family("Fira Code")
                        .flex()
                        .gap(px(6.0))
                        .items_center()
                        .child(
                            div()
                                .text_color(fg_muted())
                                .child(format!("{} files", files.len())),
                        )
                        .child(div().text_color(fg_subtle()).child("\u{2022}"))
                        .child(div().text_color(success()).child(format!("+{}", additions)))
                        .child(div().text_color(fg_subtle()).child("/"))
                        .child(div().text_color(danger()).child(format!("-{}", deletions))),
                ),
        )
        .px(px(8.0))
        .py(px(8.0))
        .children(files.iter().map(|file| {
            let path = file.path.clone();
            let is_active = selected_path == Some(file.path.as_str());
            let file_additions = file.additions;
            let file_deletions = file.deletions;
            let state = state.clone();

            div()
                .w_full()
                .mb(px(6.0))
                .px(px(12.0))
                .py(px(10.0))
                .rounded(radius())
                .border_1()
                .border_color(if is_active { accent() } else { border_muted() })
                .bg(if is_active {
                    accent_muted()
                } else {
                    bg_surface()
                })
                .flex()
                .items_start()
                .justify_between()
                .gap(px(12.0))
                .cursor_pointer()
                .text_size(px(12.0))
                .when(is_active, |el| el.text_color(fg_emphasis()))
                .when(!is_active, |el| el.text_color(fg_default()))
                .hover(|style| style.bg(hover_bg()))
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    state.update(cx, |s, cx| {
                        s.selected_file_path = Some(path.clone());
                        s.selected_diff_anchor = None;
                        cx.notify();
                    });

                    ensure_selected_file_content_loaded(&state, window, cx);
                })
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .overflow_x_hidden()
                        .min_w_0()
                        .child(
                            div()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .overflow_x_hidden()
                                .child(file.path.clone()),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .child(render_change_type_chip(&file.change_type)),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_end()
                        .gap(px(6.0))
                        .flex_shrink_0()
                        .child(
                            div()
                                .flex()
                                .gap(px(4.0))
                                .text_size(px(11.0))
                                .font_family("Fira Code")
                                .whitespace_nowrap()
                                .child(
                                    div()
                                        .text_color(success())
                                        .child(format!("+{file_additions}")),
                                )
                                .child(
                                    div()
                                        .text_color(danger())
                                        .child(format!("-{file_deletions}")),
                                ),
                        )
                        .child(render_file_stat_bar(file_additions, file_deletions)),
                )
        }))
}

pub fn ensure_selected_file_content_loaded(
    state: &Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
) {
    let model = state.clone();
    window
        .spawn(cx, async move |cx: &mut AsyncWindowContext| {
            load_selected_file_content_flow(model, cx).await;
        })
        .detach();
}

async fn load_selected_file_content_flow(model: Entity<AppState>, cx: &mut AsyncWindowContext) {
    let request = model
        .read_with(cx, |state, _| {
            let cache = state.cache.clone();
            let detail = state.active_detail()?.clone();
            let detail_key = state.active_pr_key.clone()?;
            let selected_path = state
                .selected_file_path
                .clone()
                .or_else(|| detail.files.first().map(|file| file.path.clone()))?;
            let selected_file = detail
                .files
                .iter()
                .find(|file| file.path == selected_path)
                .cloned()?;
            let parsed = find_parsed_diff_file(&detail.parsed_diff, &selected_file.path).cloned();
            let request = build_file_content_request(&detail, &selected_file, parsed.as_ref())?;

            let already_loaded = state
                .detail_states
                .get(&detail_key)
                .and_then(|detail_state| detail_state.file_content_states.get(&request.path))
                .map(|file_state| {
                    file_state.request_key.as_deref() == Some(&request.request_key)
                        && (file_state.loading || file_state.document.is_some())
                })
                .unwrap_or(false);

            Some((
                cache,
                detail_key,
                detail,
                selected_file,
                request,
                already_loaded,
            ))
        })
        .ok()
        .flatten();

    let Some((cache, detail_key, detail, selected_file, request, already_loaded)) = request else {
        return;
    };

    if already_loaded {
        return;
    }

    model
        .update(cx, |state, cx| {
            if let Some(detail_state) = state.detail_states.get_mut(&detail_key) {
                let file_state = detail_state
                    .file_content_states
                    .entry(request.path.clone())
                    .or_default();
                file_state.request_key = Some(request.request_key.clone());
                file_state.document = None;
                file_state.prepared = None;
                file_state.loading = true;
                file_state.error = None;
            }

            cx.notify();
        })
        .ok();

    let load_result =
        cx.background_executor()
            .spawn({
                let cache = cache.clone();
                let repository = detail.repository.clone();
                let path = request.path.clone();
                let reference = request.reference.clone();
                async move {
                    github::load_pull_request_file_content(&cache, &repository, &reference, &path)
                }
            })
            .await;

    let prepared_result = load_result.map(|document| {
        let prepared = prepare_file_content(&selected_file.path, &request.reference, &document);
        (document, prepared)
    });

    model
        .update(cx, |state, cx| {
            let Some(detail_state) = state.detail_states.get_mut(&detail_key) else {
                return;
            };
            let Some(file_state) = detail_state.file_content_states.get_mut(&request.path) else {
                return;
            };
            if file_state.request_key.as_deref() != Some(&request.request_key) {
                return;
            }

            file_state.loading = false;
            match prepared_result {
                Ok((document, prepared)) => {
                    file_state.document = Some(document);
                    file_state.prepared = Some(prepared);
                    file_state.error = None;
                }
                Err(error) => {
                    file_state.document = None;
                    file_state.prepared = None;
                    file_state.error = Some(error);
                }
            }

            cx.notify();
        })
        .ok();
}

#[derive(Clone)]
struct FileContentRequest {
    path: String,
    reference: String,
    request_key: String,
}

fn build_file_content_request(
    detail: &PullRequestDetail,
    file: &PullRequestFile,
    parsed: Option<&ParsedDiffFile>,
) -> Option<FileContentRequest> {
    let (path, reference) = if file.change_type == "DELETED" {
        (
            parsed
                .and_then(|parsed| parsed.previous_path.clone())
                .unwrap_or_else(|| file.path.clone()),
            detail
                .base_ref_oid
                .clone()
                .unwrap_or_else(|| detail.base_ref_name.clone()),
        )
    } else {
        (
            file.path.clone(),
            detail
                .head_ref_oid
                .clone()
                .unwrap_or_else(|| detail.head_ref_name.clone()),
        )
    };

    if path.is_empty() || reference.is_empty() {
        return None;
    }

    Some(FileContentRequest {
        request_key: format!(
            "{}:{reference}:{path}:{}",
            detail.updated_at, detail.repository
        ),
        path,
        reference,
    })
}

fn prepare_file_content(
    file_path: &str,
    reference: &str,
    document: &RepositoryFileContent,
) -> PreparedFileContent {
    let lines = document.content.as_deref().unwrap_or_default();
    let text_lines = if lines.is_empty() {
        Vec::new()
    } else {
        lines.lines().map(str::to_string).collect::<Vec<_>>()
    };
    let spans = if document.is_binary || document.size_bytes > MAX_FILE_HIGHLIGHT_BYTES {
        text_lines
            .iter()
            .map(|_| Vec::new())
            .collect::<Vec<Vec<SyntaxSpan>>>()
    } else {
        syntax::highlight_lines(file_path, text_lines.iter().map(|line| line.as_str()))
    };

    let prepared_lines = text_lines
        .into_iter()
        .zip(spans)
        .map(|(text, spans)| PreparedFileLine { text, spans })
        .collect::<Vec<_>>();

    PreparedFileContent {
        path: file_path.to_string(),
        reference: reference.to_string(),
        is_binary: document.is_binary,
        size_bytes: document.size_bytes,
        lines: Arc::new(prepared_lines),
    }
}

fn render_diff_panel(
    state: &Entity<AppState>,
    app_state: &AppState,
    detail: &PullRequestDetail,
    selected_path: Option<&str>,
    selected_anchor: Option<&DiffAnchor>,
    cx: &App,
) -> impl IntoElement {
    let files = &detail.files;
    let selected_file = selected_path
        .and_then(|p| files.iter().find(|f| f.path == p))
        .or(files.first());

    let selected_parsed =
        selected_file.and_then(|file| find_parsed_diff_file(&detail.parsed_diff, &file.path));
    let file_thread_count = selected_file
        .map(|file| {
            detail
                .review_threads
                .iter()
                .filter(|thread| thread.path == file.path)
                .count()
        })
        .unwrap_or(0);
    let diff_view_state =
        selected_file.map(|file| prepare_diff_view_state(app_state, detail, &file.path));
    let file_content_state = selected_file.and_then(|file| {
        app_state
            .active_detail_state()
            .and_then(|detail_state| detail_state.file_content_states.get(&file.path))
            .cloned()
    });

    div()
        .flex_grow()
        .min_h_0()
        .min_w_0()
        .flex()
        .flex_col()
        // Toolbar (fixed, stays above scroll)
        .child(render_diff_toolbar(
            files.len(),
            selected_file,
            selected_parsed,
            file_thread_count,
        ))
        .child(
            div()
                .flex_grow()
                .min_h_0()
                .bg(bg_canvas())
                .p(px(16.0))
                .pt(px(14.0))
                .flex()
                .flex_col()
                .child(
                    if let (Some(file), Some(diff_view_state)) = (selected_file, diff_view_state) {
                        render_file_diff(
                            state,
                            file,
                            selected_parsed,
                            file_content_state
                                .as_ref()
                                .and_then(|state| state.prepared.as_ref()),
                            selected_anchor,
                            diff_view_state,
                            cx,
                        )
                        .into_any_element()
                    } else {
                        panel_state_text("No files returned for this pull request.")
                            .into_any_element()
                    },
                ),
        )
}

fn render_diff_toolbar(
    total_files: usize,
    selected_file: Option<&PullRequestFile>,
    selected_parsed: Option<&ParsedDiffFile>,
    file_thread_count: usize,
) -> impl IntoElement {
    div()
        .flex()
        .items_start()
        .justify_between()
        .gap(px(16.0))
        .px(px(20.0))
        .py(px(12.0))
        .bg(bg_surface())
        .border_b(px(1.0))
        .border_color(border_default())
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .min_w_0()
                .child(
                    div()
                        .text_size(px(11.0))
                        .font_family("Fira Code")
                        .text_color(fg_subtle())
                        .child(format!("{total_files} changed files")),
                )
                .when_some(selected_file, |el, f| {
                    el.child(
                        div()
                            .text_size(px(14.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(fg_emphasis())
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .overflow_x_hidden()
                            .child(f.path.clone()),
                    )
                })
                .when_some(selected_parsed, |el, parsed| {
                    let rename_from = parsed
                        .previous_path
                        .as_deref()
                        .filter(|previous| *previous != parsed.path.as_str());

                    if let Some(rename_from) = rename_from {
                        el.child(
                            div()
                                .text_size(px(11.0))
                                .font_family("Fira Code")
                                .text_color(fg_muted())
                                .child(format!("renamed from {rename_from}")),
                        )
                    } else {
                        el
                    }
                }),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .flex_wrap()
                .flex_shrink_0()
                .child(badge("Unified"))
                .when(file_thread_count > 0, |el| {
                    el.child(badge(&format!("{file_thread_count} threads")))
                })
                .when_some(selected_file, |el, f| {
                    el.child(render_change_type_chip(&f.change_type))
                        .child(
                            div()
                                .text_size(px(11.0))
                                .font_family("Fira Code")
                                .text_color(success())
                                .child(format!("+{}", f.additions)),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .font_family("Fira Code")
                                .text_color(danger())
                                .child(format!("-{}", f.deletions)),
                        )
                })
                .when(
                    selected_parsed.map(|p| p.is_binary).unwrap_or(false),
                    |el| el.child(badge("binary")),
                ),
        )
}

fn render_file_diff(
    state: &Entity<AppState>,
    file: &PullRequestFile,
    parsed: Option<&ParsedDiffFile>,
    prepared_file: Option<&PreparedFileContent>,
    selected_anchor: Option<&DiffAnchor>,
    diff_view_state: DiffFileViewState,
    _cx: &App,
) -> impl IntoElement {
    let rows = diff_view_state.rows.clone();
    let parsed_file_index = diff_view_state.parsed_file_index;
    let highlighted_hunks = diff_view_state.highlighted_hunks.clone();
    let selected_anchor = selected_anchor.cloned();
    let list_state = diff_view_state.list_state.clone();

    let items = build_diff_view_items(file, parsed, prepared_file, &rows);

    if list_state.item_count() != items.len() {
        list_state.reset(items.len());
    }

    let items = Arc::new(items);
    let state = state.clone();

    div()
        .flex()
        .flex_col()
        .flex_grow()
        .min_h_0()
        .rounded(radius())
        .border_1()
        .border_color(border_default())
        .bg(bg_surface())
        .overflow_hidden()
        .shadow_sm()
        .child(
            div()
                .flex()
                .flex_col()
                .flex_grow()
                .min_h_0()
                .bg(bg_inset())
                .child(
                    list(list_state, move |ix, _window, cx| {
                        match items[ix] {
                            DiffViewItem::Gap(gap) => {
                                render_diff_gap_row(gap).into_any_element()
                            }
                            DiffViewItem::Row(row_ix) => {
                                render_virtualized_diff_row(
                                    &state,
                                    parsed_file_index,
                                    highlighted_hunks.as_deref(),
                                    &rows[row_ix],
                                    selected_anchor.as_ref(),
                                    cx,
                                )
                                .into_any_element()
                            }
                        }
                    })
                    .flex_grow()
                    .min_h_0(),
                ),
        )
}

#[derive(Clone, Copy)]
enum DiffViewItem {
    Row(usize),
    Gap(DiffGapSummary),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DiffGapPosition {
    Start,
    Between,
    End,
}

#[derive(Clone, Copy)]
struct DiffGapSummary {
    position: DiffGapPosition,
    hidden_count: usize,
    start_line: Option<usize>,
    end_line: Option<usize>,
}

fn build_diff_view_items(
    file: &PullRequestFile,
    parsed: Option<&ParsedDiffFile>,
    prepared_file: Option<&PreparedFileContent>,
    rows: &[DiffRenderRow],
) -> Vec<DiffViewItem> {
    let mut items = Vec::with_capacity(rows.len() + 4);
    let mut last_hunk_index = None;
    let last_hunk_row_index = rows.iter().rposition(|row| {
        matches!(
            row,
            DiffRenderRow::HunkHeader { .. } | DiffRenderRow::Line { .. }
        )
    });

    for (row_index, row) in rows.iter().enumerate() {
        if let DiffRenderRow::HunkHeader { hunk_index } = row {
            if let Some(gap) =
                diff_gap_before_hunk(file, parsed, prepared_file, last_hunk_index, *hunk_index)
            {
                items.push(DiffViewItem::Gap(gap));
            }
            last_hunk_index = Some(*hunk_index);
        }

        items.push(DiffViewItem::Row(row_index));

        if Some(row_index) == last_hunk_row_index {
            if let Some(last_hunk_index) = last_hunk_index {
                if let Some(gap) =
                    diff_gap_after_last_hunk(file, parsed, prepared_file, last_hunk_index)
                {
                    items.push(DiffViewItem::Gap(gap));
                }
            }
        }
    }

    items
}

fn diff_gap_before_hunk(
    file: &PullRequestFile,
    parsed: Option<&ParsedDiffFile>,
    prepared_file: Option<&PreparedFileContent>,
    previous_hunk_index: Option<usize>,
    current_hunk_index: usize,
) -> Option<DiffGapSummary> {
    let parsed = parsed?;
    let current_hunk = parsed.hunks.get(current_hunk_index)?;
    let current_first = first_visible_line_number(file, current_hunk)?;

    match previous_hunk_index {
        Some(previous_hunk_index) => {
            let previous_hunk = parsed.hunks.get(previous_hunk_index)?;
            let previous_last = last_visible_line_number(file, previous_hunk)?;
            if current_first <= previous_last.saturating_add(1) {
                return None;
            }

            let start_line = previous_last.saturating_add(1);
            let end_line = current_first.saturating_sub(1);
            let hidden_count = end_line.saturating_sub(start_line).saturating_add(1);

            Some(DiffGapSummary {
                position: DiffGapPosition::Between,
                hidden_count,
                start_line: Some(start_line),
                end_line: Some(end_line),
            })
        }
        None => {
            if current_first <= 1 {
                return None;
            }

            let total_lines = prepared_file
                .map(|prepared| prepared.lines.len())
                .unwrap_or(0);
            let end_line = current_first.saturating_sub(1);
            let hidden_count = if total_lines > 0 {
                end_line.min(total_lines)
            } else {
                end_line
            };

            Some(DiffGapSummary {
                position: DiffGapPosition::Start,
                hidden_count,
                start_line: Some(1),
                end_line: Some(end_line),
            })
        }
    }
}

fn diff_gap_after_last_hunk(
    file: &PullRequestFile,
    parsed: Option<&ParsedDiffFile>,
    prepared_file: Option<&PreparedFileContent>,
    last_hunk_index: usize,
) -> Option<DiffGapSummary> {
    let prepared_file = prepared_file?;
    let parsed = parsed?;
    let last_hunk = parsed.hunks.get(last_hunk_index)?;
    let last_visible = last_visible_line_number(file, last_hunk)?;
    let total_lines = prepared_file.lines.len();

    if total_lines <= last_visible {
        return None;
    }

    Some(DiffGapSummary {
        position: DiffGapPosition::End,
        hidden_count: total_lines.saturating_sub(last_visible),
        start_line: Some(last_visible.saturating_add(1)),
        end_line: Some(total_lines),
    })
}

fn first_visible_line_number(file: &PullRequestFile, hunk: &ParsedDiffHunk) -> Option<usize> {
    hunk.lines
        .iter()
        .find_map(|line| primary_diff_line_number(file, line))
}

fn last_visible_line_number(file: &PullRequestFile, hunk: &ParsedDiffHunk) -> Option<usize> {
    hunk.lines
        .iter()
        .rev()
        .find_map(|line| primary_diff_line_number(file, line))
}

fn primary_diff_line_number(file: &PullRequestFile, line: &ParsedDiffLine) -> Option<usize> {
    let number = if file.change_type == "DELETED" {
        line.left_line_number.or(line.right_line_number)
    } else {
        line.right_line_number.or(line.left_line_number)
    }?;

    if number > 0 {
        Some(number as usize)
    } else {
        None
    }
}

fn render_diff_gap_row(summary: DiffGapSummary) -> impl IntoElement {
    let markers = match summary.position {
        DiffGapPosition::Start => vec!["...", "\u{2193}"],
        DiffGapPosition::Between => vec!["\u{2191}", "...", "\u{2193}"],
        DiffGapPosition::End => vec!["\u{2191}", "..."],
    };

    div()
        .flex()
        .items_center()
        .w_full()
        .min_h(px(30.0))
        .bg(bg_surface())
        .border_b(px(1.0))
        .border_color(border_muted())
        .font_family("Fira Code")
        .text_size(px(11.0))
        .child(
            div()
                .w(px(96.0))
                .flex_shrink_0()
                .h_full()
                .bg(diff_context_gutter_bg())
                .border_r(px(1.0))
                .border_color(border_default()),
        )
        .child(
            div()
                .flex_grow()
                .min_w_0()
                .px(px(12.0))
                .py(px(6.0))
                .flex()
                .items_center()
                .gap(px(10.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .children(markers.into_iter().map(|marker| {
                            div()
                                .px(px(6.0))
                                .py(px(1.0))
                                .rounded(px(999.0))
                                .bg(bg_overlay())
                                .border_1()
                                .border_color(border_default())
                                .text_color(accent())
                                .child(marker)
                        })),
                )
                .child(
                    div()
                        .text_color(fg_muted())
                        .child(render_diff_gap_label(summary)),
                ),
        )
}

fn render_diff_gap_label(summary: DiffGapSummary) -> String {
    let line_label = if summary.hidden_count == 1 {
        "1 unchanged line".to_string()
    } else {
        format!("{} unchanged lines", summary.hidden_count)
    };

    match (summary.start_line, summary.end_line) {
        (Some(start), Some(end)) if start == end => {
            format!("{line_label} hidden at line {start}")
        }
        (Some(start), Some(end)) => format!("{line_label} hidden ({start}-{end})"),
        _ => format!("{line_label} hidden"),
    }
}

fn prepare_diff_view_state(
    app_state: &AppState,
    detail: &PullRequestDetail,
    file_path: &str,
) -> DiffFileViewState {
    let state_key = format!(
        "{}:{file_path}",
        app_state.active_pr_key.as_deref().unwrap_or("detached")
    );
    let revision = detail.updated_at.clone();

    let mut diff_view_states = app_state.diff_view_states.borrow_mut();
    let entry = diff_view_states.entry(state_key).or_insert_with(|| {
        let (parsed_file_index, highlighted_hunks) =
            find_parsed_diff_file_with_index(&detail.parsed_diff, file_path)
                .map(|(ix, file)| (Some(ix), Some(build_diff_highlights(file))))
                .unwrap_or((None, None));
        DiffFileViewState::new(
            Arc::new(build_diff_render_rows(detail, file_path)),
            revision.clone(),
            parsed_file_index,
            highlighted_hunks,
        )
    });

    if entry.revision != revision {
        let (parsed_file_index, highlighted_hunks) =
            find_parsed_diff_file_with_index(&detail.parsed_diff, file_path)
                .map(|(ix, file)| (Some(ix), Some(build_diff_highlights(file))))
                .unwrap_or((None, None));
        let rows = Arc::new(build_diff_render_rows(detail, file_path));
        entry.rows = rows;
        entry.revision = revision;
        entry.parsed_file_index = parsed_file_index;
        entry.highlighted_hunks = highlighted_hunks;
        entry.list_state.reset(0);
    }

    entry.clone()
}

fn render_virtualized_diff_row(
    state: &Entity<AppState>,
    parsed_file_index: Option<usize>,
    highlighted_hunks: Option<&Vec<Vec<Vec<SyntaxSpan>>>>,
    row: &DiffRenderRow,
    selected_anchor: Option<&DiffAnchor>,
    cx: &App,
) -> impl IntoElement {
    let s = state.read(cx);
    let detail = s.active_detail();
    let parsed_file =
        parsed_file_index.and_then(|ix| detail.and_then(|detail| detail.parsed_diff.get(ix)));

    match row {
        DiffRenderRow::FileCommentsHeader { count } => {
            render_diff_section_header("File comments", *count).into_any_element()
        }
        DiffRenderRow::OutdatedCommentsHeader { count } => {
            render_diff_section_header("Outdated comments", *count).into_any_element()
        }
        DiffRenderRow::FileCommentThread { thread_index } => detail
            .and_then(|detail| detail.review_threads.get(*thread_index))
            .map(|thread| {
                div()
                    .px(px(16.0))
                    .py(px(10.0))
                    .border_b(px(1.0))
                    .border_color(border_muted())
                    .child(render_review_thread(thread, selected_anchor))
                    .into_any_element()
            })
            .unwrap_or_else(|| div().into_any_element()),
        DiffRenderRow::InlineThread { thread_index } => detail
            .and_then(|detail| detail.review_threads.get(*thread_index))
            .map(|thread| {
                div()
                    .pl(px(124.0))
                    .pr(px(16.0))
                    .py(px(10.0))
                    .border_b(px(1.0))
                    .border_color(border_muted())
                    .bg(bg_inset())
                    .child(render_review_thread(thread, selected_anchor))
                    .into_any_element()
            })
            .unwrap_or_else(|| div().into_any_element()),
        DiffRenderRow::OutdatedThread { thread_index } => detail
            .and_then(|detail| detail.review_threads.get(*thread_index))
            .map(|thread| {
                div()
                    .px(px(16.0))
                    .py(px(10.0))
                    .border_b(px(1.0))
                    .border_color(border_muted())
                    .bg(bg_inset())
                    .child(render_review_thread(thread, selected_anchor))
                    .into_any_element()
            })
            .unwrap_or_else(|| div().into_any_element()),
        DiffRenderRow::HunkHeader { hunk_index } => parsed_file
            .and_then(|parsed| parsed.hunks.get(*hunk_index))
            .map(|hunk| render_hunk_header(hunk, selected_anchor).into_any_element())
            .unwrap_or_else(|| div().into_any_element()),
        DiffRenderRow::Line {
            hunk_index,
            line_index,
        } => parsed_file
            .and_then(|parsed| {
                let path = parsed.path.as_str();
                parsed
                    .hunks
                    .get(*hunk_index)
                    .and_then(|hunk| hunk.lines.get(*line_index))
                    .map(|line| {
                        let spans = highlighted_hunks
                            .and_then(|hunks| hunks.get(*hunk_index))
                            .and_then(|lines| lines.get(*line_index))
                            .map(|spans| spans.as_slice());
                        render_diff_line(path, line, spans, selected_anchor).into_any_element()
                    })
            })
            .unwrap_or_else(|| div().into_any_element()),
        DiffRenderRow::NoTextHunks => render_diff_state_row(
            if parsed_file.map(|parsed| parsed.is_binary).unwrap_or(false) {
                "Binary file not displayed in the unified diff."
            } else {
                "No textual hunks available for this file."
            },
        )
        .into_any_element(),
        DiffRenderRow::RawDiffFallback => {
            render_raw_diff_fallback(detail.map(|detail| detail.raw_diff.as_str()).unwrap_or(""))
                .into_any_element()
        }
        DiffRenderRow::NoParsedDiff => {
            render_diff_state_row("No parsed diff is available for this file.").into_any_element()
        }
    }
}

fn render_diff_section_header(label: &str, count: usize) -> impl IntoElement {
    div()
        .px(px(16.0))
        .py(px(8.0))
        .border_b(px(1.0))
        .border_color(border_default())
        .bg(bg_overlay())
        .flex()
        .items_center()
        .justify_between()
        .child(
            div()
                .text_size(px(11.0))
                .font_family("Fira Code")
                .text_color(fg_muted())
                .child(label.to_uppercase()),
        )
        .child(
            div()
                .text_size(px(11.0))
                .font_family("Fira Code")
                .text_color(fg_subtle())
                .child(count.to_string()),
        )
}

fn render_diff_state_row(message: &str) -> impl IntoElement {
    div()
        .px(px(16.0))
        .py(px(18.0))
        .border_b(px(1.0))
        .border_color(border_muted())
        .bg(bg_inset())
        .child(
            div()
                .text_size(px(12.0))
                .text_color(fg_muted())
                .child(message.to_string()),
        )
}

fn render_raw_diff_fallback(raw_diff: &str) -> impl IntoElement {
    div()
        .px(px(16.0))
        .py(px(16.0))
        .border_b(px(1.0))
        .border_color(border_muted())
        .bg(bg_inset())
        .child(
            div()
                .font_family("Fira Code")
                .text_size(px(12.0))
                .text_color(fg_muted())
                .child(if raw_diff.is_empty() {
                    "No diff returned.".to_string()
                } else {
                    raw_diff.to_string()
                }),
        )
}

fn render_change_type_chip(change_type: &str) -> impl IntoElement {
    let (bg, fg, border) = match change_type {
        "ADDED" => (success_muted(), success(), diff_add_border()),
        "DELETED" => (danger_muted(), danger(), diff_remove_border()),
        "RENAMED" | "COPIED" => (accent_muted(), accent(), accent()),
        _ => (bg_subtle(), fg_muted(), border_muted()),
    };

    div()
        .px(px(7.0))
        .py(px(2.0))
        .rounded(px(999.0))
        .border_1()
        .border_color(border)
        .bg(bg)
        .text_size(px(10.0))
        .font_family("Fira Code")
        .text_color(fg)
        .child(label_for_change_type(change_type).to_string())
}

fn render_file_stat_bar(additions: i64, deletions: i64) -> impl IntoElement {
    let total = additions + deletions;
    let segments = 8usize;
    let additions = additions.max(0) as usize;
    let add_segments = if total > 0 {
        ((additions as f32 / total as f32) * segments as f32)
            .round()
            .clamp(0.0, segments as f32) as usize
    } else {
        0
    };
    let delete_segments = if total > 0 {
        segments.saturating_sub(add_segments)
    } else {
        0
    };

    div()
        .flex()
        .gap(px(2.0))
        .children((0..segments).map(move |ix| {
            let bg = if ix < add_segments {
                success()
            } else if ix < add_segments + delete_segments {
                danger()
            } else {
                border_muted()
            };

            div().w(px(8.0)).h(px(4.0)).rounded(px(999.0)).bg(bg)
        }))
}

fn render_hunk(
    file_path: &str,
    hunk: &ParsedDiffHunk,
    line_threads: &[&PullRequestReviewThread],
    selected_anchor: Option<&DiffAnchor>,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .child(render_hunk_header(hunk, selected_anchor))
        .child(
            div()
                .flex()
                .flex_col()
                .children(hunk.lines.iter().map(|line| {
                    let threads_for_line = find_threads_for_line(file_path, line, line_threads);
                    render_diff_line_with_threads(
                        file_path,
                        line,
                        &threads_for_line,
                        selected_anchor,
                    )
                })),
        )
}

fn render_diff_line_with_threads(
    file_path: &str,
    line: &ParsedDiffLine,
    threads: &[&PullRequestReviewThread],
    selected_anchor: Option<&DiffAnchor>,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .child(render_diff_line(file_path, line, None, selected_anchor))
        .when(!threads.is_empty(), |el| {
            el.child(
                div()
                    .pl(px(124.0))
                    .pr(px(16.0))
                    .py(px(8.0))
                    .border_b(px(1.0))
                    .border_color(border_muted())
                    .bg(bg_inset())
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .children(
                        threads
                            .iter()
                            .map(|thread| render_review_thread(thread, selected_anchor)),
                    ),
            )
        })
}

fn render_diff_line(
    file_path: &str,
    line: &ParsedDiffLine,
    syntax_spans: Option<&[SyntaxSpan]>,
    selected_anchor: Option<&DiffAnchor>,
) -> impl IntoElement {
    let is_selected = line_matches_diff_anchor(line, selected_anchor);

    let left_num = line
        .left_line_number
        .map(|n| n.to_string())
        .unwrap_or_default();
    let right_num = line
        .right_line_number
        .map(|n| n.to_string())
        .unwrap_or_default();

    let marker = if line.prefix.is_empty() {
        " ".to_string()
    } else {
        line.prefix.clone()
    };

    // Subtle backgrounds with syntax-highlighted text — only gutter markers stay green/red
    let (row_bg, gutter_bg, row_border, marker_color, fallback_text_color) = if is_selected {
        (
            accent_muted(),
            bg_selected(),
            accent(),
            fg_emphasis(),
            fg_emphasis(),
        )
    } else {
        match line.kind {
            DiffLineKind::Addition => (
                diff_add_bg(),
                diff_add_gutter_bg(),
                diff_add_border(),
                success(),
                fg_default(),
            ),
            DiffLineKind::Deletion => (
                diff_remove_bg(),
                diff_remove_gutter_bg(),
                diff_remove_border(),
                danger(),
                fg_default(),
            ),
            DiffLineKind::Meta => (
                diff_meta_bg(),
                diff_context_gutter_bg(),
                border_muted(),
                fg_subtle(),
                fg_muted(),
            ),
            DiffLineKind::Context => (
                diff_context_bg(),
                diff_context_gutter_bg(),
                border_muted(),
                fg_subtle(),
                fg_default(),
            ),
        }
    };
    let number_color = if is_selected {
        fg_default()
    } else {
        fg_subtle()
    };

    div()
        .flex()
        .items_start()
        .w_full()
        .min_h(px(22.0))
        .bg(row_bg)
        .border_b(px(1.0))
        .border_color(row_border)
        .font_family("Fira Code")
        .text_size(px(12.0))
        .when(is_selected, |el| {
            el.border_l(px(2.0)).border_color(accent())
        })
        .child(
            div()
                .flex()
                .flex_shrink_0()
                .w(px(96.0))
                .bg(gutter_bg)
                .border_r(px(1.0))
                .border_color(border_default())
                .child(
                    div()
                        .w(px(48.0))
                        .px(px(8.0))
                        .flex()
                        .justify_end()
                        .text_size(px(11.0))
                        .text_color(number_color)
                        .child(left_num),
                )
                .child(
                    div()
                        .w(px(48.0))
                        .px(px(8.0))
                        .flex()
                        .justify_end()
                        .text_size(px(11.0))
                        .text_color(number_color)
                        .child(right_num),
                ),
        )
        .child(
            div()
                .w(px(20.0))
                .flex_shrink_0()
                .py(px(1.0))
                .text_color(marker_color)
                .child(marker),
        )
        .child(render_syntax_content(
            file_path,
            &line.content,
            syntax_spans,
            fallback_text_color,
        ))
}

fn render_syntax_content(
    file_path: &str,
    content: &str,
    syntax_spans: Option<&[SyntaxSpan]>,
    fallback_color: Rgba,
) -> Div {
    let content_div = div()
        .flex_grow()
        .px(px(8.0))
        .py(px(1.0))
        .whitespace_nowrap()
        .text_size(px(12.0))
        .font_family("Fira Code");

    if content.is_empty() {
        return content_div
            .text_color(fallback_color)
            .child("\u{00a0}".to_string());
    }

    let owned_spans;
    let spans = if let Some(spans) = syntax_spans {
        spans
    } else {
        owned_spans = syntax::highlight_line(file_path, content);
        owned_spans.as_slice()
    };

    if spans.is_empty() {
        return content_div
            .text_color(fallback_color)
            .child(content.to_string());
    }

    let mut text = String::new();
    let mut runs = Vec::with_capacity(spans.len());

    for span in spans {
        text.push_str(&span.text);
        runs.push(TextRun {
            len: span.text.len(),
            font: font("Fira Code"),
            color: span.color,
            background_color: None,
            underline: None,
            strikethrough: None,
        });
    }

    content_div
        .text_color(fallback_color)
        .child(StyledText::new(text).with_runs(runs))
}

fn build_diff_highlights(parsed_file: &ParsedDiffFile) -> Arc<Vec<Vec<Vec<SyntaxSpan>>>> {
    Arc::new(
        parsed_file
            .hunks
            .iter()
            .map(|hunk| {
                syntax::highlight_lines(
                    parsed_file.path.as_str(),
                    hunk.lines.iter().map(|line| line.content.as_str()),
                )
            })
            .collect(),
    )
}

fn render_review_thread(
    thread: &PullRequestReviewThread,
    selected_anchor: Option<&DiffAnchor>,
) -> impl IntoElement {
    let is_selected = thread_matches_diff_anchor(thread, selected_anchor);
    let thread_border = if is_selected {
        accent()
    } else if thread.is_resolved {
        success()
    } else if thread.is_outdated {
        border_muted()
    } else {
        border_default()
    };
    let header_bg = if is_selected {
        accent_muted()
    } else if thread.is_resolved {
        success_muted()
    } else {
        bg_emphasis()
    };

    div()
        .rounded(radius())
        .border_1()
        .border_color(thread_border)
        .bg(bg_overlay())
        .overflow_hidden()
        .shadow_sm()
        .flex()
        .flex_col()
        .child(
            div()
                .px(px(12.0))
                .py(px(8.0))
                .border_b(px(1.0))
                .border_color(border_muted())
                .bg(header_bg)
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(badge(&thread.subject_type.to_lowercase()))
                        .when(thread.is_resolved, |el| el.child(badge_success("resolved")))
                        .when(thread.is_outdated, |el| el.child(badge("outdated"))),
                ),
        )
        .child(
            div().p(px(12.0)).flex().flex_col().gap(px(8.0)).children(
                thread
                    .comments
                    .iter()
                    .map(|comment| render_thread_comment(comment)),
            ),
        )
}

fn render_thread_comment(comment: &PullRequestReviewComment) -> impl IntoElement {
    div()
        .p(px(12.0))
        .rounded(radius_sm())
        .border_1()
        .border_color(border_muted())
        .bg(bg_surface())
        .flex()
        .flex_col()
        .gap(px(6.0))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .text_size(px(12.0))
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(fg_emphasis())
                        .child(comment.author_login.clone()),
                )
                .child(
                    div().text_color(fg_subtle()).child(
                        comment
                            .published_at
                            .as_deref()
                            .unwrap_or(&comment.created_at)
                            .to_string(),
                    ),
                ),
        )
        .child(if comment.body.is_empty() {
            div()
                .text_size(px(13.0))
                .text_color(fg_muted())
                .child("No comment body.")
                .into_any_element()
        } else {
            render_markdown(&comment.body).into_any_element()
        })
}

fn render_hunk_header(
    hunk: &ParsedDiffHunk,
    selected_anchor: Option<&DiffAnchor>,
) -> impl IntoElement {
    let hunk_is_selected = selected_anchor
        .and_then(|anchor| anchor.hunk_header.as_deref())
        .map(|header| header == hunk.header)
        .unwrap_or(false)
        && selected_anchor.and_then(|anchor| anchor.line).is_none();

    div()
        .px(px(16.0))
        .py(px(7.0))
        .border_b(px(1.0))
        .border_color(if hunk_is_selected {
            accent()
        } else {
            border_default()
        })
        .bg(if hunk_is_selected {
            accent_muted()
        } else {
            diff_hunk_bg()
        })
        .text_size(px(11.0))
        .font_family("Fira Code")
        .text_color(if hunk_is_selected {
            fg_emphasis()
        } else {
            diff_hunk_fg()
        })
        .child(hunk.header.clone())
}

// Helpers

pub fn render_tour_diff_file(
    detail: &PullRequestDetail,
    file_path: Option<&str>,
    snippet: Option<&str>,
    anchor: Option<&DiffAnchor>,
) -> impl IntoElement {
    let Some(file_path) = file_path else {
        return div().into_any_element();
    };

    let file = detail
        .files
        .iter()
        .find(|candidate| candidate.path == file_path);
    let parsed_file = find_parsed_diff_file(&detail.parsed_diff, file_path);

    if let Some(parsed_file) = parsed_file {
        return nested_panel()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .mb(px(12.0))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(px(10.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(fg_subtle())
                                    .font_family("Fira Code")
                                    .child("DIFF PREVIEW"),
                            )
                            .child(
                                div()
                                    .text_size(px(14.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(fg_emphasis())
                                    .child(
                                        if parsed_file.previous_path.is_some()
                                            && parsed_file.previous_path.as_deref()
                                                != Some(&parsed_file.path)
                                        {
                                            format!(
                                                "{} -> {}",
                                                parsed_file.previous_path.as_deref().unwrap_or(""),
                                                parsed_file.path
                                            )
                                        } else {
                                            parsed_file.path.clone()
                                        },
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(6.0))
                            .when_some(file, |el, file| {
                                el.child(render_change_type_chip(&file.change_type))
                                    .child(badge(&format!(
                                        "+{} / -{}",
                                        file.additions, file.deletions
                                    )))
                            })
                            .when(parsed_file.is_binary, |el| el.child(badge("binary"))),
                    ),
            )
            .child(if parsed_file.hunks.is_empty() {
                panel_state_text("No textual hunks available for this file.").into_any_element()
            } else {
                render_tour_diff_preview(parsed_file, anchor).into_any_element()
            })
            .into_any_element();
    }

    if let Some(snippet) = snippet {
        return nested_panel()
            .child(
                div()
                    .text_size(px(10.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(fg_subtle())
                    .font_family("Fira Code")
                    .mb(px(8.0))
                    .child("DIFF PREVIEW"),
            )
            .child(
                div()
                    .font_family("Fira Code")
                    .text_size(px(12.0))
                    .text_color(fg_default())
                    .child(snippet.to_string()),
            )
            .into_any_element();
    }

    panel_state_text("No parsed diff is available for this file.").into_any_element()
}

fn render_tour_diff_preview(
    parsed_file: &ParsedDiffFile,
    anchor: Option<&DiffAnchor>,
) -> impl IntoElement {
    const MAX_PREVIEW_LINES: usize = 40;

    let total_lines: usize = parsed_file.hunks.iter().map(|h| h.lines.len()).sum();
    let highlighted_hunks = build_diff_highlights(parsed_file);

    // If anchor specifies a hunk, start there; otherwise start from the beginning
    let start_hunk = anchor
        .and_then(|a| a.hunk_header.as_ref())
        .and_then(|header| parsed_file.hunks.iter().position(|h| h.header == *header))
        .unwrap_or(0);

    let mut rendered_lines = 0usize;
    let mut elements: Vec<AnyElement> = Vec::new();
    let file_path = parsed_file.path.as_str();

    for hunk_idx in start_hunk..parsed_file.hunks.len() {
        if rendered_lines >= MAX_PREVIEW_LINES {
            break;
        }

        let hunk = &parsed_file.hunks[hunk_idx];
        elements.push(render_hunk_header(hunk, anchor).into_any_element());

        let lines_remaining = MAX_PREVIEW_LINES.saturating_sub(rendered_lines);
        let lines_to_show = lines_remaining.min(hunk.lines.len());

        for (line_idx, line) in hunk.lines[..lines_to_show].iter().enumerate() {
            let spans = highlighted_hunks
                .get(hunk_idx)
                .and_then(|lines| lines.get(line_idx))
                .map(|spans| spans.as_slice());
            elements.push(render_diff_line(file_path, line, spans, anchor).into_any_element());
        }
        rendered_lines += lines_to_show;
    }

    let hidden_lines = total_lines.saturating_sub(rendered_lines);

    div()
        .flex()
        .flex_col()
        .children(elements)
        .when(hidden_lines > 0, |el| {
            el.child(
                div()
                    .px(px(16.0))
                    .py(px(8.0))
                    .bg(bg_subtle())
                    .text_size(px(11.0))
                    .font_family("Fira Code")
                    .text_color(fg_muted())
                    .child(format!("{hidden_lines} more lines not shown")),
            )
        })
}

fn find_threads_for_line<'a>(
    file_path: &str,
    line: &ParsedDiffLine,
    threads: &'a [&PullRequestReviewThread],
) -> Vec<&'a PullRequestReviewThread> {
    threads
        .iter()
        .copied()
        .filter(|t| {
            if t.path != file_path {
                return false;
            }
            match line.kind {
                DiffLineKind::Addition | DiffLineKind::Context => {
                    let line_no = line.right_line_number;
                    if t.diff_side == "RIGHT" {
                        t.line == line_no || t.original_line == line_no
                    } else {
                        false
                    }
                }
                DiffLineKind::Deletion => {
                    let line_no = line.left_line_number;
                    if t.diff_side == "LEFT" {
                        t.line == line_no || t.original_line == line_no
                    } else {
                        false
                    }
                }
                DiffLineKind::Meta => false,
            }
        })
        .collect()
}

fn label_for_change_type(change_type: &str) -> &str {
    match change_type {
        "ADDED" => "added",
        "DELETED" => "deleted",
        "RENAMED" => "renamed",
        "COPIED" => "copied",
        _ => "modified",
    }
}
