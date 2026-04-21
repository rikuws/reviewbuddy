use std::sync::OnceLock;

use giallo::{HighlightOptions, HighlightedText, Registry, ThemeVariant};
use gpui::{Hsla, Rgba};

use crate::theme::{active_theme, ActiveTheme};

pub const MAX_HIGHLIGHT_BYTES: usize = 512 * 1024;

const LIGHT_THEME: &str = "vitesse-light";
const DARK_THEME: &str = "vitesse-black";

#[derive(Clone, Debug)]
pub struct SyntaxSpan {
    pub text: String,
    pub color: Hsla,
    pub column_start: usize,
    pub column_end: usize,
}

fn registry() -> Option<&'static Registry> {
    static REGISTRY: OnceLock<Option<Registry>> = OnceLock::new();
    REGISTRY
        .get_or_init(|| {
            let mut registry = Registry::builtin().ok()?;
            registry.link_grammars();
            Some(registry)
        })
        .as_ref()
}

fn syntax_theme_name() -> &'static str {
    match active_theme() {
        ActiveTheme::Light => LIGHT_THEME,
        ActiveTheme::Dark => DARK_THEME,
    }
}

fn language_aliases(hint: &str) -> &'static [&'static str] {
    if hint.eq_ignore_ascii_case("tsx") || hint.eq_ignore_ascii_case("typescriptreact") {
        &["tsx", "typescript"]
    } else if hint.eq_ignore_ascii_case("jsx") || hint.eq_ignore_ascii_case("javascriptreact") {
        &["jsx", "javascript"]
    } else if hint.eq_ignore_ascii_case("patch") || hint.eq_ignore_ascii_case("diff.patch") {
        &["diff"]
    } else {
        &[]
    }
}

fn push_candidate(candidates: &mut Vec<String>, candidate: impl Into<String>) {
    let candidate = candidate.into();
    if candidate.is_empty() || candidates.iter().any(|existing| existing == &candidate) {
        return;
    }
    candidates.push(candidate);
}

fn find_language(registry: &Registry, file_path: &str) -> Option<String> {
    let filename = file_path.rsplit('/').next().unwrap_or(file_path);
    let filename = filename.to_ascii_lowercase();
    let ext = filename
        .rsplit('.')
        .next()
        .filter(|ext| !ext.is_empty() && *ext != filename);

    let mut candidates = Vec::new();
    push_candidate(&mut candidates, filename.clone());

    if let Some(ext) = ext {
        push_candidate(&mut candidates, ext.to_string());
    }

    for alias in language_aliases(filename.as_str()) {
        push_candidate(&mut candidates, (*alias).to_string());
    }

    if let Some(ext) = ext {
        for alias in language_aliases(ext) {
            push_candidate(&mut candidates, (*alias).to_string());
        }
    }

    candidates
        .into_iter()
        .find(|candidate| registry.contains_grammar(candidate))
}

fn parse_hex_color(hex: &str) -> Option<Rgba> {
    let hex = hex.trim_start_matches('#');
    let parse = |value: &str| u8::from_str_radix(value, 16).ok();

    let (r, g, b, a) = match hex.len() {
        3 => (
            parse(&hex[0..1])? * 17,
            parse(&hex[1..2])? * 17,
            parse(&hex[2..3])? * 17,
            255,
        ),
        4 => (
            parse(&hex[0..1])? * 17,
            parse(&hex[1..2])? * 17,
            parse(&hex[2..3])? * 17,
            parse(&hex[3..4])? * 17,
        ),
        6 => (
            parse(&hex[0..2])?,
            parse(&hex[2..4])?,
            parse(&hex[4..6])?,
            255,
        ),
        8 => (
            parse(&hex[0..2])?,
            parse(&hex[2..4])?,
            parse(&hex[4..6])?,
            parse(&hex[6..8])?,
        ),
        _ => return None,
    };

    Some(Rgba {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a: a as f32 / 255.0,
    })
}

fn highlighted_text_to_span(token: HighlightedText) -> Option<SyntaxSpan> {
    let text = token.text;
    if text.is_empty() {
        return None;
    }

    let color_hex = match token.style {
        ThemeVariant::Single(style) => style.foreground.as_hex(),
        ThemeVariant::Dual { light, dark } => match active_theme() {
            ActiveTheme::Light => light.foreground.as_hex(),
            ActiveTheme::Dark => dark.foreground.as_hex(),
        },
    };

    Some(SyntaxSpan {
        text,
        color: parse_hex_color(&color_hex)?.into(),
        column_start: 0,
        column_end: 0,
    })
}

fn annotate_columns(mut spans: Vec<SyntaxSpan>) -> Vec<SyntaxSpan> {
    let mut next_column = 1usize;
    for span in &mut spans {
        let char_count = span.text.chars().count();
        span.column_start = next_column;
        span.column_end = next_column + char_count;
        next_column = span.column_end;
    }
    spans
}

pub fn highlight_lines<'a, I>(file_path: &str, lines: I) -> Vec<Vec<SyntaxSpan>>
where
    I: IntoIterator<Item = &'a str>,
{
    let lines = lines.into_iter().collect::<Vec<_>>();
    if lines.is_empty() {
        return Vec::new();
    }

    if lines.iter().all(|line| line.is_empty()) {
        return lines.iter().map(|_| Vec::new()).collect();
    }

    let Some(registry) = registry() else {
        return lines.iter().map(|_| Vec::new()).collect();
    };

    let Some(language) = find_language(registry, file_path) else {
        return lines.iter().map(|_| Vec::new()).collect();
    };

    let joined = lines.join("\n");
    let options =
        HighlightOptions::new(language.as_str(), ThemeVariant::Single(syntax_theme_name()))
            .merge_whitespace(false);

    let Ok(highlighted) = registry.highlight(&joined, &options) else {
        return lines.iter().map(|_| Vec::new()).collect();
    };

    let mut result = highlighted
        .tokens
        .into_iter()
        .map(|line| {
            annotate_columns(
                line.into_iter()
                    .filter_map(highlighted_text_to_span)
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<Vec<_>>();

    while result.len() < lines.len() {
        result.push(Vec::new());
    }
    result.truncate(lines.len());
    result
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rust_highlighting() {
        let spans = highlight_line("test.rs", "fn main() {");
        assert!(!spans.is_empty(), "Should produce syntax spans for Rust");
    }

    #[test]
    fn test_javascript_highlighting() {
        let spans = highlight_line("app.js", "const x = 'hello';");
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
    fn test_patch_alias_highlighting() {
        let spans = highlight_line("diff.patch", "@@ -1,1 +1,1 @@");
        assert!(
            !spans.is_empty(),
            "Expected syntax spans for patch-style diffs"
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
