use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use codex_codes::cli::AppServerBuilder;
use codex_codes::client_async::AsyncClient;
use codex_codes::jsonrpc::RequestId;
use codex_codes::protocol::{
    methods, AgentMessageDeltaNotification, CommandApprovalDecision,
    CommandExecutionApprovalResponse, ErrorNotification, FileChangeApprovalDecision,
    FileChangeApprovalResponse, ItemCompletedNotification, ItemStartedNotification,
    ReasoningDeltaNotification, ServerMessage, ThreadStartParams, ThreadStartedNotification,
    TurnCompletedNotification, TurnStartedNotification, TurnStatus,
};
use codex_codes::{CommandExecutionStatus, McpToolCallStatus, ThreadItem};
use serde_json::{json, Value};
use tokio::time::timeout as tokio_timeout;

use crate::code_tour::{
    CodeTourProgressUpdate, CodeTourProvider, CodeTourProviderStatus, GenerateCodeTourInput,
    GeneratedCodeTour,
};

use super::binary::find_codex_binary;
use super::errors::{generation_abort_message, AbortKind, AbortReason};
use super::jsonrepair::parse_tolerant;
use super::merge::{merge_tour, TourResponse};
use super::progress::make_progress;
use super::prompt::build_tour_prompt;
use super::runtime;
use super::{AgentTextResponse, CodingAgentBackend};

const OVERALL_TIMEOUT_MS: u64 = 240_000;
const INACTIVITY_TIMEOUT_MS: u64 = 60_000;
const STACK_PLAN_OVERALL_TIMEOUT_MS: u64 = 90_000;
const STACK_PLAN_INACTIVITY_TIMEOUT_MS: u64 = 35_000;
const RUNNING_TICKER_MS: u64 = 10_000;
const NEXT_MESSAGE_POLL: Duration = Duration::from_millis(250);

pub struct CodexBackend;

impl CodexBackend {
    pub fn new() -> Self {
        Self
    }
}

impl Default for CodexBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl CodingAgentBackend for CodexBackend {
    fn provider(&self) -> CodeTourProvider {
        CodeTourProvider::Codex
    }

    fn status(&self) -> Result<CodeTourProviderStatus, String> {
        let Some(_binary) = find_codex_binary() else {
            return Ok(CodeTourProviderStatus {
                provider: CodeTourProvider::Codex,
                label: "Codex".to_string(),
                available: false,
                authenticated: false,
                message: "Codex CLI is not installed on PATH.".to_string(),
                detail: "Install the Codex CLI (https://platform.openai.com/docs/codex) and sign in with `codex login` to enable AI code tours.".to_string(),
                default_model: None,
            });
        };

        Ok(CodeTourProviderStatus {
            provider: CodeTourProvider::Codex,
            label: "Codex".to_string(),
            available: true,
            authenticated: true,
            message: "Codex CLI detected.".to_string(),
            detail: "Uses the detected Codex CLI session.".to_string(),
            default_model: None,
        })
    }

    fn generate(
        &self,
        input: &GenerateCodeTourInput,
        on_progress: &mut dyn FnMut(CodeTourProgressUpdate),
    ) -> Result<GeneratedCodeTour, String> {
        let Some(binary) = find_codex_binary() else {
            return Err("Codex CLI is not installed on PATH.".to_string());
        };

        if !std::path::Path::new(&input.working_directory).is_dir() {
            return Err(format!(
                "The local checkout '{}' does not exist.",
                input.working_directory
            ));
        }

        on_progress(make_progress(
            "startup",
            "Starting Codex",
            Some("Launching the Codex app-server in the prepared local checkout.".to_string()),
            Some("Starting Codex app-server".to_string()),
        ));

        let prompt = build_tour_prompt(input);
        let working_directory = PathBuf::from(&input.working_directory);
        let input_clone = input.clone();

        let (progress_tx, progress_rx) = mpsc::channel::<CodeTourProgressUpdate>();
        let (result_tx, result_rx) = mpsc::channel::<Result<CodexTurnOutcome, String>>();

        let worker = thread::spawn(move || {
            let outcome = runtime::shared().block_on(run_codex_turn(
                binary,
                working_directory,
                prompt,
                progress_tx,
                OVERALL_TIMEOUT_MS,
                INACTIVITY_TIMEOUT_MS,
            ));
            let _ = result_tx.send(outcome);
        });

        loop {
            while let Ok(progress) = progress_rx.try_recv() {
                on_progress(progress);
            }

            match result_rx.recv_timeout(Duration::from_millis(100)) {
                Ok(outcome) => {
                    while let Ok(progress) = progress_rx.try_recv() {
                        on_progress(progress);
                    }
                    let _ = worker.join();
                    return finalize_turn(outcome, &input_clone, on_progress);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let _ = worker.join();
                    return Err(
                        "Codex worker thread exited without reporting a result.".to_string()
                    );
                }
            }
        }
    }
}

