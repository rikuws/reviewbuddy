use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::code_tour::{
    CodeTourProgressUpdate, CodeTourProvider, CodeTourProviderStatus, GenerateCodeTourInput,
    GeneratedCodeTour,
};

use super::binary::find_copilot_binary;
use super::errors::{generation_abort_message, AbortKind, AbortReason};
use super::jsonrepair::parse_tolerant;
use super::merge::{build_copilot_fallback_tour, merge_tour, TourResponse};
use super::progress::{limit_text, make_progress};
use super::prompt::build_tour_prompt;
use super::CodingAgentBackend;

const OVERALL_TIMEOUT_MS: u64 = 480_000;
const INACTIVITY_TIMEOUT_MS: u64 = 120_000;
const RUNNING_TICKER_MS: u64 = 10_000;
const POLL_INTERVAL: Duration = Duration::from_millis(120);
const MAX_PROMPT_BYTES: usize = 120_000;
const AVAILABLE_TOOLS: &str = "view,rg,glob";

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

#[derive(Debug, Default)]
struct CopilotOutcome {
    final_text: Option<String>,
    last_visible_activity: Option<String>,
    abort: Option<AbortReason>,
    error: Option<String>,
    model: Option<String>,
    saw_meaningful_progress: bool,
    stderr_text: String,
    current_turn_stream: String,
    exit_code: Option<i32>,
}

enum StreamLine {
    Stdout(String),
    Stderr(String),
}

