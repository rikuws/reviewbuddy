use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::code_tour::{
    CodeTourProgressUpdate, CodeTourProvider, CodeTourProviderStatus, GenerateCodeTourInput,
    GeneratedCodeTour,
};

use super::binary::find_copilot_binary;
use super::jsonrepair::parse_tolerant;
use super::merge::{build_copilot_fallback_tour, merge_tour, TourResponse};
use super::progress::make_progress;
use super::prompt::build_tour_prompt;
use super::CodingAgentBackend;

const OVERALL_TIMEOUT_MS: i64 = 240_000;
const INACTIVITY_TIMEOUT_MS: i64 = 60_000;
const RUNNING_TICKER_MS: i64 = 10_000;
const POLL_INTERVAL: Duration = Duration::from_millis(120);
const MAX_PROMPT_BYTES: usize = 120_000;
const ALLOW_TOOLS: &str = "view,rg,grep,glob";
const DENY_TOOLS: &str = "shell,write,create,edit,apply_patch,url,web_fetch,task";

pub struct CopilotBackend;

impl CopilotBackend {
    pub fn new() -> Self {
        Self
    }
}

impl Default for CopilotBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl CodingAgentBackend for CopilotBackend {
    fn provider(&self) -> CodeTourProvider {
        CodeTourProvider::Copilot
    }

    fn status(&self) -> Result<CodeTourProviderStatus, String> {
        let Some(binary) = find_copilot_binary() else {
            return Ok(CodeTourProviderStatus {
                provider: CodeTourProvider::Copilot,
                label: "Copilot".to_string(),
                available: false,
                authenticated: false,
                message: "GitHub Copilot CLI is not installed on PATH.".to_string(),
                detail: "Install the GitHub Copilot CLI and sign in with `copilot login` to enable AI code tours.".to_string(),
                default_model: None,
            });
        };

        let version = probe_version(&binary).unwrap_or_else(|_| "installed".to_string());

        Ok(CodeTourProviderStatus {
            provider: CodeTourProvider::Copilot,
            label: "Copilot".to_string(),
            available: true,
            authenticated: true,
            message: format!("GitHub Copilot CLI detected ({}).", version),
            detail:
                "Uses the detected Copilot CLI session. Auth errors surface on the first generate."
                    .to_string(),
            default_model: None,
        })
    }

