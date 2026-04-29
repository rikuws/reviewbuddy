use gpui::prelude::FluentBuilder;
use gpui::*;
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

use crate::code_display::render_highlighted_code_block;
use crate::selectable_text::SelectableText;
use crate::theme::*;

/// Render a markdown string into GPUI elements.
pub fn render_markdown(id_prefix: &str, text: &str) -> impl IntoElement {
    let options =
        Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS;
    let parser = Parser::new_ext(text, options);

    let mut builder = MarkdownBuilder::new(id_prefix);

    for event in parser {
        builder.push_event(event);
    }

    builder.finish()
}

struct MarkdownBuilder {
    /// Top-level block elements collected so far.
    blocks: Vec<AnyElement>,
    /// Stack of inline style flags.
    bold: bool,
    emphasis: bool,
    strikethrough: bool,
    code_span: bool,
    /// Current inline buffer — accumulated text for the current paragraph/heading.
    inline_buffer: Vec<InlineSpan>,
    /// Current block context.
    block_stack: Vec<BlockContext>,
    /// Code block accumulator.
    code_block_text: String,
    code_block_lang: Option<String>,
    /// List state.
    list_items: Vec<AnyElement>,
    list_ordered: bool,
    list_counter: u64,
    id_prefix: String,
    block_id: usize,
    /// Table state.
    table_rows: Vec<Vec<String>>,
    table_current_row: Vec<String>,
    in_table_head: bool,
}

#[derive(Clone)]
struct InlineSpan {
    text: String,
    bold: bool,
    emphasis: bool,
    strikethrough: bool,
    code: bool,
    link_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
enum BlockContext {
    Paragraph,
    Heading(u8),
    BlockQuote,
    CodeBlock,
    ListItem,
    Table,
}

impl MarkdownBuilder {
    fn new(id_prefix: &str) -> Self {
        Self {
            blocks: Vec::new(),
            bold: false,
            emphasis: false,
            strikethrough: false,
            code_span: false,
            inline_buffer: Vec::new(),
            block_stack: Vec::new(),
            code_block_text: String::new(),
            code_block_lang: None,
            list_items: Vec::new(),
            list_ordered: false,
            list_counter: 0,
            id_prefix: id_prefix.to_string(),
            block_id: 0,
            table_rows: Vec::new(),
            table_current_row: Vec::new(),
            in_table_head: false,
        }
    }

    fn next_block_id(&mut self, label: &str) -> String {
        let id = format!("{}-{label}-{}", self.id_prefix, self.block_id);
        self.block_id += 1;
        id
    }

    fn push_event(&mut self, event: Event) {
        match event {
            Event::Start(tag) => self.start_tag(tag),
            Event::End(tag) => self.end_tag(tag),
            Event::Text(text) => self.push_text(&text),
            Event::Code(code) => {
                self.inline_buffer.push(InlineSpan {
                    text: code.to_string(),
                    bold: self.bold,
                    emphasis: self.emphasis,
                    strikethrough: self.strikethrough,
                    code: true,
                    link_url: None,
                });
            }
            Event::SoftBreak => {
                self.inline_buffer.push(InlineSpan {
                    text: " ".to_string(),
                    bold: false,
                    emphasis: false,
                    strikethrough: false,
                    code: false,
                    link_url: None,
                });
            }
            Event::HardBreak => {
                self.inline_buffer.push(InlineSpan {
                    text: "\n".to_string(),
                    bold: false,
                    emphasis: false,
                    strikethrough: false,
                    code: false,
                    link_url: None,
                });
            }
            Event::Rule => {
                self.blocks.push(
                    div()
                        .w_full()
                        .h(px(1.0))
                        .bg(bg_subtle())
                        .my(px(16.0))
                        .into_any_element(),
                );
            }
            Event::TaskListMarker(checked) => {
                let marker = if checked { "\u{2611} " } else { "\u{2610} " };
                self.inline_buffer.push(InlineSpan {
                    text: marker.to_string(),
                    bold: false,
                    emphasis: false,
                    strikethrough: false,
                    code: false,
                    link_url: None,
                });
            }
            _ => {}
        }
    }

    fn start_tag(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph => {
                self.block_stack.push(BlockContext::Paragraph);
            }
            Tag::Heading { level, .. } => {
                self.block_stack.push(BlockContext::Heading(level as u8));
            }
            Tag::BlockQuote(_) => {
                self.block_stack.push(BlockContext::BlockQuote);
            }
            Tag::CodeBlock(kind) => {
                self.code_block_lang = match kind {
                    CodeBlockKind::Fenced(lang) => {
                        let l = lang.to_string();
                        if l.is_empty() {
                            None
                        } else {
                            Some(l)
                        }
                    }
                    CodeBlockKind::Indented => None,
                };
                self.code_block_text.clear();
                self.block_stack.push(BlockContext::CodeBlock);
            }
            Tag::List(start) => {
                self.list_ordered = start.is_some();
                self.list_counter = start.unwrap_or(1);
                self.list_items.clear();
            }
            Tag::Item => {
                self.block_stack.push(BlockContext::ListItem);
            }
            Tag::Emphasis => {
                self.emphasis = true;
            }
            Tag::Strong => {
                self.bold = true;
            }
            Tag::Strikethrough => {
                self.strikethrough = true;
            }
            Tag::Link { dest_url, .. } => {
                // We'll push a link span when text arrives
                self.inline_buffer.push(InlineSpan {
                    text: String::new(),
                    bold: self.bold,
                    emphasis: self.emphasis,
                    strikethrough: self.strikethrough,
                    code: false,
                    link_url: Some(dest_url.to_string()),
                });
            }
            Tag::Table(_) => {
                self.table_rows.clear();
                self.block_stack.push(BlockContext::Table);
            }
            Tag::TableHead => {
                self.in_table_head = true;
                self.table_current_row.clear();
            }
            Tag::TableRow => {
                self.table_current_row.clear();
            }
            Tag::TableCell => {}
            _ => {}
        }
    }

