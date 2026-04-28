use std::{
    fs,
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{
    cache::CacheStore,
    command_runner::CommandRunner,
    github::{RepositoryFileContent, REPOSITORY_FILE_SOURCE_LOCAL_CHECKOUT},
};

const LOCAL_DOCUMENT_CACHE_KEY_PREFIX: &str = "local-document-v2";

pub fn load_local_repository_file_content(
    cache: &CacheStore,
    repository: &str,
    checkout_root: &Path,
    reference: &str,
    path: &str,
    prefer_worktree: bool,
) -> Result<RepositoryFileContent, String> {
    if prefer_worktree {
        return match read_worktree_file(checkout_root, path) {
            Ok(bytes) => Ok(repository_file_content_from_bytes(
                repository, reference, path, bytes,
            )),
            Err(_) => load_git_file_cached(cache, repository, checkout_root, reference, path),
        };
    }

    load_git_file_cached(cache, repository, checkout_root, reference, path)
}

fn load_git_file_cached(
    cache: &CacheStore,
    repository: &str,
    checkout_root: &Path,
    reference: &str,
    path: &str,
) -> Result<RepositoryFileContent, String> {
    let git_path = validated_git_path(path)?;
    let blob_oid = git_blob_oid(checkout_root, reference, &git_path)?;
    let key = local_document_cache_key(repository, &blob_oid, &git_path);
    if let Some(cached) = cache.get::<RepositoryFileContent>(&key)? {
        return Ok(cached.value);
    }

    let bytes = read_git_blob(checkout_root, &blob_oid, path)?;
    let document = repository_file_content_from_bytes(repository, reference, path, bytes);
    cache.put(&key, &document, now_ms())?;
    Ok(document)
}

fn local_document_cache_key(repository: &str, blob_oid: &str, path: &str) -> String {
    format!("{LOCAL_DOCUMENT_CACHE_KEY_PREFIX}:{repository}:{blob_oid}:{path}")
}

fn read_worktree_file(checkout_root: &Path, path: &str) -> Result<Vec<u8>, String> {
    let relative_path = validated_repo_relative_path(path)?;
    let full_path = checkout_root.join(relative_path);
    fs::read(&full_path).map_err(|error| {
        format!(
            "Failed to read '{}' from {}: {error}",
            path,
            checkout_root.display()
        )
    })
}

fn git_blob_oid(checkout_root: &Path, reference: &str, git_path: &str) -> Result<String, String> {
    let output = CommandRunner::new("git")
        .args([
            "-C".to_string(),
            checkout_root.display().to_string(),
            "rev-parse".to_string(),
            format!("{reference}:{git_path}"),
        ])
        .run()?;

    if output.timed_out {
        return Err("Timed out resolving git object id.".to_string());
    }
    if output.exit_code != Some(0) {
        return Err(format!(
            "Failed to resolve {} from {} at {}: {}",
            git_path,
            checkout_root.display(),
            reference,
            output.stderr
        ));
    }

    let oid = output.stdout.trim().to_string();
    if oid.is_empty() {
        return Err(format!(
            "Git did not return an object id for {} at {}.",
            git_path, reference
        ));
    }
    Ok(oid)
}

fn read_git_blob(
    checkout_root: &Path,
    blob_oid: &str,
    display_path: &str,
) -> Result<Vec<u8>, String> {
    let output = CommandRunner::new("git")
        .args([
            "-C".to_string(),
            checkout_root.display().to_string(),
            "cat-file".to_string(),
            "-p".to_string(),
            blob_oid.to_string(),
        ])
        .run()?;

    if output.timed_out {
        return Err("Timed out reading git object.".to_string());
    }
    if output.exit_code != Some(0) {
        return Err(format!(
            "Failed to read {} from {} at {}: {}",
            display_path,
            checkout_root.display(),
            blob_oid,
            output.stderr
        ));
    }

    Ok(output.stdout_bytes)
}

fn validated_repo_relative_path(path: &str) -> Result<PathBuf, String> {
    let candidate = Path::new(path);
    let mut result = PathBuf::new();

    for component in candidate.components() {
        match component {
            Component::Normal(segment) => result.push(segment),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(format!("Unsupported repository-relative path '{}'.", path));
            }
        }
    }

    if result.as_os_str().is_empty() {
        return Err("The repository-relative file path is empty.".to_string());
    }

    Ok(result)
}

