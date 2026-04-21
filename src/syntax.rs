use std::sync::OnceLock;

use gpui::{Hsla, Rgba};
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::{SyntaxReference, SyntaxSet};

use crate::theme::{active_theme, ActiveTheme};

pub const MAX_HIGHLIGHT_BYTES: usize = 512 * 1024;

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

fn syntax_theme_name() -> &'static str {
    match active_theme() {
        ActiveTheme::Light => "base16-ocean.light",
        ActiveTheme::Dark => "base16-ocean.dark",
    }
}

fn find_syntax_by_hint<'a>(ss: &'a SyntaxSet, hint: &str) -> Option<&'a SyntaxReference> {
    ss.find_syntax_by_token(hint)
        .or_else(|| ss.find_syntax_by_name(hint))
}

fn syntax_aliases(hint: &str) -> &'static [&'static str] {
    if hint.eq_ignore_ascii_case("ts")
        || hint.eq_ignore_ascii_case("tsx")
        || hint.eq_ignore_ascii_case("mts")
        || hint.eq_ignore_ascii_case("cts")
        || hint.eq_ignore_ascii_case("typescript")
        || hint.eq_ignore_ascii_case("typescriptreact")
        || hint.eq_ignore_ascii_case("jsx")
        || hint.eq_ignore_ascii_case("javascriptreact")
    {
        &["JavaScript", "js"]
    } else {
        &[]
    }
}

fn find_syntax<'a>(ss: &'a SyntaxSet, file_path: &str) -> Option<&'a SyntaxReference> {
    let filename = file_path.rsplit('/').next().unwrap_or(file_path);
    let ext = filename
        .rsplit('.')
        .next()
        .filter(|ext| !ext.is_empty() && *ext != filename);

    find_syntax_by_hint(ss, filename)
        .or_else(|| ext.and_then(|ext| find_syntax_by_hint(ss, ext)))
        .or_else(|| {
            syntax_aliases(filename)
                .iter()
                .find_map(|alias| find_syntax_by_hint(ss, alias))
        })
        .or_else(|| {
            ext.and_then(|ext| {
                syntax_aliases(ext)
                    .iter()
                    .find_map(|alias| find_syntax_by_hint(ss, alias))
            })
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

    let theme = &theme_set().themes[syntax_theme_name()];
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
                    let color = boost_saturation(rgba.into());
                    Some(SyntaxSpan {
                        text,
                        color,
                        column_start,
                        column_end,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Boost saturation of syntax colors to make highlighting more vivid.
/// The base16-ocean.dark theme produces very muted colors (s: 0.12–0.42).
/// This amplifies saturation while preserving the hue relationships, giving
/// results closer to modern editor themes (IntelliJ, VS Code).
fn boost_saturation(color: Hsla) -> Hsla {
    // Don't touch near-gray text (comments, punctuation) — keep those subtle.
    if color.s < 0.08 {
        return color;
    }
    let multiplier = match active_theme() {
        ActiveTheme::Light => 1.35,
        ActiveTheme::Dark => 2.2,
    };
    let boosted_s = (color.s * multiplier).min(1.0);
    Hsla {
        s: boosted_s,
        ..color
    }
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
    fn test_typescript_highlighting_from_extension_and_language_hint() {
        let file_spans = highlight_line("app.ts", "const answer: number = 42;");
        assert!(
            !file_spans.is_empty(),
            "Expected syntax spans for TypeScript files"
        );

        let hint_spans = highlight_line("typescript", "const answer: number = 42;");
        assert!(
            !hint_spans.is_empty(),
            "Expected syntax spans for TypeScript language hints"
        );

        let tsx_spans = highlight_line("tsx", "const view = props.children;");
        assert!(
            !tsx_spans.is_empty(),
            "Expected syntax spans for TSX language hints"
        );
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