pub fn run_json_prompt(
    working_directory: &str,
    prompt: String,
) -> Result<AgentTextResponse, String> {
    let Some(binary) = find_codex_binary() else {
        return Err("Codex CLI is not installed on PATH.".to_string());
    };

    if !std::path::Path::new(working_directory).is_dir() {
        return Err(format!(
            "The local checkout '{working_directory}' does not exist."
        ));
    }

    let working_directory = PathBuf::from(working_directory);
    let (progress_tx, progress_rx) = mpsc::channel::<CodeTourProgressUpdate>();
    let (result_tx, result_rx) = mpsc::channel::<Result<CodexTurnOutcome, String>>();

    let worker = thread::spawn(move || {
        let outcome = runtime::shared().block_on(run_codex_turn(
            binary,
            working_directory,
            prompt,
            progress_tx,
            STACK_PLAN_OVERALL_TIMEOUT_MS,
            STACK_PLAN_INACTIVITY_TIMEOUT_MS,
        ));
        let _ = result_tx.send(outcome);
    });

    loop {
        while progress_rx.try_recv().is_ok() {}

        match result_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(outcome) => {
                while progress_rx.try_recv().is_ok() {}
                let _ = worker.join();
                return finalize_text_turn(outcome);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let _ = worker.join();
                return Err("Codex worker thread exited without reporting a result.".to_string());
            }
        }
    }
}

struct CodexTurnOutcome {
    final_text: Option<String>,
    last_visible_activity: Option<String>,
    abort: Option<AbortReason>,
    model: Option<String>,
    error: Option<String>,
}

fn finalize_text_turn(
    outcome: Result<CodexTurnOutcome, String>,
) -> Result<AgentTextResponse, String> {
    let outcome = outcome?;

    if let Some(abort) = &outcome.abort {
        return Err(generation_abort_message("Codex", abort));
    }

    if let Some(error) = &outcome.error {
        return Err(format!("Codex reported an error: {error}"));
    }

    let Some(final_text) = outcome.final_text.as_deref() else {
        let reason = outcome
            .last_visible_activity
            .unwrap_or_else(|| "Codex did not return a final message.".to_string());
        return Err(format!("Codex returned no final agent message: {reason}"));
    };

    let trimmed = final_text.trim();
    if trimmed.is_empty() {
        return Err("Codex returned an empty JSON response.".to_string());
    }

    Ok(AgentTextResponse {
        text: trimmed.to_string(),
        model: outcome.model,
    })
}

