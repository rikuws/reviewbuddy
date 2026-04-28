use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    process::Command,
};

use crate::{
    code_tour::DiffAnchor,
    diff::{find_parsed_diff_file, DiffLineKind, ParsedDiffFile},
    github::PullRequestDetail,
    lsp::{LspReferenceTarget, LspSymbolDetails},
    review_session::ReviewLocation,
    semantic_diff::{build_semantic_diff_file, SemanticDiffSection},
};

const MAX_CHANGED_DEPENDENCY_NEIGHBORS: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewGraphNodeKind {
    File,
    Function,
    Method,
    Type,
    Module,
    Data,
    Branch,
    Unknown,
}

impl ReviewGraphNodeKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Function => "function",
            Self::Method => "method",
            Self::Type => "type",
            Self::Module => "module",
            Self::Data => "variable",
            Self::Branch => "branch",
            Self::Unknown => "symbol",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReviewGraphNodeState {
    Focus,
    Modified,
    Impacted,
}

impl ReviewGraphNodeState {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Focus => "focus",
            Self::Modified => "modified",
            Self::Impacted => "impacted",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReviewGraphEdgeKind {
    Calls,
    Uses,
    Defines,
    Inherits,
    Composes,
    DataFlow,
    Touches,
}

impl ReviewGraphEdgeKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Calls => "calls",
            Self::Uses => "uses",
            Self::Defines => "definition",
            Self::Inherits => "inherits",
            Self::Composes => "composes",
            Self::DataFlow => "data flow",
            Self::Touches => "same diff",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ReviewSymbolGraphNode {
    pub id: String,
    pub label: String,
    pub subtitle: String,
    pub kind: ReviewGraphNodeKind,
    pub state: ReviewGraphNodeState,
    pub location: ReviewLocation,
    pub in_diff: bool,
}

#[derive(Clone, Debug)]
pub struct ReviewSymbolGraphEdge {
    pub from: String,
    pub to: String,
    pub kind: ReviewGraphEdgeKind,
}

#[derive(Clone, Debug, Default)]
pub struct ReviewSymbolGraph {
    pub headline: String,
    pub summary: String,
    pub focus_node_id: Option<String>,
    pub focus_term: Option<String>,
    pub nodes: Vec<ReviewSymbolGraphNode>,
    pub edges: Vec<ReviewSymbolGraphEdge>,
    pub modified_count: usize,
    pub impacted_count: usize,
}

#[derive(Clone, Debug)]
pub struct ReviewSymbolEvolutionEntry {
    pub oid: String,
    pub short_oid: String,
    pub title: String,
    pub committed_at: String,
    pub status_label: String,
    pub additions: usize,
    pub deletions: usize,
    pub preview: String,
    pub touches_focus: bool,
}

#[derive(Clone, Debug, Default)]
pub struct ReviewSymbolEvolutionTimeline {
    pub headline: String,
    pub summary: String,
    pub focus_term: Option<String>,
    pub entries: Vec<ReviewSymbolEvolutionEntry>,
}

#[derive(Clone, Debug, Default)]
pub struct ReviewSymbolEvolutionState {
    pub loading: bool,
    pub timeline: Option<ReviewSymbolEvolutionTimeline>,
    pub error: Option<String>,
}

#[derive(Clone, Debug)]
struct SymbolDescriptor {
    label: String,
    term: Option<String>,
    kind: ReviewGraphNodeKind,
}

#[derive(Clone, Debug)]
struct GraphSectionCandidate {
    id: String,
    path: String,
    title: String,
    descriptor: SymbolDescriptor,
    location: ReviewLocation,
    changed_lines: Vec<String>,
    body_lines: Vec<String>,
    anchor_line: Option<usize>,
    changed: bool,
}

pub fn build_review_symbol_graph(
    detail: &PullRequestDetail,
    selected_file_path: &str,
    selected_file_text: Option<&str>,
    selected_section: Option<&SemanticDiffSection>,
    focus_override: Option<&str>,
    lsp_details: Option<&LspSymbolDetails>,
) -> ReviewSymbolGraph {
    let Some(_selected_file) = detail
        .files
        .iter()
        .find(|file| file.path == selected_file_path)
    else {
        return ReviewSymbolGraph {
            headline: "Symbol graph".to_string(),
            summary: "Select a changed file to inspect symbol relationships.".to_string(),
            ..ReviewSymbolGraph::default()
        };
    };

    let candidates = build_graph_candidates(detail, selected_file_path, selected_file_text);

    let mut nodes = BTreeMap::<String, ReviewSymbolGraphNode>::new();
    let mut edges = BTreeSet::<(String, String, ReviewGraphEdgeKind)>::new();

    let mut selected_candidates = candidates
        .iter()
        .filter(|candidate| {
            candidate.path == selected_file_path
                && is_review_graph_symbol_kind(candidate.descriptor.kind)
        })
        .cloned()
        .collect::<Vec<_>>();

    selected_candidates.sort_by(|left, right| {
        left.anchor_line
            .unwrap_or(usize::MAX)
            .cmp(&right.anchor_line.unwrap_or(usize::MAX))
            .then_with(|| left.descriptor.label.cmp(&right.descriptor.label))
    });
    selected_candidates.dedup_by(|left, right| left.id == right.id);

    let selected_ids = selected_candidates
        .iter()
        .map(|candidate| candidate.id.clone())
        .collect::<BTreeSet<_>>();
    let mut changed_dependency_neighbors = 0usize;

    let trace_candidate =
        selected_trace_candidate(selected_section, focus_override, &selected_candidates);

    for candidate in &selected_candidates {
        let state = if trace_candidate.is_some_and(|focus| focus.id == candidate.id) {
            ReviewGraphNodeState::Focus
        } else if candidate.changed {
            ReviewGraphNodeState::Modified
        } else {
            ReviewGraphNodeState::Impacted
        };
        insert_candidate_node(&mut nodes, candidate, state, candidate.changed);
    }

    for source in &selected_candidates {
        for target in candidates.iter().filter(|target| target.id != source.id) {
            let Some(target_term) = target.descriptor.term.as_deref() else {
                continue;
            };
            if !is_review_graph_symbol_kind(target.descriptor.kind) {
                continue;
            }
            if !candidate_mentions(source, target_term) {
                continue;
            }

            let target_is_selected = selected_ids.contains(&target.id);
            if !target_is_selected
                && !insert_changed_dependency_node(
                    &mut nodes,
                    target,
                    &mut changed_dependency_neighbors,
                )
            {
                continue;
            }

            edges.insert((
                source.id.clone(),
                target.id.clone(),
                infer_edge_kind(
                    candidate_dependency_lines(source),
                    target_term,
                    target.descriptor.kind,
                ),
            ));
        }
    }

    for source in candidates
        .iter()
        .filter(|candidate| candidate.path != selected_file_path)
    {
        for target in &selected_candidates {
            let Some(target_term) = target.descriptor.term.as_deref() else {
                continue;
            };
            if !is_review_graph_symbol_kind(source.descriptor.kind) {
                continue;
            }
            if !candidate_mentions(source, target_term) {
                continue;
            }

            if !insert_changed_dependency_node(
                &mut nodes,
                source,
                &mut changed_dependency_neighbors,
            ) {
                continue;
            }

            edges.insert((
                source.id.clone(),
                target.id.clone(),
                infer_edge_kind(
                    candidate_dependency_lines(source),
                    target_term,
                    target.descriptor.kind,
                ),
            ));
        }
    }

    let trace_term = trace_candidate
        .and_then(|candidate| candidate.descriptor.term.clone())
        .or_else(|| focus_override.map(str::to_string));

    if let (Some(lsp_details), Some(trace_candidate), Some(trace_term)) =
        (lsp_details, trace_candidate, trace_term.as_deref())
    {
        attach_lsp_neighbors(
            trace_candidate,
            trace_term,
            lsp_details,
            &candidates,
            &mut nodes,
            &mut edges,
        );
    }

    let file_symbol_count = nodes
        .values()
        .filter(|node| {
            node.kind != ReviewGraphNodeKind::File
                && is_review_graph_symbol_kind(node.kind)
                && node.subtitle == selected_file_path
        })
        .count();
    let changed_file_symbol_count = nodes
        .values()
        .filter(|node| {
            node.in_diff
                && node.kind != ReviewGraphNodeKind::File
                && node.subtitle == selected_file_path
        })
        .count();
    let changed_dependency_count = nodes
        .values()
        .filter(|node| {
            node.in_diff
                && node.kind != ReviewGraphNodeKind::File
                && node.subtitle != selected_file_path
        })
        .count();
    let impacted_count = nodes
        .values()
        .filter(|node| !node.in_diff && node.subtitle != selected_file_path)
        .count();
    let modified_count = nodes
        .values()
        .filter(|node| node.in_diff && node.kind != ReviewGraphNodeKind::File)
        .count();

    let summary = match (
        file_symbol_count,
        changed_file_symbol_count,
        changed_dependency_count,
        impacted_count,
    ) {
        (0, _, _, _) => format!(
            "{} has no function or variable symbols available. The graph only shows functions, methods, and variables.",
            file_name(selected_file_path)
        ),
        (_, _, 0, 0) => format!(
            "{file_symbol_count} function/variable symbol{} in this file; {changed_file_symbol_count} changed. Edges show direct calls and variable uses inferred from symbol bodies.",
            if file_symbol_count == 1 { "" } else { "s" }
        ),
        (_, _, _, 0) => format!(
            "{file_symbol_count} function/variable symbol{} in this file; {changed_file_symbol_count} changed, plus {changed_dependency_count} changed one-hop call/dependency target{} in the PR.",
            if file_symbol_count == 1 { "" } else { "s" },
            if changed_dependency_count == 1 { "" } else { "s" }
        ),
        _ => format!(
            "{file_symbol_count} function/variable symbol{} in this file; {changed_file_symbol_count} changed, plus {changed_dependency_count} changed PR target{} and {impacted_count} source call site{}.",
            if file_symbol_count == 1 { "" } else { "s" },
            if changed_dependency_count == 1 { "" } else { "s" },
            if impacted_count == 1 { "" } else { "s" }
        ),
    };

    let focus_node_id = trace_candidate
        .map(|candidate| candidate.id.clone())
        .or_else(|| {
            selected_candidates
                .first()
                .map(|candidate| candidate.id.clone())
        });

    let mut nodes = nodes.into_values().collect::<Vec<_>>();
    nodes.sort_by(|left, right| {
        left.state
            .cmp(&right.state)
            .then_with(|| left.in_diff.cmp(&right.in_diff).reverse())
            .then_with(|| left.subtitle.cmp(&right.subtitle))
            .then_with(|| left.label.cmp(&right.label))
    });

    let mut edges = edges
        .into_iter()
        .map(|(from, to, kind)| ReviewSymbolGraphEdge { from, to, kind })
        .collect::<Vec<_>>();
    edges.sort_by(|left, right| {
        left.kind
            .cmp(&right.kind)
            .then_with(|| left.from.cmp(&right.from))
            .then_with(|| left.to.cmp(&right.to))
    });

    ReviewSymbolGraph {
        headline: format!("{} call/dependency graph", file_name(selected_file_path)),
        summary,
        focus_node_id,
        focus_term: trace_term,
        nodes,
        edges,
        modified_count,
        impacted_count,
    }
}

fn insert_candidate_node(
    nodes: &mut BTreeMap<String, ReviewSymbolGraphNode>,
    candidate: &GraphSectionCandidate,
    state: ReviewGraphNodeState,
    in_diff: bool,
) {
    nodes
        .entry(candidate.id.clone())
        .or_insert_with(|| ReviewSymbolGraphNode {
            id: candidate.id.clone(),
            label: candidate.descriptor.label.clone(),
            subtitle: candidate.path.clone(),
            kind: candidate.descriptor.kind,
            state,
            location: candidate.location.clone(),
            in_diff,
        });
}

fn insert_changed_dependency_node(
    nodes: &mut BTreeMap<String, ReviewSymbolGraphNode>,
    candidate: &GraphSectionCandidate,
    changed_dependency_neighbors: &mut usize,
) -> bool {
    if nodes.contains_key(&candidate.id) {
        return true;
    }

    if *changed_dependency_neighbors >= MAX_CHANGED_DEPENDENCY_NEIGHBORS {
        return false;
    }

    insert_candidate_node(nodes, candidate, ReviewGraphNodeState::Modified, true);
    *changed_dependency_neighbors += 1;
    true
}

fn is_review_graph_symbol_kind(kind: ReviewGraphNodeKind) -> bool {
    matches!(
        kind,
        ReviewGraphNodeKind::Function | ReviewGraphNodeKind::Method | ReviewGraphNodeKind::Data
    )
}

fn selected_trace_candidate<'a>(
    selected_section: Option<&SemanticDiffSection>,
    focus_override: Option<&str>,
    selected_candidates: &'a [GraphSectionCandidate],
) -> Option<&'a GraphSectionCandidate> {
    selected_section
        .and_then(|section| {
            selected_candidates
                .iter()
                .find(|candidate| candidate.id == section.id)
        })
        .or_else(|| {
            focus_override.and_then(|focus| {
                selected_candidates
                    .iter()
                    .find(|candidate| candidate_matches_focus(candidate, focus))
            })
        })
        .or_else(|| {
            selected_candidates
                .iter()
                .find(|candidate| candidate.changed)
        })
        .or_else(|| selected_candidates.first())
}

