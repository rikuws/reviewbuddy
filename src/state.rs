use std::cell::RefCell;
use std::sync::Arc;

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
use crate::syntax::SyntaxSpan;
use gpui::{px, ListAlignment, ListState, ScrollHandle};

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

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PullRequestSurface {
    Overview,
    Files,
    Tour,
}

impl PullRequestSurface {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Files => "Files",
            Self::Tour => "Tour",
        }
    }

    pub fn all() -> &'static [PullRequestSurface] {
        &[
            PullRequestSurface::Overview,
            PullRequestSurface::Files,
            PullRequestSurface::Tour,
        ]
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

#[derive(Clone, Debug)]
pub struct PreparedFileLine {
    pub line_number: usize,
    pub text: String,
    pub spans: Vec<SyntaxSpan>,
}

#[derive(Clone)]
pub struct DiffFileViewState {
    pub rows: Arc<Vec<DiffRenderRow>>,
    pub revision: String,
    pub parsed_file_index: Option<usize>,
    pub highlighted_hunks: Option<Arc<Vec<Vec<Vec<SyntaxSpan>>>>>,
    pub list_state: ListState,
}

impl DiffFileViewState {
    pub fn new(
        rows: Arc<Vec<DiffRenderRow>>,
        revision: String,
        parsed_file_index: Option<usize>,
        highlighted_hunks: Option<Arc<Vec<Vec<Vec<SyntaxSpan>>>>>,
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

    // Workspace data
    pub workspace: Option<WorkspaceSnapshot>,
    pub workspace_loading: bool,
    pub workspace_syncing: bool,
    pub workspace_error: Option<String>,

    // PR detail data (keyed by pr_key)
    pub detail_states: std::collections::HashMap<String, DetailState>,

    // Bootstrap
    pub gh_available: bool,
    pub gh_version: Option<String>,
    pub cache_path: String,
    pub bootstrap_loading: bool,

    // Selected file in diff view
    pub selected_file_path: Option<String>,
    pub selected_diff_anchor: Option<DiffAnchor>,
    pub diff_view_states: RefCell<std::collections::HashMap<String, DiffFileViewState>>,
    // Review form
    pub review_action: ReviewAction,
    pub review_body: String,
    pub review_editor_active: bool,
    pub review_loading: bool,
    pub review_message: Option<String>,
    pub review_success: bool,
    pub pr_header_compact: bool,

    // Command palette
    pub palette_open: bool,
    pub palette_query: String,
    pub palette_selected_index: usize,

    // Code tours
    pub code_tour_provider_statuses: Vec<CodeTourProviderStatus>,
    pub code_tour_provider_statuses_loaded: bool,
    pub code_tour_provider_loading: bool,
    pub code_tour_provider_error: Option<String>,
    pub automatic_tour_request_keys: std::collections::HashSet<String>,
    pub active_tour_outline_id: String,
    pub collapsed_tour_panels: std::collections::HashSet<String>,
    pub settings_scroll_handle: ScrollHandle,
    pub tour_content_scroll_handle: ScrollHandle,
    pub tour_content_list_state: ListState,
    pub code_tour_settings: CodeTourSettingsState,
    pub managed_lsp_settings: ManagedLspSettingsState,
}

impl AppState {
    pub fn new(cache: CacheStore) -> Self {
        let cache_path = cache.path().display().to_string();
        Self {
            cache: Arc::new(cache),
            lsp_session_manager: Arc::new(LspSessionManager::new()),
            active_section: SectionId::Overview,
            active_surface: PullRequestSurface::Overview,
            active_queue_id: "reviewRequested".to_string(),
            active_pr_key: None,
            open_tabs: Vec::new(),
            muted_repos: std::collections::HashSet::new(),
            workspace: None,
            workspace_loading: true,
            workspace_syncing: false,
            workspace_error: None,
            detail_states: std::collections::HashMap::new(),
            gh_available: false,
            gh_version: None,
            cache_path,
            bootstrap_loading: true,
            selected_file_path: None,
            selected_diff_anchor: None,
            diff_view_states: RefCell::new(std::collections::HashMap::new()),
            review_action: ReviewAction::Comment,
            review_body: String::new(),
            review_editor_active: false,
            review_loading: false,
            review_message: None,
            review_success: false,
            pr_header_compact: false,
            palette_open: false,
            palette_query: String::new(),
            palette_selected_index: 0,
            code_tour_provider_statuses: Vec::new(),
            code_tour_provider_statuses_loaded: false,
            code_tour_provider_loading: false,
            code_tour_provider_error: None,
            automatic_tour_request_keys: std::collections::HashSet::new(),
            active_tour_outline_id: "overview".to_string(),
            collapsed_tour_panels: std::collections::HashSet::new(),
            settings_scroll_handle: ScrollHandle::new(),
            tour_content_scroll_handle: ScrollHandle::new(),
            tour_content_list_state: ListState::new(0, ListAlignment::Top, px(600.0)),
            code_tour_settings: CodeTourSettingsState::default(),
            managed_lsp_settings: ManagedLspSettingsState::default(),
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
}
