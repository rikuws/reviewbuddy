use std::collections::{BTreeSet, HashMap};

use crate::{
    code_display::build_interactive_code_tokens,
    code_tour::DiffAnchor,
    diff::{find_parsed_diff_file, DiffLineKind, ParsedDiffFile},
    github::PullRequestDetail,
    lsp::LspReferenceTarget,
    review_session::{ReviewLocation, ReviewTaskRoute},
    semantic_diff::{build_semantic_diff_file, SemanticDiffSection},
    state::PreparedFileContent,
};

const MAX_ROUTE_STOPS: usize = 10;
const MAX_FOCUS_TERMS: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewFocusKind {
    Symbol,
    Table,
    Identifier,
}

#[derive(Clone, Debug)]
pub struct ReviewFocusTerm {
    pub term: String,
    pub kind: ReviewFocusKind,
}

#[derive(Clone, Debug)]
pub struct ReviewSymbolFocus {
    pub term: String,
    pub line: usize,
    pub column: usize,
}

pub fn collect_section_focus_terms(
    section: &SemanticDiffSection,
    parsed_file: Option<&ParsedDiffFile>,
) -> Vec<ReviewFocusTerm> {
    let mut terms = Vec::<ReviewFocusTerm>::new();
    let mut seen = BTreeSet::<String>::new();

    if let Some(symbol) = symbol_candidate_from_title(&section.title) {
        push_focus_term(&mut terms, &mut seen, symbol, ReviewFocusKind::Symbol);
    }

    if let Some(identifier) = identifier_candidate_from_title(&section.title) {
        push_focus_term(
            &mut terms,
            &mut seen,
            identifier,
            ReviewFocusKind::Identifier,
        );
    }

    for line in section_changed_lines(section, parsed_file) {
        if let Some(table) = table_candidate_from_line(&line) {
            push_focus_term(&mut terms, &mut seen, table, ReviewFocusKind::Table);
        }
        if let Some(identifier) = identifier_candidate_from_line(&line) {
            push_focus_term(
                &mut terms,
                &mut seen,
                identifier,
                ReviewFocusKind::Identifier,
            );
        }
        if terms.len() >= MAX_FOCUS_TERMS {
            break;
        }
    }

    terms.truncate(MAX_FOCUS_TERMS);
    terms
}

pub fn build_section_symbol_focus(
    section: &SemanticDiffSection,
    parsed_file: Option<&ParsedDiffFile>,
    prepared_file: Option<&PreparedFileContent>,
) -> Option<ReviewSymbolFocus> {
    let prepared_file = prepared_file?;
    let anchor_line = section
        .anchor
        .as_ref()
        .and_then(|anchor| anchor.line)
        .and_then(|line| usize::try_from(line).ok())
        .filter(|line| *line > 0)?;

    let focus_terms = collect_section_focus_terms(section, parsed_file)
        .into_iter()
        .filter(|term| term.kind != ReviewFocusKind::Table)
        .collect::<Vec<_>>();

    for focus in focus_terms {
        if let Some((line, column)) =
            locate_term_in_prepared_file(prepared_file, anchor_line, &focus.term)
        {
            return Some(ReviewSymbolFocus {
                term: focus.term,
                line,
                column,
            });
        }
    }

    None
}

