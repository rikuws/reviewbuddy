#[derive(Debug, Clone)]
pub struct AbortReason {
    pub kind: AbortKind,
    pub timeout_ms: u64,
    pub last_visible_activity: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbortKind {
    Overall,
    Inactivity,
}

pub fn generation_abort_message(provider_label: &str, reason: &AbortReason) -> String {
    let suffix = reason
        .last_visible_activity
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(|value| format!(" Last visible activity: {value}."))
        .unwrap_or_default();

    match reason.kind {
        AbortKind::Inactivity => format!(
            "{provider_label} stopped reporting progress for {} while generating the code tour.{suffix}",
            format_duration(reason.timeout_ms)
        ),
        AbortKind::Overall => format!(
            "{provider_label} timed out while generating the code tour after {}.{suffix}",
            format_duration(reason.timeout_ms)
        ),
    }
}

pub fn format_duration(timeout_ms: u64) -> String {
    let total_seconds = (timeout_ms as f64 / 1000.0).round() as u64;
    if total_seconds.is_multiple_of(60) {
        let minutes = total_seconds / 60;
        format!("{minutes} minute{}", if minutes == 1 { "" } else { "s" })
    } else {
        format!(
            "{total_seconds} second{}",
            if total_seconds == 1 { "" } else { "s" }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overall_timeout_message() {
        let reason = AbortReason {
            kind: AbortKind::Overall,
            timeout_ms: 240_000,
            last_visible_activity: Some("Reading foo.rs".to_string()),
        };
        let msg = generation_abort_message("Codex", &reason);
        assert!(msg.contains("4 minutes"));
        assert!(msg.contains("Last visible activity: Reading foo.rs."));
    }

    #[test]
    fn inactivity_timeout_message() {
        let reason = AbortReason {
            kind: AbortKind::Inactivity,
            timeout_ms: 60_000,
            last_visible_activity: None,
        };
        let msg = generation_abort_message("GitHub Copilot", &reason);
        assert!(msg.contains("stopped reporting progress"));
        assert!(msg.contains("1 minute"));
    }
}