fn validated_git_path(path: &str) -> Result<String, String> {
    let relative_path = validated_repo_relative_path(path)?;
    Ok(relative_path
        .components()
        .filter_map(|component| match component {
            Component::Normal(segment) => Some(segment.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/"))
}

fn repository_file_content_from_bytes(
    repository: &str,
    reference: &str,
    path: &str,
    bytes: Vec<u8>,
) -> RepositoryFileContent {
    let size_bytes = bytes.len();
    let is_binary = bytes.contains(&0) || std::str::from_utf8(&bytes).is_err();

    RepositoryFileContent {
        repository: repository.to_string(),
        reference: reference.to_string(),
        path: path.to_string(),
        content: if is_binary {
            None
        } else {
            Some(String::from_utf8(bytes).unwrap_or_default())
        },
        is_binary,
        size_bytes,
        source: REPOSITORY_FILE_SOURCE_LOCAL_CHECKOUT.to_string(),
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
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

    use crate::{cache::CacheStore, github::REPOSITORY_FILE_SOURCE_LOCAL_CHECKOUT};

    use super::load_local_repository_file_content;

    static NEXT_TEST_ID: AtomicUsize = AtomicUsize::new(0);

    struct GitTestRepository {
        root: PathBuf,
        _workspace: PathBuf,
    }

    impl GitTestRepository {
        fn new() -> Self {
            let workspace = unique_test_directory("local-documents");
            let root = workspace.join("repo");
            fs::create_dir_all(&root).expect("failed to create repo directory");
            run_git(&root, ["init"]);
            run_git(&root, ["config", "user.name", "Remiss Tests"]);
            run_git(&root, ["config", "user.email", "remiss-tests@example.com"]);
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
            fs::write(full_path, contents).expect("failed to write file");
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

    #[test]
    fn loads_worktree_contents_from_local_checkout() {
        let repository = GitTestRepository::new();
        repository.write_file("src/lib.rs", "pub fn value() -> i32 { 1 }\n");
        let head = repository.commit_all("initial");
        let cache =
            CacheStore::new(unique_test_directory("local-documents-cache").join("cache.sqlite3"))
                .expect("failed to create cache");

        let document = load_local_repository_file_content(
            &cache,
            "openai/example",
            &repository.root,
            &head,
            "src/lib.rs",
            true,
        )
        .expect("failed to load local file");

        assert_eq!(
            document.content.as_deref(),
            Some("pub fn value() -> i32 { 1 }\n")
        );
        assert_eq!(document.source, REPOSITORY_FILE_SOURCE_LOCAL_CHECKOUT);
        assert!(!document.is_binary);
    }

    #[test]
    fn worktree_reads_do_not_return_stale_cached_content() {
        let repository = GitTestRepository::new();
        repository.write_file("src/lib.rs", "pub fn value() -> i32 { 1 }\n");
        let head = repository.commit_all("initial");
        let cache =
            CacheStore::new(unique_test_directory("local-documents-cache").join("cache.sqlite3"))
                .expect("failed to create cache");

        let first = load_local_repository_file_content(
            &cache,
            "openai/example",
            &repository.root,
            &head,
            "src/lib.rs",
            true,
        )
        .expect("failed to load first worktree file");
        repository.write_file("src/lib.rs", "pub fn value() -> i32 { 2 }\n");
        let second = load_local_repository_file_content(
            &cache,
            "openai/example",
            &repository.root,
            &head,
            "src/lib.rs",
            true,
        )
        .expect("failed to load changed worktree file");

        assert_eq!(
            first.content.as_deref(),
            Some("pub fn value() -> i32 { 1 }\n")
        );
        assert_eq!(
            second.content.as_deref(),
            Some("pub fn value() -> i32 { 2 }\n")
        );
    }

    #[test]
    fn loads_historical_contents_from_git_objects() {
        let repository = GitTestRepository::new();
        repository.write_file("src/lib.rs", "pub fn value() -> i32 { 1 }\n");
        let initial_head = repository.commit_all("initial");
        repository.write_file("src/lib.rs", "pub fn value() -> i32 { 2 }\n");
        let _current_head = repository.commit_all("second");
        let cache =
            CacheStore::new(unique_test_directory("local-documents-cache").join("cache.sqlite3"))
                .expect("failed to create cache");

        let document = load_local_repository_file_content(
            &cache,
            "openai/example",
            &repository.root,
            &initial_head,
            "src/lib.rs",
            false,
        )
        .expect("failed to load historical file");

        assert_eq!(
            document.content.as_deref(),
            Some("pub fn value() -> i32 { 1 }\n")
        );
        assert_eq!(document.reference, initial_head);
    }
}