fn finalize_turn(
    outcome: Result<CodexTurnOutcome, String>,
    input: &GenerateCodeTourInput,
    on_progress: &mut dyn FnMut(CodeTourProgressUpdate),
) -> Result<GeneratedCodeTour, String> {
    let outcome = outcome?;

    if let Some(abort) = &outcome.abort {
        let summary = generation_abort_message("Codex", abort);
        on_progress(make_progress(
            "timeout",
            summary.clone(),
            Some(
                "Aborting the Codex run so the app can surface the failure without waiting."
                    .to_string(),
            ),
            Some(summary.clone()),
        ));
        return Err(summary);
    }

    if let Some(error) = &outcome.error {
        return Err(format!("Codex reported an error: {error}"));
    }

    on_progress(make_progress(
        "finalizing",
        "Codex finished the draft",
        Some(
            "Parsing the structured response and merging it into the final code tour.".to_string(),
        ),
        Some("Finalizing Codex output".to_string()),
    ));

    let Some(final_text) = outcome.final_text.as_deref() else {
        let reason = outcome
            .last_visible_activity
            .unwrap_or_else(|| "Codex did not return a final message.".to_string());
        return Err(format!("Codex returned no final agent message: {reason}"));
    };

    let trimmed = final_text.trim();
    if trimmed.is_empty() {
        return Err("Codex returned an empty code tour response.".to_string());
    }

    match parse_tolerant::<TourResponse>(trimmed) {
        Ok(response) => Ok(merge_tour(response, input, outcome.model)),
        Err(error) => Err(format!(
            "Codex did not return a usable JSON code tour: {}",
            error.message
        )),
    }
}

async fn run_codex_turn(
    binary: String,
    working_directory: PathBuf,
    prompt: String,
    progress_tx: mpsc::Sender<CodeTourProgressUpdate>,
    overall_timeout_ms: u64,
    inactivity_timeout_ms: u64,
) -> Result<CodexTurnOutcome, String> {
    let builder = AppServerBuilder::new()
        .command(&binary)
        .working_directory(&working_directory);

    let mut client = AsyncClient::start_with(builder)
        .await
        .map_err(|error| format!("Failed to start the Codex app-server: {error}"))?;

    let thread_response = client
        .thread_start(&ThreadStartParams {
            instructions: None,
            tools: None,
        })
        .await
        .map_err(|error| format!("Failed to open a Codex thread: {error}"))?;
    let thread_id = thread_response.thread_id().to_string();
    let model = thread_response.model.clone();

    let turn_start_params = build_turn_start_params(&thread_id, &prompt);

    client
        .request::<_, Value>(methods::TURN_START, &turn_start_params)
        .await
        .map_err(|error| format!("Failed to start a Codex turn: {error}"))?;

    let start = Instant::now();
    let mut last_activity = Instant::now();
    let mut last_ticker = Instant::now();

    let mut outcome = CodexTurnOutcome {
        final_text: None,
        last_visible_activity: None,
        abort: None,
        model,
        error: None,
    };
    let mut streaming_message = String::new();

    loop {
        let now = Instant::now();
        if now.duration_since(start) > Duration::from_millis(overall_timeout_ms) {
            outcome.abort = Some(AbortReason {
                kind: AbortKind::Overall,
                timeout_ms: overall_timeout_ms,
                last_visible_activity: outcome.last_visible_activity.clone(),
            });
            break;
        }
        if now.duration_since(last_activity) > Duration::from_millis(inactivity_timeout_ms) {
            outcome.abort = Some(AbortReason {
                kind: AbortKind::Inactivity,
                timeout_ms: inactivity_timeout_ms,
                last_visible_activity: outcome.last_visible_activity.clone(),
            });
            break;
        }

        if now.duration_since(last_ticker) >= Duration::from_millis(RUNNING_TICKER_MS) {
            last_ticker = now;
            let elapsed_s = now.duration_since(start).as_secs();
            let _ = progress_tx.send(make_progress(
                "running",
                "Codex is still working",
                Some(format!("Elapsed: {elapsed_s}s.")),
                Some("Codex still working".to_string()),
            ));
        }

        let next = tokio_timeout(NEXT_MESSAGE_POLL, client.next_message()).await;
        let message = match next {
            Err(_) => continue,
            Ok(Ok(Some(message))) => message,
            Ok(Ok(None)) => {
                outcome.error = Some("Codex app-server closed the connection.".to_string());
                break;
            }
            Ok(Err(error)) => {
                outcome.error = Some(format!("Codex app-server error: {error}"));
                break;
            }
        };

        last_activity = Instant::now();

        match message {
            ServerMessage::Notification { method, params } => {
                let finished = handle_notification(
                    &method,
                    params,
                    &progress_tx,
                    &mut outcome,
                    &mut streaming_message,
                );
                if finished {
                    break;
                }
            }
            ServerMessage::Request { id, method, params } => {
                handle_request(&mut client, id, &method, params, &progress_tx).await;
            }
        }
    }

    let _ = client.shutdown().await;
    Ok(outcome)
}