pub fn build_changed_touch_route(
    detail: &PullRequestDetail,
    selected_file_path: &str,
    selected_section: Option<&SemanticDiffSection>,
    focus_terms: &[ReviewFocusTerm],
) -> Option<ReviewTaskRoute> {
    let focus = preferred_focus_term(focus_terms)?;
    let current_location = selected_section
        .and_then(|section| section.anchor.clone())
        .map(|anchor| ReviewLocation::from_diff(selected_file_path.to_string(), Some(anchor)))
        .unwrap_or_else(|| ReviewLocation::from_diff(selected_file_path.to_string(), None));

    let mut stops = vec![current_location.clone()];
    let mut seen = BTreeSet::from([current_location.stable_key()]);
    let focus_lower = focus.term.to_ascii_lowercase();

    for file in &detail.files {
        let parsed = find_parsed_diff_file(&detail.parsed_diff, &file.path);
        let semantic = build_semantic_diff_file(file, parsed, &detail.review_threads);

        for section in &semantic.sections {
            let Some(anchor) = section.anchor.clone() else {
                continue;
            };

            if !section_matches_focus(section, parsed, &focus_lower) {
                continue;
            }

            let location = ReviewLocation::from_diff(file.path.clone(), Some(anchor));
            if !seen.insert(location.stable_key()) {
                continue;
            }

            stops.push(location);
            if stops.len() >= MAX_ROUTE_STOPS {
                break;
            }
        }

        if stops.len() >= MAX_ROUTE_STOPS {
            break;
        }
    }

    (stops.len() > 1).then(|| ReviewTaskRoute {
        id: format!("changes:{}", focus.term.to_ascii_lowercase()),
        title: format!("Changes touching {}", focus.term),
        summary: format!(
            "Walk the changed sections that touch {} before leaving this pull request.",
            focus.term
        ),
        stops,
    })
}

pub fn build_callsite_route(
    detail: &PullRequestDetail,
    current_location: ReviewLocation,
    term: &str,
    references: &[LspReferenceTarget],
) -> Option<ReviewTaskRoute> {
    let changed_file_order = detail
        .files
        .iter()
        .enumerate()
        .map(|(index, file)| (file.path.clone(), index))
        .collect::<HashMap<_, _>>();

    let mut references = references.to_vec();
    references.sort_by(|left, right| {
        let left_rank = changed_file_order
            .get(&left.path)
            .copied()
            .unwrap_or(usize::MAX);
        let right_rank = changed_file_order
            .get(&right.path)
            .copied()
            .unwrap_or(usize::MAX);

        left_rank
            .cmp(&right_rank)
            .then_with(|| left.path.cmp(&right.path))
            .then_with(|| left.line.cmp(&right.line))
            .then_with(|| left.column.cmp(&right.column))
    });

    let mut stops = vec![current_location.clone()];
    let mut seen = BTreeSet::from([current_location.stable_key()]);

    for reference in references {
        let location = if changed_file_order.contains_key(&reference.path) {
            ReviewLocation::from_diff(
                reference.path.clone(),
                Some(diff_anchor_for_reference(detail, &reference)),
            )
        } else {
            ReviewLocation::from_source(
                reference.path.clone(),
                Some(reference.line),
                Some(format!("Call site of {term}")),
            )
        };

        if !seen.insert(location.stable_key()) {
            continue;
        }

        stops.push(location);
        if stops.len() >= MAX_ROUTE_STOPS {
            break;
        }
    }

    let changed_callsites = stops
        .iter()
        .skip(1)
        .filter(|location| location.mode == crate::review_session::ReviewCenterMode::SemanticDiff)
        .count();

    (stops.len() > 1).then(|| ReviewTaskRoute {
        id: format!("callsites:{}", term.to_ascii_lowercase()),
        title: format!("Call sites of {term}"),
        summary: if changed_callsites > 0 {
            format!(
                "Start with {changed_callsites} changed call site{} and then follow the wider usage graph.",
                if changed_callsites == 1 { "" } else { "s" }
            )
        } else {
            format!("Walk the checkout usages of {term} from the changed symbol.")
        },
        stops,
    })
}

fn push_focus_term(
    terms: &mut Vec<ReviewFocusTerm>,
    seen: &mut BTreeSet<String>,
    term: String,
    kind: ReviewFocusKind,
) {
    let normalized = term.trim();
    if normalized.len() < 3 {
        return;
    }

    let key = normalized.to_ascii_lowercase();
    if !seen.insert(key) {
        return;
    }

    terms.push(ReviewFocusTerm {
        term: normalized.to_string(),
        kind,
    });
}

