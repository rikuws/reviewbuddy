use std::sync::OnceLock;

use gpui::{Hsla, Rgba};
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::{SyntaxReference, SyntaxSet};

#[derive(Clone)]
pub struct SyntaxSpan {
    pub text: String,
    pub color: Hsla,
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

    // Try extension (e.g., "rs" from "src/main.rs")
    let ext = filename.rsplit('.').next().unwrap_or("");
    if !ext.is_empty() && ext != filename {
        if let Some(s) = ss.find_syntax_by_extension(ext) {
            if s.name != "Plain Text" {
                return Some(s);
            }
        }
    }

    // Try full filename (handles Makefile, Dockerfile, etc.)
    ss.find_syntax_by_extension(filename)
        .filter(|s| s.name != "Plain Text")
}

/// Highlight a single line of code, returning colored spans.
///
/// Returns an empty vec for unknown file types or empty content,
/// which signals the caller to use its fallback text color.
pub fn highlight_line(file_path: &str, content: &str) -> Vec<SyntaxSpan> {
    if content.is_empty() {
        return Vec::new();
    }

    let ss = syntax_set();
    let syntax = match find_syntax(ss, file_path) {
        Some(s) => s,
        None => return Vec::new(),
    };

    let theme = &theme_set().themes["base16-ocean.dark"];
    let mut h = HighlightLines::new(syntax, theme);
    let line = format!("{content}\n");

    match h.highlight_line(&line, ss) {
        Ok(spans) => spans
            .into_iter()
            .map(|(style, text)| {
                let text = text.trim_end_matches('\n').to_string();
                let rgba = Rgba {
                    r: style.foreground.r as f32 / 255.0,
                    g: style.foreground.g as f32 / 255.0,
                    b: style.foreground.b as f32 / 255.0,
                    a: style.foreground.a as f32 / 255.0,
                };
                SyntaxSpan {
                    text,
                    color: rgba.into(),
                }
            })
            .filter(|span| !span.text.is_empty())
            .collect(),
        Err(_) => Vec::new(),
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
    fn test_unknown_extension() {
        let spans = highlight_line("file.xyzabc", "some text");
        assert!(spans.is_empty(), "Unknown extension should return empty");
    }

    #[test]
    fn test_empty_content() {
        let spans = highlight_line("test.rs", "");
        assert!(spans.is_empty(), "Empty content should return empty");
    }
}
