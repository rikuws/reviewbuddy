use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{app_storage, cache::CacheStore, command_runner::CommandRunner, gh};

const LOCAL_REPO_LINK_KEY_PREFIX: &str = "local-repo-link-v1:";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalRepositoryStatus {
    pub repository: String,
    pub path: Option<String>,
    pub source: String,
    pub exists: bool,
    pub is_valid_repository: bool,
    pub current_head_oid: Option<String>,
    pub expected_head_oid: Option<String>,
    pub matches_expected_head: bool,
    pub is_worktree_clean: bool,
    pub ready_for_local_features: bool,
    pub message: String,
}

impl LocalRepositoryStatus {
    pub fn ready_for_snapshot_features(&self) -> bool {
        self.is_valid_repository && self.matches_expected_head && self.path.is_some()
    }

    pub fn should_prefer_worktree_contents(&self) -> bool {
        self.ready_for_snapshot_features() && self.is_worktree_clean
    }
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
    resolve_local_repository_status(cache, repository, None)
}

pub fn load_local_repository_status_for_pull_request(
    cache: &CacheStore,
    repository: &str,
    head_ref_oid: Option<&str>,
) -> Result<LocalRepositoryStatus, String> {
    resolve_local_repository_status(cache, repository, head_ref_oid)
}

pub fn load_or_prepare_local_repository_for_pull_request(
    cache: &CacheStore,
    repository: &str,
    pull_request_number: i64,
    head_ref_oid: Option<&str>,
) -> Result<LocalRepositoryStatus, String> {
    let status = resolve_local_repository_status(cache, repository, head_ref_oid)?;
    if status.source == "managed" && !status.ready_for_local_features {
        prepare_local_repository_for_pull_request(
            cache,
            repository,
            pull_request_number,
            head_ref_oid,
        )?;
        return resolve_local_repository_status(cache, repository, head_ref_oid);
    }

    Ok(status)
}

pub fn ensure_local_repository_for_pull_request(
    cache: &CacheStore,
    repository: &str,
    pull_request_number: i64,
    head_ref_oid: Option<&str>,
) -> Result<LocalRepositoryStatus, String> {
    let status = load_or_prepare_local_repository_for_pull_request(
        cache,
        repository,
        pull_request_number,
        head_ref_oid,
    )?;

    if status.ready_for_local_features {
        return Ok(status);
    }

    if status.source == "linked" {
        let managed_status = load_or_prepare_managed_repository_for_pull_request(
            cache,
            repository,
            pull_request_number,
            head_ref_oid,
        )?;

        if managed_status.ready_for_local_features {
            return Ok(managed_status);
        }

        return Err(managed_status.message.clone());
    }

    Err(status.message.clone())
}

fn resolve_local_repository_status(
    cache: &CacheStore,
    repository: &str,
    expected_head_oid: Option<&str>,
) -> Result<LocalRepositoryStatus, String> {
    if let Some(link) = cache.get::<LocalRepositoryLink>(&local_repo_link_key(repository))? {
        return inspect_repository_candidate(
            repository,
            PathBuf::from(link.value.path),
            "linked".to_string(),
            Some("Using your linked checkout.".to_string()),
            expected_head_oid,
        );
    }

    inspect_managed_repository_candidate(repository, expected_head_oid)
}