    fn end_tag(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                self.block_stack.pop();
                let id = self.next_block_id("paragraph");
                let el = self.flush_inline_paragraph(&id);
                self.blocks.push(el);
            }
            TagEnd::Heading(level) => {
                self.block_stack.pop();
                let id = self.next_block_id("heading");
                let el = self.flush_inline_heading(&id, level as u8);
                self.blocks.push(el);
            }
            TagEnd::BlockQuote(_) => {
                self.block_stack.pop();
                let id = self.next_block_id("blockquote");
                let el = self.flush_inline_blockquote(&id);
                self.blocks.push(el);
            }
            TagEnd::CodeBlock => {
                self.block_stack.pop();
                let text = std::mem::take(&mut self.code_block_text);
                let lang = self.code_block_lang.take();
                let code_id = self.next_block_id("code");
                self.blocks
                    .push(render_code_block(&code_id, &text, lang.as_deref()));
            }
            TagEnd::List(_) => {
                let items = std::mem::take(&mut self.list_items);
                self.blocks.push(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .my(px(8.0))
                        .children(items)
                        .into_any_element(),
                );
            }
            TagEnd::Item => {
                self.block_stack.pop();
                let spans = std::mem::take(&mut self.inline_buffer);
                let prefix = if self.list_ordered {
                    let n = self.list_counter;
                    self.list_counter += 1;
                    format!("{n}. ")
                } else {
                    "\u{2022} ".to_string()
                };
                let item_id = self.next_block_id("list-item");
                self.list_items
                    .push(render_list_item(&item_id, &prefix, &spans));
            }
            TagEnd::Emphasis => {
                self.emphasis = false;
            }
            TagEnd::Strong => {
                self.bold = false;
            }
            TagEnd::Strikethrough => {
                self.strikethrough = false;
            }
            TagEnd::Link => {
                // Link text was accumulated into the last span with link_url
            }
            TagEnd::Table => {
                self.block_stack.pop();
                let rows = std::mem::take(&mut self.table_rows);
                let table_id = self.next_block_id("table");
                self.blocks.push(render_table(&table_id, &rows));
            }
            TagEnd::TableHead => {
                self.in_table_head = false;
                let row = std::mem::take(&mut self.table_current_row);
                self.table_rows.push(row);
            }
            TagEnd::TableRow => {
                let row = std::mem::take(&mut self.table_current_row);
                self.table_rows.push(row);
            }
            TagEnd::TableCell => {
                // Flush inline text into the cell
                let text: String = self.inline_buffer.drain(..).map(|s| s.text).collect();
                self.table_current_row.push(text);
            }
            _ => {}
        }
    }

    fn push_text(&mut self, text: &str) {
        // If we're in a code block, accumulate raw text
        if self.block_stack.last() == Some(&BlockContext::CodeBlock) {
            self.code_block_text.push_str(text);
            return;
        }

        // Check if the last inline span is a link placeholder with empty text
        if let Some(last) = self.inline_buffer.last_mut() {
            if last.link_url.is_some() && last.text.is_empty() {
                last.text = text.to_string();
                return;
            }
        }

        self.inline_buffer.push(InlineSpan {
            text: text.to_string(),
            bold: self.bold,
            emphasis: self.emphasis,
            strikethrough: self.strikethrough,
            code: self.code_span,
            link_url: None,
        });
    }

    fn flush_inline_paragraph(&mut self, id: &str) -> AnyElement {
        let spans = std::mem::take(&mut self.inline_buffer);
        render_inline_block(id, &spans, px(13.0), fg_default(), false)
    }

    fn flush_inline_heading(&mut self, id: &str, level: u8) -> AnyElement {
        let spans = std::mem::take(&mut self.inline_buffer);
        let size = match level {
            1 => px(22.0),
            2 => px(18.0),
            3 => px(16.0),
            _ => px(14.0),
        };
        render_inline_block(id, &spans, size, fg_emphasis(), true)
    }

    fn flush_inline_blockquote(&mut self, id: &str) -> AnyElement {
        let spans = std::mem::take(&mut self.inline_buffer);
        div()
            .w_full()
            .min_w_0()
            .p(px(12.0))
            .rounded(radius_sm())
            .bg(bg_subtle())
            .my(px(8.0))
            .child(
                render_inline_div(id, &spans, px(13.0), fg_muted(), FontWeight::NORMAL)
                    .w_full()
                    .min_w_0(),
            )
            .into_any_element()
    }

    fn finish(self) -> Div {
        div()
            .flex()
            .flex_col()
            .w_full()
            .min_w_0()
            .gap(px(8.0))
            .children(self.blocks)
    }
}