    fn generate(
        &self,
        input: &GenerateCodeTourInput,
        on_progress: &mut dyn FnMut(CodeTourProgressUpdate),
    ) -> Result<GeneratedCodeTour, String> {
        let Some(binary) = find_copilot_binary() else {
            return Err("GitHub Copilot CLI is not installed on PATH.".to_string());
        };

        if !Path::new(&input.working_directory).is_dir() {
            return Err(format!(
                "The linked local repository '{}' does not exist.",
                input.working_directory
            ));
        }

        on_progress(make_progress(
            "startup",
            "Starting GitHub Copilot",
            Some("Launching the local Copilot CLI in the linked checkout.".to_string()),
            Some("Starting Copilot CLI".to_string()),
        ));

        let mut prompt = build_tour_prompt(input);
        if prompt.len() > MAX_PROMPT_BYTES {
            prompt.truncate(MAX_PROMPT_BYTES);
        }

        let mut child = Command::new(&binary)
            .arg("-p")
            .arg(&prompt)
            .arg("-s")
            .arg("--allow-tool")
            .arg(ALLOW_TOOLS)
            .arg("--deny-tool")
            .arg(DENY_TOOLS)
            .current_dir(&input.working_directory)
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("Failed to launch the Copilot CLI: {error}"))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "Failed to capture the Copilot CLI stdout.".to_string())?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "Failed to capture the Copilot CLI stderr.".to_string())?;

        let start_ms = now_ms();
        let last_activity = Arc::new(AtomicI64::new(start_ms));
        let stdout_bytes = Arc::new(Mutex::new(Vec::<u8>::new()));
        let stderr_bytes = Arc::new(Mutex::new(Vec::<u8>::new()));

        let stdout_handle = spawn_reader(stdout, Arc::clone(&stdout_bytes), Arc::clone(&last_activity));
        let stderr_handle = spawn_reader(stderr, Arc::clone(&stderr_bytes), Arc::clone(&last_activity));

        on_progress(make_progress(
            "running",
            "GitHub Copilot is inspecting the checkout",
            Some("Waiting for Copilot to finish gathering context.".to_string()),
            Some("Inspecting the linked checkout".to_string()),
        ));

        let mut last_ticker_ms = start_ms;
        let mut abort_reason: Option<String> = None;

        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => {}
                Err(error) => {
                    let _ = child.kill();
                    return Err(format!("Failed to poll the Copilot CLI: {error}"));
                }
            }

            let now = now_ms();

            if now - start_ms > OVERALL_TIMEOUT_MS {
                abort_reason = Some("overall".to_string());
                break;
            }

            if now - last_activity.load(Ordering::Relaxed) > INACTIVITY_TIMEOUT_MS {
                abort_reason = Some("inactivity".to_string());
                break;
            }

            if now - last_ticker_ms >= RUNNING_TICKER_MS {
                last_ticker_ms = now;
                on_progress(make_progress(
                    "running",
                    "GitHub Copilot is still working",
                    Some(format!(
                        "Elapsed: {}s.",
                        ((now - start_ms) / 1000).max(0)
                    )),
                    Some("Copilot still working".to_string()),
                ));
            }

            thread::sleep(POLL_INTERVAL);
        }

        if let Some(kind) = &abort_reason {
            let _ = child.kill();
            let _ = child.wait();
            let summary = match kind.as_str() {
                "inactivity" => "GitHub Copilot stopped reporting progress.".to_string(),
                _ => "GitHub Copilot timed out while generating the code tour.".to_string(),
            };
            on_progress(make_progress(
                "timeout",
                summary.clone(),
                Some(
                    "Aborting the Copilot run so the app can surface the failure without waiting."
                        .to_string(),
                ),
                Some(summary.clone()),
            ));
            return Err(summary);
        }

        let _ = stdout_handle.join();
        let _ = stderr_handle.join();

        on_progress(make_progress(
            "finalizing",
            "GitHub Copilot finished the draft",
            Some("Parsing the structured response and merging it into the final code tour.".to_string()),
            Some("Finalizing Copilot output".to_string()),
        ));

        let stdout_bytes = match stdout_bytes.lock() {
            Ok(guard) => guard.clone(),
            Err(_) => Vec::new(),
        };
        let stderr_bytes = match stderr_bytes.lock() {
            Ok(guard) => guard.clone(),
            Err(_) => Vec::new(),
        };

        let stdout_text = String::from_utf8_lossy(&stdout_bytes).to_string();
        let stderr_text = String::from_utf8_lossy(&stderr_bytes).trim().to_string();

        if stdout_text.trim().is_empty() {
            let reason = if stderr_text.is_empty() {
                "GitHub Copilot returned an empty code tour response.".to_string()
            } else {
                format!("GitHub Copilot reported: {stderr_text}")
            };
            return Ok(build_copilot_fallback_tour(input, None, reason));
        }

        match parse_tolerant::<TourResponse>(&stdout_text) {
            Ok(response) => Ok(merge_tour(response, input, None)),
            Err(error) => Ok(build_copilot_fallback_tour(
                input,
                None,
                format!(
                    "GitHub Copilot did not return a usable JSON code tour: {}",
                    error.message
                ),
            )),
        }
    }
}

fn probe_version(binary: &str) -> Result<String, String> {
    let output = Command::new(binary)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| format!("Failed to run `{binary} --version`: {error}"))?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(stdout
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("installed")
        .to_string())
}

fn spawn_reader<R: Read + Send + 'static>(
    mut reader: R,
    sink: Arc<Mutex<Vec<u8>>>,
    last_activity: Arc<AtomicI64>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut buffer = [0u8; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    if let Ok(mut guard) = sink.lock() {
                        guard.extend_from_slice(&buffer[..n]);
                    }
                    last_activity.store(now_ms(), Ordering::Relaxed);
                }
                Err(_) => break,
            }
        }
    })
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_millis() as i64)
        .unwrap_or_default()
}
