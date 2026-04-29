use std::cell::RefCell;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::cache::CacheStore;
use crate::code_tour::{
    build_tour_request_key, CodeTourProvider, CodeTourProviderStatus, CodeTourSettings, DiffAnchor,
    GeneratedCodeTour,
};
use crate::diff::DiffRenderRow;
use crate::github::{
    PullRequestDetail, PullRequestDetailSnapshot, PullRequestQueue, PullRequestSummary,
    RepositoryFileContent, ReviewAction, WorkspaceSnapshot,
};
use crate::local_repo::LocalRepositoryStatus;
use crate::lsp::{LspServerStatus, LspSessionManager, LspSymbolDetails};
use crate::managed_lsp::{ManagedServerInstallStatus, ManagedServerKind};
use crate::notifications;
use crate::review_graph::ReviewSymbolEvolutionState;
use crate::review_queue::ReviewQueue;
use crate::review_session::{
    add_waymark, load_review_session, location_label, push_history_location, push_route_location,
    remove_waymark, save_review_session, ReviewCenterMode, ReviewInspectorMode, ReviewLocation,
    ReviewSessionDocument, ReviewSessionState, ReviewSourceTarget, ReviewTaskRoute, ReviewWaymark,
};
use crate::semantic_diff::SemanticDiffFile;
use crate::syntax::{self, SyntaxSpan};
use crate::theme::{self, ThemePreference};
use gpui::{point, px, ListAlignment, ListState, Pixels, Point, ScrollHandle, WindowAppearance};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SectionId {
    Overview,
    Pulls,
    Issues,
    Reviews,
    Settings,
}

impl SectionId {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Pulls => "Pull Requests",
            Self::Issues => "Issues",
            Self::Reviews => "Reviews",
            Self::Settings => "Settings",
        }
    }

    pub fn all() -> &'static [SectionId] {
        &[
            SectionId::Overview,
            SectionId::Pulls,
            SectionId::Issues,
            SectionId::Reviews,
            SectionId::Settings,
        ]
    }
}

fn random_overview_greeting_index() -> usize {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0);
    let mixed = nanos ^ nanos.rotate_left(13) ^ nanos.rotate_right(7);

    mixed as usize
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PullRequestSurface {
    Overview,
    Files,
}

impl PullRequestSurface {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Overview => "Briefing",
            Self::Files => "Review",
        }
    }

    pub fn all() -> &'static [PullRequestSurface] {
        &[PullRequestSurface::Overview, PullRequestSurface::Files]
    }
}

pub fn pr_key(repository: &str, number: i64) -> String {
    format!("{repository}#{number}")
}

#[derive(Clone, Debug)]
pub struct DetailState {
    pub snapshot: Option<PullRequestDetailSnapshot>,
    pub loading: bool,
    pub syncing: bool,
    pub error: Option<String>,
    pub local_repository_status: Option<LocalRepositoryStatus>,
    pub local_repository_loading: bool,
    pub local_repository_error: Option<String>,
    pub tour_states: std::collections::HashMap<CodeTourProvider, CodeTourState>,
    pub file_content_states: std::collections::HashMap<String, FileContentState>,
    pub lsp_statuses: std::collections::HashMap<String, LspServerStatus>,
    pub lsp_loading_paths: std::collections::HashSet<String>,
    pub lsp_symbol_states: std::collections::HashMap<String, LspSymbolState>,
    pub review_evolution_states: std::collections::HashMap<String, ReviewSymbolEvolutionState>,
    pub review_route_loading: bool,
    pub review_route_message: Option<String>,
    pub review_route_error: Option<String>,
    pub review_session: ReviewSessionState,
}