fn preferred_focus_term(focus_terms: &[ReviewFocusTerm]) -> Option<&ReviewFocusTerm> {
    focus_terms
        .iter()
        .find(|term| term.kind == ReviewFocusKind::Table)
        .or_else(|| {
            focus_terms
                .iter()
                .find(|term| term.kind == ReviewFocusKind::Symbol)
        })
        .or_else(|| focus_terms.first())
}

fn section_changed_lines(
    section: &SemanticDiffSection,
    parsed_file: Option<&ParsedDiffFile>,
) -> Vec<String> {
    let Some(parsed_file) = parsed_file else {
        return Vec::new();
    };

    section
        .hunk_indices
        .iter()
        .filter_map(|hunk_index| parsed_file.hunks.get(*hunk_index))
        .flat_map(|hunk| {
            hunk.lines.iter().filter_map(|line| {
                matches!(line.kind, DiffLineKind::Addition | DiffLineKind::Deletion)
                    .then_some(line.content.clone())
            })
        })
        .collect()
}

fn section_matches_focus(
    section: &SemanticDiffSection,
    parsed_file: Option<&ParsedDiffFile>,
    focus_lower: &str,
) -> bool {
    if section.title.to_ascii_lowercase().contains(focus_lower)
        || section.summary.to_ascii_lowercase().contains(focus_lower)
    {
        return true;
    }

    section_changed_lines(section, parsed_file)
        .into_iter()
        .any(|line| line.to_ascii_lowercase().contains(focus_lower))
}

fn symbol_candidate_from_title(title: &str) -> Option<String> {
    [
        "fn ",
        "struct ",
        "enum ",
        "trait ",
        "impl ",
        "type ",
        "class ",
        "interface ",
        "function ",
        "def ",
        "func ",
    ]
    .iter()
    .find_map(|pattern| title.trim().strip_prefix(pattern))
    .and_then(extract_identifier)
}

fn identifier_candidate_from_title(title: &str) -> Option<String> {
    title
        .split(|character: char| !character.is_alphanumeric() && character != '_')
        .filter(|token| token.len() >= 3 && token.chars().any(char::is_alphabetic))
        .find(|token| !is_common_identifier(token))
        .map(str::to_string)
}

fn identifier_candidate_from_line(line: &str) -> Option<String> {
    build_interactive_code_tokens(line)
        .into_iter()
        .map(|token| token.text)
        .find(|token| token.len() >= 4 && !is_common_identifier(token))
}

fn extract_identifier(value: &str) -> Option<String> {
    let trimmed = value.trim();
    let ident = trimmed
        .split(|character: char| {
            !character.is_alphanumeric() && character != '_' && character != '.'
        })
        .find(|token| !token.is_empty())?;

    Some(ident.trim_matches('.').to_string())
}

fn table_candidate_from_line(line: &str) -> Option<String> {
    let lower = line.to_ascii_lowercase();
    [
        " from ",
        " join ",
        " update ",
        " into ",
        " table ",
        " references ",
    ]
    .iter()
    .find_map(|pattern| {
        lower
            .find(pattern)
            .and_then(|index| extract_sql_identifier(&line[index + pattern.len()..]))
    })
}

fn extract_sql_identifier(value: &str) -> Option<String> {
    let trimmed = value
        .trim()
        .trim_start_matches(['"', '`', '\''])
        .trim_start_matches('(');
    let identifier = trimmed
        .chars()
        .take_while(|character| {
            character.is_alphanumeric()
                || matches!(character, '_' | '.' | '$')
                || *character == '"'
                || *character == '`'
        })
        .collect::<String>()
        .trim_matches(['"', '`'])
        .to_string();

    (!identifier.is_empty() && !is_common_identifier(&identifier)).then_some(identifier)
}

fn is_common_identifier(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "self"
            | "this"
            | "true"
            | "false"
            | "none"
            | "null"
            | "some"
            | "result"
            | "option"
            | "string"
            | "value"
            | "table"
            | "select"
            | "from"
            | "join"
            | "where"
            | "into"
    )
}

