use std::sync::Arc;

use gpui::prelude::*;
use gpui::*;

use crate::code_tour::{line_matches_diff_anchor, thread_matches_diff_anchor, DiffAnchor};
use crate::diff::{
    build_diff_render_rows, find_parsed_diff_file, find_parsed_diff_file_with_index, DiffLineKind,
    DiffRenderRow, ParsedDiffFile, ParsedDiffHunk, ParsedDiffLine,
};
use crate::github::{
    PullRequestDetail, PullRequestFile, PullRequestReviewComment, PullRequestReviewThread,
};
use crate::markdown::render_markdown;
use crate::state::*;
use crate::syntax;
use crate::theme::*;

use super::sections::{badge, badge_success, nested_panel, panel_state_text};

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
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    state.update(cx, |s, cx| {
                        s.selected_file_path = Some(path.clone());
                        s.selected_diff_anchor = None;
                        cx.notify();
                    });
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

fn render_diff_panel(
    state: &Entity<AppState>,
    app_state: &AppState,
    detail: &PullRequestDetail,
    selected_path: Option<&str>,
    selected_anchor: Option<&DiffAnchor>,
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

    div()
        .flex_grow()
        .min_w_0()
        .flex()
        .flex_col()
        // Toolbar
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
                .child(
                    if let (Some(file), Some(diff_view_state)) = (selected_file, diff_view_state) {
                        render_file_diff(
                            state,
                            detail,
                            file,
                            selected_parsed,
                            selected_anchor,
                            diff_view_state,
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
                })
                .when(
                    selected_parsed.map(|p| p.is_binary).unwrap_or(false),
                    |el| el.child(badge("binary")),
                ),
        )
}

fn render_file_diff(
    state: &Entity<AppState>,
    detail: &PullRequestDetail,
    file: &PullRequestFile,
    parsed: Option<&ParsedDiffFile>,
    selected_anchor: Option<&DiffAnchor>,
    diff_view_state: DiffFileViewState,
) -> impl IntoElement {
    let rename_from = parsed
        .and_then(|parsed| parsed.previous_path.as_deref())
        .filter(|previous| *previous != file.path.as_str());
    let hunk_count = parsed.map(|parsed| parsed.hunks.len()).unwrap_or(0);
    let comment_count = detail
        .review_threads
        .iter()
        .filter(|thread| thread.path == file.path)
        .count();
    let row_count = diff_view_state.rows.len();
    let row_model = diff_view_state.rows.clone();
    let list_state = diff_view_state.list_state.clone();
    let parsed_file_index = diff_view_state.parsed_file_index;
    let state_for_rows = state.clone();
    let selected_anchor = selected_anchor.cloned();

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
                .px(px(16.0))
                .py(px(12.0))
                .border_b(px(1.0))
                .border_color(border_default())
                .bg(bg_overlay())
                .flex()
                .items_start()
                .justify_between()
                .gap(px(12.0))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .min_w_0()
                        .child(
                            div()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(fg_emphasis())
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .overflow_x_hidden()
                                .child(file.path.clone()),
                        )
                        .when_some(rename_from, |el, previous| {
                            el.child(
                                div()
                                    .text_size(px(11.0))
                                    .font_family("Fira Code")
                                    .text_color(fg_muted())
                                    .child(format!("renamed from {previous}")),
                            )
                        })
                        .when(rename_from.is_none(), |el| {
                            el.child(
                                div()
                                    .text_size(px(11.0))
                                    .font_family("Fira Code")
                                    .text_color(fg_muted())
                                    .child(format!("{hunk_count} hunks \u{2022} {row_count} rows")),
                            )
                        }),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .flex_wrap()
                        .flex_shrink_0()
                        .child(render_change_type_chip(&file.change_type))
                        .when(comment_count > 0, |el| {
                            el.child(badge(&format!("{comment_count} threads")))
                        })
                        .when(
                            parsed.map(|parsed| parsed.is_binary).unwrap_or(false),
                            |el| el.child(badge("binary")),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .font_family("Fira Code")
                                .text_color(success())
                                .child(format!("+{}", file.additions)),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .font_family("Fira Code")
                                .text_color(danger())
                                .child(format!("-{}", file.deletions)),
                        ),
                ),
        )
        .child(
            div().flex_grow().min_h_0().bg(bg_inset()).child(
                list(list_state, move |ix, _, cx| {
                    row_model
                        .get(ix)
                        .map(|row| {
                            render_virtualized_diff_row(
                                &state_for_rows,
                                parsed_file_index,
                                row,
                                selected_anchor.as_ref(),
                                cx,
                            )
                            .into_any_element()
                        })
                        .unwrap_or_else(|| div().into_any_element())
                })
                .w_full()
                .h_full(),
            ),
        )
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
    let parsed_file_index =
        find_parsed_diff_file_with_index(&detail.parsed_diff, file_path).map(|(ix, _)| ix);

    let mut diff_view_states = app_state.diff_view_states.borrow_mut();
    let entry = diff_view_states.entry(state_key).or_insert_with(|| {
        DiffFileViewState::new(
            Arc::new(build_diff_render_rows(detail, file_path)),
            revision.clone(),
            parsed_file_index,
        )
    });

    if entry.revision != revision {
        let rows = Arc::new(build_diff_render_rows(detail, file_path));
        if entry.rows.len() != rows.len() {
            entry.list_state.reset(rows.len());
        }
        entry.rows = rows;
        entry.revision = revision.clone();
        entry.parsed_file_index = parsed_file_index;
    }

    entry.clone()
}

fn render_virtualized_diff_row(
    state: &Entity<AppState>,
    parsed_file_index: Option<usize>,
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
                    .map(|line| render_diff_line(path, line, selected_anchor).into_any_element())
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
        .child(render_diff_line(file_path, line, selected_anchor))
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
            fallback_text_color,
        ))
}

fn render_syntax_content(file_path: &str, content: &str, fallback_color: Rgba) -> Div {
    let content_div = div()
        .flex_grow()
        .min_w_0()
        .px(px(8.0))
        .py(px(1.0))
        .whitespace_nowrap()
        .overflow_x_hidden()
        .text_size(px(12.0))
        .font_family("Fira Code");

    if content.is_empty() {
        return content_div
            .text_color(fallback_color)
            .child("\u{00a0}".to_string());
    }

    let spans = syntax::highlight_line(file_path, content);

    if spans.is_empty() {
        return content_div
            .text_color(fallback_color)
            .child(content.to_string());
    }

    let mut text = String::new();
    let mut runs = Vec::with_capacity(spans.len());

    for span in &spans {
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

    // If anchor specifies a hunk, start there; otherwise start from the beginning
    let start_hunk = anchor
        .and_then(|a| a.hunk_header.as_ref())
        .and_then(|header| {
            parsed_file
                .hunks
                .iter()
                .position(|h| h.header == *header)
        })
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

        for line in &hunk.lines[..lines_to_show] {
            elements.push(render_diff_line(file_path, line, anchor).into_any_element());
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
