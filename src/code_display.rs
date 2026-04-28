use std::{
    collections::BTreeMap,
    ops::Range,
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

use gpui::prelude::*;
use gpui::*;

use crate::lsp;
use crate::markdown::render_markdown;
use crate::review_session::ReviewLocation;
use crate::selectable_text::SelectableText;
use crate::state::{AppState, PreparedFileContent, PreparedFileLine};
use crate::syntax::{self, SyntaxSpan};
use crate::theme::*;
use crate::views::diff_view::{load_local_source_file_content_flow, open_review_source_location};

static CODE_BLOCK_ID: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Debug)]
pub struct HighlightedCodeLine {
    pub text: String,
    pub spans: Vec<SyntaxSpan>,
    pub line_number: Option<usize>,
}

#[derive(Clone)]
pub struct PreparedFileLspContext {
    state: Entity<AppState>,
    detail_key: String,
    lsp_session_manager: Arc<lsp::LspSessionManager>,
    repo_root: PathBuf,
    file_path: String,
    reference: String,
    document_text: Arc<str>,
}

#[derive(Clone)]
struct PreparedFileLineLspContext {
    file: PreparedFileLspContext,
    line_number: usize,
}

#[derive(Clone, Debug)]
pub struct InteractiveCodeToken {
    pub byte_range: Range<usize>,
    pub column_start: usize,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreparedFileLineDiffKind {
    Addition,
}

pub type PreparedFileLineDiffs = Arc<BTreeMap<usize, PreparedFileLineDiffKind>>;

#[derive(Clone)]
struct PreparedFileLspQuery {
    state: Entity<AppState>,
    detail_key: String,
    lsp_session_manager: Arc<lsp::LspSessionManager>,
    repo_root: PathBuf,
    query_key: String,
    token_label: String,
    request: lsp::LspTextDocumentRequest,
}

pub fn code_text_runs(spans: &[SyntaxSpan]) -> Option<Vec<TextRun>> {
    if spans.is_empty() {
        return None;
    }

    let mut runs = Vec::with_capacity(spans.len());

    for span in spans {
        if span.text.is_empty() {
            continue;
        }

        runs.push(TextRun {
            len: span.text.len(),
            font: font("Fira Code"),
            color: span.color,
            background_color: None,
            underline: None,
            strikethrough: None,
        });
    }

    (!runs.is_empty()).then_some(runs)
}

fn styled_selectable_code_text(
    id: impl Into<SharedString>,
    text: &str,
    spans: &[SyntaxSpan],
) -> SelectableText {
    let text = text.to_string();
    if let Some(runs) = code_text_runs(spans) {
        SelectableText::new(id, text).with_runs(runs)
    } else {
        SelectableText::new(id, text)
    }
}

pub fn render_highlighted_code_block(source_hint: &str, text: &str) -> AnyElement {
    render_highlighted_code_block_lines(highlighted_code_lines(source_hint, text))
}

pub fn render_highlighted_code_block_with_line_numbers(
    source_hint: &str,
    text: &str,
    start_line: usize,
) -> AnyElement {
    render_highlighted_code_block_lines(numbered_highlighted_code_lines(
        source_hint,
        text,
        start_line,
    ))
}

fn render_highlighted_code_block_lines(lines: Vec<HighlightedCodeLine>) -> AnyElement {
    div()
        .w_full()
        .min_w_0()
        .rounded(radius())
        .bg(bg_inset())
        .overflow_hidden()
        .child(
            div()
                .px(px(16.0))
                .py(px(12.0))
                .child(render_highlighted_code_lines(lines)),
        )
        .into_any_element()
}

pub fn render_highlighted_code_content(source_hint: &str, text: &str) -> impl IntoElement {
    render_highlighted_code_lines(highlighted_code_lines(source_hint, text))
}

pub fn highlighted_code_lines(source_hint: &str, text: &str) -> Vec<HighlightedCodeLine> {
    let lines = split_code_lines(text);
    let highlighted = syntax::highlight_lines(source_hint, lines.iter().map(|line| line.as_str()));

    lines
        .into_iter()
        .zip(highlighted)
        .map(|(text, spans)| HighlightedCodeLine {
            text,
            spans,
            line_number: None,
        })
        .collect()
}

pub fn build_prepared_file_lsp_context(
    state: &Entity<AppState>,
    file_path: &str,
    prepared_file: Option<&PreparedFileContent>,
    cx: &App,
) -> Option<PreparedFileLspContext> {
    let prepared_file = prepared_file?;
    if prepared_file.is_binary || prepared_file.text.is_empty() {
        return None;
    }

    let app_state = state.read(cx);
    let detail_key = app_state.active_pr_key.clone()?;
    let detail_state = app_state.detail_states.get(&detail_key)?;
    let local_repo_status = detail_state.local_repository_status.as_ref()?;
    if !local_repo_status.ready_for_snapshot_features() {
        return None;
    }

    let repo_root = PathBuf::from(local_repo_status.path.as_ref()?);
    let lsp_status = detail_state.lsp_statuses.get(file_path)?;
    if !lsp_status.is_ready()
        || (!lsp_status.capabilities.hover_supported
            && !lsp_status.capabilities.signature_help_supported)
    {
        return None;
    }

    Some(PreparedFileLspContext {
        state: state.clone(),
        detail_key,
        lsp_session_manager: app_state.lsp_session_manager.clone(),
        repo_root,
        file_path: file_path.to_string(),
        reference: prepared_file.reference.clone(),
        document_text: prepared_file.text.clone(),
    })
}

pub fn prepared_file_has_line(prepared_file: &PreparedFileContent, line_number: usize) -> bool {
    line_number > 0 && prepared_file.lines.get(line_number - 1).is_some()
}

pub fn render_prepared_file_excerpt_with_line_numbers(
    prepared_file: &PreparedFileContent,
    start_line: usize,
    line_count: usize,
    lsp_context: Option<&PreparedFileLspContext>,
) -> AnyElement {
    let lines = prepared_excerpt_range(prepared_file.lines.len(), start_line, line_count)
        .map(|range| prepared_file.lines[range].to_vec())
        .unwrap_or_default();

    render_prepared_code_block_lines(lines, lsp_context, None)
}

pub fn render_prepared_file_with_line_numbers(
    prepared_file: &PreparedFileContent,
    lsp_context: Option<&PreparedFileLspContext>,
) -> AnyElement {
    render_prepared_code_block_lines(prepared_file.lines.as_ref().clone(), lsp_context, None)
}

pub fn render_prepared_file_with_line_numbers_and_diffs(
    prepared_file: &PreparedFileContent,
    lsp_context: Option<&PreparedFileLspContext>,
    diff_lines: PreparedFileLineDiffs,
) -> AnyElement {
    render_prepared_code_block_lines(
        prepared_file.lines.as_ref().clone(),
        lsp_context,
        Some(diff_lines),
    )
}

fn numbered_highlighted_code_lines(
    source_hint: &str,
    text: &str,
    start_line: usize,
) -> Vec<HighlightedCodeLine> {
    highlighted_code_lines(source_hint, text)
        .into_iter()
        .enumerate()
        .map(|(index, mut line)| {
            line.line_number = Some(start_line + index);
            line
        })
        .collect()
}

fn render_highlighted_code_lines(lines: Vec<HighlightedCodeLine>) -> impl IntoElement {
    let block_id = CODE_BLOCK_ID.fetch_add(1, Ordering::Relaxed);
    let show_line_numbers = lines.iter().any(|line| line.line_number.is_some());

    div()
        .w_full()
        .min_w_0()
        .id(ElementId::Name(format!("code-block-{block_id}").into()))
        .overflow_x_scroll()
        .child(
            div()
                .w_full()
                .min_w_0()
                .whitespace_nowrap()
                .font_family("Fira Code")
                .text_size(px(12.0))
                .text_color(fg_default())
                .flex()
                .flex_col()
                .children(lines.into_iter().enumerate().map(|(line_ix, line)| {
                    render_code_line(block_id, line_ix, line, show_line_numbers)
                })),
        )
}

fn render_prepared_code_block_lines(
    lines: Vec<PreparedFileLine>,
    lsp_context: Option<&PreparedFileLspContext>,
    diff_lines: Option<PreparedFileLineDiffs>,
) -> AnyElement {
    div()
        .w_full()
        .min_w_0()
        .rounded(radius())
        .bg(bg_inset())
        .overflow_hidden()
        .child(
            div()
                .px(px(16.0))
                .py(px(12.0))
                .child(render_prepared_code_lines(lines, lsp_context, diff_lines)),
        )
        .into_any_element()
}

fn render_prepared_code_lines(
    lines: Vec<PreparedFileLine>,
    lsp_context: Option<&PreparedFileLspContext>,
    diff_lines: Option<PreparedFileLineDiffs>,
) -> impl IntoElement {
    let block_id = CODE_BLOCK_ID.fetch_add(1, Ordering::Relaxed);
    let show_diff_markers = diff_lines.is_some();

    div()
        .w_full()
        .min_w_0()
        .id(ElementId::Name(
            format!("prepared-code-block-{block_id}").into(),
        ))
        .overflow_x_scroll()
        .child(
            div()
                .w_full()
                .min_w_0()
                .whitespace_nowrap()
                .font_family("Fira Code")
                .text_size(px(12.0))
                .text_color(fg_default())
                .flex()
                .flex_col()
                .children(lines.into_iter().map(move |line| {
                    let line_lsp_context = lsp_context.map(|context| PreparedFileLineLspContext {
                        file: context.clone(),
                        line_number: line.line_number,
                    });
                    let diff_kind = diff_lines
                        .as_ref()
                        .and_then(|diff_lines| diff_lines.get(&line.line_number))
                        .copied();
                    render_prepared_code_line_with_diff(
                        block_id,
                        line,
                        line_lsp_context,
                        diff_kind,
                        show_diff_markers,
                    )
                })),
        )
}

fn render_code_line(
    block_id: usize,
    line_ix: usize,
    line: HighlightedCodeLine,
    show_line_numbers: bool,
) -> Div {
    let line_div = div()
        .w_full()
        .min_w_0()
        .font_family("Fira Code")
        .flex()
        .items_start();
    let line_number = line.line_number;
    let code = render_code_line_content(block_id, line_ix, line.text, line.spans);

    if show_line_numbers {
        line_div
            .child(
                div()
                    .w(px(56.0))
                    .flex_shrink_0()
                    .pr(px(12.0))
                    .text_align(TextAlign::Right)
                    .text_color(fg_subtle())
                    .child(
                        line_number
                            .map(|value| value.to_string())
                            .unwrap_or_default(),
                    ),
            )
            .child(code)
    } else {
        line_div.child(code)
    }
}

fn render_code_line_content(
    block_id: usize,
    line_ix: usize,
    line: String,
    spans: Vec<SyntaxSpan>,
) -> Div {
    let code_div = div().w_full().min_w_0().font_family("Fira Code");

    if let Some(runs) = code_text_runs(&spans) {
        code_div.child(
            SelectableText::new(format!("code-block-{block_id}-line-{line_ix}"), line)
                .with_runs(runs),
        )
    } else if line.is_empty() {
        code_div.child("\u{00a0}".to_string())
    } else {
        code_div.child(SelectableText::new(
            format!("code-block-{block_id}-line-{line_ix}"),
            line,
        ))
    }
}

fn render_prepared_code_line_with_diff(
    block_id: usize,
    line: PreparedFileLine,
    lsp_context: Option<PreparedFileLineLspContext>,
    diff_kind: Option<PreparedFileLineDiffKind>,
    show_diff_markers: bool,
) -> Div {
    let (row_bg, gutter_bg, row_border, marker, marker_color) = match diff_kind {
        Some(PreparedFileLineDiffKind::Addition) => (
            diff_add_bg(),
            diff_add_gutter_bg(),
            diff_add_border(),
            "+",
            success(),
        ),
        None => (
            transparent(),
            transparent(),
            transparent(),
            " ",
            fg_subtle(),
        ),
    };

    div()
        .w_full()
        .min_w_0()
        .min_h(px(22.0))
        .font_family("Fira Code")
        .flex()
        .items_start()
        .bg(row_bg)
        .border_b(px(1.0))
        .border_color(row_border)
        .child(
            div()
                .w(px(56.0))
                .flex_shrink_0()
                .pr(px(12.0))
                .bg(gutter_bg)
                .text_align(TextAlign::Right)
                .text_color(fg_subtle())
                .child(line.line_number.to_string()),
        )
        .when(show_diff_markers, |el| {
            el.child(
                div()
                    .w(px(16.0))
                    .flex_shrink_0()
                    .py(px(1.0))
                    .text_color(marker_color)
                    .child(marker.to_string()),
            )
        })
        .child(render_prepared_code_line_content(
            block_id,
            line,
            lsp_context,
        ))
}

fn render_prepared_code_line_content(
    block_id: usize,
    line: PreparedFileLine,
    lsp_context: Option<PreparedFileLineLspContext>,
) -> Div {
    let code_div = div().w_full().min_w_0().font_family("Fira Code");

    if line.text.is_empty() {
        return code_div.child("\u{00a0}".to_string());
    }

    let token_ranges = Arc::new(build_interactive_code_tokens(&line.text));

    if let Some(lsp_context) = lsp_context.filter(|_| !token_ranges.is_empty()) {
        let hover_context = lsp_context.clone();
        let hover_tokens = token_ranges.clone();
        let tooltip_context = lsp_context.clone();
        let tooltip_tokens = token_ranges.clone();
        let click_context = lsp_context.clone();
        let click_tokens = token_ranges.clone();
        let click_ranges: Vec<std::ops::Range<usize>> =
            token_ranges.iter().map(|t| t.byte_range.clone()).collect();
        let interactive = styled_selectable_code_text(
            format!(
                "prepared-code-lsp:{}:{}:{}",
                block_id, lsp_context.file.file_path, lsp_context.line_number
            ),
            &line.text,
            &line.spans,
        )
        .on_click(click_ranges, move |range_ix, window, cx| {
            let token = &click_tokens[range_ix];
            let Some(query) =
                click_context.query_for_index(token.byte_range.start, click_tokens.as_ref())
            else {
                return;
            };
            navigate_to_prepared_file_lsp_definition(query, window, cx);
        })
        .on_hover(move |index, _event, window, cx| {
            let Some(index) = index else {
                return;
            };
            let Some(query) = hover_context.query_for_index(index, hover_tokens.as_ref()) else {
                return;
            };
            request_prepared_file_lsp_details(query, window, cx);
        })
        .tooltip(move |index, _window, cx| {
            let query = tooltip_context.query_for_index(index, tooltip_tokens.as_ref())?;
            Some(build_lsp_hover_tooltip_view(
                query.state.clone(),
                query.detail_key.clone(),
                query.query_key.clone(),
                query.token_label.clone(),
                cx,
            ))
        });

        return code_div.child(interactive);
    }

    code_div.child(styled_selectable_code_text(
        format!("prepared-code-{block_id}-line-{}", line.line_number),
        &line.text,
        &line.spans,
    ))
}

fn split_code_lines(text: &str) -> Vec<String> {
    text.split('\n')
        .map(|line| line.trim_end_matches('\r').to_string())
        .collect()
}

impl PreparedFileLineLspContext {
    fn query_for_index(
        &self,
        index: usize,
        tokens: &[InteractiveCodeToken],
    ) -> Option<PreparedFileLspQuery> {
        let token = tokens
            .iter()
            .find(|token| token.byte_range.contains(&index))?;

        Some(PreparedFileLspQuery {
            state: self.file.state.clone(),
            detail_key: self.file.detail_key.clone(),
            lsp_session_manager: self.file.lsp_session_manager.clone(),
            repo_root: self.file.repo_root.clone(),
            query_key: format!(
                "{}:{}:{}:{}",
                self.file.file_path, self.file.reference, self.line_number, token.column_start
            ),
            token_label: display_lsp_token_label(&token.text),
            request: lsp::LspTextDocumentRequest {
                file_path: self.file.file_path.clone(),
                document_text: self.file.document_text.clone(),
                line: self.line_number,
                column: token.column_start,
            },
        })
    }
}

pub fn build_interactive_code_tokens(text: &str) -> Vec<InteractiveCodeToken> {
    let mut tokens = Vec::new();
    let mut token_start_byte = None;
    let mut token_start_column = 0usize;
    let mut column = 1usize;

    for (byte_index, character) in text.char_indices() {
        if is_interactive_token_character(character) {
            if token_start_byte.is_none() {
                token_start_byte = Some(byte_index);
                token_start_column = column;
            }
        } else if let Some(start_byte) = token_start_byte.take() {
            tokens.push(InteractiveCodeToken {
                byte_range: start_byte..byte_index,
                column_start: token_start_column,
                text: text[start_byte..byte_index].to_string(),
            });
        }

        column += 1;
    }

    if let Some(start_byte) = token_start_byte {
        tokens.push(InteractiveCodeToken {
            byte_range: start_byte..text.len(),
            column_start: token_start_column,
            text: text[start_byte..].to_string(),
        });
    }

    tokens
}

fn is_interactive_token_character(character: char) -> bool {
    character == '_' || character.is_alphanumeric()
}

fn display_lsp_token_label(text: &str) -> String {
    let trimmed = text.trim();
    let mut label = trimmed.chars().take(48).collect::<String>();
    if trimmed.chars().count() > 48 {
        label.push('…');
    }
    label
}

fn should_request_prepared_file_lsp_details(query: &PreparedFileLspQuery, cx: &App) -> bool {
    query
        .state
        .read(cx)
        .detail_states
        .get(&query.detail_key)
        .and_then(|detail_state| detail_state.lsp_symbol_states.get(&query.query_key))
        .map(|state| !state.loading && state.details.is_none() && state.error.is_none())
        .unwrap_or(true)
}

fn request_prepared_file_lsp_details(
    query: PreparedFileLspQuery,
    window: &mut Window,
    cx: &mut App,
) {
    if !should_request_prepared_file_lsp_details(&query, cx) {
        return;
    }

    let query_key = query.query_key.clone();
    let detail_key = query.detail_key.clone();
    let state = query.state.clone();

    state.update(cx, |state, cx| {
        let Some(detail_state) = state.detail_states.get_mut(&detail_key) else {
            return;
        };
        let symbol_state = detail_state
            .lsp_symbol_states
            .entry(query_key.clone())
            .or_default();
        if symbol_state.loading || symbol_state.details.is_some() || symbol_state.error.is_some() {
            return;
        }
        symbol_state.loading = true;
        symbol_state.details = None;
        symbol_state.error = None;
        cx.notify();
    });

    window
        .spawn(cx, {
            let state = state.clone();
            let detail_key = detail_key.clone();
            let query_key = query_key.clone();
            let lsp_session_manager = query.lsp_session_manager.clone();
            let repo_root = query.repo_root.clone();
            let request = query.request.clone();
            async move |cx: &mut AsyncWindowContext| {
                let result = cx
                    .background_executor()
                    .spawn(async move { lsp_session_manager.symbol_details(&repo_root, &request) })
                    .await;

                state
                    .update(cx, |state, cx| {
                        let Some(detail_state) = state.detail_states.get_mut(&detail_key) else {
                            return;
                        };
                        let symbol_state = detail_state
                            .lsp_symbol_states
                            .entry(query_key.clone())
                            .or_default();
                        symbol_state.loading = false;
                        match result {
                            Ok(details) => {
                                symbol_state.details = Some(details);
                                symbol_state.error = None;
                            }
                            Err(error) => {
                                symbol_state.details = None;
                                symbol_state.error = Some(error);
                            }
                        }
                        cx.notify();
                    })
                    .ok();
            }
        })
        .detach();
}

fn navigate_to_prepared_file_lsp_definition(
    query: PreparedFileLspQuery,
    window: &mut Window,
    cx: &mut App,
) {
    // Try to read cached definition targets
    let targets = query
        .state
        .read(cx)
        .detail_states
        .get(&query.detail_key)
        .and_then(|detail_state| detail_state.lsp_symbol_states.get(&query.query_key))
        .and_then(|symbol_state| symbol_state.details.as_ref())
        .map(|details| details.definition_targets.clone());

    if let Some(targets) = targets.filter(|t| !t.is_empty()) {
        let target = &targets[0];
        open_review_source_location(
            &query.state,
            target.path.clone(),
            Some(target.line),
            Some("Jumped to definition".to_string()),
            window,
            cx,
        );
        return;
    }

    // Not cached — fetch definition asynchronously, then navigate
    let state = query.state.clone();
    window
        .spawn(cx, {
            let lsp_session_manager = query.lsp_session_manager.clone();
            let repo_root = query.repo_root.clone();
            let request = query.request.clone();
            async move |cx: &mut AsyncWindowContext| {
                let result = cx
                    .background_executor()
                    .spawn(async move { lsp_session_manager.definition(&repo_root, &request) })
                    .await;

                if let Ok(targets) = result {
                    if let Some(target) = targets.first() {
                        let target = target.clone();
                        state
                            .update(cx, |state, cx| {
                                state.navigate_to_review_location(
                                    ReviewLocation::from_source(
                                        target.path.clone(),
                                        Some(target.line),
                                        Some("Jumped to definition".to_string()),
                                    ),
                                    true,
                                );
                                state.persist_active_review_session();
                                cx.notify();
                            })
                            .ok();
                        load_local_source_file_content_flow(state, target.path.clone(), cx).await;
                    }
                }
            }
        })
        .detach();
}

pub fn build_lsp_hover_tooltip_view(
    state: Entity<AppState>,
    detail_key: String,
    query_key: String,
    token_label: String,
    cx: &mut App,
) -> AnyView {
    AnyView::from(cx.new(move |cx| {
        SharedLspHoverTooltipView::new(state, detail_key, query_key, token_label, cx)
    }))
}

struct SharedLspHoverTooltipView {
    state: Entity<AppState>,
    detail_key: String,
    query_key: String,
    token_label: String,
}

impl SharedLspHoverTooltipView {
    fn new(
        state: Entity<AppState>,
        detail_key: String,
        query_key: String,
        token_label: String,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&state, |_, _, cx| {
            cx.notify();
        })
        .detach();

