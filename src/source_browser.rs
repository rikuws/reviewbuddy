use std::{collections::BTreeMap, sync::Arc};

use gpui::prelude::*;
use gpui::*;

use crate::{
    code_display::{
        build_prepared_file_lsp_context, render_prepared_file_with_line_numbers,
        render_prepared_file_with_line_numbers_and_diffs, PreparedFileLineDiffKind,
        PreparedFileLineDiffs,
    },
    diff::{DiffLineKind, ParsedDiffFile},
    review_session::ReviewSourceTarget,
    state::AppState,
    theme::*,
};

pub fn render_source_browser(
    state: &Entity<AppState>,
    target: &ReviewSourceTarget,
    parsed: Option<&ParsedDiffFile>,
    cx: &App,
) -> AnyElement {
    let prepared_file = {
        let app_state = state.read(cx);
        app_state
            .active_detail_state()
            .and_then(|detail_state| detail_state.file_content_states.get(&target.path))
            .and_then(|file_state| file_state.prepared.as_ref())
            .cloned()
    };

    let focus_line = target.line.filter(|line| *line > 0);

    let shell = div()
        .flex()
        .flex_col()
        .flex_grow()
        .min_h_0()
        .rounded(radius())
        .border_1()
        .border_color(border_default())
        .bg(bg_surface())
        .overflow_hidden()
        .child(
            div()
                .px(px(18.0))
                .py(px(12.0))
                .border_b(px(1.0))
                .border_color(border_default())
                .flex()
                .items_start()
                .justify_between()
                .gap(px(16.0))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .min_w_0()
                        .child(
                            div()
                                .text_size(px(10.0))
                                .font_family("Fira Code")
                                .text_color(fg_subtle())
                                .child("FULL FILE"),
                        )
                        .child(
                            div()
                                .text_size(px(14.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(fg_emphasis())
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .overflow_x_hidden()
                                .child(source_location_label(&target.path, focus_line)),
                        )
                        .when_some(target.reason.clone(), |el, reason| {
                            el.child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(fg_muted())
                                    .child(reason),
                            )
                        }),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(6.0))
                        .flex_wrap()
                        .flex_shrink_0()
                        .child(source_badge("read-only")),
                ),
        );

    let Some(prepared_file) = prepared_file else {
        return shell
            .child(
                div()
                    .flex_grow()
                    .min_h_0()
                    .p(px(18.0))
                    .child(source_state_text(
                        "Loading source context from the local checkout...",
                    )),
            )
            .into_any_element();
    };

    let lsp_context =
        build_prepared_file_lsp_context(state, target.path.as_str(), Some(&prepared_file), cx);
    let full_file = if let Some(parsed) = parsed {
        render_prepared_file_with_line_numbers_and_diffs(
            &prepared_file,
            lsp_context.as_ref(),
            build_full_file_diff_lines(parsed),
        )
    } else {
        render_prepared_file_with_line_numbers(&prepared_file, lsp_context.as_ref())
    };

    shell
        .child(
            div()
                .flex_grow()
                .min_h_0()
                .id("source-browser-scroll")
                .overflow_y_scroll()
                .p(px(18.0))
                .flex()
                .flex_col()
                .gap(px(16.0))
                .child(source_panel("Full file").child(full_file)),
        )
        .into_any_element()
}

fn build_full_file_diff_lines(parsed: &ParsedDiffFile) -> PreparedFileLineDiffs {
    let mut lines = BTreeMap::new();

    for line in parsed.hunks.iter().flat_map(|hunk| hunk.lines.iter()) {
        if line.kind != DiffLineKind::Addition {
            continue;
        }

        if let Some(line_number) = line
            .right_line_number
            .and_then(|line_number| usize::try_from(line_number).ok())
            .filter(|line_number| *line_number > 0)
        {
            lines.insert(line_number, PreparedFileLineDiffKind::Addition);
        }
    }

    Arc::new(lines)
}

fn source_location_label(path: &str, line: Option<usize>) -> String {
    match line {
        Some(line) => format!("{path}:{line}"),
        None => path.to_string(),
    }
}

fn source_panel(title: &str) -> Div {
    div().flex().flex_col().gap(px(10.0)).child(
        div()
            .text_size(px(11.0))
            .font_family("Fira Code")
            .text_color(fg_subtle())
            .child(title.to_ascii_uppercase()),
    )
}

fn source_state_text(message: &str) -> impl IntoElement {
    div()
        .text_size(px(12.0))
        .text_color(fg_muted())
        .child(message.to_string())
}

fn source_badge(label: &str) -> impl IntoElement {
    div()
        .px(px(7.0))
        .py(px(2.0))
        .rounded(px(999.0))
        .bg(bg_emphasis())
        .text_size(px(10.0))
        .font_family("Fira Code")
        .text_color(fg_muted())
        .child(label.to_string())
}