fn candidate_matches_focus(candidate: &GraphSectionCandidate, focus: &str) -> bool {
    candidate
        .descriptor
        .term
        .as_deref()
        .map(|term| token_matches_term(term, focus) || token_matches_term(focus, term))
        .unwrap_or(false)
        || token_matches_term(&candidate.descriptor.label, focus)
        || mentions_term(&candidate.title, focus)
}

fn candidate_mentions(candidate: &GraphSectionCandidate, term: &str) -> bool {
    mentions_term(&candidate.title, term)
        || candidate
            .changed_lines
            .iter()
            .any(|line| mentions_term(line, term))
        || candidate
            .body_lines
            .iter()
            .any(|line| mentions_term(line, term))
}

fn candidate_dependency_lines(candidate: &GraphSectionCandidate) -> &[String] {
    if candidate.body_lines.is_empty() {
        &candidate.changed_lines
    } else {
        &candidate.body_lines
    }
}

pub fn load_symbol_evolution_timeline(
    repo_root: &Path,
    base_oid: &str,
    head_oid: &str,
    file_path: &str,
    focus_term: Option<&str>,
    limit: usize,
) -> Result<ReviewSymbolEvolutionTimeline, String> {
    let commit_output = run_git(
        repo_root,
        vec![
            "rev-list".to_string(),
            format!("{base_oid}..{head_oid}"),
            "--".to_string(),
            file_path.to_string(),
        ],
    )?;
    let mut commits = commit_output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(str::trim)
        .map(str::to_string)
        .collect::<Vec<_>>();

    commits.truncate(limit.max(1));
    commits.reverse();

    let focus_lower = focus_term.map(|term| term.to_ascii_lowercase());
    let mut entries = Vec::new();

    for oid in commits {
        let show_output = run_git(
            repo_root,
            vec![
                "show".to_string(),
                "--no-ext-diff".to_string(),
                "--no-color".to_string(),
                "--unified=0".to_string(),
                "--format=%H%x1f%h%x1f%s%x1f%cI".to_string(),
                oid.clone(),
                "--".to_string(),
                file_path.to_string(),
            ],
        )?;
        let mut lines = show_output.lines();
        let header = lines.next().unwrap_or_default();
        let mut header_parts = header.split('\u{1f}');
        let full_oid = header_parts.next().unwrap_or_default().to_string();
        let short_oid = header_parts.next().unwrap_or_default().to_string();
        let title = header_parts.next().unwrap_or_default().to_string();
        let committed_at = header_parts.next().unwrap_or_default().to_string();
        let patch = lines.collect::<Vec<_>>().join("\n");
        let additions = patch
            .lines()
            .filter(|line| line.starts_with('+') && !line.starts_with("+++"))
            .count();
        let deletions = patch
            .lines()
            .filter(|line| line.starts_with('-') && !line.starts_with("---"))
            .count();
        let touches_focus = focus_lower
            .as_deref()
            .map(|focus_lower| patch.to_ascii_lowercase().contains(focus_lower))
            .unwrap_or(true);

        entries.push(ReviewSymbolEvolutionEntry {
            oid: full_oid,
            short_oid,
            title,
            committed_at,
            status_label: classify_evolution_entry(&patch, focus_term, touches_focus),
            additions,
            deletions,
            preview: evolution_preview(&patch, focus_term),
            touches_focus,
        });
    }

    let focus_hits = entries.iter().filter(|entry| entry.touches_focus).count();
    Ok(ReviewSymbolEvolutionTimeline {
        headline: focus_term
            .map(|term| format!("{term} across related diffs"))
            .unwrap_or_else(|| format!("{} across related diffs", file_name(file_path))),
        summary: if let Some(focus_term) = focus_term {
            format!(
                "{focus_hits} of {} file-touching commit{} mention {}.",
                entries.len(),
                if entries.len() == 1 { "" } else { "s" },
                focus_term
            )
        } else {
            format!(
                "{} commit{} touch {} in this stack.",
                entries.len(),
                if entries.len() == 1 { "" } else { "s" },
                file_name(file_path)
            )
        },
        focus_term: focus_term.map(str::to_string),
        entries,
    })
}

