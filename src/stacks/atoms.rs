use std::collections::BTreeSet;

use sha1::{Digest, Sha1};

use crate::{
    diff::{DiffLineKind, ParsedDiffFile, ParsedDiffHunk},
    github::{PullRequestDetail, PullRequestFile, PullRequestReviewThread},
    semantic_diff::{build_semantic_diff_file, SemanticChangeKind, SemanticDiffSection},
};

use super::model::{
    ChangeAtom, ChangeAtomSource, ChangeRole, LineRange, StackWarning, STACK_GENERATOR_VERSION,
};

const HUGE_FILE_CHANGED_LINES: usize = 1_500;

pub fn extract_change_atoms(detail: &PullRequestDetail) -> Vec<ChangeAtom> {
    let mut atoms = Vec::new();

    for file in &detail.files {
        let parsed = crate::diff::find_parsed_diff_file(&detail.parsed_diff, &file.path);
        atoms.extend(extract_file_atoms(detail, file, parsed));
    }

    atoms
}

pub fn classify_change_role(path: &str, semantic_kind: Option<SemanticChangeKind>) -> ChangeRole {
    let lower = path.to_ascii_lowercase();
    if is_generated_path(&lower) {
        return ChangeRole::Generated;
    }
    if lower.ends_with(".md")
        || lower.ends_with(".rst")
        || lower.starts_with("docs/")
        || lower.contains("/docs/")
    {
        return ChangeRole::Docs;
    }
    if lower.contains("/tests/")
        || lower.starts_with("tests/")
        || lower.contains("/test/")
        || lower.contains("_test.")
        || lower.contains(".spec.")
        || lower.contains(".test.")
    {
        return ChangeRole::Tests;
    }
    if lower.ends_with(".toml")
        || lower.ends_with(".yaml")
        || lower.ends_with(".yml")
        || lower.ends_with(".json")
        || lower.ends_with(".lock")
        || lower.ends_with(".ini")
        || lower.ends_with(".config")
    {
        return ChangeRole::Config;
    }
    if lower.contains("migration")
        || lower.contains("schema")
        || lower.contains("model")
        || lower.contains("entity")
        || lower.contains("types")
        || lower.contains("domain")
        || matches!(semantic_kind, Some(SemanticChangeKind::Type))
    {
        return ChangeRole::Foundation;
    }
    if lower.contains("controller")
        || lower.contains("route")
        || lower.contains("api/")
        || lower.contains("/api")
        || lower.contains("adapter")
        || lower.contains("repository")
        || lower.contains("persistence")
        || lower.contains("client")
    {
        return ChangeRole::Integration;
    }
    if lower.contains("view")
        || lower.contains("component")
        || lower.contains("screen")
        || lower.contains("page")
        || lower.ends_with(".tsx")
        || lower.ends_with(".jsx")
        || lower.ends_with(".css")
        || lower.ends_with(".scss")
    {
        return ChangeRole::Presentation;
    }

    ChangeRole::CoreLogic
}

pub fn atom_patch_hash(path: &str, hunks: &[&ParsedDiffHunk]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(STACK_GENERATOR_VERSION.as_bytes());
    hasher.update(path.as_bytes());
    for hunk in hunks {
        hasher.update(hunk.header.as_bytes());
        for line in &hunk.lines {
            hasher.update(line.prefix.as_bytes());
            hasher.update(line.content.as_bytes());
            if let Some(line_number) = line.left_line_number {
                hasher.update(line_number.to_string().as_bytes());
            }
            if let Some(line_number) = line.right_line_number {
                hasher.update(line_number.to_string().as_bytes());
            }
        }
    }
    format!("{:x}", hasher.finalize())
}

