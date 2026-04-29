use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub const STACK_GENERATOR_VERSION: &str = "virtual-stacks-v1";

pub type ReviewStackId = String;
pub type ReviewStackLayerId = String;
pub type ChangeAtomId = String;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewStack {
    pub id: ReviewStackId,
    pub repository: String,
    pub selected_pr_number: i64,
    pub source: StackSource,
    pub kind: StackKind,
    pub confidence: Confidence,
    pub trunk_branch: Option<String>,
    pub base_oid: Option<String>,
    pub head_oid: Option<String>,
    pub layers: Vec<ReviewStackLayer>,
    #[serde(default)]
    pub atoms: Vec<ChangeAtom>,
    #[serde(default)]
    pub warnings: Vec<StackWarning>,
    pub provider: Option<StackProviderMetadata>,
    pub generated_at_ms: i64,
    pub generator_version: String,
}

impl ReviewStack {
    pub fn selected_layer(&self, selected_layer_id: Option<&str>) -> Option<&ReviewStackLayer> {
        selected_layer_id
            .and_then(|id| self.layers.iter().find(|layer| layer.id == id))
            .or_else(|| self.layers.first())
    }

    pub fn selected_layer_index(&self, selected_layer_id: Option<&str>) -> Option<usize> {
        let selected = self.selected_layer(selected_layer_id)?;
        self.layers.iter().position(|layer| layer.id == selected.id)
    }

    pub fn atom(&self, atom_id: &str) -> Option<&ChangeAtom> {
        self.atoms.iter().find(|atom| atom.id == atom_id)
    }

    pub fn atoms_for_layer(&self, layer: &ReviewStackLayer) -> Vec<&ChangeAtom> {
        layer
            .atom_ids
            .iter()
            .filter_map(|atom_id| self.atom(atom_id))
            .collect()
    }