fn attach_lsp_neighbors(
    focus_candidate: &GraphSectionCandidate,
    focus_term: &str,
    lsp_details: &LspSymbolDetails,
    candidates: &[GraphSectionCandidate],
    nodes: &mut BTreeMap<String, ReviewSymbolGraphNode>,
    edges: &mut BTreeSet<(String, String, ReviewGraphEdgeKind)>,
) {
    for target in lsp_details.definition_targets.iter().take(2) {
        let Some(candidate) = candidate_for_reference(candidates, target)
            .filter(|candidate| is_review_graph_symbol_kind(candidate.descriptor.kind))
        else {
            continue;
        };
        if candidate.id == focus_candidate.id {
            continue;
        }

        insert_candidate_node(
            nodes,
            candidate,
            if candidate.changed {
                ReviewGraphNodeState::Modified
            } else {
                ReviewGraphNodeState::Impacted
            },
            candidate.changed,
        );
        edges.insert((
            focus_candidate.id.clone(),
            candidate.id.clone(),
            ReviewGraphEdgeKind::Defines,
        ));
    }

    for target in &lsp_details.reference_targets {
        let Some(candidate) = candidate_for_reference(candidates, target)
            .filter(|candidate| is_review_graph_symbol_kind(candidate.descriptor.kind))
        else {
            continue;
        };

        if candidate.id == focus_candidate.id {
            continue;
        }

        insert_candidate_node(
            nodes,
            candidate,
            if candidate.changed {
                ReviewGraphNodeState::Modified
            } else {
                ReviewGraphNodeState::Impacted
            },
            candidate.changed,
        );

        edges.insert((
            candidate.id.clone(),
            focus_candidate.id.clone(),
            infer_edge_kind(
                candidate_dependency_lines(candidate),
                focus_term,
                focus_candidate.descriptor.kind,
            ),
        ));
    }
}

