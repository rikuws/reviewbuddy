use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::{cache::CacheStore, code_tour::DiffAnchor};

const REVIEW_SESSION_CACHE_KEY_PREFIX: &str = "review-session-v2";
const MAX_WAYMARKS: usize = 16;
const MAX_ROUTE_LOCATIONS: usize = 24;
const MAX_HISTORY_LOCATIONS: usize = 48;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ReviewCenterMode {
    #[default]
    SemanticDiff,
    SourceBrowser,
}

impl ReviewCenterMode {
    pub fn label(&self) -> &'static str {
        match self {
            Self::SemanticDiff => "Semantic diff",
            Self::SourceBrowser => "Source browser",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ReviewInspectorMode {
    #[default]
    Graph,
    Context,
    Evolution,
}

impl ReviewInspectorMode {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Graph => "Graph",
            Self::Context => "Context",
            Self::Evolution => "Evolution",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewSourceTarget {
    pub path: String,
    pub line: Option<usize>,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewLocation {
    pub label: String,
    pub file_path: String,
    #[serde(default)]
    pub anchor: Option<DiffAnchor>,
    #[serde(default)]
    pub mode: ReviewCenterMode,
    #[serde(default)]
    pub source_line: Option<usize>,
    #[serde(default)]
    pub source_reason: Option<String>,
}

impl ReviewLocation {
    pub fn from_diff(file_path: impl Into<String>, anchor: Option<DiffAnchor>) -> Self {
        let file_path = file_path.into();
        let line = anchor
            .as_ref()
            .and_then(|anchor| anchor.line)
            .and_then(|line| usize::try_from(line).ok())
            .filter(|line| *line > 0);

        Self {
            label: location_label(&file_path, line),
            file_path,
            anchor,
            mode: ReviewCenterMode::SemanticDiff,
            source_line: None,
            source_reason: None,
        }
    }

    pub fn from_source(
        file_path: impl Into<String>,
        line: Option<usize>,
        reason: Option<String>,
    ) -> Self {
        let file_path = file_path.into();
        let label = location_label(&file_path, line);

        Self {
            label,
            file_path,
            anchor: None,
            mode: ReviewCenterMode::SourceBrowser,
            source_line: line.filter(|line| *line > 0),
            source_reason: reason,
        }
    }

    pub fn as_source_target(&self) -> Option<ReviewSourceTarget> {
        (self.mode == ReviewCenterMode::SourceBrowser).then(|| ReviewSourceTarget {
            path: self.file_path.clone(),
            line: self.source_line,
            reason: self.source_reason.clone(),
        })
    }

    pub fn stable_key(&self) -> String {
        match self.mode {
            ReviewCenterMode::SemanticDiff => format!(
                "diff:{}:{}:{}:{}",
                self.file_path,
                self.anchor
                    .as_ref()
                    .and_then(|anchor| anchor.hunk_header.as_deref())
                    .unwrap_or(""),
                self.anchor
                    .as_ref()
                    .and_then(|anchor| anchor.line)
                    .unwrap_or_default(),
                self.anchor
                    .as_ref()
                    .and_then(|anchor| anchor.thread_id.as_deref())
                    .unwrap_or(""),
            ),
            ReviewCenterMode::SourceBrowser => format!(
                "source:{}:{}",
                self.file_path,
                self.source_line.unwrap_or_default(),
            ),
        }
    }

    pub fn same_spot_as(&self, other: &Self) -> bool {
        self.stable_key() == other.stable_key()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewWaymark {
    pub id: String,
    pub name: String,
    pub location: ReviewLocation,
    #[serde(default)]
    pub created_at_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewTaskRoute {
    pub id: String,
    pub title: String,
    pub summary: String,
    #[serde(default)]
    pub stops: Vec<ReviewLocation>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewSessionDocument {
    pub selected_file_path: Option<String>,
    pub selected_diff_anchor: Option<DiffAnchor>,
    #[serde(default)]
    pub center_mode: ReviewCenterMode,
    #[serde(default)]
    pub inspector_mode: ReviewInspectorMode,
    #[serde(default = "default_true")]
    pub show_file_tree: bool,
    #[serde(default = "default_true")]
    pub show_inspector: bool,
    #[serde(default)]
    pub source_target: Option<ReviewSourceTarget>,
    #[serde(default)]
    pub waymarks: Vec<ReviewWaymark>,
    #[serde(default)]
    pub task_route: Option<ReviewTaskRoute>,
    #[serde(default)]
    pub route: Vec<ReviewLocation>,
    #[serde(default)]
    pub history_back: Vec<ReviewLocation>,
    #[serde(default)]
    pub history_forward: Vec<ReviewLocation>,
    #[serde(default)]
    pub last_read: Option<ReviewLocation>,
    #[serde(default)]
    pub collapsed_sections: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct ReviewSessionState {
    pub loaded: bool,
    pub error: Option<String>,
    pub center_mode: ReviewCenterMode,
    pub inspector_mode: ReviewInspectorMode,
    pub show_file_tree: bool,
    pub show_inspector: bool,
    pub source_target: Option<ReviewSourceTarget>,
    pub waymarks: Vec<ReviewWaymark>,
    pub task_route: Option<ReviewTaskRoute>,
    pub route: Vec<ReviewLocation>,
    pub history_back: Vec<ReviewLocation>,
    pub history_forward: Vec<ReviewLocation>,
    pub last_read: Option<ReviewLocation>,
    pub collapsed_sections: HashSet<String>,
}

impl Default for ReviewSessionState {
    fn default() -> Self {
        Self {
            loaded: false,
            error: None,
            center_mode: ReviewCenterMode::SemanticDiff,
            inspector_mode: ReviewInspectorMode::Graph,
            show_file_tree: true,
            show_inspector: true,
            source_target: None,
            waymarks: Vec::new(),
            task_route: None,
            route: Vec::new(),
            history_back: Vec::new(),
            history_forward: Vec::new(),
            last_read: None,
            collapsed_sections: HashSet::new(),
        }
    }
}

impl ReviewSessionState {
    pub fn from_document(document: ReviewSessionDocument) -> Self {
        Self {
            loaded: true,
            error: None,
            center_mode: document.center_mode,
            inspector_mode: document.inspector_mode,
            show_file_tree: document.show_file_tree,
            show_inspector: document.show_inspector,
            source_target: document.source_target,
            waymarks: document.waymarks,
            task_route: document.task_route,
            route: document.route,
            history_back: document.history_back,
            history_forward: document.history_forward,
            last_read: document.last_read,
            collapsed_sections: document.collapsed_sections.into_iter().collect(),
        }
    }

    pub fn to_document(
        &self,
        selected_file_path: Option<&str>,
        selected_diff_anchor: Option<&DiffAnchor>,
    ) -> ReviewSessionDocument {
        ReviewSessionDocument {
            selected_file_path: selected_file_path.map(str::to_string),
            selected_diff_anchor: selected_diff_anchor.cloned(),
            center_mode: self.center_mode,
            inspector_mode: self.inspector_mode,
            show_file_tree: self.show_file_tree,
            show_inspector: self.show_inspector,
            source_target: self.source_target.clone(),
            waymarks: self.waymarks.clone(),
            task_route: self.task_route.clone(),
            route: self.route.clone(),
            history_back: self.history_back.clone(),
            history_forward: self.history_forward.clone(),
            last_read: self.last_read.clone(),
            collapsed_sections: self.collapsed_sections.iter().cloned().collect(),
        }
    }

    pub fn waymark_for_location(&self, location: &ReviewLocation) -> Option<&ReviewWaymark> {
        self.waymarks
            .iter()
            .find(|waymark| waymark.location.same_spot_as(location))
    }
}

fn default_true() -> bool {
    true
}

pub fn location_label(file_path: &str, line: Option<usize>) -> String {
    match line.filter(|line| *line > 0) {
        Some(line) => format!("{file_path}:{line}"),
        None => file_path.to_string(),
    }
}

pub fn review_session_cache_key(detail_key: &str) -> String {
    format!("{REVIEW_SESSION_CACHE_KEY_PREFIX}:{detail_key}")
}

pub fn load_review_session(
    cache: &CacheStore,
    detail_key: &str,
) -> Result<Option<ReviewSessionDocument>, String> {
    let cache_key = review_session_cache_key(detail_key);
    Ok(cache
        .get::<ReviewSessionDocument>(&cache_key)?
        .map(|document| document.value))
}

pub fn save_review_session(
    cache: &CacheStore,
    detail_key: &str,
    document: &ReviewSessionDocument,
) -> Result<(), String> {
    let cache_key = review_session_cache_key(detail_key);
    cache.put(&cache_key, document, now_ms())
}

pub fn push_route_location(route: &mut Vec<ReviewLocation>, location: ReviewLocation) {
    push_recent_item(route, location, MAX_ROUTE_LOCATIONS);
}

pub fn push_history_location(history: &mut Vec<ReviewLocation>, location: ReviewLocation) {
    if history
        .last()
        .map(|existing| existing.same_spot_as(&location))
        .unwrap_or(false)
    {
        return;
    }

    history.push(location);
    if history.len() > MAX_HISTORY_LOCATIONS {
        let overflow = history.len() - MAX_HISTORY_LOCATIONS;
        history.drain(0..overflow);
    }
}

fn push_recent_item(items: &mut Vec<ReviewLocation>, location: ReviewLocation, max_len: usize) {
    if items
        .first()
        .map(|existing| existing.same_spot_as(&location))
        .unwrap_or(false)
    {
        return;
    }

    items.retain(|existing| !existing.same_spot_as(&location));
    items.insert(0, location);
    if items.len() > max_len {
        items.truncate(max_len);
    }
}

pub fn add_waymark(
    waymarks: &mut Vec<ReviewWaymark>,
    location: ReviewLocation,
    name: impl Into<String>,
) -> ReviewWaymark {
    let name = sanitize_waymark_name(name.into(), &location);
    let created_at_ms = now_ms();

    if let Some(existing) = waymarks
        .iter_mut()
        .find(|waymark| waymark.location.same_spot_as(&location))
    {
        existing.name = name;
        return existing.clone();
    }

    let waymark = ReviewWaymark {
        id: format!("wm-{created_at_ms}-{}", waymarks.len()),
        name,
        location,
        created_at_ms,
    };
    waymarks.push(waymark.clone());
    if waymarks.len() > MAX_WAYMARKS {
        let overflow = waymarks.len() - MAX_WAYMARKS;
        waymarks.drain(0..overflow);
    }
    waymark
}

pub fn remove_waymark(waymarks: &mut Vec<ReviewWaymark>, waymark_id: &str) -> bool {
    let Some(index) = waymarks.iter().position(|waymark| waymark.id == waymark_id) else {
        return false;
    };

    waymarks.remove(index);
    true
}

fn sanitize_waymark_name(name: String, location: &ReviewLocation) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        location.label.clone()
    } else {
        trimmed.chars().take(48).collect()
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{
        add_waymark, location_label, push_history_location, push_route_location, ReviewLocation,
    };

    #[test]
    fn location_label_uses_line_when_present() {
        assert_eq!(location_label("src/main.rs", Some(42)), "src/main.rs:42");
        assert_eq!(location_label("src/main.rs", None), "src/main.rs");
    }

    #[test]
    fn push_route_location_moves_existing_item_to_front() {
        let mut route = vec![
            ReviewLocation::from_diff("src/one.rs", None),
            ReviewLocation::from_diff("src/two.rs", None),
        ];

        push_route_location(&mut route, ReviewLocation::from_diff("src/two.rs", None));

        assert_eq!(route[0].file_path, "src/two.rs");
        assert_eq!(route.len(), 2);
    }

    #[test]
    fn push_history_location_deduplicates_trailing_entry() {
        let mut history = vec![ReviewLocation::from_diff("src/one.rs", None)];

        push_history_location(&mut history, ReviewLocation::from_diff("src/one.rs", None));
        push_history_location(&mut history, ReviewLocation::from_diff("src/two.rs", None));

        assert_eq!(history.len(), 2);
        assert_eq!(history[1].file_path, "src/two.rs");
    }

    #[test]
    fn add_waymark_updates_existing_location_without_reordering() {
        let mut waymarks = Vec::new();
        add_waymark(
            &mut waymarks,
            ReviewLocation::from_diff("src/one.rs", None),
            "First",
        );

        let updated = add_waymark(
            &mut waymarks,
            ReviewLocation::from_diff("src/one.rs", None),
            "Renamed",
        );

        assert_eq!(waymarks.len(), 1);
        assert_eq!(updated.name, "Renamed");
        assert_eq!(waymarks[0].name, "Renamed");
    }
}