#[derive(Copy, Clone)]
enum StreamKind {
    Stdout,
    Stderr,
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
            Some(
                "Launching the local Copilot CLI with streamed progress in the linked checkout."
                    .to_string(),
            ),
            Some("Starting Copilot CLI".to_string()),
        ));

        let mut prompt = build_tour_prompt(input);
        if prompt.len() > MAX_PROMPT_BYTES {
            prompt.truncate(MAX_PROMPT_BYTES);
        }

        let mut child = Command::new(&binary)
            .arg("-p")
            .arg(&prompt)
            .arg("--output-format")
            .arg("json")
            .arg("--stream")
            .arg("on")
            .arg("--allow-all-tools")
            .arg("--available-tools")
            .arg(AVAILABLE_TOOLS)
            .arg("--no-ask-user")
            .arg("--no-color")
            .arg("--log-level")
            .arg("error")
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

        let (line_tx, line_rx) = mpsc::channel::<StreamLine>();
        let stdout_handle = spawn_line_reader(stdout, StreamKind::Stdout, line_tx.clone());
        let stderr_handle = spawn_line_reader(stderr, StreamKind::Stderr, line_tx);

        on_progress(make_progress(
            "running",
            "GitHub Copilot is inspecting the checkout",
            Some("Waiting for streamed Copilot events from the linked repository.".to_string()),
            Some("Waiting for Copilot event stream".to_string()),
        ));

        let start = Instant::now();
        let mut last_activity = Instant::now();
        let mut last_ticker = Instant::now();
        let mut exit_status: Option<ExitStatus> = None;
        let mut outcome = CopilotOutcome::default();

        loop {
            while let Ok(line) = line_rx.try_recv() {
                handle_stream_line(line, &mut outcome, on_progress);
                last_activity = Instant::now();
            }

            match child.try_wait() {
                Ok(Some(status)) => {
                    outcome.exit_code = status.code();
                    exit_status = Some(status);
                    break;
                }
                Ok(None) => {}
                Err(error) => {
                    let _ = child.kill();
                    return Err(format!("Failed to poll the Copilot CLI: {error}"));
                }
            }

            let now = Instant::now();

            if now.duration_since(start) > Duration::from_millis(OVERALL_TIMEOUT_MS) {
                outcome.abort = Some(AbortReason {
                    kind: AbortKind::Overall,
                    timeout_ms: OVERALL_TIMEOUT_MS,
                    last_visible_activity: outcome.last_visible_activity.clone(),
                });
                break;
            }

            if !outcome.saw_meaningful_progress
                && now.duration_since(last_activity) > Duration::from_millis(INACTIVITY_TIMEOUT_MS)
            {
                outcome.abort = Some(AbortReason {
                    kind: AbortKind::Inactivity,
                    timeout_ms: INACTIVITY_TIMEOUT_MS,
                    last_visible_activity: outcome.last_visible_activity.clone(),
                });
                break;
            }

            if now.duration_since(last_ticker) >= Duration::from_millis(RUNNING_TICKER_MS) {
                last_ticker = now;
                let elapsed_s = now.duration_since(start).as_secs();
                on_progress(make_progress(
                    "running",
                    "GitHub Copilot is still working",
                    Some(format!("Elapsed: {elapsed_s}s.")),
                    outcome.last_visible_activity.clone(),
                ));
            }

            match line_rx.recv_timeout(POLL_INTERVAL) {
                Ok(line) => {
                    handle_stream_line(line, &mut outcome, on_progress);
                    last_activity = Instant::now();
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {}
            }
        }

        let timed_out = outcome.abort.clone();
        if timed_out.is_some() {
            let _ = child.kill();
            let _ = child.wait();
        }

        let _ = stdout_handle.join();
        let _ = stderr_handle.join();
        while let Ok(line) = line_rx.try_recv() {
            handle_stream_line(line, &mut outcome, on_progress);
        }
        promote_stream_to_final(&mut outcome);

        if let Some(abort) = timed_out {
            if !has_usable_final_text(&outcome) {
                let summary = generation_abort_message("GitHub Copilot", &abort);
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
        }

        if let Some(error) = &outcome.error {
            return Err(error.clone());
        }

        on_progress(make_progress(
            "finalizing",
            "GitHub Copilot finished the draft",
            Some(
                "Parsing the structured response and merging it into the final code tour."
                    .to_string(),
            ),
            Some("Finalizing Copilot output".to_string()),
        ));

        let Some(final_text) = outcome.final_text.as_deref() else {
            return Ok(build_copilot_fallback_tour(
                input,
                outcome.model.clone(),
                fallback_reason(&outcome, exit_status.as_ref()),
            ));
        };

        let trimmed = final_text.trim();
        if trimmed.is_empty() {
            return Ok(build_copilot_fallback_tour(
                input,
                outcome.model.clone(),
                fallback_reason(&outcome, exit_status.as_ref()),
            ));
        }

        match parse_tolerant::<TourResponse>(trimmed) {
            Ok(response) => Ok(merge_tour(response, input, outcome.model)),
            Err(error) => Ok(build_copilot_fallback_tour(
                input,
                outcome.model,
                format!(
                    "GitHub Copilot did not return a usable JSON code tour: {}",
                    error.message
                ),
            )),
        }
    }
}

fn handle_stream_line(
    line: StreamLine,
    outcome: &mut CopilotOutcome,
    on_progress: &mut dyn FnMut(CodeTourProgressUpdate),
) {
    match line {
        StreamLine::Stdout(line) => handle_stdout_line(&line, outcome, on_progress),
        StreamLine::Stderr(line) => {
            append_line(&mut outcome.stderr_text, &line);
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                outcome.last_visible_activity = Some(limit_text(trimmed, 180));
            }
        }
    }
}

fn handle_stdout_line(
    line: &str,
    outcome: &mut CopilotOutcome,
    on_progress: &mut dyn FnMut(CodeTourProgressUpdate),
) {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return;
    }

    let event = match serde_json::from_str::<Value>(trimmed) {
        Ok(event) => event,
        Err(_) => {
            append_line(&mut outcome.current_turn_stream, trimmed);
            outcome.last_visible_activity = Some(limit_text(trimmed, 180));
            return;
        }
    };

    handle_json_event(&event, outcome, on_progress);
}