impl Default for DetailState {
    fn default() -> Self {
        Self {
            snapshot: None,
            loading: false,
            syncing: false,
            error: None,
            local_repository_status: None,
            local_repository_loading: false,
            local_repository_error: None,
            tour_states: std::collections::HashMap::new(),
            file_content_states: std::collections::HashMap::new(),
            lsp_statuses: std::collections::HashMap::new(),
            lsp_loading_paths: std::collections::HashSet::new(),
            lsp_symbol_states: std::collections::HashMap::new(),
            review_evolution_states: std::collections::HashMap::new(),
            review_route_loading: false,
            review_route_message: None,
            review_route_error: None,
            review_session: ReviewSessionState::default(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct CodeTourState {
    pub request_key: Option<String>,
    pub document: Option<GeneratedCodeTour>,
    pub loading: bool,
    pub generating: bool,
    pub progress_summary: Option<String>,
    pub progress_detail: Option<String>,
    pub progress_log: Vec<String>,
    pub progress_log_file_path: Option<String>,
    pub error: Option<String>,
    pub message: Option<String>,
    pub success: bool,
}

impl Default for CodeTourState {
    fn default() -> Self {
        Self {
            request_key: None,
            document: None,
            loading: false,
            generating: false,
            progress_summary: None,
            progress_detail: None,
            progress_log: Vec::new(),
            progress_log_file_path: None,
            error: None,
            message: None,
            success: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct FileContentState {
    pub request_key: Option<String>,
    pub document: Option<RepositoryFileContent>,
    pub prepared: Option<PreparedFileContent>,
    pub loading: bool,
    pub error: Option<String>,
}

impl Default for FileContentState {
    fn default() -> Self {
        Self {
            request_key: None,
            document: None,
            prepared: None,
            loading: false,
            error: None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct LspSymbolState {
    pub loading: bool,
    pub details: Option<LspSymbolDetails>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct ManagedLspSettingsState {
    pub statuses: std::collections::HashMap<ManagedServerKind, ManagedServerInstallStatus>,
    pub loading: bool,
    pub loaded: bool,
    pub installing: std::collections::HashSet<ManagedServerKind>,
    pub install_errors: std::collections::HashMap<ManagedServerKind, String>,
    pub install_messages: std::collections::HashMap<ManagedServerKind, String>,
}

#[derive(Clone, Debug)]
pub struct CodeTourSettingsState {
    pub settings: CodeTourSettings,
    pub loading: bool,
    pub loaded: bool,
    pub error: Option<String>,
    pub background_syncing: bool,
    pub background_message: Option<String>,
    pub background_error: Option<String>,
}

impl Default for CodeTourSettingsState {
    fn default() -> Self {
        Self {
            settings: CodeTourSettings::default(),
            loading: false,
            loaded: false,
            error: None,
            background_syncing: false,
            background_message: None,
            background_error: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct PreparedFileContent {
    pub path: String,
    pub reference: String,
    pub is_binary: bool,
    pub size_bytes: usize,
    pub text: Arc<str>,
    pub lines: Arc<Vec<PreparedFileLine>>,
}

impl PreparedFileContent {
    pub fn rehighlighted(&self) -> Self {
        let text_lines = if self.text.is_empty() {
            Vec::new()
        } else {
            self.text
                .lines()
                .map(str::to_string)
                .collect::<Vec<String>>()
        };
        let spans = if self.is_binary || self.size_bytes > syntax::MAX_HIGHLIGHT_BYTES {
            text_lines
                .iter()
                .map(|_| Vec::new())
                .collect::<Vec<Vec<SyntaxSpan>>>()
        } else {
            syntax::highlight_lines(
                self.path.as_str(),
                text_lines.iter().map(|line| line.as_str()),
            )
        };

        let lines = text_lines
            .into_iter()
            .zip(spans)
            .enumerate()
            .map(|(index, (text, spans))| PreparedFileLine {
                line_number: index + 1,
                text,
                spans,
            })
            .collect::<Vec<_>>();

        Self {
            lines: Arc::new(lines),
            ..self.clone()
        }
    }
}

#[derive(Clone, Debug)]
pub struct PreparedFileLine {
    pub line_number: usize,
    pub text: String,
    pub spans: Vec<SyntaxSpan>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiffInlineRange {
    pub column_start: usize,
    pub column_end: usize,
}

#[derive(Clone, Debug, Default)]
pub struct DiffLineHighlight {
    pub syntax_spans: Vec<SyntaxSpan>,
    pub emphasis_ranges: Vec<DiffInlineRange>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ReviewLineActionMode {
    #[default]
    Menu,
    Comment,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewLineActionTarget {
    pub anchor: DiffAnchor,
    pub label: String,
}

impl ReviewLineActionTarget {
    pub fn stable_key(&self) -> String {
        format!(
            "{}:{}:{}",
            self.anchor.file_path,
            self.anchor.side.as_deref().unwrap_or(""),
            self.anchor.line.unwrap_or_default()
        )
    }

    pub fn review_location(&self) -> ReviewLocation {
        ReviewLocation::from_diff(self.anchor.file_path.clone(), Some(self.anchor.clone()))
    }
}

#[derive(Clone)]
pub struct DiffFileViewState {
    pub rows: Arc<Vec<DiffRenderRow>>,
    pub revision: String,
    pub parsed_file_index: Option<usize>,
    pub highlighted_hunks: Option<Arc<Vec<Vec<DiffLineHighlight>>>>,
    pub list_state: ListState,
}

impl DiffFileViewState {
    pub fn new(
        rows: Arc<Vec<DiffRenderRow>>,
        revision: String,
        parsed_file_index: Option<usize>,
        highlighted_hunks: Option<Arc<Vec<Vec<DiffLineHighlight>>>>,
    ) -> Self {
        Self {
            rows,
            revision,
            parsed_file_index,
            highlighted_hunks,
            list_state: ListState::new(0, ListAlignment::Top, px(400.0)),
        }
    }
}

#[derive(Clone)]
pub struct CachedReviewQueue {
    pub revision: String,
    pub queue: Arc<ReviewQueue>,
}

#[derive(Clone)]
pub struct CachedSemanticDiffFile {
    pub revision: String,
    pub semantic: Arc<SemanticDiffFile>,
}

#[derive(Clone, Debug)]
pub enum ReviewFileTreeRow {
    Directory {
        name: String,
        depth: usize,
    },
    File {
        path: String,
        name: String,
        depth: usize,
        additions: i64,
        deletions: i64,
    },
}

#[derive(Clone)]
pub struct CachedReviewFileTree {
    pub revision: String,
    pub rows: Arc<Vec<ReviewFileTreeRow>>,
}

pub struct AppState {
    pub cache: Arc<CacheStore>,
    pub lsp_session_manager: Arc<LspSessionManager>,

    // Navigation
    pub active_section: SectionId,
    pub active_surface: PullRequestSurface,
    pub active_queue_id: String,
    pub active_pr_key: Option<String>,
    pub open_tabs: Vec<PullRequestSummary>,
    pub muted_repos: std::collections::HashSet<String>,
    pub overview_greeting_index: usize,

    // Workspace data
    pub workspace: Option<WorkspaceSnapshot>,
    pub workspace_loading: bool,
    pub workspace_syncing: bool,
    pub workspace_error: Option<String>,

    // PR detail data (keyed by pr_key)
    pub detail_states: std::collections::HashMap<String, DetailState>,
    pub unread_review_comment_ids: std::collections::BTreeSet<String>,

    // Bootstrap
    pub gh_available: bool,
    pub gh_version: Option<String>,
    pub cache_path: String,
    pub bootstrap_loading: bool,
    pub theme_preference: ThemePreference,
    pub window_appearance: WindowAppearance,
    pub app_sidebar_collapsed: bool,

    // Selected file in diff view
    pub selected_file_path: Option<String>,
    pub selected_diff_anchor: Option<DiffAnchor>,
    pub diff_view_states: RefCell<std::collections::HashMap<String, DiffFileViewState>>,
    pub review_queue_cache: RefCell<std::collections::HashMap<String, CachedReviewQueue>>,
    pub semantic_diff_cache: RefCell<std::collections::HashMap<String, CachedSemanticDiffFile>>,
    pub review_file_tree_cache: RefCell<std::collections::HashMap<String, CachedReviewFileTree>>,
    pub review_file_tree_list_states: RefCell<std::collections::HashMap<String, ListState>>,
    pub review_nav_list_states: RefCell<std::collections::HashMap<String, ListState>>,
    pub source_browser_list_states: RefCell<std::collections::HashMap<String, ListState>>,
    // Review form
    pub review_action: ReviewAction,
    pub review_body: String,
    pub review_editor_active: bool,
    pub review_loading: bool,
    pub review_message: Option<String>,
    pub review_success: bool,
    pub waymark_draft: String,
    pub active_review_line_action: Option<ReviewLineActionTarget>,
    pub active_review_line_action_position: Option<Point<Pixels>>,
    pub review_line_action_mode: ReviewLineActionMode,
    pub review_graph_expanded: bool,
    pub review_graph_selected_node_id: Option<String>,
    pub review_graph_pan_offset: Point<Pixels>,
    pub review_graph_zoom: f32,
    pub review_graph_panning: bool,
    pub review_graph_last_pan_position: Option<Point<Pixels>>,
    pub inline_comment_draft: String,
    pub inline_comment_loading: bool,
    pub inline_comment_error: Option<String>,
    pub pr_header_compact: bool,

    // Command palette
    pub palette_open: bool,
    pub palette_query: String,
    pub palette_selected_index: usize,
    pub waypoint_spotlight_open: bool,
    pub waypoint_spotlight_query: String,
    pub waypoint_spotlight_selected_index: usize,

    // Code tours
    pub code_tour_provider_statuses: Vec<CodeTourProviderStatus>,
    pub code_tour_provider_statuses_loaded: bool,
    pub code_tour_provider_loading: bool,
    pub code_tour_provider_error: Option<String>,
    pub automatic_tour_request_keys: std::collections::HashSet<String>,
    pub settings_scroll_handle: ScrollHandle,
    pub ai_tour_section_list_state: ListState,
    pub code_tour_settings: CodeTourSettingsState,
    pub managed_lsp_settings: ManagedLspSettingsState,
}

impl AppState {
    pub fn new(cache: CacheStore) -> Self {
        let theme_preference = theme::load_theme_settings(&cache)
            .unwrap_or_default()
            .preference;
        theme::set_active_theme(theme::resolve_theme(
            theme_preference,
            WindowAppearance::Light,
        ));
        let cache_path = cache.path().display().to_string();
        let unread_review_comment_ids =
            notifications::load_unread_review_comment_ids(&cache).unwrap_or_default();
        let mut state = Self {
            cache: Arc::new(cache),
            lsp_session_manager: Arc::new(LspSessionManager::new()),
            active_section: SectionId::Overview,
            active_surface: PullRequestSurface::Overview,
            active_queue_id: "reviewRequested".to_string(),
            active_pr_key: None,
            open_tabs: Vec::new(),
            muted_repos: std::collections::HashSet::new(),
            overview_greeting_index: random_overview_greeting_index(),
            workspace: None,
            workspace_loading: true,
            workspace_syncing: false,
            workspace_error: None,
            detail_states: std::collections::HashMap::new(),
            unread_review_comment_ids,
            gh_available: false,
            gh_version: None,
            cache_path,
            bootstrap_loading: true,
            theme_preference,
            window_appearance: WindowAppearance::Light,
            app_sidebar_collapsed: true,
            selected_file_path: None,
            selected_diff_anchor: None,
            diff_view_states: RefCell::new(std::collections::HashMap::new()),
            review_queue_cache: RefCell::new(std::collections::HashMap::new()),
            semantic_diff_cache: RefCell::new(std::collections::HashMap::new()),
            review_file_tree_cache: RefCell::new(std::collections::HashMap::new()),
            review_file_tree_list_states: RefCell::new(std::collections::HashMap::new()),
            review_nav_list_states: RefCell::new(std::collections::HashMap::new()),
            source_browser_list_states: RefCell::new(std::collections::HashMap::new()),
            review_action: ReviewAction::Comment,
            review_body: String::new(),
            review_editor_active: false,
            review_loading: false,
            review_message: None,
            review_success: false,
            waymark_draft: String::new(),
            active_review_line_action: None,
            active_review_line_action_position: None,
            review_line_action_mode: ReviewLineActionMode::Menu,
            review_graph_expanded: false,
            review_graph_selected_node_id: None,
            review_graph_pan_offset: point(px(0.0), px(0.0)),
            review_graph_zoom: 1.0,
            review_graph_panning: false,
            review_graph_last_pan_position: None,
            inline_comment_draft: String::new(),
            inline_comment_loading: false,
            inline_comment_error: None,
            pr_header_compact: false,
            palette_open: false,
            palette_query: String::new(),
            palette_selected_index: 0,
            waypoint_spotlight_open: false,
            waypoint_spotlight_query: String::new(),
            waypoint_spotlight_selected_index: 0,
            code_tour_provider_statuses: Vec::new(),
            code_tour_provider_statuses_loaded: false,
            code_tour_provider_loading: false,
            code_tour_provider_error: None,
            automatic_tour_request_keys: std::collections::HashSet::new(),
            settings_scroll_handle: ScrollHandle::new(),
            ai_tour_section_list_state: ListState::new(0, ListAlignment::Top, px(720.0)),
            code_tour_settings: CodeTourSettingsState::default(),
            managed_lsp_settings: ManagedLspSettingsState::default(),
        };

        state.restore_debug_pull_request_from_cache();
        state
    }

    pub fn resolved_theme(&self) -> theme::ActiveTheme {
        theme::resolve_theme(self.theme_preference, self.window_appearance)
    }

    pub fn set_active_section(&mut self, section: SectionId) {
        if section == SectionId::Overview {
            self.overview_greeting_index = random_overview_greeting_index();
        }
        self.active_section = section;
    }

    pub fn set_theme_preference(&mut self, preference: ThemePreference) {
        let previous = self.resolved_theme();
        self.theme_preference = preference;
        self.apply_theme_change(previous);
    }

    pub fn set_window_appearance(&mut self, appearance: WindowAppearance) {
        let previous = self.resolved_theme();
        self.window_appearance = appearance;
        self.apply_theme_change(previous);
    }

    fn apply_theme_change(&mut self, previous: theme::ActiveTheme) {
        let next = self.resolved_theme();
        theme::set_active_theme(next);
        if next != previous {
            self.refresh_theme_dependent_state();
        }
    }

    fn refresh_theme_dependent_state(&mut self) {
        for detail_state in self.detail_states.values_mut() {
            for file_state in detail_state.file_content_states.values_mut() {
                if let Some(prepared) = file_state.prepared.as_ref() {
                    file_state.prepared = Some(prepared.rehighlighted());
                }
            }
        }

        for diff_view_state in self.diff_view_states.borrow_mut().values_mut() {
            diff_view_state.highlighted_hunks = None;
        }
    }

    pub fn active_queue(&self) -> Option<&PullRequestQueue> {
        self.workspace
            .as_ref()?
            .queues
            .iter()
            .find(|q| q.id == self.active_queue_id)
            .or_else(|| self.workspace.as_ref()?.queues.first())
    }

    pub fn active_pr(&self) -> Option<&PullRequestSummary> {
        let key = self.active_pr_key.as_ref()?;
        self.open_tabs
            .iter()
            .find(|tab| pr_key(&tab.repository, tab.number) == *key)
    }

    pub fn active_detail(&self) -> Option<&PullRequestDetail> {
        let key = self.active_pr_key.as_ref()?;
        self.detail_states
            .get(key)?
            .snapshot
            .as_ref()?
            .detail
            .as_ref()
    }

    pub fn is_review_comment_unread(&self, comment_id: &str) -> bool {
        self.unread_review_comment_ids.contains(comment_id)
    }

    pub fn unread_review_comment_ids_for_detail(&self, detail: &PullRequestDetail) -> Vec<String> {
        detail
            .review_threads
            .iter()
            .flat_map(|thread| &thread.comments)
            .filter(|comment| self.is_review_comment_unread(&comment.id))
            .map(|comment| comment.id.clone())
            .collect()
    }

    pub fn mark_review_comments_read<I>(&mut self, comment_ids: I)
    where
        I: IntoIterator<Item = String>,
    {
        let comment_ids = comment_ids.into_iter().collect::<Vec<_>>();
        if comment_ids.is_empty() {
            return;
        }

        match notifications::mark_review_comments_read(self.cache.as_ref(), comment_ids.clone()) {
            Ok(unread_ids) => {
                self.unread_review_comment_ids = unread_ids;
            }
            Err(error) => {
                eprintln!("Failed to persist review comment read state: {error}");
                for comment_id in comment_ids {
                    self.unread_review_comment_ids.remove(&comment_id);
                }
            }
        }
    }

    pub fn active_detail_state(&self) -> Option<&DetailState> {
        let key = self.active_pr_key.as_ref()?;
        self.detail_states.get(key)
    }

    pub fn active_tour_state(&self) -> Option<&CodeTourState> {
        let detail_state = self.active_detail_state()?;
        detail_state
            .tour_states
            .get(&self.code_tour_settings.settings.provider)
    }

    pub fn active_review_session(&self) -> Option<&ReviewSessionState> {
        self.active_detail_state()
            .map(|detail_state| &detail_state.review_session)
    }

    pub fn active_review_session_mut(&mut self) -> Option<&mut ReviewSessionState> {
        let key = self.active_pr_key.clone()?;
        self.detail_states
            .get_mut(&key)
            .map(|detail_state| &mut detail_state.review_session)
    }

    pub fn active_local_repository_status(&self) -> Option<&LocalRepositoryStatus> {
        self.active_detail_state()?.local_repository_status.as_ref()
    }

    pub fn selected_tour_provider_status(&self) -> Option<&CodeTourProviderStatus> {
        self.code_tour_provider_statuses
            .iter()
            .find(|status| status.provider == self.code_tour_settings.settings.provider)
    }

    pub fn active_tour_request_key(&self) -> Option<String> {
        let detail = self.active_detail()?;
        Some(build_tour_request_key(
            detail,
            self.code_tour_settings.settings.provider,
        ))
    }

    pub fn selected_tour_provider(&self) -> CodeTourProvider {
        self.code_tour_settings.settings.provider
    }

    pub fn section_count(&self, section: SectionId) -> i64 {
        match section {
            SectionId::Overview => 0,
            SectionId::Pulls => self
                .workspace
                .as_ref()
                .map(|w| w.queues.iter().map(|q| q.total_count).sum())
                .unwrap_or(0),
            SectionId::Issues => 0,
            SectionId::Reviews => self
                .workspace
                .as_ref()
                .and_then(|w| w.queues.iter().find(|q| q.id == "reviewRequested"))
                .map(|q| q.total_count)
                .unwrap_or(0),
            SectionId::Settings => 0,
        }
    }

    pub fn viewer_name(&self) -> &str {
        self.workspace
            .as_ref()
            .and_then(|w| w.viewer.as_ref())
            .and_then(|v| v.name.as_deref().or(Some(v.login.as_str())))
            .unwrap_or("developer")
    }

    pub fn is_authenticated(&self) -> bool {
        self.workspace
            .as_ref()
            .map(|w| w.auth.is_authenticated)
            .unwrap_or(false)
    }

    pub fn review_queue(&self) -> Option<&PullRequestQueue> {
        self.workspace
            .as_ref()?
            .queues
            .iter()
            .find(|q| q.id == "reviewRequested")
    }

    pub fn authored_queue(&self) -> Option<&PullRequestQueue> {
        self.workspace
            .as_ref()?
            .queues
            .iter()
            .find(|q| q.id == "authored")
    }

    pub fn current_review_location(&self) -> Option<ReviewLocation> {
        let session = self.active_review_session();
        if let Some(source_target) = session.and_then(|session| {
            (session.center_mode == ReviewCenterMode::SourceBrowser)
                .then(|| session.source_target.clone())
                .flatten()
        }) {
            return Some(ReviewLocation::from_source(
                source_target.path,
                source_target.line,
                source_target.reason,
            ));
        }

        self.selected_file_path.clone().map(|file_path| {
            if session
                .map(|session| session.center_mode == ReviewCenterMode::AiTour)
                .unwrap_or(false)
            {
                ReviewLocation::from_ai_tour(file_path, self.selected_diff_anchor.clone())
            } else {
                ReviewLocation::from_diff(file_path, self.selected_diff_anchor.clone())
            }
        })
    }

    pub fn selected_diff_line_target(&self) -> Option<ReviewLineActionTarget> {
        let file_path = self.selected_file_path.clone()?;
        let anchor = self.selected_diff_anchor.as_ref()?;
        let side = anchor.side.as_deref()?;
        let line = anchor.line?;
        let line_number = usize::try_from(line).ok().filter(|line| *line > 0)?;

        Some(ReviewLineActionTarget {
            anchor: DiffAnchor {
                file_path: file_path.clone(),
                hunk_header: anchor.hunk_header.clone(),
                line: Some(line),
                side: Some(side.to_string()),
                thread_id: None,
            },
            label: location_label(&file_path, Some(line_number)),
        })
    }

    pub fn active_review_task_route(&self) -> Option<&ReviewTaskRoute> {
        self.active_review_session()
            .and_then(|session| session.task_route.as_ref())
    }

    pub fn apply_review_session_document(
        &mut self,
        detail_key: &str,
        document: Option<ReviewSessionDocument>,
    ) {
        if let Some(document) = document {
            self.selected_file_path = document.selected_file_path.clone();
            self.selected_diff_anchor = document.selected_diff_anchor.clone();
            if let Some(detail_state) = self.detail_states.get_mut(detail_key) {
                detail_state.review_session = ReviewSessionState::from_document(document);
            }
        } else {
            if let Some(detail_state) = self.detail_states.get_mut(detail_key) {
                detail_state.review_session.loaded = true;
                detail_state.review_session.error = None;
            }
        }
    }

    pub fn navigate_to_review_location(&mut self, location: ReviewLocation, push_history: bool) {
        let previous = if push_history {
            self.current_review_location()
        } else {
            None
        };
        let Some(detail_key) = self.active_pr_key.clone() else {
            return;
        };

        let path_is_changed = self
            .detail_states
            .get(&detail_key)
            .and_then(|detail_state| detail_state.snapshot.as_ref())
            .and_then(|snapshot| snapshot.detail.as_ref())
            .map(|detail| {
                detail
                    .files
                    .iter()
                    .any(|file| file.path == location.file_path)
            })
            .unwrap_or(false);

        match location.mode {
            ReviewCenterMode::SemanticDiff | ReviewCenterMode::AiTour => {
                self.selected_file_path = Some(location.file_path.clone());
                self.selected_diff_anchor = location.anchor.clone();
            }
            ReviewCenterMode::SourceBrowser => {
                if path_is_changed {
                    self.selected_file_path = Some(location.file_path.clone());
                }
            }
        }

        let Some(session) = self.active_review_session_mut() else {
            return;
        };

        if push_history {
            if let Some(previous) = previous.filter(|previous| previous != &location) {
                push_history_location(&mut session.history_back, previous);
                session.history_forward.clear();
            }
        }

        session.center_mode = location.mode;
        session.source_target = location.as_source_target();
        session.last_read = Some(location.clone());
        push_route_location(&mut session.route, location);
    }

    pub fn current_waymark(&self) -> Option<&ReviewWaymark> {
        self.active_review_session().and_then(|session| {
            self.selected_diff_line_target()
                .map(|target| target.review_location())
                .or_else(|| self.current_review_location())
                .and_then(|location| session.waymark_for_location(&location))
        })
    }

    pub fn add_waymark_for_current_review_location(
        &mut self,
        name: impl Into<String>,
    ) -> Option<ReviewWaymark> {
        let location = self
            .selected_diff_line_target()
            .map(|target| target.review_location())
            .or_else(|| self.current_review_location())?;
        let session = self.active_review_session_mut()?;
        Some(add_waymark(&mut session.waymarks, location, name))
    }

    pub fn remove_review_waymark(&mut self, waymark_id: &str) -> bool {
        let Some(session) = self.active_review_session_mut() else {
            return false;
        };

        remove_waymark(&mut session.waymarks, waymark_id)
    }

    pub fn navigate_review_back(&mut self) -> bool {
        let current = self.current_review_location();
        let target = {
            let Some(session) = self.active_review_session_mut() else {
                return false;
            };
            session.history_back.pop()
        };

        let Some(target) = target else {
            return false;
        };

        if let Some(current) = current {
            if let Some(session) = self.active_review_session_mut() {
                push_history_location(&mut session.history_forward, current);
            }
        }

        self.navigate_to_review_location(target, false);
        true
    }

    pub fn navigate_review_forward(&mut self) -> bool {
        let current = self.current_review_location();
        let target = {
            let Some(session) = self.active_review_session_mut() else {
                return false;
            };
            session.history_forward.pop()
        };

        let Some(target) = target else {
            return false;
        };

        if let Some(current) = current {
            if let Some(session) = self.active_review_session_mut() {
                push_history_location(&mut session.history_back, current);
            }
        }

        self.navigate_to_review_location(target, false);
        true
    }

    pub fn toggle_review_section_collapse(&mut self, section_id: &str) {
        let Some(session) = self.active_review_session_mut() else {
            return;
        };

        if !session.collapsed_sections.insert(section_id.to_string()) {
            session.collapsed_sections.remove(section_id);
        }
    }

    pub fn is_review_section_collapsed(&self, section_id: &str) -> bool {
        self.active_review_session()
            .map(|session| session.collapsed_sections.contains(section_id))
            .unwrap_or(false)
    }

    pub fn set_review_center_mode(&mut self, mode: ReviewCenterMode) {
        if let Some(session) = self.active_review_session_mut() {
            session.center_mode = mode;
            if mode != ReviewCenterMode::SourceBrowser {
                session.source_target = None;
            }
        }
    }

    pub fn set_review_inspector_mode(&mut self, mode: ReviewInspectorMode) {
        if let Some(session) = self.active_review_session_mut() {
            session.inspector_mode = mode;
            session.show_inspector = true;
        }
    }

    pub fn set_review_file_tree_visible(&mut self, visible: bool) {
        if let Some(session) = self.active_review_session_mut() {
            session.show_file_tree = visible;
        }
    }

    pub fn set_review_inspector_visible(&mut self, visible: bool) {
        if let Some(session) = self.active_review_session_mut() {
            session.show_inspector = visible;
        }
    }

    pub fn set_review_source_target(&mut self, target: ReviewSourceTarget) {
        if let Some(session) = self.active_review_session_mut() {
            session.center_mode = ReviewCenterMode::SourceBrowser;
            session.source_target = Some(target);
        }
    }

    pub fn set_active_review_task_route(&mut self, route: Option<ReviewTaskRoute>) {
        if let Some(session) = self.active_review_session_mut() {
            session.task_route = route;
        }
    }

    pub fn persist_active_review_session(&self) {
        let Some(detail_key) = self.active_pr_key.as_deref() else {
            return;
        };
        let Some(session) = self.active_review_session() else {
            return;
        };

        let document = session.to_document(
            self.selected_file_path.as_deref(),
            self.selected_diff_anchor.as_ref(),
        );
        let _ = save_review_session(self.cache.as_ref(), detail_key, &document);
    }

    fn restore_debug_pull_request_from_cache(&mut self) {
        let Some((repository, number)) = std::env::var("REVIEWBUDDY_DEBUG_OPEN_PR")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .and_then(|value| parse_debug_pull_request_target(&value))
        else {
            return;
        };

        let Ok(snapshot) =
            crate::github::load_pull_request_detail(self.cache.as_ref(), &repository, number)
        else {
            return;
        };
        let Some(detail) = snapshot.detail.clone() else {
            return;
        };

        let summary = PullRequestSummary {
            repository: detail.repository.clone(),
            number: detail.number,
            title: detail.title.clone(),
            author_login: detail.author_login.clone(),
            author_avatar_url: detail.author_avatar_url.clone(),
            is_draft: detail.is_draft,
            comments_count: detail.comments_count,
            additions: detail.additions,
            deletions: detail.deletions,
            changed_files: detail.changed_files,
            state: detail.state.clone(),
            review_decision: detail.review_decision.clone(),
            updated_at: detail.updated_at.clone(),
            url: detail.url.clone(),
        };
        let detail_key = pr_key(&repository, number);

        self.open_tabs.insert(0, summary);
        self.set_active_section(SectionId::Pulls);
        self.active_surface = PullRequestSurface::Files;
        self.active_pr_key = Some(detail_key.clone());
        self.detail_states
            .entry(detail_key.clone())
            .or_default()
            .snapshot = Some(snapshot);

        if let Ok(document) = load_review_session(self.cache.as_ref(), &detail_key) {
            self.apply_review_session_document(&detail_key, document);
        }
    }
}

fn parse_debug_pull_request_target(target: &str) -> Option<(String, i64)> {
    let (repository, number) = target.trim().rsplit_once('#')?;
    let number = number.parse::<i64>().ok()?;
    Some((repository.to_string(), number))
}