fn extract_file_atoms(
    detail: &PullRequestDetail,
    file: &PullRequestFile,
    parsed: Option<&ParsedDiffFile>,
) -> Vec<ChangeAtom> {
    let semantic = build_semantic_diff_file(file, parsed, &detail.review_threads);
    let changed_lines = file.additions.saturating_add(file.deletions).max(0) as usize;
    let role = classify_change_role(&file.path, Some(semantic.file_kind));

    if parsed.map(|parsed| parsed.is_binary).unwrap_or(false) {
        return vec![placeholder_atom(
            detail,
            file,
            ChangeAtomSource::BinaryPlaceholder,
            role,
            "Binary file cannot be split into hunks.",
        )];
    }

    if is_generated_path(&file.path.to_ascii_lowercase())
        || changed_lines >= HUGE_FILE_CHANGED_LINES
    {
        return vec![placeholder_atom(
            detail,
            file,
            ChangeAtomSource::GeneratedPlaceholder,
            ChangeRole::Generated,
            "Generated or very large file is grouped for manual review.",
        )];
    }

    let Some(parsed) = parsed else {
        return vec![placeholder_atom(
            detail,
            file,
            ChangeAtomSource::File,
            role,
            "No parsed hunks were available; file is grouped as one atom.",
        )];
    };

    if parsed.hunks.is_empty() {
        return vec![placeholder_atom(
            detail,
            file,
            ChangeAtomSource::File,
            role,
            "No textual hunks were available; file is grouped as one atom.",
        )];
    }

    let mut atoms = Vec::new();
    let mut covered_hunks = BTreeSet::<usize>::new();

    for section in &semantic.sections {
        if section.hunk_indices.is_empty() {
            continue;
        }
        covered_hunks.extend(section.hunk_indices.iter().copied());
        atoms.push(section_atom(detail, file, parsed, section));
    }

    for (hunk_index, hunk) in parsed.hunks.iter().enumerate() {
        if covered_hunks.contains(&hunk_index) {
            continue;
        }
        atoms.push(hunk_atom(
            detail,
            file,
            hunk,
            hunk_index,
            semantic.file_kind,
        ));
    }

    atoms
}

fn section_atom(
    detail: &PullRequestDetail,
    file: &PullRequestFile,
    parsed: &ParsedDiffFile,
    section: &SemanticDiffSection,
) -> ChangeAtom {
    let hunks = section
        .hunk_indices
        .iter()
        .filter_map(|index| parsed.hunks.get(*index))
        .collect::<Vec<_>>();
    let patch_hash = atom_patch_hash(&file.path, &hunks);
    let role = classify_change_role(&file.path, Some(section.kind));
    let (defined_symbols, referenced_symbols) =
        extract_symbols(&hunks, Some(section.title.as_str()));
    let symbol_name = primary_symbol_name(section.title.as_str()).or_else(|| {
        defined_symbols
            .first()
            .map(|symbol| symbol.trim().to_string())
    });
    let (old_range, new_range) = atom_ranges(&hunks);
    let thread_ids = review_thread_ids_for_hunks(&detail.review_threads, &file.path, &hunks);
    let warnings = if section.line_count == 0 {
        vec![StackWarning::for_path(
            "empty-section",
            "Semantic section has no changed lines.",
            file.path.clone(),
        )]
    } else {
        Vec::new()
    };

    ChangeAtom {
        id: stable_atom_id(
            detail,
            file.path.as_str(),
            parsed.previous_path.as_deref(),
            &ChangeAtomSource::SemanticSection {
                section_id: section.id.clone(),
            },
            symbol_name.as_deref(),
            &section
                .hunk_indices
                .iter()
                .filter_map(|index| parsed.hunks.get(*index).map(|hunk| hunk.header.as_str()))
                .collect::<Vec<_>>()
                .join("\n"),
            patch_hash.as_str(),
        ),
        source: ChangeAtomSource::SemanticSection {
            section_id: section.id.clone(),
        },
        path: file.path.clone(),
        previous_path: parsed.previous_path.clone(),
        role,
        semantic_kind: Some(section.kind.label().to_string()),
        symbol_name,
        defined_symbols,
        referenced_symbols,
        old_range,
        new_range,
        hunk_headers: hunks.iter().map(|hunk| hunk.header.clone()).collect(),
        hunk_indices: section.hunk_indices.clone(),
        additions: section.additions,
        deletions: section.deletions,
        patch_hash,
        risk_score: atom_risk_score(role, section.additions, section.deletions, thread_ids.len()),
        review_thread_ids: thread_ids,
        warnings,
    }
}