fn locate_term_in_prepared_file(
    prepared_file: &PreparedFileContent,
    anchor_line: usize,
    term: &str,
) -> Option<(usize, usize)> {
    for line_number in line_search_order(prepared_file.lines.len(), anchor_line, 12) {
        let line = prepared_file.lines.get(line_number - 1)?;
        let tokens = build_interactive_code_tokens(&line.text);
        if let Some(token) = tokens
            .iter()
            .find(|token| token_matches_focus_term(&token.text, term))
        {
            return Some((line_number, token.column_start));
        }
    }

    None
}

fn line_search_order(total_lines: usize, anchor_line: usize, radius: usize) -> Vec<usize> {
    let mut lines = Vec::new();
    if anchor_line == 0 || total_lines == 0 {
        return lines;
    }

    lines.push(anchor_line.min(total_lines));
    for offset in 1..=radius {
        if let Some(line) = anchor_line.checked_sub(offset).filter(|line| *line > 0) {
            lines.push(line);
        }
        let next = anchor_line + offset;
        if next <= total_lines {
            lines.push(next);
        }
    }

    lines
}

fn token_matches_focus_term(token: &str, term: &str) -> bool {
    token == term
        || token == term.rsplit("::").next().unwrap_or(term)
        || token == term.rsplit('.').next().unwrap_or(term)
}

