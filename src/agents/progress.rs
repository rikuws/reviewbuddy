use std::env;

use crate::code_tour::CodeTourProgressUpdate;

const CODE_TOUR_LOG_DIR_ENV: &str = "GH_UI_CODE_TOUR_LOG_DIR";

fn current_log_file_path() -> Option<String> {
    if let Ok(value) = env::var(CODE_TOUR_LOG_DIR_ENV) {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    None
}

pub fn make_progress(
    stage: impl Into<String>,
    summary: impl Into<String>,
    detail: Option<String>,
    log: Option<String>,
) -> CodeTourProgressUpdate {
    CodeTourProgressUpdate {
        stage: stage.into(),
        summary: limit_text(&summary.into(), 160),
        detail: detail.map(|value| limit_text(&value, 240)),
        log: log.map(|value| limit_text(&value, 240)),
        log_file_path: current_log_file_path(),
    }
}

pub fn limit_text(value: &str, max_length: usize) -> String {
    let normalized = value.trim();
    if normalized.chars().count() <= max_length {
        return normalized.to_string();
    }

    let truncated = normalized
        .chars()
        .take(max_length.saturating_sub(1))
        .collect::<String>();
    format!("{}…", truncated.trim_end())
}
