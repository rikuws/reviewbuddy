use std::path::PathBuf;

const APP_DATA_DIR: &str = "gh-ui-tool";
const MANAGED_LSP_DIR: &str = "lsp-servers";
const MANAGED_REPOSITORIES_DIR: &str = "managed-repositories";

pub fn data_dir_root() -> PathBuf {
    if let Some(data_dir) = dirs::data_local_dir() {
        return data_dir.join(APP_DATA_DIR);
    }

    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(format!(".{APP_DATA_DIR}"))
}

pub fn cache_path() -> PathBuf {
    data_dir_root().join("cache.sqlite3")
}

pub fn managed_servers_root() -> PathBuf {
    data_dir_root().join(MANAGED_LSP_DIR)
}

pub fn managed_repositories_root() -> PathBuf {
    data_dir_root().join(MANAGED_REPOSITORIES_DIR)
}
