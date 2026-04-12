use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use serde::{Deserialize, Serialize};

use crate::{cache::CacheStore, gh};

const LOCAL_REPO_LINK_KEY_PREFIX: &str = "local-repo-link-v1:";
const MANAGED_REPO_DIRECTORY_NAME: &str = "managed-repositories";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalRepositoryStatus {
    pub repository: String,
    pub path: Option<String>,
    pub source: String,
    pub exists: bool,
    pub is_valid_repository: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocalRepositoryLink {
    path: String,
}

pub fn load_local_repository_status(
    cache: &CacheStore,
    repository: &str,
) -> Result<LocalRepositoryStatus, String> {
    resolve_local_repository_status(cache, repository)
}

pub fn ensure_local_repository(
    cache: &CacheStore,
    repository: &str,
) -> Result<LocalRepositoryStatus, String> {
    prepare_local_repository(cache, repository)?;
    resolve_local_repository_status(cache, repository)
}

fn resolve_local_repository_status(
    cache: &CacheStore,
    repository: &str,
) -> Result<LocalRepositoryStatus, String> {
    if let Some(link) = cache.get::<LocalRepositoryLink>(&local_repo_link_key(repository))? {
        return inspect_repository_candidate(
            repository,
            PathBuf::from(link.value.path),
            "linked".to_string(),
            Some("Using your linked checkout.".to_string()),
        );
    }

    inspect_managed_repository_candidate(repository)
}

fn inspect_repository_candidate(
    repository: &str,
    candidate: PathBuf,
    source: String,
    default_message: Option<String>,
) -> Result<LocalRepositoryStatus, String> {
    let exists = candidate.exists();

    if !exists {
        let message = if source == "linked" {
            "The linked checkout no longer exists. Pick another folder or switch back to the app-managed checkout.".to_string()
        } else {
            format!(
                "The app will create and manage a hidden checkout in {} when a tour needs local code context.",
                candidate.display()
            )
        };

        return Ok(LocalRepositoryStatus {
            repository: repository.to_string(),
            path: Some(candidate.display().to_string()),
            source,
            exists: false,
            is_valid_repository: false,
            message,
        });
    }

    let root = resolve_git_root(&candidate)?;
    let Some(root) = root else {
        let message = if source == "linked" {
            format!(
                "'{}' is not a git repository. Pick the repository root or any folder inside it.",
                candidate.display()
            )
        } else {
            format!(
                "The app-managed checkout at '{}' is missing its git metadata. Remove the folder and try again.",
                candidate.display()
            )
        };

        return Ok(LocalRepositoryStatus {
            repository: repository.to_string(),
            path: Some(candidate.display().to_string()),
            source,
            exists: true,
            is_valid_repository: false,
            message,
        });
    };

    if !repository_matches_git_remote(repository, &root)? {
        let message = if source == "linked" {
            format!(
                "{} does not match {}. Use a clone whose remotes point at that repository.",
                root.display(),
                repository
            )
        } else {
            format!(
                "The app-managed checkout at {} does not match {}. Remove the folder and try again.",
                root.display(),
                repository
            )
        };

        return Ok(LocalRepositoryStatus {
            repository: repository.to_string(),
            path: Some(root.display().to_string()),
            source,
            exists: true,
            is_valid_repository: false,
            message,
        });
    }

    Ok(LocalRepositoryStatus {
        repository: repository.to_string(),
        path: Some(root.display().to_string()),
        source,
        exists: true,
        is_valid_repository: true,
        message: default_message.unwrap_or_else(|| format!("Using {}.", root.display())),
    })
}

fn prepare_local_repository(cache: &CacheStore, repository: &str) -> Result<PathBuf, String> {
    if let Some(link) = cache.get::<LocalRepositoryLink>(&local_repo_link_key(repository))? {
        return validate_linked_repository(repository, PathBuf::from(link.value.path));
    }

    ensure_managed_repository(repository)
}

fn inspect_managed_repository_candidate(repository: &str) -> Result<LocalRepositoryStatus, String> {
    let managed_path = managed_repository_path(repository)?;

    inspect_repository_candidate(
        repository,
        managed_path,
        "managed".to_string(),
        Some("Using the app-managed checkout.".to_string()),
    )
}

fn validate_linked_repository(repository: &str, candidate: PathBuf) -> Result<PathBuf, String> {
    if !candidate.exists() {
        return Err(
            "The linked checkout no longer exists. Pick another folder or switch back to the app-managed checkout."
                .to_string(),
        );
    }

    let root = resolve_git_root(&candidate)?.ok_or_else(|| {
        format!(
            "'{}' is not a git repository. Pick the repository root or any folder inside it.",
            candidate.display()
        )
    })?;

    ensure_repository_matches_path(repository, &root)?;

    Ok(root)
}

fn ensure_managed_repository(repository: &str) -> Result<PathBuf, String> {
    let target = managed_repository_path(repository)?;

    if target.exists() {
        let root = resolve_git_root(&target)?.ok_or_else(|| {
            format!(
                "The app-managed checkout at '{}' is missing its git metadata. Remove the folder and try again.",
                target.display()
            )
        })?;

        if repository_matches_git_remote(repository, &root)? {
            return Ok(root);
        }

        return Err(format!(
            "The app-managed checkout at {} does not match {}. Remove the folder and try again.",
            root.display(),
            repository
        ));
    }

    let Some(parent) = target.parent() else {
        return Err(format!(
            "Failed to resolve the parent folder for the app-managed checkout '{}'.",
            target.display()
        ));
    };

    fs::create_dir_all(parent).map_err(|error| {
        format!(
            "Failed to create the app-managed checkout folder '{}': {error}",
            parent.display()
        )
    })?;

    let output = gh::run_owned(vec![
        "repo".to_string(),
        "clone".to_string(),
        repository.to_string(),
        target.display().to_string(),
    ])?;

    if output.exit_code != Some(0) {
        return Err(combine_process_error(
            output,
            &format!("Failed to create the app-managed checkout for {repository}"),
        ));
    }

    let root = resolve_git_root(&target)?.ok_or_else(|| {
        format!(
            "The app-managed checkout at '{}' was created but is not a git repository.",
            target.display()
        )
    })?;

    if repository_matches_git_remote(repository, &root)? {
        Ok(root)
    } else {
        Err(format!(
            "The app-managed checkout at {} does not match {}.",
            root.display(),
            repository
        ))
    }
}

fn ensure_repository_matches_path(repository: &str, path: &Path) -> Result<(), String> {
    let root = resolve_git_root(path)?
        .ok_or_else(|| format!("'{}' is not inside a git repository.", path.display()))?;

    if repository_matches_git_remote(repository, &root)? {
        Ok(())
    } else {
        Err(format!(
            "{} does not match {}. Link a clone whose remotes point at that repository.",
            root.display(),
            repository
        ))
    }
}

fn resolve_git_root(path: &Path) -> Result<Option<PathBuf>, String> {
    if !path.exists() {
        return Ok(None);
    }

    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|error| format!("Failed to launch git: {error}"))?;

    if !output.status.success() {
        return Ok(None);
    }

    let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if root.is_empty() {
        return Ok(None);
    }

    Ok(Some(PathBuf::from(root)))
}