fn inspect_repository_candidate(
    repository: &str,
    candidate: PathBuf,
    source: String,
    default_message: Option<String>,
    expected_head_oid: Option<&str>,
) -> Result<LocalRepositoryStatus, String> {
    let exists = candidate.exists();
    let expected_head_oid = normalized_expected_head_oid(expected_head_oid);

    if !exists {
        let message = if source == "linked" {
            "The linked checkout no longer exists. Pick another folder or switch back to the app-managed checkout.".to_string()
        } else {
            "The app will create and manage a hidden checkout when a pull request needs local code context.".to_string()
        };

        return Ok(LocalRepositoryStatus {
            repository: repository.to_string(),
            path: Some(candidate.display().to_string()),
            source,
            exists: false,
            is_valid_repository: false,
            current_head_oid: None,
            expected_head_oid,
            matches_expected_head: false,
            is_worktree_clean: false,
            ready_for_local_features: false,
            message,
        });
    }

    let root = resolve_git_root(&candidate)?;
    let Some(root) = root else {
        let message = if source == "linked" {
            "The linked checkout is not a git repository. Pick the repository root or any folder inside it.".to_string()
        } else {
            "The app-managed checkout is missing its git metadata. Remove it from app storage and try again.".to_string()
        };

        return Ok(LocalRepositoryStatus {
            repository: repository.to_string(),
            path: Some(candidate.display().to_string()),
            source,
            exists: true,
            is_valid_repository: false,
            current_head_oid: None,
            expected_head_oid,
            matches_expected_head: false,
            is_worktree_clean: false,
            ready_for_local_features: false,
            message,
        });
    };

    if !repository_matches_git_remote(repository, &root)? {
        let message = if source == "linked" {
            format!(
                "The linked checkout does not match {}. Use a clone whose remotes point at that repository.",
                repository
            )
        } else {
            format!(
                "The app-managed checkout does not match {}. Remove it from app storage and try again.",
                repository
            )
        };

        return Ok(LocalRepositoryStatus {
            repository: repository.to_string(),
            path: Some(root.display().to_string()),
            source,
            exists: true,
            is_valid_repository: false,
            current_head_oid: None,
            expected_head_oid,
            matches_expected_head: false,
            is_worktree_clean: false,
            ready_for_local_features: false,
            message,
        });
    }

    let current_head_oid = current_head_oid(&root)?;
    let matches_expected_head = expected_head_oid
        .as_ref()
        .map(|expected| current_head_oid.as_deref() == Some(expected.as_str()))
        .unwrap_or(true);
    let is_worktree_clean = worktree_is_clean(&root)?;
    let ready_for_local_features = matches_expected_head && is_worktree_clean;

    let message = if let Some(expected_head) = expected_head_oid.as_deref() {
        if !matches_expected_head {
            if source == "linked" {
                format!(
                    "The linked checkout is on {}, but this pull request expects {}. Check out the PR head commit or switch back to the app-managed checkout.",
                    current_head_oid.as_deref().unwrap_or("unknown"),
                    expected_head
                )
            } else {
                format!(
                    "The app-managed checkout is out of date. The app will refresh it to pull request head {} before local code features run.",
                    expected_head
                )
            }
        } else if !is_worktree_clean {
            if source == "linked" {
                "The linked checkout has local changes. Commit, stash, or discard them before using local code features, or switch back to the app-managed checkout.".to_string()
            } else {
                "The app-managed checkout has local changes. Remove it from app storage and let the app recreate it for local code features.".to_string()
            }
        } else {
            default_message.unwrap_or_else(|| {
                format!(
                    "Using your checkout at pull request head {}.",
                    expected_head
                )
            })
        }
    } else if !is_worktree_clean {
        if source == "linked" {
            "The linked checkout has local changes. Commit, stash, or discard them before using local code features.".to_string()
        } else {
            "The app-managed checkout has local changes. Remove it from app storage and let the app recreate it.".to_string()
        }
    } else {
        default_message.unwrap_or_else(|| "Using your checkout.".to_string())
    };

    Ok(LocalRepositoryStatus {
        repository: repository.to_string(),
        path: Some(root.display().to_string()),
        source,
        exists: true,
        is_valid_repository: true,
        current_head_oid,
        expected_head_oid,
        matches_expected_head,
        is_worktree_clean,
        ready_for_local_features,
        message,
    })
}

fn prepare_local_repository_for_pull_request(
    _cache: &CacheStore,
    repository: &str,
    pull_request_number: i64,
    head_ref_oid: Option<&str>,
) -> Result<PathBuf, String> {
    ensure_managed_repository_for_pull_request(repository, pull_request_number, head_ref_oid)
}

fn load_or_prepare_managed_repository_for_pull_request(
    cache: &CacheStore,
    repository: &str,
    pull_request_number: i64,
    head_ref_oid: Option<&str>,
) -> Result<LocalRepositoryStatus, String> {
    let status = inspect_managed_repository_candidate(repository, head_ref_oid)?;
    if status.ready_for_local_features {
        return Ok(status);
    }

    prepare_local_repository_for_pull_request(
        cache,
        repository,
        pull_request_number,
        head_ref_oid,
    )?;
    inspect_managed_repository_candidate(repository, head_ref_oid)
}