fn build_graph_candidates(
    detail: &PullRequestDetail,
    selected_file_path: &str,
    selected_file_text: Option<&str>,
) -> Vec<GraphSectionCandidate> {
    let mut candidates = Vec::new();

    for file in &detail.files {
        let parsed = find_parsed_diff_file(&detail.parsed_diff, &file.path);
        let selected_file_symbols = if file.path == selected_file_path {
            selected_file_text
                .map(|text| build_file_symbol_candidates(&file.path, text, parsed))
                .unwrap_or_default()
        } else {
            Vec::new()
        };

        if !selected_file_symbols.is_empty() {
            candidates.extend(selected_file_symbols);
            continue;
        }

        let semantic = build_semantic_diff_file(file, parsed, &detail.review_threads);

        for section in &semantic.sections {
            let descriptor = extract_symbol_descriptor(&section.title)
                .or_else(|| extract_symbol_descriptor_from_changed_lines(section, parsed));
            let Some(descriptor) = descriptor else {
                continue;
            };

            candidates.push(GraphSectionCandidate {
                id: section.id.clone(),
                path: file.path.clone(),
                title: section.title.clone(),
                descriptor,
                location: section_location(&file.path, section.anchor.as_ref()),
                changed_lines: section_changed_lines(section, parsed),
                body_lines: section_body_lines(section, parsed),
                anchor_line: anchor_line(section.anchor.as_ref()),
                changed: true,
            });
        }
    }

    candidates
}

fn build_file_symbol_candidates(
    path: &str,
    text: &str,
    parsed_file: Option<&ParsedDiffFile>,
) -> Vec<GraphSectionCandidate> {
    let lines = text.lines().map(str::to_string).collect::<Vec<_>>();
    let changed_lines_by_number = changed_lines_by_number(parsed_file);
    let mut symbols = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| {
            extract_symbol_descriptor_from_source_line(line).and_then(|descriptor| {
                is_review_graph_symbol_kind(descriptor.kind).then_some((
                    index + 1,
                    line,
                    descriptor,
                ))
            })
        })
        .collect::<Vec<_>>();

    symbols.sort_by_key(|(line_number, _, _)| *line_number);
    let mut candidates = Vec::new();

    for (index, (line_number, title, descriptor)) in symbols.iter().enumerate() {
        let end_line = symbols
            .get(index + 1)
            .map(|(next_line, _, _)| next_line.saturating_sub(1))
            .unwrap_or(lines.len())
            .max(*line_number);
        let body_lines = lines
            .get(line_number.saturating_sub(1)..end_line)
            .unwrap_or_default()
            .to_vec();
        let changed_lines = changed_lines_by_number
            .iter()
            .filter(|(changed_line, _)| {
                **changed_line >= *line_number && **changed_line <= end_line
            })
            .flat_map(|(_, lines)| lines.clone())
            .collect::<Vec<_>>();
        let anchor = DiffAnchor {
            file_path: path.to_string(),
            hunk_header: hunk_header_for_line(parsed_file, *line_number),
            line: Some(*line_number as i64),
            side: Some("RIGHT".to_string()),
            thread_id: None,
        };

        candidates.push(GraphSectionCandidate {
            id: format!("{}::symbol:{}:{}", path, descriptor.label, line_number),
            path: path.to_string(),
            title: title.trim().to_string(),
            descriptor: descriptor.clone(),
            location: ReviewLocation::from_diff(path.to_string(), Some(anchor)),
            changed_lines,
            body_lines,
            anchor_line: Some(*line_number),
            changed: !changed_lines_by_number.is_empty()
                && changed_lines_by_number
                    .keys()
                    .any(|changed_line| *changed_line >= *line_number && *changed_line <= end_line),
        });
    }

    candidates
}

fn extract_symbol_descriptor_from_changed_lines(
    section: &SemanticDiffSection,
    parsed_file: Option<&ParsedDiffFile>,
) -> Option<SymbolDescriptor> {
    section_changed_lines(section, parsed_file)
        .iter()
        .find_map(|line| extract_symbol_descriptor_from_source_line(line))
}