fn diff_anchor_for_reference(
    detail: &PullRequestDetail,
    target: &LspReferenceTarget,
) -> DiffAnchor {
    let hunk_header = find_parsed_diff_file(&detail.parsed_diff, &target.path).and_then(|parsed| {
        parsed.hunks.iter().find_map(|hunk| {
            hunk.lines
                .iter()
                .any(|line| {
                    line.right_line_number == Some(target.line as i64)
                        || line.left_line_number == Some(target.line as i64)
                })
                .then(|| hunk.header.clone())
        })
    });

    DiffAnchor {
        file_path: target.path.clone(),
        hunk_header,
        line: Some(target.line as i64),
        side: None,
        thread_id: None,
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        diff::{ParsedDiffFile, ParsedDiffHunk, ParsedDiffLine},
        github::{PullRequestDetail, PullRequestFile},
        semantic_diff::{SemanticChangeKind, SemanticDiffSection},
    };

    use super::{
        build_changed_touch_route, build_section_symbol_focus, collect_section_focus_terms,
        ReviewFocusKind,
    };

    fn sample_section() -> SemanticDiffSection {
        SemanticDiffSection {
            id: "src/db.rs::0".to_string(),
            title: "fn load_accounts".to_string(),
            kind: SemanticChangeKind::Logic,
            summary: "logic in fn load_accounts with +2 / -0.".to_string(),
            additions: 2,
            deletions: 0,
            thread_count: 0,
            line_count: 2,
            anchor: Some(crate::code_tour::DiffAnchor {
                file_path: "src/db.rs".to_string(),
                hunk_header: Some("@@ -1,1 +1,2 @@ fn load_accounts".to_string()),
                line: Some(2),
                side: None,
                thread_id: None,
            }),
            hunk_indices: vec![0],
        }
    }

    #[test]
    fn collects_symbol_and_table_focus_terms() {
        let parsed = ParsedDiffFile {
            path: "src/db.rs".to_string(),
            previous_path: None,
            is_binary: false,
            hunks: vec![ParsedDiffHunk {
                header: "@@ -1,1 +1,2 @@ fn load_accounts".to_string(),
                lines: vec![ParsedDiffLine {
                    prefix: "+".to_string(),
                    kind: crate::diff::DiffLineKind::Addition,
                    content: "SELECT * FROM accounts".to_string(),
                    left_line_number: None,
                    right_line_number: Some(2),
                }],
            }],
        };

        let terms = collect_section_focus_terms(&sample_section(), Some(&parsed));

        assert_eq!(terms[0].term, "load_accounts");
        assert_eq!(terms[0].kind, ReviewFocusKind::Symbol);
        assert!(terms.iter().any(|term| term.term == "accounts"));
    }

    #[test]
    fn builds_symbol_focus_from_prepared_file() {
        let parsed = ParsedDiffFile {
            path: "src/db.rs".to_string(),
            previous_path: None,
            is_binary: false,
            hunks: vec![],
        };
        let prepared = crate::state::PreparedFileContent {
            path: "src/db.rs".to_string(),
            reference: "HEAD".to_string(),
            is_binary: false,
            size_bytes: 20,
            text: "fn load_accounts() {}".into(),
            lines: std::sync::Arc::new(vec![crate::state::PreparedFileLine {
                line_number: 1,
                text: "fn load_accounts() {}".to_string(),
                spans: Vec::new(),
            }]),
        };
        let mut section = sample_section();
        section.anchor.as_mut().unwrap().line = Some(1);

        let focus = build_section_symbol_focus(&section, Some(&parsed), Some(&prepared)).unwrap();

        assert_eq!(focus.term, "load_accounts");
        assert_eq!(focus.line, 1);
    }

    #[test]
    fn builds_changed_touch_route_in_file_order() {
        let detail = PullRequestDetail {
            id: "pr".to_string(),
            repository: "acme/api".to_string(),
            number: 1,
            title: "PR".to_string(),
            body: String::new(),
            url: String::new(),
            author_login: "octocat".to_string(),
            author_avatar_url: None,
            state: "OPEN".to_string(),
            is_draft: false,
            review_decision: None,
            base_ref_name: "main".to_string(),
            head_ref_name: "feature".to_string(),
            base_ref_oid: None,
            head_ref_oid: None,
            additions: 10,
            deletions: 0,
            changed_files: 2,
            comments_count: 0,
            commits_count: 1,
            created_at: String::new(),
            updated_at: String::new(),
            labels: Vec::new(),
            reviewers: Vec::new(),
            reviewer_avatar_urls: std::collections::BTreeMap::new(),
            comments: Vec::new(),
            latest_reviews: Vec::new(),
            review_threads: Vec::new(),
            files: vec![
                PullRequestFile {
                    path: "src/db.rs".to_string(),
                    additions: 4,
                    deletions: 0,
                    change_type: "MODIFIED".to_string(),
                },
                PullRequestFile {
                    path: "src/routes.rs".to_string(),
                    additions: 6,
                    deletions: 0,
                    change_type: "MODIFIED".to_string(),
                },
            ],
            raw_diff: String::new(),
            parsed_diff: vec![
                ParsedDiffFile {
                    path: "src/db.rs".to_string(),
                    previous_path: None,
                    is_binary: false,
                    hunks: vec![ParsedDiffHunk {
                        header: "@@ -1,1 +1,2 @@ fn load_accounts".to_string(),
                        lines: vec![ParsedDiffLine {
                            prefix: "+".to_string(),
                            kind: crate::diff::DiffLineKind::Addition,
                            content: "SELECT * FROM accounts".to_string(),
                            left_line_number: None,
                            right_line_number: Some(2),
                        }],
                    }],
                },
                ParsedDiffFile {
                    path: "src/routes.rs".to_string(),
                    previous_path: None,
                    is_binary: false,
                    hunks: vec![ParsedDiffHunk {
                        header: "@@ -1,1 +1,2 @@ fn route".to_string(),
                        lines: vec![ParsedDiffLine {
                            prefix: "+".to_string(),
                            kind: crate::diff::DiffLineKind::Addition,
                            content: "sqlx::query(\"select * from accounts\")".to_string(),
                            left_line_number: None,
                            right_line_number: Some(2),
                        }],
                    }],
                },
            ],
            data_completeness: crate::github::PullRequestDataCompleteness::default(),
        };

        let route = build_changed_touch_route(
            &detail,
            "src/db.rs",
            Some(&sample_section()),
            &[super::ReviewFocusTerm {
                term: "accounts".to_string(),
                kind: ReviewFocusKind::Table,
            }],
        )
        .unwrap();

        assert_eq!(route.stops.len(), 2);
        assert_eq!(route.stops[1].file_path, "src/routes.rs");
    }
}
