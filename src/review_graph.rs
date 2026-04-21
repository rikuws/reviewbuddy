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
    semantic_diff::{build_semantic_diff_file, SemanticChangeKind, SemanticDiffSection},
};

const MAX_CHANGED_NEIGHBORS: usize = 10;
const MAX_EXTERNAL_NEIGHBORS: usize = 5;

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
            Self::Data => "data",
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
            Self::Touches => "touches",
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
    anchor_line: Option<usize>,
}

pub fn build_review_symbol_graph(
    detail: &PullRequestDetail,
    selected_file_path: &str,
    selected_section: Option<&SemanticDiffSection>,
    focus_override: Option<&str>,
    lsp_details: Option<&LspSymbolDetails>,
) -> ReviewSymbolGraph {
    let Some(selected_file) = detail
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

    let selected_parsed = find_parsed_diff_file(&detail.parsed_diff, selected_file_path);
    let selected_semantic =
        build_semantic_diff_file(selected_file, selected_parsed, &detail.review_threads);
    let candidates = build_graph_candidates(detail, selected_file_path);

    let focus_candidate = selected_section
        .and_then(|section| {
            candidates
                .iter()
                .find(|candidate| candidate.id == section.id)
                .cloned()
        })
        .or_else(|| {
            candidates
                .iter()
                .find(|candidate| candidate.path == selected_file_path)
                .cloned()
        })
        .or_else(|| {
            selected_semantic
                .sections
                .first()
                .map(|section| GraphSectionCandidate {
                    id: section.id.clone(),
                    path: selected_file_path.to_string(),
                    title: section.title.clone(),
                    descriptor: fallback_descriptor(section),
                    location: section_location(selected_file_path, section.anchor.as_ref()),
                    changed_lines: section_changed_lines(section, selected_parsed),
                    anchor_line: anchor_line(section.anchor.as_ref()),
                })
        })
        .unwrap_or_else(|| GraphSectionCandidate {
            id: format!("{selected_file_path}::file"),
            path: selected_file_path.to_string(),
            title: selected_file_path.to_string(),
            descriptor: SymbolDescriptor {
                label: file_name(selected_file_path),
                term: None,
                kind: ReviewGraphNodeKind::File,
            },
            location: ReviewLocation::from_diff(selected_file_path.to_string(), None),
            changed_lines: Vec::new(),
            anchor_line: None,
        });

    let focus_label = focus_override
        .map(str::to_string)
        .unwrap_or_else(|| focus_candidate.descriptor.label.clone());
    let focus_node = ReviewSymbolGraphNode {
        id: focus_candidate.id.clone(),
        label: focus_label.clone(),
        subtitle: focus_candidate.path.clone(),
        kind: focus_candidate.descriptor.kind,
        state: ReviewGraphNodeState::Focus,
        location: focus_candidate.location.clone(),
        in_diff: true,
    };

    let focus_term = focus_override
        .map(str::to_string)
        .or_else(|| focus_candidate.descriptor.term.clone())
        .or_else(|| lsp_details.and_then(|_| Some(focus_label.clone())));

    let mut nodes = BTreeMap::<String, ReviewSymbolGraphNode>::new();
    let mut edges = BTreeSet::<(String, String, ReviewGraphEdgeKind)>::new();
    nodes.insert(focus_node.id.clone(), focus_node.clone());

    let mut changed_neighbors = 0usize;
    for candidate in candidates
        .iter()
        .filter(|candidate| candidate.id != focus_candidate.id)
    {
        let mut edge_kind = None;

        if let Some(candidate_term) = candidate.descriptor.term.as_deref() {
            if mentions_term(&focus_candidate.title, candidate_term)
                || focus_candidate
                    .changed_lines
                    .iter()
                    .any(|line| mentions_term(line, candidate_term))
            {
                edge_kind = Some(infer_edge_kind(
                    &focus_candidate.changed_lines,
                    candidate_term,
                    candidate.descriptor.kind,
                ));
                edges.insert((
                    focus_candidate.id.clone(),
                    candidate.id.clone(),
                    edge_kind.unwrap(),
                ));
            }
        }

        if let Some(focus_term) = focus_term.as_deref() {
            if mentions_term(&candidate.title, focus_term)
                || candidate
                    .changed_lines
                    .iter()
                    .any(|line| mentions_term(line, focus_term))
            {
                let kind = infer_edge_kind(
                    &candidate.changed_lines,
                    focus_term,
                    focus_candidate.descriptor.kind,
                );
                edges.insert((candidate.id.clone(), focus_candidate.id.clone(), kind));
                edge_kind = Some(kind);
            }
        }

        if edge_kind.is_none() {
            continue;
        }

        if changed_neighbors >= MAX_CHANGED_NEIGHBORS {
            continue;
        }

        nodes
            .entry(candidate.id.clone())
            .or_insert_with(|| ReviewSymbolGraphNode {
                id: candidate.id.clone(),
                label: candidate.descriptor.label.clone(),
                subtitle: candidate.path.clone(),
                kind: candidate.descriptor.kind,
                state: ReviewGraphNodeState::Modified,
                location: candidate.location.clone(),
                in_diff: true,
            });
        changed_neighbors += 1;
    }

    if edges.is_empty() {
        for candidate in candidates
            .iter()
            .filter(|candidate| {
                candidate.path == selected_file_path && candidate.id != focus_candidate.id
            })
            .take(MAX_CHANGED_NEIGHBORS)
        {
            nodes
                .entry(candidate.id.clone())
                .or_insert_with(|| ReviewSymbolGraphNode {
                    id: candidate.id.clone(),
                    label: candidate.descriptor.label.clone(),
                    subtitle: candidate.path.clone(),
                    kind: candidate.descriptor.kind,
                    state: ReviewGraphNodeState::Modified,
                    location: candidate.location.clone(),
                    in_diff: true,
                });
            edges.insert((
                focus_candidate.id.clone(),
                candidate.id.clone(),
                ReviewGraphEdgeKind::Touches,
            ));
        }
    }

    if let Some(lsp_details) = lsp_details {
        if let Some(focus_term) = focus_term.as_deref() {
            attach_lsp_neighbors(
                detail,
                &focus_candidate,
                focus_term,
                lsp_details,
                &candidates,
                &mut nodes,
                &mut edges,
            );
        }
    }

    let mut nodes = nodes.into_values().collect::<Vec<_>>();
    nodes.sort_by(|left, right| {
        left.state
            .cmp(&right.state)
            .then_with(|| left.in_diff.cmp(&right.in_diff).reverse())
            .then_with(|| left.label.cmp(&right.label))
            .then_with(|| left.subtitle.cmp(&right.subtitle))
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

    let modified_count = nodes
        .iter()
        .filter(|node| node.state == ReviewGraphNodeState::Modified)
        .count();
    let impacted_count = nodes
        .iter()
        .filter(|node| node.state == ReviewGraphNodeState::Impacted)
        .count();

    ReviewSymbolGraph {
        headline: if focus_candidate.descriptor.kind == ReviewGraphNodeKind::File {
            format!("Changed symbols in {}", file_name(selected_file_path))
        } else {
            format!("Graph around {focus_label}")
        },
        summary: if focus_term.is_some() {
            format!(
                "{modified_count} modified and {impacted_count} impacted neighbor{} around this focus.",
                if modified_count + impacted_count == 1 {
                    ""
                } else {
                    "s"
                }
            )
        } else {
            format!(
                "{} changed symbol{} in {}.",
                modified_count.max(nodes.len().saturating_sub(1)),
                if modified_count.max(nodes.len().saturating_sub(1)) == 1 {
                    ""
                } else {
                    "s"
                },
                file_name(selected_file_path)
            )
        },
        focus_node_id: Some(focus_candidate.id.clone()),
        focus_term,
        nodes,
        edges,
        modified_count,
        impacted_count,
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

pub fn review_location_for_reference(
    detail: &PullRequestDetail,
    target: &LspReferenceTarget,
) -> ReviewLocation {
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

    let is_changed = detail.files.iter().any(|file| file.path == target.path);
    if is_changed {
        ReviewLocation::from_diff(
            target.path.clone(),
            Some(DiffAnchor {
                file_path: target.path.clone(),
                hunk_header,
                line: Some(target.line as i64),
                side: None,
                thread_id: None,
            }),
        )
    } else {
        ReviewLocation::from_source(
            target.path.clone(),
            Some(target.line),
            Some("Impacted neighbor".to_string()),
        )
    }
}

fn attach_lsp_neighbors(
    detail: &PullRequestDetail,
    focus_candidate: &GraphSectionCandidate,
    focus_term: &str,
    lsp_details: &LspSymbolDetails,
    candidates: &[GraphSectionCandidate],
    nodes: &mut BTreeMap<String, ReviewSymbolGraphNode>,
    edges: &mut BTreeSet<(String, String, ReviewGraphEdgeKind)>,
) {
    for target in lsp_details.definition_targets.iter().take(2) {
        let location = review_location_for_reference(detail, target);
        let node_id = location.stable_key();
        let in_diff = detail.files.iter().any(|file| file.path == target.path);
        if node_id == focus_candidate.location.stable_key() {
            continue;
        }

        nodes
            .entry(node_id.clone())
            .or_insert_with(|| ReviewSymbolGraphNode {
                id: node_id.clone(),
                label: if in_diff {
                    display_changed_reference_label(candidates, target)
                } else {
                    file_name(&target.path)
                },
                subtitle: format!("{}:{}", target.path, target.line),
                kind: if in_diff {
                    candidate_kind_for_reference(candidates, target)
                } else {
                    ReviewGraphNodeKind::Unknown
                },
                state: if in_diff {
                    ReviewGraphNodeState::Modified
                } else {
                    ReviewGraphNodeState::Impacted
                },
                location,
                in_diff,
            });
        edges.insert((
            focus_candidate.id.clone(),
            node_id,
            ReviewGraphEdgeKind::Defines,
        ));
    }

    let mut external_neighbors = 0usize;
    for target in &lsp_details.reference_targets {
        let location = review_location_for_reference(detail, target);
        let node_id = if let Some(candidate) = candidate_for_reference(candidates, target) {
            candidate.id.clone()
        } else {
            location.stable_key()
        };

        if node_id == focus_candidate.id {
            continue;
        }

        let in_diff = detail.files.iter().any(|file| file.path == target.path);
        if !in_diff && external_neighbors >= MAX_EXTERNAL_NEIGHBORS {
            continue;
        }

        if !in_diff {
            external_neighbors += 1;
        }

        nodes.entry(node_id.clone()).or_insert_with(|| {
            if let Some(candidate) = candidate_for_reference(candidates, target) {
                ReviewSymbolGraphNode {
                    id: candidate.id.clone(),
                    label: candidate.descriptor.label.clone(),
                    subtitle: candidate.path.clone(),
                    kind: candidate.descriptor.kind,
                    state: ReviewGraphNodeState::Modified,
                    location: candidate.location.clone(),
                    in_diff: true,
                }
            } else {
                ReviewSymbolGraphNode {
                    id: node_id.clone(),
                    label: if in_diff {
                        file_name(&target.path)
                    } else {
                        file_name(&target.path)
                    },
                    subtitle: format!("{}:{}", target.path, target.line),
                    kind: ReviewGraphNodeKind::Unknown,
                    state: ReviewGraphNodeState::Impacted,
                    location,
                    in_diff,
                }
            }
        });

        let edge_kind = if in_diff {
            ReviewGraphEdgeKind::Calls
        } else {
            ReviewGraphEdgeKind::Uses
        };
        if mentions_reference_target(target, focus_term) {
            edges.insert((node_id, focus_candidate.id.clone(), edge_kind));
        } else {
            edges.insert((
                node_id,
                focus_candidate.id.clone(),
                ReviewGraphEdgeKind::Uses,
            ));
        }
    }
}

fn build_graph_candidates(
    detail: &PullRequestDetail,
    selected_file_path: &str,
) -> Vec<GraphSectionCandidate> {
    let mut candidates = Vec::new();

    for file in &detail.files {
        let parsed = find_parsed_diff_file(&detail.parsed_diff, &file.path);
        let semantic = build_semantic_diff_file(file, parsed, &detail.review_threads);

        for section in &semantic.sections {
            let descriptor = extract_symbol_descriptor(&section.title).or_else(|| {
                (file.path == selected_file_path).then(|| fallback_descriptor(section))
            });
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
                anchor_line: anchor_line(section.anchor.as_ref()),
            });
        }
    }

    candidates
}

fn extract_symbol_descriptor(title: &str) -> Option<SymbolDescriptor> {
    let normalized = strip_leading_modifiers(title.trim());
    let candidates = [
        ("fn ", ReviewGraphNodeKind::Function),
        ("struct ", ReviewGraphNodeKind::Type),
        ("enum ", ReviewGraphNodeKind::Type),
        ("trait ", ReviewGraphNodeKind::Type),
        ("class ", ReviewGraphNodeKind::Type),
        ("interface ", ReviewGraphNodeKind::Type),
        ("type ", ReviewGraphNodeKind::Type),
        ("impl ", ReviewGraphNodeKind::Method),
        ("mod ", ReviewGraphNodeKind::Module),
        ("module ", ReviewGraphNodeKind::Module),
        ("const ", ReviewGraphNodeKind::Data),
        ("let ", ReviewGraphNodeKind::Data),
        ("match ", ReviewGraphNodeKind::Branch),
        ("switch ", ReviewGraphNodeKind::Branch),
        ("if ", ReviewGraphNodeKind::Branch),
        ("guard ", ReviewGraphNodeKind::Branch),
    ];

    for (prefix, kind) in candidates {
        if let Some(rest) = normalized.strip_prefix(prefix) {
            let symbol = extract_symbol_token(rest);
            return Some(SymbolDescriptor {
                label: symbol.clone().unwrap_or_else(|| title.trim().to_string()),
                term: symbol,
                kind,
            });
        }
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

fn fallback_descriptor(section: &SemanticDiffSection) -> SymbolDescriptor {
    SymbolDescriptor {
        label: section.title.clone(),
        term: first_identifier(&section.title),
        kind: match section.kind {
            SemanticChangeKind::Type => ReviewGraphNodeKind::Type,
            SemanticChangeKind::DataFlow => ReviewGraphNodeKind::Branch,
            _ => ReviewGraphNodeKind::Unknown,
        },
    }
}

fn strip_leading_modifiers(mut value: &str) -> &str {
    loop {
        let next = value
            .strip_prefix("pub ")
            .or_else(|| value.strip_prefix("pub(crate) "))
            .or_else(|| value.strip_prefix("pub(super) "))
            .or_else(|| value.strip_prefix("async "))
            .or_else(|| value.strip_prefix("unsafe "))
            .or_else(|| value.strip_prefix("static "));
        let Some(next) = next else {
            break;
        };
        value = next.trim_start();
    }
    value
}

fn extract_symbol_token(value: &str) -> Option<String> {
    let token = value
        .chars()
        .take_while(|ch| {
            ch.is_ascii_alphanumeric() || matches!(ch, '_' | ':' | '.' | '<' | '>' | '-')
        })
        .collect::<String>()
        .trim_matches(|ch: char| matches!(ch, '<' | '>' | '-' | ':'))
        .trim()
        .to_string();

    (!token.is_empty()).then_some(token)
}

fn first_identifier(value: &str) -> Option<String> {
    value
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == ':' || ch == '.'))
        .find(|token| token.len() >= 3 && !token.chars().all(|ch| ch.is_ascii_digit()))
        .map(str::to_string)
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

fn candidate_kind_for_reference(
    candidates: &[GraphSectionCandidate],
    target: &LspReferenceTarget,
) -> ReviewGraphNodeKind {
    candidate_for_reference(candidates, target)
        .map(|candidate| candidate.descriptor.kind)
        .unwrap_or(ReviewGraphNodeKind::Unknown)
}

fn display_changed_reference_label(
    candidates: &[GraphSectionCandidate],
    target: &LspReferenceTarget,
) -> String {
    candidate_for_reference(candidates, target)
        .map(|candidate| candidate.descriptor.label.clone())
        .unwrap_or_else(|| file_name(&target.path))
}

fn mentions_reference_target(target: &LspReferenceTarget, focus_term: &str) -> bool {
    target
        .path
        .split('/')
        .any(|segment| token_matches_term(segment, focus_term))
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
    use super::{extract_symbol_descriptor, ReviewGraphNodeKind};

    #[test]
    fn extracts_function_descriptor() {
        let descriptor = extract_symbol_descriptor("pub async fn render_graph_panel")
            .expect("expected descriptor");
        assert_eq!(descriptor.label, "render_graph_panel");
        assert_eq!(descriptor.kind, ReviewGraphNodeKind::Function);
    }
}