fn extract_symbol_descriptor_from_source_line(line: &str) -> Option<SymbolDescriptor> {
    let descriptor = extract_symbol_descriptor(line)?;
    if descriptor.kind == ReviewGraphNodeKind::Data && source_line_indent(line) > 0 {
        return None;
    }

    Some(descriptor)
}

fn extract_symbol_descriptor(title: &str) -> Option<SymbolDescriptor> {
    let normalized = strip_leading_modifiers(strip_line_comment(title.trim()));
    if normalized.is_empty() {
        return None;
    }

    if let Some(descriptor) = extract_variable_symbol_descriptor(normalized) {
        return Some(descriptor);
    }

    let candidates = [
        ("fn ", ReviewGraphNodeKind::Function),
        ("function ", ReviewGraphNodeKind::Function),
        ("def ", ReviewGraphNodeKind::Function),
        ("func ", ReviewGraphNodeKind::Function),
        ("fun ", ReviewGraphNodeKind::Function),
        ("struct ", ReviewGraphNodeKind::Type),
        ("enum ", ReviewGraphNodeKind::Type),
        ("trait ", ReviewGraphNodeKind::Type),
        ("class ", ReviewGraphNodeKind::Type),
        ("interface ", ReviewGraphNodeKind::Type),
        ("type ", ReviewGraphNodeKind::Type),
        ("impl ", ReviewGraphNodeKind::Method),
        ("mod ", ReviewGraphNodeKind::Module),
        ("module ", ReviewGraphNodeKind::Module),
        ("match ", ReviewGraphNodeKind::Branch),
        ("switch ", ReviewGraphNodeKind::Branch),
        ("if ", ReviewGraphNodeKind::Branch),
        ("guard ", ReviewGraphNodeKind::Branch),
    ];

    for (prefix, kind) in candidates {
        if let Some(rest) = normalized.strip_prefix(prefix) {
            let symbol = if prefix == "func " {
                extract_go_function_symbol_token(rest)
            } else {
                extract_symbol_token(rest)
            };
            return Some(SymbolDescriptor {
                label: symbol.clone().unwrap_or_else(|| title.trim().to_string()),
                term: symbol,
                kind,
            });
        }
    }

    if let Some(symbol) = extract_method_signature_symbol(normalized) {
        return Some(SymbolDescriptor {
            label: symbol.clone(),
            term: Some(symbol),
            kind: ReviewGraphNodeKind::Method,
        });
    }

    if normalized.contains("::") || normalized.contains('.') {
        let symbol = extract_symbol_token(normalized);
        return Some(SymbolDescriptor {
            label: symbol.clone().unwrap_or_else(|| title.trim().to_string()),
            term: symbol.clone(),
            kind: if normalized.contains("::") || normalized.contains('.') {
                ReviewGraphNodeKind::Method
            } else {
                ReviewGraphNodeKind::Unknown
            },
        });
    }

    None
}

fn source_line_indent(line: &str) -> usize {
    line.chars()
        .take_while(|ch| ch.is_ascii_whitespace())
        .map(|ch| if ch == '\t' { 4 } else { 1 })
        .sum()
}

fn strip_line_comment(value: &str) -> &str {
    value
        .split_once("//")
        .map(|(before, _)| before.trim_end())
        .unwrap_or(value)
}

fn strip_leading_modifiers(mut value: &str) -> &str {
    loop {
        let next = value
            .strip_prefix("pub ")
            .or_else(|| value.strip_prefix("pub(crate) "))
            .or_else(|| value.strip_prefix("pub(super) "))
            .or_else(|| value.strip_prefix("export "))
            .or_else(|| value.strip_prefix("default "))
            .or_else(|| value.strip_prefix("private "))
            .or_else(|| value.strip_prefix("protected "))
            .or_else(|| value.strip_prefix("public "))
            .or_else(|| value.strip_prefix("async "))
            .or_else(|| value.strip_prefix("unsafe "))
            .or_else(|| value.strip_prefix("static "))
            .or_else(|| value.strip_prefix("final "))
            .or_else(|| value.strip_prefix("override "))
            .or_else(|| value.strip_prefix("open "))
            .or_else(|| value.strip_prefix("internal "))
            .or_else(|| value.strip_prefix("sealed "))
            .or_else(|| value.strip_prefix("abstract "));
        let Some(next) = next else {
            break;
        };
        value = next.trim_start();
    }
    value
}

fn extract_variable_symbol_descriptor(value: &str) -> Option<SymbolDescriptor> {
    let rest = value
        .strip_prefix("const ")
        .or_else(|| value.strip_prefix("let "))
        .or_else(|| value.strip_prefix("var "))?;
    let symbol = extract_symbol_token(rest)?;
    let after_symbol = rest.get(symbol.len()..).unwrap_or_default();
    let kind = if after_symbol.contains("=>")
        || after_symbol.contains("function")
        || after_symbol.contains("async")
        || after_symbol.trim_start().starts_with('=')
            && after_symbol.contains('(')
            && after_symbol.contains(')')
    {
        ReviewGraphNodeKind::Function
    } else {
        ReviewGraphNodeKind::Data
    };

    Some(SymbolDescriptor {
        label: symbol.clone(),
        term: Some(symbol),
        kind,
    })
}

fn extract_go_function_symbol_token(value: &str) -> Option<String> {
    let trimmed = value.trim_start();
    if trimmed.starts_with('(') {
        let after_receiver = trimmed.split_once(')')?.1.trim_start();
        return extract_symbol_token(after_receiver);
    }

    extract_symbol_token(trimmed)
}

