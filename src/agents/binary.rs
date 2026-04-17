use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub fn find_codex_binary() -> Option<String> {
    find_tool_binary(
        "codex",
        "GH_UI_TOOL_CODEX_BINARY",
        &[
            "/opt/homebrew/bin/codex",
            "/usr/local/bin/codex",
            "/usr/bin/codex",
        ],
        Some(home_relative_candidate(".codex/bin/codex")),
    )
}

pub fn find_copilot_binary() -> Option<String> {
    find_tool_binary(
        "copilot",
        "GH_UI_TOOL_COPILOT_BINARY",
        &[
            "/opt/homebrew/bin/copilot",
            "/usr/local/bin/copilot",
            "/usr/bin/copilot",
        ],
        None,
    )
}

fn find_tool_binary(
    name: &str,
    env_var: &str,
    well_known: &[&str],
    extra_candidate: Option<PathBuf>,
) -> Option<String> {
    if let Ok(value) = env::var(env_var) {
        let trimmed = value.trim();
        if !trimmed.is_empty() && Path::new(trimmed).is_file() {
            return Some(trimmed.to_string());
        }
    }

    if let Ok(path_value) = env::var("PATH") {
        for segment in path_value.split(':') {
            let segment = segment.trim();
            if segment.is_empty() {
                continue;
            }
            let candidate = Path::new(segment).join(name);
            if candidate.is_file() {
                return Some(candidate.to_string_lossy().into_owned());
            }
        }
    }

    for candidate in well_known {
        if Path::new(candidate).is_file() {
            return Some((*candidate).to_string());
        }
    }

    if let Some(extra) = extra_candidate {
        if extra.is_file() {
            return Some(extra.to_string_lossy().into_owned());
        }
    }

    match Command::new(name)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        Ok(status) if status.success() => Some(name.to_string()),
        _ => None,
    }
}

fn home_relative_candidate(suffix: &str) -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/"))
        .join(suffix)
}