fn handle_json_event(
    event: &Value,
    outcome: &mut CopilotOutcome,
    on_progress: &mut dyn FnMut(CodeTourProgressUpdate),
) {
    let event_type = event
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let data = event.get("data").cloned().unwrap_or(Value::Null);

    match event_type {
        "session.tools_updated" => {
            if let Some(model) = data.get("model").and_then(Value::as_str) {
                outcome.model = Some(model.to_string());
                outcome.last_visible_activity = Some(format!("Using model {model}"));
            }
        }
        "assistant.turn_start" => {
            outcome.current_turn_stream.clear();
            outcome.saw_meaningful_progress = true;
            let turn_id = data.get("turnId").and_then(Value::as_str).unwrap_or("0");
            let (summary, detail, log) = if turn_id == "0" {
                (
                    "GitHub Copilot is inspecting the checkout",
                    "Copilot started its first turn and is gathering repository context.",
                    "Started Copilot turn 0".to_string(),
                )
            } else {
                (
                    "GitHub Copilot is drafting the code tour",
                    "Copilot started another turn and is preparing the final structured response.",
                    format!("Started Copilot turn {turn_id}"),
                )
            };
            on_progress(make_progress(
                "running",
                summary,
                Some(detail.to_string()),
                Some(log.clone()),
            ));
            outcome.last_visible_activity = Some(log);
        }
        "assistant.message_delta" => {
            if let Some(delta) = data.get("deltaContent").and_then(Value::as_str) {
                outcome.saw_meaningful_progress = true;
                outcome.current_turn_stream.push_str(delta);
                let snippet = limit_text(&outcome.current_turn_stream, 180);
                if !snippet.is_empty() {
                    outcome.last_visible_activity = Some(snippet);
                }
            }
        }
        "assistant.message" => handle_assistant_message(&data, outcome, on_progress),
        "tool.execution_start" => {
            outcome.saw_meaningful_progress = true;
            let log = tool_activity_summary(&data);
            on_progress(make_progress(
                "tool",
                "GitHub Copilot is using a repository tool",
                Some(log.clone()),
                Some(log.clone()),
            ));
            outcome.last_visible_activity = Some(log);
        }
        "tool.execution_complete" => {
            outcome.saw_meaningful_progress = true;
            let success = data.get("success").and_then(Value::as_bool).unwrap_or(true);
            let log = tool_activity_summary(&data);
            if success {
                outcome.last_visible_activity = Some(format!("Completed {log}"));
            } else {
                let detail = format!("Tool failed: {log}");
                on_progress(make_progress(
                    "tool_failed",
                    "A GitHub Copilot tool step failed",
                    Some(detail.clone()),
                    Some(detail.clone()),
                ));
                outcome.last_visible_activity = Some(detail);
            }
        }
        "session.info" => {
            if let Some(message) = data.get("message").and_then(Value::as_str) {
                let trimmed = message.trim();
                if !trimmed.is_empty() {
                    if trimmed.contains("Unknown tool name") {
                        outcome.error =
                            Some(format!("GitHub Copilot CLI configuration error: {trimmed}"));
                    }
                    outcome.last_visible_activity = Some(limit_text(trimmed, 180));
                }
            }
        }
        "assistant.reasoning" => {
            if let Some(content) = data.get("content").and_then(Value::as_str) {
                let trimmed = content.trim();
                if !trimmed.is_empty() {
                    outcome.last_visible_activity = Some(limit_text(trimmed, 180));
                }
            }
        }
        "result" => {
            if let Some(code) = event.get("exitCode").and_then(Value::as_i64) {
                outcome.exit_code = Some(code as i32);
            }
        }
        _ => {}
    }
}