fn extract_method_signature_symbol(value: &str) -> Option<String> {
    let trimmed = strip_leading_modifiers(value.trim_start());
    let open_paren = trimmed.find('(')?;
    let before_paren = trimmed[..open_paren].trim_end();
    if before_paren.is_empty()
        || before_paren.contains('=')
        || before_paren.contains("=>")
        || before_paren.contains(' ')
            && !before_paren
                .split_whitespace()
                .last()
                .map(|token| is_identifier_like(token))
                .unwrap_or(false)
    {
        return None;
    }

    let name = before_paren
        .split_whitespace()
        .last()
        .unwrap_or(before_paren)
        .trim_matches(|ch: char| !is_symbol_token_char(ch));
    if name.is_empty() || is_control_or_call_keyword(name) {
        return None;
    }

    let after_paren = trimmed[open_paren..].trim_end();
    if !(after_paren.contains('{')
        || after_paren.ends_with(':')
        || after_paren.contains("=>")
        || after_paren.contains("):")
        || after_paren.contains(") ->"))
    {
        return None;
    }

    Some(name.to_string())
}

fn is_identifier_like(value: &str) -> bool {
    let mut chars = value.chars();
    chars
        .next()
        .map(|ch| ch.is_ascii_alphabetic() || ch == '_')
        .unwrap_or(false)
        && chars.all(is_symbol_token_char)
}

fn is_symbol_token_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | ':' | '.' | '<' | '>' | '-')
}

fn is_control_or_call_keyword(value: &str) -> bool {
    matches!(
        value,
        "if" | "for"
            | "while"
            | "switch"
            | "catch"
            | "return"
            | "await"
            | "throw"
            | "new"
            | "else"
            | "do"
            | "match"
            | "guard"
            | "with"
    )
}

fn extract_symbol_token(value: &str) -> Option<String> {
    let token = value
        .chars()
        .take_while(|ch| is_symbol_token_char(*ch))
        .collect::<String>()
        .trim_matches(|ch: char| matches!(ch, '<' | '>' | '-' | ':'))
        .trim()
        .to_string();

    (!token.is_empty()).then_some(token)
}

fn section_location(path: &str, anchor: Option<&DiffAnchor>) -> ReviewLocation {
    ReviewLocation::from_diff(path.to_string(), anchor.cloned())
}

fn anchor_line(anchor: Option<&DiffAnchor>) -> Option<usize> {
    anchor
        .and_then(|anchor| anchor.line)
        .and_then(|line| usize::try_from(line).ok())
        .filter(|line| *line > 0)
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
        .filter_map(|index| parsed_file.hunks.get(*index))
        .flat_map(|hunk| {
            hunk.lines.iter().filter_map(|line| {
                matches!(line.kind, DiffLineKind::Addition | DiffLineKind::Deletion)
                    .then(|| line.content.clone())
            })
        })
        .collect()
}

fn section_body_lines(
    section: &SemanticDiffSection,
    parsed_file: Option<&ParsedDiffFile>,
) -> Vec<String> {
    let Some(parsed_file) = parsed_file else {
        return Vec::new();
    };

    section
        .hunk_indices
        .iter()
        .filter_map(|index| parsed_file.hunks.get(*index))
        .flat_map(|hunk| {
            hunk.lines
                .iter()
                .filter(|line| line.kind != DiffLineKind::Meta)
                .map(|line| line.content.clone())
        })
        .collect()
}

fn changed_lines_by_number(parsed_file: Option<&ParsedDiffFile>) -> BTreeMap<usize, Vec<String>> {
    let mut lines = BTreeMap::<usize, Vec<String>>::new();
    let Some(parsed_file) = parsed_file else {
        return lines;
    };

    for hunk in &parsed_file.hunks {
        for line in &hunk.lines {
            if !matches!(line.kind, DiffLineKind::Addition | DiffLineKind::Deletion) {
                continue;
            }

            let line_number = line
                .right_line_number
                .or(line.left_line_number)
                .and_then(|line| usize::try_from(line).ok())
                .filter(|line| *line > 0);
            let Some(line_number) = line_number else {
                continue;
            };

            lines
                .entry(line_number)
                .or_default()
                .push(line.content.clone());
        }
    }

    lines
}

fn hunk_header_for_line(
    parsed_file: Option<&ParsedDiffFile>,
    line_number: usize,
) -> Option<String> {
    let line_number = i64::try_from(line_number).ok()?;
    parsed_file.and_then(|parsed_file| {
        parsed_file.hunks.iter().find_map(|hunk| {
            hunk.lines
                .iter()
                .any(|line| {
                    line.right_line_number == Some(line_number)
                        || line.left_line_number == Some(line_number)
                })
                .then(|| hunk.header.clone())
        })
    })
}

fn infer_edge_kind(
    changed_lines: &[String],
    term: &str,
    target_kind: ReviewGraphNodeKind,
) -> ReviewGraphEdgeKind {
    let lower_term = term.to_ascii_lowercase();
    let lower_lines = changed_lines
        .iter()
        .map(|line| line.to_ascii_lowercase())
        .collect::<Vec<_>>();

    if lower_lines.iter().any(|line| {
        line.contains(&format!("{lower_term}("))
            || line.contains(&format!("{lower_term}::"))
            || line.contains(&format!("{lower_term}."))
    }) {
        return ReviewGraphEdgeKind::Calls;
    }

    if lower_lines.iter().any(|line| {
        line.contains(&format!("extends {lower_term}"))
            || line.contains(&format!("implements {lower_term}"))
            || line.contains(&format!(": {lower_term}"))
    }) && target_kind == ReviewGraphNodeKind::Type
    {
        return ReviewGraphEdgeKind::Inherits;
    }

    if lower_lines.iter().any(|line| {
        line.contains(&format!(" {lower_term} "))
            || line.contains(&format!("<{lower_term}>"))
            || line.contains(&format!("({lower_term})"))
    }) && matches!(
        target_kind,
        ReviewGraphNodeKind::Type | ReviewGraphNodeKind::Data
    ) {
        return ReviewGraphEdgeKind::Composes;
    }

    if lower_lines.iter().any(|line| {
        line.contains('=')
            || line.contains("return ")
            || line.contains("yield ")
            || line.contains("await ")
    }) {
        return ReviewGraphEdgeKind::DataFlow;
    }

    ReviewGraphEdgeKind::Uses
}