fn inspect_managed_repository_candidate(
    repository: &str,
    expected_head_oid: Option<&str>,
) -> Result<LocalRepositoryStatus, String> {
    let managed_path = managed_repository_path(repository)?;

    inspect_repository_candidate(
        repository,
        managed_path,
        "managed".to_string(),
        Some("Using the app-managed checkout.".to_string()),
        expected_head_oid,
    )
}

fn ensure_managed_repository(repository: &str) -> Result<PathBuf, String> {
    let target = managed_repository_path(repository)?;

    if target.exists() {
        let root = resolve_git_root(&target)?.ok_or_else(|| {
            "The app-managed checkout is missing its git metadata. Remove it from app storage and try again.".to_string()
        })?;

        if repository_matches_git_remote(repository, &root)? {
            return Ok(root);
        }

        return Err(format!(
            "The app-managed checkout does not match {}. Remove it from app storage and try again.",
            repository
        ));
    }

    let Some(parent) = target.parent() else {
        return Err(
            "Failed to resolve the app-managed checkout folder inside app storage.".to_string(),
        );
    };

    fs::create_dir_all(parent).map_err(|error| {
        format!("Failed to create the app-managed checkout folder in app storage: {error}")
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
        "The app-managed checkout was created but is not a git repository.".to_string()
    })?;

    if repository_matches_git_remote(repository, &root)? {
        Ok(root)
    } else {
        Err(format!(
            "The app-managed checkout does not match {}.",
            repository
        ))
    }
}

fn ensure_managed_repository_for_pull_request(
    repository: &str,
    pull_request_number: i64,
    head_ref_oid: Option<&str>,
) -> Result<PathBuf, String> {
    let root = ensure_managed_repository(repository)?;
    sync_managed_repository_to_pull_request(&root, repository, pull_request_number, head_ref_oid)?;
    Ok(root)
}

fn sync_managed_repository_to_pull_request(
    root: &Path,
    repository: &str,
    pull_request_number: i64,
    head_ref_oid: Option<&str>,
) -> Result<(), String> {
    let expected_head = head_ref_oid
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    if expected_head.is_some() && current_head_oid(root)? == expected_head.clone() {
        return Ok(());
    }

    let output = gh::run_owned_in(
        vec![
            "pr".to_string(),
            "checkout".to_string(),
            pull_request_number.to_string(),
            "--detach".to_string(),
        ],
        Some(root),
    )?;

    if output.exit_code != Some(0) {
        return Err(combine_process_error(
            output,
            &format!(
                "Failed to update the app-managed checkout to pull request #{pull_request_number} for {repository}"
            ),
        ));
    }

    if let Some(expected_head) = expected_head {
        let current_head = current_head_oid(root)?.unwrap_or_else(|| "unknown".to_string());
        if current_head != expected_head {
            return Err(format!(
                "The app-managed checkout did not reach pull request #{pull_request_number}. Expected HEAD {expected_head}, but found {current_head}.",
            ));
        }
    }

    Ok(())
}

fn run_git(
    path: &Path,
    args: impl IntoIterator<Item = impl Into<String>>,
) -> Result<gh::CommandOutput, String> {
    let mut command_args = vec!["-C".to_string(), path.display().to_string()];
    command_args.extend(args.into_iter().map(Into::into));
    let output = CommandRunner::new("git").args(command_args).run()?;
    if output.timed_out {
        return Err("git command timed out after 120 seconds.".to_string());
    }
    Ok(output)
}

fn resolve_git_root(path: &Path) -> Result<Option<PathBuf>, String> {
    if !path.exists() {
        return Ok(None);
    }

    let output = run_git(path, ["rev-parse", "--show-toplevel"])?;

    if output.exit_code != Some(0) {
        return Ok(None);
    }

    let root = output.stdout.trim().to_string();
    if root.is_empty() {
        return Ok(None);
    }

    Ok(Some(PathBuf::from(root)))
}

fn current_head_oid(path: &Path) -> Result<Option<String>, String> {
    let output = run_git(path, ["rev-parse", "HEAD"])?;

    if output.exit_code != Some(0) {
        return Ok(None);
    }

    let head = output.stdout.trim().to_string();
    if head.is_empty() {
        return Ok(None);
    }

    Ok(Some(head))
}