fn handle_assistant_message(
    data: &Value,
    outcome: &mut CopilotOutcome,
    on_progress: &mut dyn FnMut(CodeTourProgressUpdate),
) {
    let content = data
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if content.is_empty() {
        return;
    }

    outcome.saw_meaningful_progress = true;
    let phase = data.get("phase").and_then(Value::as_str);
    let tool_requests = data
        .get("toolRequests")
        .and_then(Value::as_array)
        .map(|items| items.len())
        .unwrap_or_default();

    if phase == Some("final_answer") {
        outcome.final_text = Some(content.to_string());
        outcome.current_turn_stream = content.to_string();
        on_progress(make_progress(
            "drafting",
            "GitHub Copilot drafted the code tour response",
            Some(limit_text(content, 240)),
            Some("Copilot drafted the final response".to_string()),
        ));
        outcome.last_visible_activity = Some("Copilot drafted the final response".to_string());
        return;
    }

    let detail = limit_text(content, 240);
    let log = summarize_tool_request(data).unwrap_or_else(|| detail.clone());

    if tool_requests > 0 {
        on_progress(make_progress(
            "running",
            "GitHub Copilot is inspecting the checkout",
            Some(detail.clone()),
            Some(log.clone()),
        ));
    } else {
        on_progress(make_progress(
            "running",
            "GitHub Copilot sent a progress update",
            Some(detail.clone()),
            Some(log.clone()),
        ));
    }

    outcome.last_visible_activity = Some(log);
}

fn summarize_tool_request(data: &Value) -> Option<String> {
    data.get("toolRequests")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|item| item.get("name").and_then(Value::as_str))
        .map(|name| format!("Tool: {name}"))
}

fn tool_activity_summary(data: &Value) -> String {
    let tool_name = data
        .get("toolName")
        .and_then(Value::as_str)
        .unwrap_or("tool");

    match preferred_argument(data.get("arguments").unwrap_or(&Value::Null)) {
        Some(arg) => format!("{tool_name}: {}", limit_text(&arg, 180)),
        None => format!("Tool: {tool_name}"),
    }
}

fn preferred_argument(arguments: &Value) -> Option<String> {
    let object = arguments.as_object()?;

    for key in ["path", "pattern", "query", "command", "url"] {
        if let Some(value) = object
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Some(value.to_string());
        }
    }

    if object.is_empty() {
        None
    } else {
        serde_json::to_string(arguments).ok()
    }
}

fn fallback_reason(outcome: &CopilotOutcome, exit_status: Option<&ExitStatus>) -> String {
    if let Some(error) = &outcome.error {
        return error.clone();
    }

    let stderr_text = outcome.stderr_text.trim();
    if !stderr_text.is_empty() {
        return format!("GitHub Copilot reported: {stderr_text}");
    }

    if let Some(activity) = outcome
        .last_visible_activity
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return format!(
            "GitHub Copilot returned no final response. Last visible activity: {activity}."
        );
    }

    match exit_status.and_then(ExitStatus::code).or(outcome.exit_code) {
        Some(code) => format!("GitHub Copilot exited with status code {code}."),
        None => "GitHub Copilot returned an empty code tour response.".to_string(),
    }
}

fn promote_stream_to_final(outcome: &mut CopilotOutcome) {
    if outcome.final_text.is_none() {
        let trimmed = outcome.current_turn_stream.trim();
        if !trimmed.is_empty() {
            outcome.final_text = Some(trimmed.to_string());
        }
    }
}

fn has_usable_final_text(outcome: &CopilotOutcome) -> bool {
    outcome
        .final_text
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_some()
}

fn append_line(target: &mut String, line: &str) {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return;
    }
    if !target.is_empty() {
        target.push('\n');
    }
    target.push_str(trimmed);
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

