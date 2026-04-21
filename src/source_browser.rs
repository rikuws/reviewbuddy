use gpui::prelude::*;
use gpui::*;

use crate::{
    code_display::{
        build_prepared_file_lsp_context, prepared_file_has_line,
        render_prepared_file_excerpt_with_line_numbers, render_prepared_file_with_line_numbers,
    },
    review_session::ReviewSourceTarget,
    state::AppState,
    theme::*,
};

const SOURCE_FOCUS_CONTEXT_LINES: usize = 24;
const MAX_FULL_SOURCE_LINES: usize = 220;

pub fn render_source_browser(
    state: &Entity<AppState>,
    target: &ReviewSourceTarget,
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
                                .child("SOURCE BROWSER"),
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
                        .child(source_badge("read-only"))
                        .child(source_badge("local checkout")),
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
    let show_focus_excerpt = focus_line
        .filter(|line| prepared_file_has_line(&prepared_file, *line))
        .is_some();
    let show_full_file = prepared_file.lines.len() <= MAX_FULL_SOURCE_LINES;
    let focus_panel = show_focus_excerpt.then(|| {
        let line = focus_line.unwrap_or(1);
        let start_line = line.saturating_sub(6).max(1);

        source_panel("Focused context")
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(fg_muted())
                    .mb(px(10.0))
                    .child(format!(
                        "Showing the selected definition around line {}.",
                        line
                    )),
            )
            .child(render_prepared_file_excerpt_with_line_numbers(
                &prepared_file,
                start_line,
                SOURCE_FOCUS_CONTEXT_LINES,
                lsp_context.as_ref(),
            ))
            .into_any_element()
    });
    let expanded_panel = source_panel(if show_full_file {
        "Full file"
    } else {
        "Expanded excerpt"
    })
    .when(!show_full_file, |el| {
        el.child(
            div()
                .text_size(px(12.0))
                .text_color(fg_muted())
                .mb(px(10.0))
                .child(format!(
                    "This file is large, so the source browser is showing a focused excerpt instead of all {} lines.",
                    prepared_file.lines.len()
                )),
        )
    })
    .child(if show_full_file {
        render_prepared_file_with_line_numbers(&prepared_file, lsp_context.as_ref())
    } else {
        let start_line = focus_line.unwrap_or(1).saturating_sub(12).max(1);
        render_prepared_file_excerpt_with_line_numbers(
            &prepared_file,
            start_line,
            80,
            lsp_context.as_ref(),
        )
    })
    .into_any_element();

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
                .when_some(focus_panel, |el, panel| el.child(panel))
                .child(expanded_panel),
        )
        .into_any_element()
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