fn handle_notification(
    method: &str,
    params: Option<Value>,
    progress_tx: &mpsc::Sender<CodeTourProgressUpdate>,
    outcome: &mut CodexTurnOutcome,
    streaming_message: &mut String,
) -> bool {
    match method {
        methods::THREAD_STARTED => {
            if let Some(params) = params.clone() {
                let _: Result<ThreadStartedNotification, _> = serde_json::from_value(params);
            }
            let _ = progress_tx.send(make_progress(
                "thread",
                "Codex started a new thread",
                Some("The agent is ready to inspect the prepared local checkout.".to_string()),
                Some("Started Codex thread".to_string()),
            ));
            outcome.last_visible_activity = Some("Started Codex thread".to_string());
        }
        methods::TURN_STARTED => {
            if let Some(params) = params.clone() {
                let _: Result<TurnStartedNotification, _> = serde_json::from_value(params);
            }
            let _ = progress_tx.send(make_progress(
                "turn",
                "Codex is inspecting the change",
                Some(
                    "Walking the changed files and related callsites from the checkout."
                        .to_string(),
                ),
                Some("Inspecting the changed files".to_string()),
            ));
            outcome.last_visible_activity = Some("Inspecting the changed files".to_string());
        }
        methods::ITEM_STARTED => {
            if let Some(params) = params {
                if let Ok(notif) = serde_json::from_value::<ItemStartedNotification>(params) {
                    progress_for_item(&notif.item, ItemLifecycle::Started, progress_tx, outcome);
                }
            }
        }
        methods::ITEM_COMPLETED => {
            if let Some(params) = params {
                if let Ok(notif) = serde_json::from_value::<ItemCompletedNotification>(params) {
                    if let ThreadItem::AgentMessage(ref msg) = notif.item {
                        if !msg.text.trim().is_empty() {
                            outcome.final_text = Some(msg.text.clone());
                        }
                    }
                    progress_for_item(&notif.item, ItemLifecycle::Completed, progress_tx, outcome);
                }
            }
        }
        methods::AGENT_MESSAGE_DELTA => {
            if let Some(params) = params {
                if let Ok(notif) = serde_json::from_value::<AgentMessageDeltaNotification>(params) {
                    streaming_message.push_str(&notif.delta);
                }
            }
        }
        methods::REASONING_DELTA => {
            if let Some(params) = params {
                if let Ok(notif) = serde_json::from_value::<ReasoningDeltaNotification>(params) {
                    let trimmed = notif.delta.trim();
                    if !trimmed.is_empty() {
                        let snippet = short_text(trimmed, 240);
                        let _ = progress_tx.send(make_progress(
                            "reasoning",
                            "Codex is reasoning through the change",
                            Some(snippet.clone()),
                            Some(short_text(trimmed, 180)),
                        ));
                        outcome.last_visible_activity = Some(snippet);
                    }
                }
            }
        }
        methods::TURN_COMPLETED => {
            if let Some(params) = params {
                if let Ok(notif) = serde_json::from_value::<TurnCompletedNotification>(params) {
                    if outcome.final_text.is_none() {
                        outcome.final_text = final_agent_message(&notif);
                    }
                    if matches!(notif.turn.status, TurnStatus::Failed) {
                        outcome.error = notif
                            .turn
                            .error
                            .map(|err| err.message)
                            .or_else(|| Some("Codex turn failed.".to_string()));
                    }
                }
            }
            if outcome.final_text.is_none() && !streaming_message.trim().is_empty() {
                outcome.final_text = Some(std::mem::take(streaming_message));
            }
            let _ = progress_tx.send(make_progress(
                "finalizing",
                "Codex finished gathering context",
                Some("Formatting the structured code tour response.".to_string()),
                Some("Codex finished its turn".to_string()),
            ));
            return true;
        }
        methods::ERROR => {
            if let Some(params) = params {
                if let Ok(notif) = serde_json::from_value::<ErrorNotification>(params) {
                    outcome.error = Some(notif.error);
                }
            }
        }
        _ => {}
    }

    false
}