fn spawn_line_reader<R: std::io::Read + Send + 'static>(
    reader: R,
    kind: StreamKind,
    sender: mpsc::Sender<StreamLine>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let reader = BufReader::new(reader);
        for line in reader.lines() {
            let Ok(line) = line else {
                break;
            };
            let message = match kind {
                StreamKind::Stdout => StreamLine::Stdout(line),
                StreamKind::Stderr => StreamLine::Stderr(line),
            };
            if sender.send(message).is_err() {
                break;
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn session_tools_updated_captures_model() {
        let event = json!({
            "type": "session.tools_updated",
            "data": {
                "model": "gpt-5.4"
            }
        });

        let mut outcome = CopilotOutcome::default();
        let mut progress = Vec::new();
        handle_json_event(&event, &mut outcome, &mut |update| progress.push(update));

        assert_eq!(outcome.model.as_deref(), Some("gpt-5.4"));
        assert!(progress.is_empty());
    }

    #[test]
    fn assistant_message_delta_updates_current_turn_stream() {
        let event = json!({
            "type": "assistant.message_delta",
            "data": {
                "deltaContent": "{\"summary\""
            }
        });

        let mut outcome = CopilotOutcome::default();
        let mut progress = Vec::new();
        handle_json_event(&event, &mut outcome, &mut |update| progress.push(update));

        assert_eq!(outcome.current_turn_stream, "{\"summary\"");
        assert_eq!(
            outcome.last_visible_activity.as_deref(),
            Some("{\"summary\"")
        );
        assert!(progress.is_empty());
    }

    #[test]
    fn assistant_final_answer_sets_final_text_and_progress() {
        let event = json!({
            "type": "assistant.message",
            "data": {
                "content": "{\"summary\":\"done\"}",
                "toolRequests": [],
                "phase": "final_answer"
            }
        });

        let mut outcome = CopilotOutcome::default();
        let mut progress = Vec::new();
        handle_json_event(&event, &mut outcome, &mut |update| progress.push(update));

        assert_eq!(
            outcome.final_text.as_deref(),
            Some("{\"summary\":\"done\"}")
        );
        assert_eq!(outcome.current_turn_stream, "{\"summary\":\"done\"}");
        assert_eq!(progress.len(), 1);
        assert_eq!(progress[0].stage, "drafting");
        assert!(progress[0].summary.contains("drafted"));
    }

    #[test]
    fn assistant_turn_start_marks_meaningful_progress() {
        let event = json!({
            "type": "assistant.turn_start",
            "data": {
                "turnId": "6"
            }
        });

        let mut outcome = CopilotOutcome::default();
        let mut progress = Vec::new();
        handle_json_event(&event, &mut outcome, &mut |update| progress.push(update));

        assert!(outcome.saw_meaningful_progress);
        assert_eq!(progress.len(), 1);
        assert_eq!(
            progress[0].summary,
            "GitHub Copilot is drafting the code tour"
        );
    }

    #[test]
    fn promote_stream_to_final_uses_buffered_stream() {
        let mut outcome = CopilotOutcome {
            current_turn_stream: "{\"summary\":\"done\"}".to_string(),
            ..CopilotOutcome::default()
        };

        promote_stream_to_final(&mut outcome);

        assert_eq!(
            outcome.final_text.as_deref(),
            Some("{\"summary\":\"done\"}")
        );
        assert!(has_usable_final_text(&outcome));
    }

    #[test]
    fn tool_execution_start_reports_tool_progress() {
        let event = json!({
            "type": "tool.execution_start",
            "data": {
                "toolName": "view",
                "arguments": {
                    "path": "/tmp/repo"
                }
            }
        });

        let mut outcome = CopilotOutcome::default();
        let mut progress = Vec::new();
        handle_json_event(&event, &mut outcome, &mut |update| progress.push(update));

        assert_eq!(progress.len(), 1);
        assert_eq!(progress[0].stage, "tool");
        assert_eq!(progress[0].detail.as_deref(), Some("view: /tmp/repo"));
        assert_eq!(
            outcome.last_visible_activity.as_deref(),
            Some("view: /tmp/repo")
        );
    }

    #[test]
    fn session_info_unknown_tool_sets_error() {
        let event = json!({
            "type": "session.info",
            "data": {
                "message": "Unknown tool name in the tool allowlist: \"grep\""
            }
        });

        let mut outcome = CopilotOutcome::default();
        let mut progress = Vec::new();
        handle_json_event(&event, &mut outcome, &mut |update| progress.push(update));

        assert_eq!(
            outcome.error.as_deref(),
            Some("GitHub Copilot CLI configuration error: Unknown tool name in the tool allowlist: \"grep\"")
        );
        assert!(progress.is_empty());
    }
}