fn hunk_atom(
    detail: &PullRequestDetail,
    file: &PullRequestFile,
    hunk: &ParsedDiffHunk,
    hunk_index: usize,
    file_kind: SemanticChangeKind,
) -> ChangeAtom {
    let hunks = vec![hunk];
    let patch_hash = atom_patch_hash(&file.path, &hunks);
    let role = classify_change_role(&file.path, Some(file_kind));
    let (defined_symbols, referenced_symbols) = extract_symbols(&hunks, None);
    let symbol_name = hunk
        .header
        .split("@@")
        .last()
        .and_then(primary_symbol_name)
        .or_else(|| defined_symbols.first().cloned());
    let thread_ids = review_thread_ids_for_hunks(&detail.review_threads, &file.path, &hunks);
    let additions = hunk
        .lines
        .iter()
        .filter(|line| line.kind == DiffLineKind::Addition)
        .count();
    let deletions = hunk
        .lines
        .iter()
        .filter(|line| line.kind == DiffLineKind::Deletion)
        .count();
    let source = ChangeAtomSource::Hunk { hunk_index };

    ChangeAtom {
        id: stable_atom_id(
            detail,
            file.path.as_str(),
            None,
            &source,
            symbol_name.as_deref(),
            hunk.header.as_str(),
            patch_hash.as_str(),
        ),
        source,
        path: file.path.clone(),
        previous_path: None,
        role,
        semantic_kind: Some(file_kind.label().to_string()),
        symbol_name,
        defined_symbols,
        referenced_symbols,
        old_range: line_range_for_hunk(hunk, false),
        new_range: line_range_for_hunk(hunk, true),
        hunk_headers: vec![hunk.header.clone()],
        hunk_indices: vec![hunk_index],
        additions,
        deletions,
        patch_hash,
        risk_score: atom_risk_score(role, additions, deletions, thread_ids.len()),
        review_thread_ids: thread_ids,
        warnings: Vec::new(),
    }
}

fn placeholder_atom(
    detail: &PullRequestDetail,
    file: &PullRequestFile,
    source: ChangeAtomSource,
    role: ChangeRole,
    warning: &str,
) -> ChangeAtom {
    let patch_hash = placeholder_hash(detail, file, &source);
    let additions = file.additions.max(0) as usize;
    let deletions = file.deletions.max(0) as usize;

    ChangeAtom {
        id: stable_atom_id(
            detail,
            file.path.as_str(),
            None,
            &source,
            None,
            file.path.as_str(),
            patch_hash.as_str(),
        ),
        source,
        path: file.path.clone(),
        previous_path: None,
        role,
        semantic_kind: None,
        symbol_name: None,
        defined_symbols: Vec::new(),
        referenced_symbols: Vec::new(),
        old_range: None,
        new_range: None,
        hunk_headers: Vec::new(),
        hunk_indices: Vec::new(),
        additions,
        deletions,
        patch_hash,
        risk_score: atom_risk_score(role, additions, deletions, 0),
        review_thread_ids: detail
            .review_threads
            .iter()
            .filter(|thread| thread.path == file.path && !thread.is_resolved)
            .map(|thread| thread.id.clone())
            .collect(),
        warnings: vec![StackWarning::for_path(
            "manual-review",
            warning.to_string(),
            file.path.clone(),
        )],
    }
}

fn stable_atom_id(
    detail: &PullRequestDetail,
    path: &str,
    previous_path: Option<&str>,
    source: &ChangeAtomSource,
    symbol_name: Option<&str>,
    hunk_key: &str,
    patch_hash: &str,
) -> String {
    let mut hasher = Sha1::new();
    for part in [
        detail.repository.as_str(),
        &detail.number.to_string(),
        detail.base_ref_oid.as_deref().unwrap_or_default(),
        detail.head_ref_oid.as_deref().unwrap_or_default(),
        path,
        previous_path.unwrap_or_default(),
        source.stable_kind(),
        symbol_name.unwrap_or_default(),
        &normalize_hunk_key(hunk_key),
        patch_hash,
    ] {
        hasher.update(part.as_bytes());
        hasher.update([0]);
    }
    format!("atom-{:x}", hasher.finalize())
}

fn placeholder_hash(
    detail: &PullRequestDetail,
    file: &PullRequestFile,
    source: &ChangeAtomSource,
) -> String {
    let mut hasher = Sha1::new();
    for part in [
        detail.repository.as_str(),
        &detail.number.to_string(),
        detail.base_ref_oid.as_deref().unwrap_or_default(),
        detail.head_ref_oid.as_deref().unwrap_or_default(),
        file.path.as_str(),
        source.stable_kind(),
        &file.additions.to_string(),
        &file.deletions.to_string(),
        &file.change_type,
    ] {
        hasher.update(part.as_bytes());
        hasher.update([0]);
    }
    format!("{:x}", hasher.finalize())
}