fn worktree_is_clean(path: &Path) -> Result<bool, String> {
    let output = run_git(path, ["status", "--porcelain", "--untracked-files=normal"])?;

    if output.exit_code != Some(0) {
        return Ok(false);
    }

    Ok(output.stdout.trim().is_empty())
}

fn repository_matches_git_remote(repository: &str, path: &Path) -> Result<bool, String> {
    let output = run_git(path, ["remote"])?;

    if output.exit_code != Some(0) {
        return Ok(false);
    }

    let target = repository.to_ascii_lowercase();
    let remote_names = output.stdout;

    for remote_name in remote_names.lines().filter(|line| !line.trim().is_empty()) {
        let remote_output = run_git(path, ["remote", "get-url", remote_name])?;

        if remote_output.exit_code != Some(0) {
            continue;
        }

        if normalized_remote_repository(&remote_output.stdout)
            .is_some_and(|normalized| normalized == target)
        {
            return Ok(true);
        }
    }

    Ok(false)
}

fn managed_repository_path(repository: &str) -> Result<PathBuf, String> {
    Ok(
        app_storage::managed_repositories_root()
            .join(managed_repository_directory_name(repository)),
    )
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

fn normalized_expected_head_oid(head_ref_oid: Option<&str>) -> Option<String> {
    head_ref_oid
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
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
    use std::{
        fs,
        path::{Path, PathBuf},
        process::Command,
        sync::atomic::{AtomicUsize, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    use crate::cache::CacheStore;

    use super::{
        ensure_local_repository_for_pull_request, load_local_repository_status_for_pull_request,
        local_repo_link_key, managed_repository_directory_name, managed_repository_path,
        normalized_remote_repository, LocalRepositoryLink,
    };

    static NEXT_TEST_ID: AtomicUsize = AtomicUsize::new(0);

    struct GitTestRepository {
        root: PathBuf,
        _workspace: PathBuf,
    }

    impl GitTestRepository {
        fn new(remote_repository: &str) -> Self {
            let workspace = unique_test_directory("local-repo");
            let root = workspace.join("repo");
            fs::create_dir_all(&root).expect("failed to create repo directory");
            run_git(&root, ["init"]);
            run_git(&root, ["config", "user.name", "Remiss Tests"]);
            run_git(&root, ["config", "user.email", "remiss-tests@example.com"]);
            run_git(
                &root,
                [
                    "remote",
                    "add",
                    "origin",
                    &format!("git@github.com:{remote_repository}.git"),
                ],
            );
            Self {
                root,
                _workspace: workspace,
            }
        }

        fn write_file(&self, path: &str, contents: &str) {
            let full_path = self.root.join(path);
            if let Some(parent) = full_path.parent() {
                fs::create_dir_all(parent).expect("failed to create parent directory");
            }
            fs::write(full_path, contents).expect("failed to write test file");
        }

        fn commit_all(&self, message: &str) -> String {
            run_git(&self.root, ["add", "."]);
            run_git(&self.root, ["commit", "-m", message]);
            self.head_oid()
        }

        fn head_oid(&self) -> String {
            git_output(&self.root, ["rev-parse", "HEAD"])
        }
    }

    fn unique_test_directory(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time went backwards")
            .as_nanos();
        let test_id = NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "remiss-{prefix}-{nanos}-{test_id}-{}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("failed to create temp directory");
        path
    }

    fn run_git<const N: usize>(path: &Path, args: [&str; N]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(path)
            .args(args)
            .output()
            .expect("failed to run git");

        if !output.status.success() {
            panic!(
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    fn git_output<const N: usize>(path: &Path, args: [&str; N]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(path)
            .args(args)
            .output()
            .expect("failed to run git");

        if !output.status.success() {
            panic!(
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&output.stderr)
            );
        }

        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn create_managed_clone(repository: &str, source: &Path) -> PathBuf {
        let managed_path = managed_repository_path(repository).expect("failed to resolve path");
        if managed_path.exists() {
            fs::remove_dir_all(&managed_path).expect("failed to remove existing managed repo");
        }
        if let Some(parent) = managed_path.parent() {
            fs::create_dir_all(parent).expect("failed to create managed repo parent");
        }

        let output = Command::new("git")
            .arg("clone")
            .arg(source)
            .arg(&managed_path)
            .output()
            .expect("failed to clone managed repo");

        if !output.status.success() {
            panic!(
                "git clone failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        let remote_url = format!("git@github.com:{repository}.git");
        run_git(
            &managed_path,
            ["remote", "set-url", "origin", remote_url.as_str()],
        );
        managed_path
    }

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

    #[test]
    fn linked_repository_status_requires_expected_head() {
        let repository = GitTestRepository::new("openai/example");
        repository.write_file("src/lib.rs", "pub fn value() -> i32 { 1 }\n");
        let initial_head = repository.commit_all("initial");

        repository.write_file("src/lib.rs", "pub fn value() -> i32 { 2 }\n");
        let current_head = repository.commit_all("second");

        let cache =
            CacheStore::new(unique_test_directory("local-repo-cache").join("cache.sqlite3"))
                .expect("failed to create cache");
        cache
            .put(
                &local_repo_link_key("openai/example"),
                &LocalRepositoryLink {
                    path: repository.root.display().to_string(),
                },
                0,
            )
            .expect("failed to write link");

        let status = load_local_repository_status_for_pull_request(
            &cache,
            "openai/example",
            Some(&initial_head),
        )
        .expect("failed to load status");

        assert!(status.is_valid_repository);
        assert_eq!(
            status.current_head_oid.as_deref(),
            Some(current_head.as_str())
        );
        assert_eq!(
            status.expected_head_oid.as_deref(),
            Some(initial_head.as_str())
        );
        assert!(!status.matches_expected_head);
        assert!(!status.ready_for_local_features);
        assert!(status.message.contains("expects"));
        assert!(!status
            .message
            .contains(&repository.root.display().to_string()));
    }

    #[test]
    fn linked_repository_status_requires_clean_worktree() {
        let repository = GitTestRepository::new("openai/example");
        repository.write_file("src/lib.rs", "pub fn value() -> i32 { 1 }\n");
        let head = repository.commit_all("initial");
        repository.write_file("src/lib.rs", "pub fn value() -> i32 { 3 }\n");

        let cache =
            CacheStore::new(unique_test_directory("local-repo-cache").join("cache.sqlite3"))
                .expect("failed to create cache");
        cache
            .put(
                &local_repo_link_key("openai/example"),
                &LocalRepositoryLink {
                    path: repository.root.display().to_string(),
                },
                0,
            )
            .expect("failed to write link");

        let status =
            load_local_repository_status_for_pull_request(&cache, "openai/example", Some(&head))
                .expect("failed to load status");

        assert!(status.matches_expected_head);
        assert!(!status.is_worktree_clean);
        assert!(!status.ready_for_local_features);
        assert!(status.ready_for_snapshot_features());
        assert!(!status.should_prefer_worktree_contents());
        assert!(status.message.contains("local changes"));
        assert!(!status
            .message
            .contains(&repository.root.display().to_string()));
    }

    #[test]
    fn ensure_local_repository_falls_back_to_managed_checkout_when_linked_repo_is_dirty() {
        let repository_name = format!(
            "openai/example-fallback-{}",
            NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed)
        );
        let linked_repository = GitTestRepository::new(&repository_name);
        linked_repository.write_file("src/lib.rs", "pub fn value() -> i32 { 1 }\n");
        let head = linked_repository.commit_all("initial");
        linked_repository.write_file("src/lib.rs", "pub fn value() -> i32 { 2 }\n");

        let managed_path = create_managed_clone(&repository_name, &linked_repository.root);
        let cache =
            CacheStore::new(unique_test_directory("local-repo-cache").join("cache.sqlite3"))
                .expect("failed to create cache");
        cache
            .put(
                &local_repo_link_key(&repository_name),
                &LocalRepositoryLink {
                    path: linked_repository.root.display().to_string(),
                },
                0,
            )
            .expect("failed to write link");

        let status =
            ensure_local_repository_for_pull_request(&cache, &repository_name, 42, Some(&head))
                .expect("failed to ensure repository");

        assert_eq!(status.source, "managed");
        assert!(status.ready_for_local_features);
        assert!(status.is_worktree_clean);
        assert_eq!(status.current_head_oid.as_deref(), Some(head.as_str()));
        assert_eq!(
            status.path.as_deref(),
            Some(managed_path.to_string_lossy().as_ref())
        );

        let _ = fs::remove_dir_all(managed_path);
    }
}