fn repository_matches_git_remote(repository: &str, path: &Path) -> Result<bool, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .arg("remote")
        .output()
        .map_err(|error| format!("Failed to launch git: {error}"))?;

    if !output.status.success() {
        return Ok(false);
    }

    let target = repository.to_ascii_lowercase();
    let remote_names = String::from_utf8_lossy(&output.stdout);

    for remote_name in remote_names.lines().filter(|line| !line.trim().is_empty()) {
        let remote_output = Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["remote", "get-url", remote_name])
            .output()
            .map_err(|error| format!("Failed to launch git: {error}"))?;

        if !remote_output.status.success() {
            continue;
        }

        let remote_url = String::from_utf8_lossy(&remote_output.stdout);
        if normalized_remote_repository(&remote_url).is_some_and(|normalized| normalized == target)
        {
            return Ok(true);
        }
    }

    Ok(false)
}

fn managed_repository_path(repository: &str) -> Result<PathBuf, String> {
    let base = if let Some(data_dir) = dirs::data_local_dir() {
        data_dir
            .join("gh-ui-tool")
            .join(MANAGED_REPO_DIRECTORY_NAME)
    } else {
        std::env::current_dir()
            .map_err(|error| format!("Failed to resolve the current working directory: {error}"))?
            .join(".gh-ui-tool")
            .join(MANAGED_REPO_DIRECTORY_NAME)
    };

    Ok(base.join(managed_repository_directory_name(repository)))
}

fn managed_repository_directory_name(repository: &str) -> String {
    let mut result = String::new();

    for character in repository.chars() {
        match character {
            'a'..='z' | '0'..='9' | '-' | '_' | '.' => result.push(character),
            'A'..='Z' => result.push(character.to_ascii_lowercase()),
            '/' | '\\' => result.push_str("__"),
            _ => result.push('-'),
        }
    }

    if result.is_empty() {
        "repository".to_string()
    } else {
        result
    }
}

fn local_repo_link_key(repository: &str) -> String {
    format!("{LOCAL_REPO_LINK_KEY_PREFIX}{repository}")
}

fn combine_process_error(output: gh::CommandOutput, prefix: &str) -> String {
    if !output.stderr.is_empty() {
        format!("{prefix}: {}", output.stderr)
    } else if !output.stdout.is_empty() {
        format!("{prefix}: {}", output.stdout)
    } else {
        prefix.to_string()
    }
}

fn normalized_remote_repository(remote_url: &str) -> Option<String> {
    let trimmed = remote_url.trim().trim_end_matches(".git");

    let repository_path = if let Some((_, remainder)) = trimmed.split_once("://") {
        let (_, path) = remainder.split_once('/')?;
        path
    } else if let Some((_, path)) = trimmed.split_once(':') {
        path
    } else {
        return None;
    };

    let mut segments = repository_path
        .trim_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty());

    let owner = segments.next()?;
    let name = segments.next()?;

    Some(format!(
        "{}/{}",
        owner.to_ascii_lowercase(),
        name.to_ascii_lowercase()
    ))
}

#[cfg(test)]
mod tests {
    use super::{managed_repository_directory_name, normalized_remote_repository};

    #[test]
    fn normalizes_https_remote_urls() {
        assert_eq!(
            normalized_remote_repository("https://github.com/openai/example.git"),
            Some("openai/example".to_string())
        );
    }

    #[test]
    fn normalizes_ssh_remote_urls() {
        assert_eq!(
            normalized_remote_repository("git@github.com:OpenAI/Example.git"),
            Some("openai/example".to_string())
        );
    }

    #[test]
    fn normalizes_enterprise_remote_urls() {
        assert_eq!(
            normalized_remote_repository("ssh://git@github.example.com/acme/widgets.git"),
            Some("acme/widgets".to_string())
        );
    }

    #[test]
    fn rejects_non_repository_urls() {
        assert_eq!(normalized_remote_repository("not-a-remote"), None);
    }

    #[test]
    fn sanitizes_managed_repository_directory_names() {
        assert_eq!(
            managed_repository_directory_name("OpenAI/example.repo"),
            "openai__example.repo".to_string()
        );
    }
}