fn normalize_hunk_key(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn atom_ranges(hunks: &[&ParsedDiffHunk]) -> (Option<LineRange>, Option<LineRange>) {
    let mut left_numbers = Vec::new();
    let mut right_numbers = Vec::new();
    for hunk in hunks {
        for line in &hunk.lines {
            if let Some(number) = line.left_line_number {
                left_numbers.push(number);
            }
            if let Some(number) = line.right_line_number {
                right_numbers.push(number);
            }
        }
    }

    (
        range_from_numbers(&left_numbers),
        range_from_numbers(&right_numbers),
    )
}

fn line_range_for_hunk(hunk: &ParsedDiffHunk, right: bool) -> Option<LineRange> {
    let numbers = hunk
        .lines
        .iter()
        .filter_map(|line| {
            if right {
                line.right_line_number
            } else {
                line.left_line_number
            }
        })
        .collect::<Vec<_>>();
    range_from_numbers(&numbers)
}

fn range_from_numbers(numbers: &[i64]) -> Option<LineRange> {
    let start = numbers.iter().copied().min()?;
    let end = numbers.iter().copied().max()?;
    Some(LineRange { start, end })
}

fn review_thread_ids_for_hunks(
    threads: &[PullRequestReviewThread],
    path: &str,
    hunks: &[&ParsedDiffHunk],
) -> Vec<String> {
    threads
        .iter()
        .filter(|thread| {
            thread.path == path
                && !thread.is_resolved
                && hunks.iter().any(|hunk| {
                    thread_line_in_hunk(hunk, thread.line)
                        || thread_line_in_hunk(hunk, thread.original_line)
                })
        })
        .map(|thread| thread.id.clone())
        .collect()
}

fn thread_line_in_hunk(hunk: &ParsedDiffHunk, line_number: Option<i64>) -> bool {
    let Some(line_number) = line_number else {
        return false;
    };

    hunk.lines.iter().any(|line| {
        line.left_line_number == Some(line_number) || line.right_line_number == Some(line_number)
    })
}

fn atom_risk_score(
    role: ChangeRole,
    additions: usize,
    deletions: usize,
    unresolved_threads: usize,
) -> i64 {
    let mut score = (additions + deletions) as i64;
    score += unresolved_threads as i64 * 35;
    score += match role {
        ChangeRole::Foundation => 24,
        ChangeRole::CoreLogic => 18,
        ChangeRole::Integration => 16,
        ChangeRole::Presentation => 8,
        ChangeRole::Tests => 4,
        ChangeRole::Config => 12,
        ChangeRole::Docs => -10,
        ChangeRole::Generated => -8,
        ChangeRole::Unknown => 10,
    };
    score.max(0)
}

fn extract_symbols(hunks: &[&ParsedDiffHunk], title: Option<&str>) -> (Vec<String>, Vec<String>) {
    let mut defined = BTreeSet::<String>::new();
    let mut referenced = BTreeSet::<String>::new();

    if let Some(title) = title.and_then(primary_symbol_name) {
        defined.insert(title);
    }

    for hunk in hunks {
        for line in &hunk.lines {
            if !matches!(line.kind, DiffLineKind::Addition | DiffLineKind::Context) {
                continue;
            }
            if let Some(symbol) = declaration_symbol(line.content.as_str()) {
                defined.insert(symbol);
            }
            for token in identifier_tokens(line.content.as_str()) {
                referenced.insert(token);
            }
        }
    }

    for symbol in &defined {
        referenced.remove(symbol);
    }

    (
        defined.into_iter().take(64).collect(),
        referenced.into_iter().take(128).collect(),
    )
}

fn primary_symbol_name(value: &str) -> Option<String> {
    let trimmed = value
        .trim()
        .trim_start_matches("pub ")
        .trim_start_matches("async ")
        .trim_start_matches("export ")
        .trim_start_matches("default ");

    declaration_symbol(trimmed).or_else(|| {
        let token = trimmed
            .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == ':'))
            .find(|token| token.len() > 2 && !is_keyword(token))?;
        Some(token.to_string())
    })
}