fn mentions_term(text: &str, term: &str) -> bool {
    text.split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == ':' || ch == '.'))
        .filter(|token| !token.is_empty())
        .any(|token| token_matches_term(token, term))
}

fn token_matches_term(token: &str, term: &str) -> bool {
    token.eq_ignore_ascii_case(term)
        || token.eq_ignore_ascii_case(term.rsplit("::").next().unwrap_or(term))
        || token.eq_ignore_ascii_case(term.rsplit('.').next().unwrap_or(term))
}

fn candidate_for_reference<'a>(
    candidates: &'a [GraphSectionCandidate],
    target: &LspReferenceTarget,
) -> Option<&'a GraphSectionCandidate> {
    candidates
        .iter()
        .filter(|candidate| candidate.path == target.path)
        .min_by_key(|candidate| {
            candidate
                .anchor_line
                .map(|line| line.abs_diff(target.line))
                .unwrap_or(usize::MAX)
        })
}

fn classify_evolution_entry(patch: &str, focus_term: Option<&str>, touches_focus: bool) -> String {
    let Some(focus_term) = focus_term else {
        return "file diff".to_string();
    };
    let lower_focus = focus_term.to_ascii_lowercase();
    let added = patch
        .lines()
        .filter(|line| line.starts_with('+') && !line.starts_with("+++"))
        .any(|line| line.to_ascii_lowercase().contains(&lower_focus));
    let removed = patch
        .lines()
        .filter(|line| line.starts_with('-') && !line.starts_with("---"))
        .any(|line| line.to_ascii_lowercase().contains(&lower_focus));

    match (touches_focus, added, removed) {
        (false, _, _) => "neighbor diff".to_string(),
        (true, true, true) => "reshaped".to_string(),
        (true, true, false) => "introduced".to_string(),
        (true, false, true) => "removed".to_string(),
        (true, false, false) => "touched".to_string(),
    }
}

fn evolution_preview(patch: &str, focus_term: Option<&str>) -> String {
    let focus_lower = focus_term.map(|term| term.to_ascii_lowercase());

    for line in patch.lines() {
        if line.starts_with("@@") {
            return line.to_string();
        }

        if line.starts_with('+') || line.starts_with('-') {
            let matches_focus = focus_lower
                .as_deref()
                .map(|focus_lower| line.to_ascii_lowercase().contains(focus_lower))
                .unwrap_or(true);
            if matches_focus {
                return line.chars().take(120).collect();
            }
        }
    }

    "No focused hunk preview.".to_string()
}

