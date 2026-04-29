use crate::code_tour::{
    CodeTourProgressUpdate, CodeTourProvider, CodeTourProviderStatus, GenerateCodeTourInput,
    GeneratedCodeTour,
};

pub mod binary;
pub mod codex;
pub mod copilot;
pub mod errors;
pub mod jsonrepair;
pub mod merge;
pub mod progress;
pub mod prompt;
pub mod runtime;
pub mod schema;

pub trait CodingAgentBackend: Send + Sync {
    #[allow(dead_code)]
    fn provider(&self) -> CodeTourProvider;
    fn status(&self) -> Result<CodeTourProviderStatus, String>;
    fn generate(
        &self,
        input: &GenerateCodeTourInput,
        on_progress: &mut dyn FnMut(CodeTourProgressUpdate),
    ) -> Result<GeneratedCodeTour, String>;
}

#[derive(Clone, Debug)]
pub struct AgentTextResponse {
    pub text: String,
    pub model: Option<String>,
}

pub fn run_json_prompt(
    provider: CodeTourProvider,
    working_directory: &str,
    prompt: String,
) -> Result<AgentTextResponse, String> {
    match provider {
        CodeTourProvider::Codex => codex::run_json_prompt(working_directory, prompt),
        CodeTourProvider::Copilot => copilot::run_json_prompt(working_directory, prompt),
    }
}

pub fn backend_for(provider: CodeTourProvider) -> Box<dyn CodingAgentBackend> {
    match provider {
        CodeTourProvider::Codex => Box::new(codex::CodexBackend::new()),
        CodeTourProvider::Copilot => Box::new(copilot::CopilotBackend::new()),
    }
}

pub fn load_all_statuses() -> Vec<CodeTourProviderStatus> {
    CodeTourProvider::all()
        .iter()
        .map(|provider| {
            let backend = backend_for(*provider);
            backend
                .status()
                .unwrap_or_else(|error| CodeTourProviderStatus {
                    provider: *provider,
                    label: provider.label().to_string(),
                    available: false,
                    authenticated: false,
                    message: error.clone(),
                    detail: error,
                    default_model: None,
                })
        })
        .collect()
}