#[derive(Copy, Clone, Eq, PartialEq)]
enum ItemLifecycle {
    Started,
    Completed,
}

fn progress_for_item(
    item: &ThreadItem,
    lifecycle: ItemLifecycle,
    progress_tx: &mpsc::Sender<CodeTourProgressUpdate>,
    outcome: &mut CodexTurnOutcome,
) {
    match item {
        ThreadItem::CommandExecution(cmd) if lifecycle == ItemLifecycle::Started => {
            let summary = format!("Command: {}", short_text(&cmd.command, 160));
            let _ = progress_tx.send(make_progress(
                "command",
                "Codex is running a checkout command",
                Some(short_text(&cmd.command, 240)),
                Some(summary.clone()),
            ));
            outcome.last_visible_activity = Some(summary);
        }
        ThreadItem::CommandExecution(cmd)
            if lifecycle == ItemLifecycle::Completed
                && matches!(cmd.status, CommandExecutionStatus::Failed) =>
        {
            let summary = format!("Command failed: {}", short_text(&cmd.command, 160));
            let _ = progress_tx.send(make_progress(
                "command_failed",
                "A Codex command failed",
                Some(short_text(&cmd.command, 240)),
                Some(summary.clone()),
            ));
            outcome.last_visible_activity = Some(summary);
        }
        ThreadItem::McpToolCall(tool) if lifecycle == ItemLifecycle::Started => {
            let tool_ref = format!("{}/{}", tool.server, tool.tool);
            let _ = progress_tx.send(make_progress(
                "tool",
                "Codex is using a tool",
                Some(tool_ref.clone()),
                Some(format!("Tool: {tool_ref}")),
            ));
            outcome.last_visible_activity = Some(format!("Tool: {tool_ref}"));
        }
        ThreadItem::McpToolCall(tool)
            if lifecycle == ItemLifecycle::Completed
                && matches!(tool.status, McpToolCallStatus::Failed) =>
        {
            let tool_ref = format!("{}/{}", tool.server, tool.tool);
            let detail = tool
                .error
                .as_ref()
                .map(|err| short_text(&err.message, 240))
                .unwrap_or_else(|| format!("Tool failed: {tool_ref}"));
            let _ = progress_tx.send(make_progress(
                "tool_failed",
                "A Codex tool step failed",
                Some(detail.clone()),
                Some(format!("Tool failed: {tool_ref}")),
            ));
            outcome.last_visible_activity = Some(format!("Tool failed: {tool_ref}"));
        }
        ThreadItem::TodoList(list) => {
            let next = list
                .items
                .iter()
                .find(|entry| !entry.completed)
                .map(|entry| short_text(&entry.text, 240))
                .unwrap_or_else(|| "Updating the current plan for the code tour run.".to_string());
            let _ = progress_tx.send(make_progress(
                "planning",
                "Codex is updating its review plan",
                Some(next.clone()),
                Some(next.clone()),
            ));
            outcome.last_visible_activity = Some(next);
        }
        ThreadItem::Reasoning(reasoning) if lifecycle == ItemLifecycle::Completed => {
            let detail = short_text(&reasoning.text, 240);
            let _ = progress_tx.send(make_progress(
                "reasoning",
                "Codex is reasoning through the change",
                Some(detail.clone()),
                Some(short_text(&reasoning.text, 180)),
            ));
            outcome.last_visible_activity = Some(detail);
        }
        ThreadItem::WebSearch(search) if lifecycle == ItemLifecycle::Started => {
            let detail = short_text(&search.query, 240);
            let _ = progress_tx.send(make_progress(
                "search",
                "Codex is searching for context",
                Some(detail.clone()),
                Some(short_text(&search.query, 180)),
            ));
            outcome.last_visible_activity = Some(detail);
        }
        ThreadItem::AgentMessage(_) if lifecycle == ItemLifecycle::Completed => {
            let _ = progress_tx.send(make_progress(
                "drafting",
                "Codex drafted the code tour response",
                Some("Finalizing the structured output for the app.".to_string()),
                Some("Codex drafted the final response".to_string()),
            ));
            outcome.last_visible_activity = Some("Codex drafted the final response".to_string());
        }
        _ => {}
    }
}