fn run_git(repo_root: &Path, args: Vec<String>) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(&args)
        .output()
        .map_err(|error| format!("Failed to launch git: {error}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        return Err(if !stderr.is_empty() {
            stderr
        } else if !stdout.is_empty() {
            stdout
        } else {
            format!("git {:?} failed", args)
        });
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn file_name(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

#[cfg(test)]
mod tests {
    use super::{
        build_review_symbol_graph, extract_symbol_descriptor, ReviewGraphEdgeKind,
        ReviewGraphNodeKind, ReviewGraphNodeState, ReviewSymbolGraph,
    };
    use crate::{
        diff::parse_unified_diff,
        github::{PullRequestDetail, PullRequestFile},
        lsp::{LspDefinitionTarget, LspSymbolDetails},
    };

    #[test]
    fn extracts_function_descriptor() {
        let descriptor = extract_symbol_descriptor("pub async fn render_graph_panel")
            .expect("expected descriptor");
        assert_eq!(descriptor.label, "render_graph_panel");
        assert_eq!(descriptor.kind, ReviewGraphNodeKind::Function);
    }

    #[test]
    fn builds_selected_file_dependency_network() {
        let raw_diff = r#"diff --git a/src/orders.rs b/src/orders.rs
index 1111111..2222222 100644
--- a/src/orders.rs
+++ b/src/orders.rs
@@ -1,5 +1,7 @@ pub fn place_order(order: Order) {
 pub fn place_order(order: Order) {
+    validate_order(order);
+    send_receipt(order);
 }
@@ -5,4 +5,5 @@ pub fn validate_order(order: Order) {
 pub fn validate_order(order: Order) {
+    check_inventory(order);
 }
diff --git a/src/receipts.rs b/src/receipts.rs
index 3333333..4444444 100644
--- a/src/receipts.rs
+++ b/src/receipts.rs
@@ -1,3 +1,4 @@ pub fn send_receipt(order: Order) {
 pub fn send_receipt(order: Order) {
+    place_order(order);
 }
"#;
        let detail = detail_with_diff(raw_diff);
        let selected_file_text = r#"pub fn place_order(order: Order) {
    validate_order(order);
    send_receipt(order);
}

pub fn validate_order(order: Order) {
    check_inventory(order);
}
"#;

        let graph = build_review_symbol_graph(
            &detail,
            "src/orders.rs",
            Some(selected_file_text),
            None,
            None,
            None,
        );

        assert_eq!(graph.headline, "orders.rs call/dependency graph");
        assert!(
            graph
                .nodes
                .iter()
                .all(|node| node.kind != ReviewGraphNodeKind::File),
            "graph should not use the filename as a node"
        );

        assert_node(&graph, "place_order", true);
        assert_node(&graph, "validate_order", true);
        assert_node(&graph, "send_receipt", true);
        assert_focus_node(&graph, "place_order");
        assert_edge(
            &graph,
            "place_order",
            "validate_order",
            ReviewGraphEdgeKind::Calls,
        );
        assert_edge(
            &graph,
            "place_order",
            "send_receipt",
            ReviewGraphEdgeKind::Calls,
        );
        assert_edge(
            &graph,
            "send_receipt",
            "place_order",
            ReviewGraphEdgeKind::Calls,
        );
    }

    #[test]
    fn limits_graph_nodes_to_functions_methods_and_variables() {
        let raw_diff = r#"diff --git a/src/orders.rs b/src/orders.rs
index 1111111..2222222 100644
--- a/src/orders.rs
+++ b/src/orders.rs
@@ -1,9 +1,11 @@
 struct Order;
 const TAX_RATE: f32 = 0.2;
 pub fn place_order(order: Order) {
+    let local_total = TAX_RATE;
+    apply_tax(local_total);
 }
 pub fn apply_tax(total: f32) -> f32 {
     total
 }
"#;
        let detail = detail_with_diff(raw_diff);
        let selected_file_text = r#"struct Order;
const TAX_RATE: f32 = 0.2;

pub fn place_order(order: Order) {
    let local_total = TAX_RATE;
    apply_tax(local_total);
}

pub fn apply_tax(total: f32) -> f32 {
    total
}
"#;

        let graph = build_review_symbol_graph(
            &detail,
            "src/orders.rs",
            Some(selected_file_text),
            None,
            None,
            None,
        );

        assert_node(&graph, "TAX_RATE", false);
        assert_node(&graph, "place_order", true);
        assert_node(&graph, "apply_tax", false);
        assert_no_node(&graph, "Order");
        assert_no_node(&graph, "local_total");
        assert!(
            graph.nodes.iter().all(|node| matches!(
                node.kind,
                ReviewGraphNodeKind::Function
                    | ReviewGraphNodeKind::Method
                    | ReviewGraphNodeKind::Data
            )),
            "graph should only contain function, method, and variable nodes"
        );
    }

    #[test]
    fn lsp_references_do_not_create_line_nodes() {
        let raw_diff = r#"diff --git a/src/orders.rs b/src/orders.rs
index 1111111..2222222 100644
--- a/src/orders.rs
+++ b/src/orders.rs
@@ -1,3 +1,4 @@ pub fn place_order(order: Order) {
 pub fn place_order(order: Order) {
+    validate_order(order);
 }
"#;
        let detail = detail_with_diff(raw_diff);
        let selected_file_text = r#"pub fn place_order(order: Order) {
    validate_order(order);
}
"#;
        let lsp_details = LspSymbolDetails {
            reference_targets: vec![LspDefinitionTarget {
                uri: "file:///repo/src/external.rs".to_string(),
                path: "src/external.rs".to_string(),
                line: 42,
                column: 8,
            }],
            ..LspSymbolDetails::default()
        };

        let graph = build_review_symbol_graph(
            &detail,
            "src/orders.rs",
            Some(selected_file_text),
            None,
            Some("place_order"),
            Some(&lsp_details),
        );

        assert!(
            graph
                .nodes
                .iter()
                .all(|node| node.kind != ReviewGraphNodeKind::Unknown),
            "LSP references should be mapped to symbol nodes or omitted"
        );
        assert_no_node(&graph, "reference line 42");
    }

    fn assert_node(graph: &ReviewSymbolGraph, label: &str, in_diff: bool) {
        let node = graph
            .nodes
            .iter()
            .find(|node| node.label == label)
            .unwrap_or_else(|| panic!("missing node {label}"));
        assert_eq!(node.in_diff, in_diff);
    }

    fn assert_no_node(graph: &ReviewSymbolGraph, label: &str) {
        assert!(
            graph.nodes.iter().all(|node| node.label != label),
            "unexpected node {label}"
        );
    }

    fn assert_focus_node(graph: &ReviewSymbolGraph, label: &str) {
        let node = graph
            .nodes
            .iter()
            .find(|node| node.label == label)
            .unwrap_or_else(|| panic!("missing node {label}"));
        assert_eq!(node.state, ReviewGraphNodeState::Focus);
        assert_eq!(graph.focus_node_id.as_deref(), Some(node.id.as_str()));
    }

    fn assert_edge(
        graph: &ReviewSymbolGraph,
        from_label: &str,
        to_label: &str,
        kind: ReviewGraphEdgeKind,
    ) {
        let from_id = graph
            .nodes
            .iter()
            .find(|node| node.label == from_label)
            .unwrap_or_else(|| panic!("missing from node {from_label}"))
            .id
            .clone();
        let to_id = graph
            .nodes
            .iter()
            .find(|node| node.label == to_label)
            .unwrap_or_else(|| panic!("missing to node {to_label}"))
            .id
            .clone();
        assert!(
            graph
                .edges
                .iter()
                .any(|edge| edge.from == from_id && edge.to == to_id && edge.kind == kind),
            "missing {kind:?} edge from {from_label} to {to_label}"
        );
    }

    fn detail_with_diff(raw_diff: &str) -> PullRequestDetail {
        PullRequestDetail {
            id: "PR_1".to_string(),
            repository: "owner/repo".to_string(),
            number: 1,
            title: "Test PR".to_string(),
            body: String::new(),
            url: String::new(),
            author_login: "author".to_string(),
            author_avatar_url: None,
            state: "OPEN".to_string(),
            is_draft: false,
            review_decision: None,
            base_ref_name: "main".to_string(),
            head_ref_name: "feature".to_string(),
            base_ref_oid: None,
            head_ref_oid: None,
            additions: 4,
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
                    path: "src/orders.rs".to_string(),
                    additions: 3,
                    deletions: 0,
                    change_type: "MODIFIED".to_string(),
                },
                PullRequestFile {
                    path: "src/receipts.rs".to_string(),
                    additions: 1,
                    deletions: 0,
                    change_type: "MODIFIED".to_string(),
                },
            ],
            raw_diff: raw_diff.to_string(),
            parsed_diff: parse_unified_diff(raw_diff),
            data_completeness: crate::github::PullRequestDataCompleteness::default(),
        }
    }
}