        Self {
            state,
            detail_key,
            query_key,
            token_label,
        }
    }
}

impl Render for SharedLspHoverTooltipView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let app_state = self.state.read(cx);
        let symbol_state = app_state
            .detail_states
            .get(&self.detail_key)
            .and_then(|detail_state| detail_state.lsp_symbol_states.get(&self.query_key));

        div()
            .w(px(440.0))
            .max_w(px(560.0))
            .min_w(px(360.0))
            .rounded(radius())
            .border_1()
            .border_color(border_default())
            .bg(bg_overlay())
            .shadow_sm()
            .child(
                div()
                    .px(px(12.0))
                    .py(px(8.0))
                    .border_b(px(1.0))
                    .border_color(border_default())
                    .bg(bg_surface())
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex_grow()
                            .min_w_0()
                            .font_family("Fira Code")
                            .text_size(px(12.0))
                            .text_color(fg_emphasis())
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .overflow_x_hidden()
                            .child(self.token_label.clone()),
                    )
                    .child(
                        div()
                            .px(px(6.0))
                            .py(px(2.0))
                            .rounded(radius_sm())
                            .bg(bg_emphasis())
                            .text_size(px(10.0))
                            .font_family("Fira Code")
                            .text_color(accent())
                            .child("LSP"),
                    ),
            )
            .child(
                div()
                    .id("lsp-hover-scroll")
                    .max_h(px(480.0))
                    .overflow_y_scroll()
                    .w_full()
                    .min_w_0()
                    .px(px(12.0))
                    .py(px(10.0))
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .child(match symbol_state {
                        Some(state) if state.loading => div()
                            .text_size(px(12.0))
                            .text_color(fg_muted())
                            .child("Loading symbol info…")
                            .into_any_element(),
                        Some(state) => {
                            if let Some(error) = state.error.as_deref() {
                                div()
                                    .text_size(px(12.0))
                                    .text_color(danger())
                                    .child(error.to_string())
                                    .into_any_element()
                            } else if let Some(details) = state.details.as_ref() {
                                render_lsp_symbol_details(
                                    details,
                                    &format!("lsp-symbol-details-{}", self.query_key),
                                )
                                .into_any_element()
                            } else {
                                div()
                                    .text_size(px(12.0))
                                    .text_color(fg_muted())
                                    .child("No symbol details available.")
                                    .into_any_element()
                            }
                        }
                        None => div()
                            .text_size(px(12.0))
                            .text_color(fg_muted())
                            .child("Loading symbol info…")
                            .into_any_element(),
                    }),
            )
    }
}

