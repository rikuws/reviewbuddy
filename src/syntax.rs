use std::sync::OnceLock;

use gpui::{Hsla, Rgba};
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::{SyntaxReference, SyntaxSet};

#[derive(Clone, Debug)]
pub struct SyntaxSpan {
    pub text: String,
    pub color: Hsla,
    pub column_start: usize,
    pub column_end: usize,
}

fn syntax_set() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(|| SyntaxSet::load_defaults_newlines())
}

fn theme_set() -> &'static ThemeSet {
    static SET: OnceLock<ThemeSet> = OnceLock::new();
    SET.get_or_init(ThemeSet::load_defaults)
}

fn find_syntax<'a>(ss: &'a SyntaxSet, file_path: &str) -> Option<&'a SyntaxReference> {
    let filename = file_path.rsplit('/').next().unwrap_or(file_path);

    ss.find_syntax_by_token(filename)
        .or_else(|| {
            let ext = filename.rsplit('.').next().unwrap_or("");
            if !ext.is_empty() && ext != filename {
                ss.find_syntax_by_extension(ext)
            } else {
                None
            }
        })
        .filter(|s| s.name != "Plain Text")
}

pub fn highlight_lines<'a, I>(file_path: &str, lines: I) -> Vec<Vec<SyntaxSpan>>
where
    I: IntoIterator<Item = &'a str>,
{
    let ss = syntax_set();
    let syntax = match find_syntax(ss, file_path) {
        Some(syntax) => syntax,
        None => {
            return lines
                .into_iter()
                .map(|_| Vec::new())
                .collect::<Vec<Vec<SyntaxSpan>>>()
        }
    };

    let theme = &theme_set().themes["base16-ocean.dark"];
    let mut highlighter = HighlightLines::new(syntax, theme);

    lines
        .into_iter()
        .map(|line| highlight_with_state(&mut highlighter, ss, line))
        .collect()
}

/// Highlight a single line of code, returning colored spans.
///
/// Returns an empty vec for unknown file types or empty content,
/// which signals the caller to use its fallback text color.
pub fn highlight_line(file_path: &str, content: &str) -> Vec<SyntaxSpan> {
    highlight_lines(file_path, [content])
        .into_iter()
        .next()
        .unwrap_or_default()
}

fn highlight_with_state(
    highlighter: &mut HighlightLines<'_>,
    syntax_set: &SyntaxSet,
    content: &str,
) -> Vec<SyntaxSpan> {
    if content.is_empty() {
        return Vec::new();
    }

    let line = format!("{content}\n");

    highlighter
        .highlight_line(&line, syntax_set)
        .map(|spans| {
            let mut next_column = 1usize;
            spans
                .into_iter()
                .filter_map(|(style, text)| {
                    let text = text.trim_end_matches('\n').to_string();
                    if text.is_empty() {
                        return None;
                    }

                    let column_start = next_column;
                    let column_end = column_start + text.chars().count();
                    next_column = column_end;
                    let rgba = Rgba {
                        r: style.foreground.r as f32 / 255.0,
                        g: style.foreground.g as f32 / 255.0,
                        b: style.foreground.b as f32 / 255.0,
                        a: style.foreground.a as f32 / 255.0,
                    };
                    Some(SyntaxSpan {
                        text,
                        color: rgba.into(),
                        column_start,
                        column_end,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rust_highlighting() {
        let spans = highlight_line("test.rs", "fn main() {");
        eprintln!("Rust spans count: {}", spans.len());
        for s in &spans {
            eprintln!("  [{:?}] {:?}", s.text, s.color);
        }
        assert!(!spans.is_empty(), "Should produce syntax spans for Rust");
    }

    #[test]
    fn test_javascript_highlighting() {
        let spans = highlight_line("app.js", "const x = 'hello';");
        eprintln!("JS spans count: {}", spans.len());
        for s in &spans {
            eprintln!("  [{:?}] {:?}", s.text, s.color);
        }
        assert!(!spans.is_empty(), "Should produce syntax spans for JS");
    }

    #[test]
    fn test_unknown_extension() {
        let spans = highlight_line("file.xyzabc", "some text");
        assert!(spans.is_empty(), "Unknown extension should return empty");
    }

    #[test]
    fn test_empty_content() {
        let spans = highlight_line("test.rs", "");
        assert!(spans.is_empty(), "Empty content should return empty");
    }

    #[test]
    fn test_stateful_multiline_highlighting() {
        let lines = vec!["const message = `hello", "${name}`;"];
        let highlighted = highlight_lines("app.js", lines.iter().copied());

        assert_eq!(highlighted.len(), 2);
        assert!(
            highlighted.iter().any(|line| !line.is_empty()),
            "Expected syntax spans across multiline input"
        );
    }

    #[test]
    fn test_spans_include_column_offsets() {
        let spans = highlight_line("app.js", "const answer = 42;");
        assert!(!spans.is_empty(), "Expected highlighted spans");

        let mut expected_column = 1usize;
        for span in spans {
            assert_eq!(span.column_start, expected_column);
            assert!(span.column_end > span.column_start);
            expected_column = span.column_end;
        }
    }
}