    pub fn first_file_for_layer(&self, layer: &ReviewStackLayer) -> Option<String> {
        self.atoms_for_layer(layer)
            .into_iter()
            .find(|atom| !atom.path.is_empty())
            .map(|atom| atom.path.clone())
            .or_else(|| layer.pr.as_ref().map(|pr| pr.head_ref_name.clone()))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum StackKind {
    Real,
    Virtual,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum StackSource {
    GitHubNative,
    BranchTopology,
    LocalStackMetadata,
    VirtualCommits,
    VirtualSemantic,
    VirtualAi,
}

impl StackSource {
    pub fn label(&self) -> &'static str {
        match self {
            Self::GitHubNative => "GitHub native",
            Self::BranchTopology => "Branch topology",
            Self::LocalStackMetadata => "Local metadata",
            Self::VirtualCommits => "Virtual commits",
            Self::VirtualSemantic => "Virtual semantic",
            Self::VirtualAi => "Virtual AI",
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub enum Confidence {
    High,
    Medium,
    Low,
}

impl Confidence {
    pub fn label(&self) -> &'static str {
        match self {
            Self::High => "High confidence",
            Self::Medium => "Medium confidence",
            Self::Low => "Low confidence",
        }
    }

    pub fn min(self, other: Self) -> Self {
        use Confidence::*;
        match (self, other) {
            (Low, _) | (_, Low) => Low,
            (Medium, _) | (_, Medium) => Medium,
            (High, High) => High,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewStackLayer {
    pub id: ReviewStackLayerId,
    pub index: usize,
    pub title: String,
    pub summary: String,
    pub rationale: String,
    pub pr: Option<StackPullRequestRef>,
    pub virtual_layer: Option<VirtualLayerRef>,
    pub base_oid: Option<String>,
    pub head_oid: Option<String>,
    #[serde(default)]
    pub atom_ids: Vec<ChangeAtomId>,
    #[serde(default)]
    pub depends_on_layer_ids: Vec<ReviewStackLayerId>,
    pub metrics: LayerMetrics,
    pub status: LayerReviewStatus,
    pub confidence: Confidence,
    #[serde(default)]
    pub warnings: Vec<StackWarning>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StackPullRequestRef {
    pub repository: String,
    pub number: i64,
    pub title: String,
    pub url: String,
    pub base_ref_name: String,
    pub head_ref_name: String,
    #[serde(default)]
    pub base_ref_oid: Option<String>,
    #[serde(default)]
    pub head_ref_oid: Option<String>,
    pub review_decision: Option<String>,
    pub state: String,
    pub is_draft: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VirtualLayerRef {
    pub source: StackSource,
    pub role: ChangeRole,
    pub source_label: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LayerMetrics {
    pub file_count: usize,
    pub atom_count: usize,
    pub additions: usize,
    pub deletions: usize,
    pub changed_lines: usize,
    pub unresolved_thread_count: usize,
    pub risk_score: i64,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum LayerReviewStatus {
    #[default]
    NotReviewed,
    Reviewing,
    Reviewed,
    ChangedSinceReview,
}

impl LayerReviewStatus {
    pub fn label(&self) -> &'static str {
        match self {
            Self::NotReviewed => "Not reviewed",
            Self::Reviewing => "Reviewing",
            Self::Reviewed => "Reviewed",
            Self::ChangedSinceReview => "Changed",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeAtom {
    pub id: ChangeAtomId,
    pub source: ChangeAtomSource,
    pub path: String,
    pub previous_path: Option<String>,
    pub role: ChangeRole,
    pub semantic_kind: Option<String>,
    pub symbol_name: Option<String>,
    #[serde(default)]
    pub defined_symbols: Vec<String>,
    #[serde(default)]
    pub referenced_symbols: Vec<String>,
    pub old_range: Option<LineRange>,
    pub new_range: Option<LineRange>,
    #[serde(default)]
    pub hunk_headers: Vec<String>,
    #[serde(default)]
    pub hunk_indices: Vec<usize>,
    pub additions: usize,
    pub deletions: usize,
    pub patch_hash: String,
    pub risk_score: i64,
    #[serde(default)]
    pub review_thread_ids: Vec<String>,
    #[serde(default)]
    pub warnings: Vec<StackWarning>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ChangeAtomSource {
    File,
    Hunk { hunk_index: usize },
    SemanticSection { section_id: String },
    Commit { commit_oid: String },
    GeneratedPlaceholder,
    BinaryPlaceholder,
}

impl ChangeAtomSource {
    pub fn stable_kind(&self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Hunk { .. } => "hunk",
            Self::SemanticSection { .. } => "semantic-section",
            Self::Commit { .. } => "commit",
            Self::GeneratedPlaceholder => "generated",
            Self::BinaryPlaceholder => "binary",
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub enum ChangeRole {
    Foundation,
    CoreLogic,
    Integration,
    Presentation,
    Tests,
    Docs,
    Config,
    Generated,
    Unknown,
}

impl ChangeRole {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Foundation => "Foundation",
            Self::CoreLogic => "Core behavior",
            Self::Integration => "Integration",
            Self::Presentation => "Presentation",
            Self::Tests => "Tests",
            Self::Docs => "Docs",
            Self::Config => "Config",
            Self::Generated => "Generated",
            Self::Unknown => "Unassigned",
        }
    }

    pub fn order(&self) -> usize {
        match self {
            Self::Foundation => 0,
            Self::CoreLogic => 1,
            Self::Integration => 2,
            Self::Presentation => 3,
            Self::Tests => 4,
            Self::Docs => 5,
            Self::Config => 0,
            Self::Generated => 6,
            Self::Unknown => 7,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LineRange {
    pub start: i64,
    pub end: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StackWarning {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub path: Option<String>,
}

impl StackWarning {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            path: None,
        }
    }

    pub fn for_path(
        code: impl Into<String>,
        message: impl Into<String>,
        path: impl Into<String>,
    ) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            path: Some(path.into()),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StackProviderMetadata {
    pub provider: String,
    #[serde(default)]
    pub raw_payload: Option<serde_json::Value>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum StackDiffMode {
    WholePr,
    CurrentLayerOnly,
    UpToCurrentLayer,
    CurrentAndDependents,
    SinceLastReviewed,
}

impl Default for StackDiffMode {
    fn default() -> Self {
        Self::WholePr
    }
}

impl StackDiffMode {
    pub fn label(&self) -> &'static str {
        match self {
            Self::WholePr => "Whole PR",
            Self::CurrentLayerOnly => "Current layer",
            Self::UpToCurrentLayer => "Up to layer",
            Self::CurrentAndDependents => "Dependents",
            Self::SinceLastReviewed => "Reviewed",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct LayerDiffFilter {
    pub mode: StackDiffMode,
    pub selected_layer_id: Option<ReviewStackLayerId>,
    pub visible_atom_ids: std::collections::BTreeSet<ChangeAtomId>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StackReviewProgress {
    pub stack_id: ReviewStackId,
    pub repository: String,
    pub pr_number: i64,
    #[serde(default)]
    pub reviewed_layer_ids: Vec<ReviewStackLayerId>,
    #[serde(default)]
    pub reviewed_atom_ids: Vec<ChangeAtomId>,
    pub current_layer_id: Option<ReviewStackLayerId>,
    pub last_location: Option<crate::review_session::ReviewLocation>,
    pub updated_at_ms: i64,
}

#[derive(Clone, Debug)]
pub struct RepoContext {
    pub open_pull_requests: Vec<StackPullRequestRef>,
    pub local_repo_path: Option<PathBuf>,
    pub trunk_branch: Option<String>,
}

impl RepoContext {
    pub fn empty() -> Self {
        Self {
            open_pull_requests: Vec::new(),
            local_repo_path: None,
            trunk_branch: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct StackDiscoveryOptions {
    pub enable_github_native: bool,
    pub enable_branch_topology: bool,
    pub enable_local_metadata: bool,
    pub enable_ai_virtual: bool,
    pub enable_virtual_commits: bool,
    pub enable_virtual_semantic: bool,
    pub ai_provider: Option<crate::code_tour::CodeTourProvider>,
    pub sizing: VirtualStackSizing,
}

impl Default for StackDiscoveryOptions {
    fn default() -> Self {
        Self {
            enable_github_native: true,
            enable_branch_topology: true,
            enable_local_metadata: true,
            enable_ai_virtual: true,
            enable_virtual_commits: true,
            enable_virtual_semantic: true,
            ai_provider: None,
            sizing: VirtualStackSizing::default(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct VirtualStackSizing {
    pub target_min_layers: usize,
    pub target_max_layers: usize,
    pub max_layer_changed_lines: usize,
    pub max_layer_files: usize,
    pub max_atom_changed_lines: usize,
}

impl Default for VirtualStackSizing {
    fn default() -> Self {
        Self {
            target_min_layers: 3,
            target_max_layers: 8,
            max_layer_changed_lines: 300,
            max_layer_files: 8,
            max_atom_changed_lines: 120,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StackDiscoveryError {
    pub message: String,
}

impl StackDiscoveryError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for StackDiscoveryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for StackDiscoveryError {}

pub fn stack_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}