fn render_lsp_symbol_details(details: &lsp::LspSymbolDetails, id_prefix: &str) -> AnyElement {
    if details.is_empty() {
        return div()
            .text_size(px(12.0))
            .text_color(fg_muted())
            .child("No LSP details are available for this token.")
            .into_any_element();
    }

    div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(12.0))
        .when_some(details.hover.as_ref(), |el, hover| {
            el.child(
                div()
                    .w_full()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(render_lsp_section_label("HOVER"))
                    .child(div().w_full().min_w_0().child(render_markdown(
                        &format!("{id_prefix}-hover"),
                        &hover.markdown,
                    ))),
            )
        })
        .when_some(details.signature_help.as_ref(), |el, signature| {
            el.child(
                div()
                    .w_full()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(render_lsp_section_label("SIGNATURE"))
                            .when_some(signature.active_parameter.as_ref(), |el, parameter| {
                                el.child(
                                    div()
                                        .px(px(6.0))
                                        .py(px(2.0))
                                        .rounded(radius_sm())
                                        .bg(bg_emphasis())
                                        .text_size(px(10.0))
                                        .text_color(fg_emphasis())
                                        .child(format!("active: {parameter}")),
                                )
                            }),
                    )
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .font_family("Fira Code")
                            .text_size(px(12.0))
                            .text_color(fg_default())
                            .whitespace_normal()
                            .child(signature.label.clone()),
                    )
                    .when_some(signature.documentation.as_deref(), |el, documentation| {
                        el.child(div().w_full().min_w_0().child(render_markdown(
                            &format!("{id_prefix}-signature"),
                            documentation,
                        )))
                    }),
            )
        })
        .when(!details.definition_targets.is_empty(), |el| {
            el.child(
                div()
                    .w_full()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(render_lsp_section_label("DEFINITION"))
                    .children(details.definition_targets.iter().map(|target| {
                        div()
                            .w_full()
                            .min_w_0()
                            .font_family("Fira Code")
                            .text_size(px(12.0))
                            .text_color(fg_default())
                            .whitespace_normal()
                            .child(format!("{}:{}", target.path, target.line))
                    })),
            )
        })
        .when(!details.reference_targets.is_empty(), |el| {
            let extra_count = details.reference_targets.len().saturating_sub(6);
            el.child(
                div()
                    .w_full()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(render_lsp_section_label("REFERENCES"))
                    .children(details.reference_targets.iter().take(6).map(|target| {
                        div()
                            .w_full()
                            .min_w_0()
                            .font_family("Fira Code")
                            .text_size(px(12.0))
                            .text_color(fg_default())
                            .whitespace_normal()
                            .child(format!("{}:{}", target.path, target.line))
                    }))
                    .when(extra_count > 0, |el| {
                        el.child(
                            div()
                                .text_size(px(11.0))
                                .text_color(fg_muted())
                                .child(format!("+{extra_count} more references")),
                        )
                    }),
            )
        })
        .into_any_element()
}