fn declaration_symbol(line: &str) -> Option<String> {
    let trimmed = line.trim();
    for prefix in [
        "fn ",
        "struct ",
        "enum ",
        "trait ",
        "impl ",
        "mod ",
        "type ",
        "class ",
        "interface ",
        "function ",
        "def ",
        "func ",
        "const ",
        "let ",
    ] {
        let Some(rest) = trimmed.strip_prefix(prefix) else {
            continue;
        };
        let name = rest
            .trim()
            .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == ':'))
            .find(|segment| !segment.is_empty())?;
        if prefix == "impl " {
            return Some(format!("impl {name}"));
        }
        return Some(name.to_string());
    }
    None
}

fn identifier_tokens(line: &str) -> Vec<String> {
    line.split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == ':'))
        .filter(|token| token.len() >= 3)
        .filter(|token| !is_keyword(token))
        .map(str::to_string)
        .collect()
}

fn is_keyword(token: &str) -> bool {
    matches!(
        token,
        "the"
            | "and"
            | "for"
            | "let"
            | "var"
            | "const"
            | "return"
            | "match"
            | "switch"
            | "case"
            | "else"
            | "self"
            | "Self"
            | "true"
            | "false"
            | "null"
            | "None"
            | "Some"
            | "pub"
            | "use"
            | "mod"
            | "impl"
            | "fn"
            | "class"
            | "struct"
            | "enum"
            | "trait"
            | "type"
            | "async"
            | "await"
    )
}

fn is_generated_path(lower: &str) -> bool {
    lower.contains("/generated/")
        || lower.contains("/gen/")
        || lower.ends_with(".generated.ts")
        || lower.ends_with(".generated.rs")
        || lower.ends_with(".pb.go")
        || lower.ends_with(".pb.rs")
        || lower.ends_with(".min.js")
        || lower.ends_with(".snap")
}

#[cfg(test)]
mod tests {
    use crate::{
        diff::parse_unified_diff,
        github::{PullRequestDataCompleteness, PullRequestDetail, PullRequestFile},
    };

    use super::extract_change_atoms;

    #[test]
    fn extracts_one_atom_for_each_semantic_hunk() {
        let raw_diff = r#"diff --git a/src/model.rs b/src/model.rs
--- a/src/model.rs
+++ b/src/model.rs
@@ -1,3 +1,6 @@
+pub struct User {
+    id: String,
+}
 fn existing() {}
diff --git a/src/service.rs b/src/service.rs
--- a/src/service.rs
+++ b/src/service.rs
@@ -1,3 +1,5 @@ fn load()
 fn load() {
+    let user = User { id: "1".into() };
+    save(user);
 }
"#;
        let detail = PullRequestDetail {
            id: "pr".to_string(),
            repository: "acme/repo".to_string(),
            number: 1,
            title: "PR".to_string(),
            body: String::new(),
            url: String::new(),
            author_login: "octo".to_string(),
            author_avatar_url: None,
            state: "OPEN".to_string(),
            is_draft: false,
            review_decision: None,
            base_ref_name: "main".to_string(),
            head_ref_name: "feature".to_string(),
            base_ref_oid: Some("base".to_string()),
            head_ref_oid: Some("head".to_string()),
            additions: 5,
            deletions: 0,
            changed_files: 2,
            comments_count: 0,
            commits_count: 1,
            created_at: String::new(),
            updated_at: "now".to_string(),
            labels: Vec::new(),
            reviewers: Vec::new(),
            reviewer_avatar_urls: Default::default(),
            comments: Vec::new(),
            latest_reviews: Vec::new(),
            review_threads: Vec::new(),
            files: vec![
                PullRequestFile {
                    path: "src/model.rs".to_string(),
                    additions: 3,
                    deletions: 0,
                    change_type: "MODIFIED".to_string(),
                },
                PullRequestFile {
                    path: "src/service.rs".to_string(),
                    additions: 2,
                    deletions: 0,
                    change_type: "MODIFIED".to_string(),
                },
            ],
            raw_diff: raw_diff.to_string(),
            parsed_diff: parse_unified_diff(raw_diff),
            data_completeness: PullRequestDataCompleteness::default(),
        };

        let atoms = extract_change_atoms(&detail);

        assert_eq!(atoms.len(), 2);
        assert!(atoms.iter().all(|atom| !atom.id.is_empty()));
        assert_eq!(
            atoms
                .iter()
                .map(|atom| atom.hunk_indices.len())
                .sum::<usize>(),
            2
        );
    }
}