fn render_inline_block(
    id: &str,
    spans: &[InlineSpan],
    base_size: Pixels,
    base_color: Rgba,
    heading: bool,
) -> AnyElement {
    let base_weight = if heading {
        FontWeight::SEMIBOLD
    } else {
        FontWeight::NORMAL
    };
    let mut el = render_inline_div(id, spans, base_size, base_color, base_weight)
        .w_full()
        .min_w_0();
    if heading {
        el = el.mt(px(12.0)).mb(px(4.0));
    } else {
        el = el.my(px(2.0));
    }
    el.into_any_element()
}

fn render_inline_div(
    id: &str,
    spans: &[InlineSpan],
    base_size: Pixels,
    base_color: Rgba,
    base_weight: FontWeight,
) -> Div {
    if spans.is_empty() {
        return div().w_full().min_w_0();
    }

    let mut text = String::new();
    let mut runs = Vec::with_capacity(spans.len());
    let base_color_hsla: Hsla = base_color.into();
    let code_color: Hsla = fg_emphasis().into();
    let code_bg: Hsla = bg_emphasis().into();

    for span in spans {
        if span.text.is_empty() {
            continue;
        }

        let weight = if span.bold {
            if base_weight == FontWeight::SEMIBOLD {
                FontWeight::BOLD
            } else {
                FontWeight::SEMIBOLD
            }
        } else {
            base_weight
        };

        let mut font = if span.code {
            font(mono_font_family())
        } else {
            font(ui_font_family())
        };
        font.weight = weight;
        font.style = if span.emphasis {
            FontStyle::Italic
        } else {
            FontStyle::Normal
        };

        let color = if span.code || span.link_url.is_some() {
            code_color
        } else {
            base_color_hsla
        };

        text.push_str(&span.text);
        runs.push(TextRun {
            len: span.text.len(),
            font,
            color,
            background_color: span.code.then_some(code_bg),
            underline: span.link_url.as_ref().map(|_| UnderlineStyle {
                thickness: px(1.0),
                color: Some(color),
                wavy: false,
            }),
            strikethrough: span.strikethrough.then_some(StrikethroughStyle {
                thickness: px(1.0),
                color: Some(color),
            }),
        });
    }

    div()
        .w_full()
        .min_w_0()
        .whitespace_normal()
        .text_size(base_size)
        .text_color(base_color)
        .font_weight(base_weight)
        .child(SelectableText::new(id.to_string(), text).with_runs(runs))
}

fn render_code_block(_id: &str, text: &str, lang: Option<&str>) -> AnyElement {
    div()
        .w_full()
        .min_w_0()
        .my(px(8.0))
        .child(render_highlighted_code_block(
            lang.unwrap_or_default(),
            text,
        ))
        .into_any_element()
}

fn render_list_item(id: &str, prefix: &str, spans: &[InlineSpan]) -> AnyElement {
    div()
        .flex()
        .items_start()
        .w_full()
        .min_w_0()
        .gap(px(4.0))
        .pl(px(8.0))
        .child(
            div()
                .text_size(px(13.0))
                .text_color(fg_muted())
                .flex_shrink_0()
                .child(prefix.to_string()),
        )
        .child(
            render_inline_div(id, spans, px(13.0), fg_default(), FontWeight::NORMAL)
                .flex_1()
                .min_w_0(),
        )
        .into_any_element()
}

fn render_table(id: &str, rows: &[Vec<String>]) -> AnyElement {
    if rows.is_empty() {
        return div().into_any_element();
    }

    div()
        .w_full()
        .min_w_0()
        .my(px(8.0))
        .rounded(radius())
        .bg(bg_surface())
        .overflow_hidden()
        .flex()
        .flex_col()
        .children(rows.iter().enumerate().map(|(i, row)| {
            let is_header = i == 0;
            div()
                .flex()
                .when(is_header, |el: Div| {
                    el.bg(bg_emphasis()).font_weight(FontWeight::SEMIBOLD)
                })
                .when(!is_header && i % 2 == 0, |el: Div| el.bg(bg_subtle()))
                .children(row.iter().enumerate().map(|(column_ix, cell)| {
                    div()
                        .flex_1()
                        .min_w_0()
                        .px(px(12.0))
                        .py(px(6.0))
                        .whitespace_normal()
                        .text_size(px(12.0))
                        .text_color(if is_header {
                            fg_emphasis()
                        } else {
                            fg_default()
                        })
                        .child(SelectableText::new(
                            format!("{id}-cell-{i}-{column_ix}"),
                            cell.clone(),
                        ))
                }))
        }))
        .into_any_element()
}