fn render_lsp_section_label(label: &str) -> impl IntoElement {
    div()
        .text_size(px(11.0))
        .font_family("Fira Code")
        .text_color(accent())
        .child(label.to_string())
}

fn prepared_excerpt_range(
    total_lines: usize,
    start_line: usize,
    line_count: usize,
) -> Option<Range<usize>> {
    if total_lines == 0 || start_line == 0 {
        return None;
    }

    let start_index = start_line - 1;
    if start_index >= total_lines {
        return None;
    }

    let end_index = total_lines.min(start_index + line_count.max(1));
    Some(start_index..end_index)
}

#[cfg(test)]
mod tests {
    use super::{build_interactive_code_tokens, prepared_excerpt_range};

    #[test]
    fn prepared_excerpt_range_clamps_to_available_lines() {
        assert_eq!(prepared_excerpt_range(8, 3, 4), Some(2..6));
        assert_eq!(prepared_excerpt_range(8, 8, 4), Some(7..8));
    }

    #[test]
    fn prepared_excerpt_range_rejects_invalid_starts() {
        assert_eq!(prepared_excerpt_range(8, 0, 4), None);
        assert_eq!(prepared_excerpt_range(8, 9, 4), None);
    }

    #[test]
    fn interactive_tokens_are_built_from_raw_code_text() {
        let tokens = build_interactive_code_tokens("fun parse_value(input: String)");
        let token_texts = tokens
            .iter()
            .map(|token| token.text.as_str())
            .collect::<Vec<_>>();

        assert_eq!(token_texts, vec!["fun", "parse_value", "input", "String"]);
        assert_eq!(
            tokens
                .iter()
                .find(|token| token.text == "parse_value")
                .map(|token| token.column_start),
            Some(5)
        );
    }
}