fn final_agent_message(notif: &TurnCompletedNotification) -> Option<String> {
    for item in notif.turn.items.iter().rev() {
        if let ThreadItem::AgentMessage(msg) = item {
            if !msg.text.trim().is_empty() {
                return Some(msg.text.clone());
            }
        }
    }
    None
}

async fn handle_request(
    client: &mut AsyncClient,
    id: RequestId,
    method: &str,
    _params: Option<Value>,
    progress_tx: &mpsc::Sender<CodeTourProgressUpdate>,
) {
    match method {
        methods::CMD_EXEC_APPROVAL => {
            let _ = progress_tx.send(make_progress(
                "tool_failed",
                "Codex requested a command that is not allowed",
                Some(
                    "Tours run with a read-only sandbox; the command was declined automatically."
                        .to_string(),
                ),
                Some("Declined a Codex command approval".to_string()),
            ));
            let response = CommandExecutionApprovalResponse {
                decision: CommandApprovalDecision::Decline,
            };
            let _ = client.respond(id, &response).await;
        }
        methods::FILE_CHANGE_APPROVAL => {
            let _ = progress_tx.send(make_progress(
                "tool_failed",
                "Codex requested a file change that is not allowed",
                Some("Tours never edit files; the change was declined automatically.".to_string()),
                Some("Declined a Codex file change approval".to_string()),
            ));
            let response = FileChangeApprovalResponse {
                decision: FileChangeApprovalDecision::Decline,
            };
            let _ = client.respond(id, &response).await;
        }
        _ => {
            let _ = client
                .respond_error(id, -32601, "method not implemented")
                .await;
        }
    }
}

fn short_text(value: &str, limit: usize) -> String {
    let trimmed = value.trim();
    if trimmed.chars().count() <= limit {
        return trimmed.to_string();
    }
    let truncated: String = trimmed.chars().take(limit.saturating_sub(1)).collect();
    format!("{}…", truncated.trim_end())
}

fn build_turn_start_params(thread_id: &str, prompt: &str) -> Value {
    json!({
        "threadId": thread_id,
        "input": [
            {
                "type": "text",
                "text": prompt,
            }
        ],
        // Newer Codex app-server builds use `effort`; older ones still expect
        // `reasoningEffort`. Sending both keeps this request compatible across
        // the CLI versions we have seen in the wild.
        "effort": "low",
        "reasoningEffort": "low",
        "sandboxPolicy": compatible_read_only_sandbox_policy(),
    })
}

fn compatible_read_only_sandbox_policy() -> Value {
    json!({
        // Current app-server builds require a tagged sandbox policy object.
        "type": "readOnly",
        "networkAccess": false,
        // Older app-server builds accepted the legacy `mode` field.
        "mode": "read-only",
    })
}

#[cfg(test)]
mod tests {
    use super::{build_turn_start_params, compatible_read_only_sandbox_policy};

    #[test]
    fn compatible_read_only_sandbox_policy_includes_new_and_legacy_fields() {
        let policy = compatible_read_only_sandbox_policy();

        assert_eq!(policy["type"], "readOnly");
        assert_eq!(policy["mode"], "read-only");
        assert_eq!(policy["networkAccess"], false);
    }

    #[test]
    fn turn_start_params_cover_old_and_new_codex_field_names() {
        let params = build_turn_start_params("thread-123", "hello");

        assert_eq!(params["threadId"], "thread-123");
        assert_eq!(params["effort"], "low");
        assert_eq!(params["reasoningEffort"], "low");
        assert_eq!(params.pointer("/input/0/type").unwrap(), "text");
        assert_eq!(params.pointer("/input/0/text").unwrap(), "hello");
        assert_eq!(params.pointer("/sandboxPolicy/type").unwrap(), "readOnly");
    }
}
