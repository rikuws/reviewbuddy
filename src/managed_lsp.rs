use std::{
    env,
    fs::{self, File},
    io::{BufReader, Read, Write},
    path::{Path, PathBuf},
    process::Command,
    sync::{Mutex, OnceLock},
};

use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha1::{Digest, Sha1};
use tar::Archive;
use zip::ZipArchive;

use crate::app_storage;

const USER_AGENT: &str = concat!("remiss/", env!("CARGO_PKG_VERSION"));
const MINIMUM_JDTLS_JAVA_MAJOR_VERSION: u32 = 21;
const MANAGED_LSP_DIR: &str = "lsp-servers";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ManagedServerKind {
    RustAnalyzer,
    TypescriptLanguageServer,
    Pyright,
    Gopls,
    KotlinLsp,
    Jdtls,
    Roslyn,
    VscodeHtmlLanguageServer,
    VscodeCssLanguageServer,
    VscodeJsonLanguageServer,
    VscodeMarkdownLanguageServer,
    YamlLanguageServer,
    BashLanguageServer,
    DockerfileLanguageServer,
    Intelephense,
    SvelteLanguageServer,
}

impl ManagedServerKind {
    pub fn all() -> &'static [ManagedServerKind] {
        &[
            ManagedServerKind::RustAnalyzer,
            ManagedServerKind::TypescriptLanguageServer,
            ManagedServerKind::Pyright,
            ManagedServerKind::Gopls,
            ManagedServerKind::KotlinLsp,
            ManagedServerKind::Jdtls,
            ManagedServerKind::Roslyn,
            ManagedServerKind::VscodeHtmlLanguageServer,
            ManagedServerKind::VscodeCssLanguageServer,
            ManagedServerKind::VscodeJsonLanguageServer,
            ManagedServerKind::VscodeMarkdownLanguageServer,
            ManagedServerKind::YamlLanguageServer,
            ManagedServerKind::BashLanguageServer,
            ManagedServerKind::DockerfileLanguageServer,
            ManagedServerKind::Intelephense,
            ManagedServerKind::SvelteLanguageServer,
        ]
    }

    pub fn language_label(&self) -> &'static str {
        match self {
            ManagedServerKind::RustAnalyzer => "Rust",
            ManagedServerKind::TypescriptLanguageServer => "TypeScript / JavaScript",
            ManagedServerKind::Pyright => "Python",
            ManagedServerKind::Gopls => "Go",
            ManagedServerKind::KotlinLsp => "Kotlin",
            ManagedServerKind::Jdtls => "Java",
            ManagedServerKind::Roslyn => "C#",
            ManagedServerKind::VscodeHtmlLanguageServer => "HTML",
            ManagedServerKind::VscodeCssLanguageServer => "CSS / SCSS / Less",
            ManagedServerKind::VscodeJsonLanguageServer => "JSON / JSONC",
            ManagedServerKind::VscodeMarkdownLanguageServer => "Markdown",
            ManagedServerKind::YamlLanguageServer => "YAML",
            ManagedServerKind::BashLanguageServer => "Shell scripts",
            ManagedServerKind::DockerfileLanguageServer => "Dockerfiles",
            ManagedServerKind::Intelephense => "PHP",
            ManagedServerKind::SvelteLanguageServer => "Svelte",
        }
    }

    pub fn runtime_note(&self) -> Option<&'static str> {
        match self {
            ManagedServerKind::TypescriptLanguageServer
            | ManagedServerKind::Pyright
            | ManagedServerKind::VscodeHtmlLanguageServer
            | ManagedServerKind::VscodeCssLanguageServer
            | ManagedServerKind::VscodeJsonLanguageServer
            | ManagedServerKind::VscodeMarkdownLanguageServer
            | ManagedServerKind::YamlLanguageServer
            | ManagedServerKind::BashLanguageServer
            | ManagedServerKind::DockerfileLanguageServer
            | ManagedServerKind::Intelephense
            | ManagedServerKind::SvelteLanguageServer => {
                Some("Uses an app-managed Node.js runtime.")
            }
            ManagedServerKind::Gopls => {
                Some("Install still needs the Go toolchain on PATH so Remiss can build gopls.")
            }
            ManagedServerKind::Jdtls => {
                Some("Download is app-managed, but running JDTLS still needs Java 21+.")
            }
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ManagedServerConfiguration {
    pub command: String,
    pub args: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManagedServerInstallState {
    NotInstalled,
    Installed,
    Broken,
}

#[derive(Clone, Debug)]
pub struct ManagedServerInstallStatus {
    pub state: ManagedServerInstallState,
    pub version: Option<String>,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct InstalledServerRecord {
    version: String,
    install_dir: String,
    command_path: String,
}

#[derive(Clone, Copy)]
struct NodeHostedLanguageServerSpec {
    package_name: &'static str,
    entrypoint_relative_path: &'static str,
    args: &'static [&'static str],
    extra_packages: &'static [&'static str],
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    #[serde(default)]
    assets: Vec<GithubReleaseAsset>,
}

#[derive(Debug, Deserialize)]
struct GithubReleaseAsset {
    name: String,
    browser_download_url: String,
}

#[derive(Debug, Deserialize)]
struct GithubTag {
    name: String,
}

#[derive(Debug, Deserialize)]
struct NugetVersions {
    versions: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct NodeRelease {
    version: String,
    files: Vec<String>,
    lts: Value,
}

#[derive(Debug, Deserialize)]
struct NpmPackageMetadata {
    version: String,
}

#[derive(Debug, Deserialize)]
struct GoModuleVersion {
    #[serde(rename = "Version")]
    version: String,
}

#[derive(Debug)]
struct NodeRuntimePaths {
    install_dir: PathBuf,
    node: PathBuf,
    npm: PathBuf,
}

pub fn managed_server_display_name(kind: ManagedServerKind) -> &'static str {
    match kind {
        ManagedServerKind::RustAnalyzer => "managed rust-analyzer",
        ManagedServerKind::TypescriptLanguageServer => "managed TypeScript language server",
        ManagedServerKind::Pyright => "managed Pyright",
        ManagedServerKind::Gopls => "managed gopls",
        ManagedServerKind::KotlinLsp => "managed Kotlin LSP",
        ManagedServerKind::Jdtls => "managed JDTLS",
        ManagedServerKind::Roslyn => "managed Roslyn",
        ManagedServerKind::VscodeHtmlLanguageServer => "managed HTML language server",
        ManagedServerKind::VscodeCssLanguageServer => "managed CSS language server",
        ManagedServerKind::VscodeJsonLanguageServer => "managed JSON language server",
        ManagedServerKind::VscodeMarkdownLanguageServer => "managed Markdown language server",
        ManagedServerKind::YamlLanguageServer => "managed YAML language server",
        ManagedServerKind::BashLanguageServer => "managed Bash language server",
        ManagedServerKind::DockerfileLanguageServer => "managed Dockerfile language server",
        ManagedServerKind::Intelephense => "managed Intelephense",
        ManagedServerKind::SvelteLanguageServer => "managed Svelte language server",
    }
}

pub fn resolve_managed_server(
    repo_root: &Path,
    kind: ManagedServerKind,
) -> Result<ManagedServerConfiguration, String> {
    let _guard = managed_install_lock()
        .lock()
        .map_err(|_| "Managed LSP installer is unavailable.".to_string())?;
    resolve_managed_server_locked(repo_root, kind)
}

pub fn install_managed_server(
    kind: ManagedServerKind,
) -> Result<ManagedServerInstallStatus, String> {
    let _guard = managed_install_lock()
        .lock()
        .map_err(|_| "Managed LSP installer is unavailable.".to_string())?;

    let result = (|| -> Result<(), String> {
        match kind {
            ManagedServerKind::RustAnalyzer => {
                install_rust_analyzer()?;
            }
            ManagedServerKind::TypescriptLanguageServer => {
                install_typescript_language_server()?;
            }
            ManagedServerKind::Pyright => {
                install_pyright()?;
            }
            ManagedServerKind::Gopls => {
                install_gopls()?;
            }
            ManagedServerKind::KotlinLsp => {
                install_kotlin_lsp()?;
            }
            ManagedServerKind::Jdtls => {
                ensure_jdtls_installed()?;
            }
            ManagedServerKind::Roslyn => {
                install_roslyn()?;
            }
            ManagedServerKind::VscodeHtmlLanguageServer
            | ManagedServerKind::VscodeCssLanguageServer
            | ManagedServerKind::VscodeJsonLanguageServer
            | ManagedServerKind::VscodeMarkdownLanguageServer
            | ManagedServerKind::YamlLanguageServer
            | ManagedServerKind::BashLanguageServer
            | ManagedServerKind::DockerfileLanguageServer
            | ManagedServerKind::Intelephense
            | ManagedServerKind::SvelteLanguageServer => {
                install_node_hosted_language_server(kind)?;
            }
        }
        Ok(())
    })();

    result.map_err(|error| sanitize_managed_server_message(&error))?;

    Ok(inspect_managed_server(kind))
}

pub fn inspect_managed_server(kind: ManagedServerKind) -> ManagedServerInstallStatus {
    match kind {
        ManagedServerKind::RustAnalyzer
        | ManagedServerKind::Gopls
        | ManagedServerKind::KotlinLsp
        | ManagedServerKind::Roslyn => inspect_command_record(kind),
        ManagedServerKind::TypescriptLanguageServer
        | ManagedServerKind::Pyright
        | ManagedServerKind::VscodeHtmlLanguageServer
        | ManagedServerKind::VscodeCssLanguageServer
        | ManagedServerKind::VscodeJsonLanguageServer
        | ManagedServerKind::VscodeMarkdownLanguageServer
        | ManagedServerKind::YamlLanguageServer
        | ManagedServerKind::BashLanguageServer
        | ManagedServerKind::DockerfileLanguageServer
        | ManagedServerKind::Intelephense
        | ManagedServerKind::SvelteLanguageServer => inspect_node_hosted_record(
            kind,
            Path::new(node_hosted_language_server_spec(kind).entrypoint_relative_path),
        ),
        ManagedServerKind::Jdtls => inspect_jdtls_record(),
    }
}

fn resolve_managed_server_locked(
    repo_root: &Path,
    kind: ManagedServerKind,
) -> Result<ManagedServerConfiguration, String> {
    match kind {
        ManagedServerKind::RustAnalyzer => install_rust_analyzer(),
        ManagedServerKind::TypescriptLanguageServer => install_typescript_language_server(),
        ManagedServerKind::Pyright => install_pyright(),
        ManagedServerKind::Gopls => install_gopls(),
        ManagedServerKind::KotlinLsp => install_kotlin_lsp(),
        ManagedServerKind::Jdtls => install_jdtls(repo_root),
        ManagedServerKind::Roslyn => install_roslyn(),
        ManagedServerKind::VscodeHtmlLanguageServer
        | ManagedServerKind::VscodeCssLanguageServer
        | ManagedServerKind::VscodeJsonLanguageServer
        | ManagedServerKind::VscodeMarkdownLanguageServer
        | ManagedServerKind::YamlLanguageServer
        | ManagedServerKind::BashLanguageServer
        | ManagedServerKind::DockerfileLanguageServer
        | ManagedServerKind::Intelephense
        | ManagedServerKind::SvelteLanguageServer => install_node_hosted_language_server(kind),
    }
}

fn install_kotlin_lsp() -> Result<ManagedServerConfiguration, String> {
    if let Some(record) = read_installed_server_record(ManagedServerKind::KotlinLsp)? {
        let command = PathBuf::from(&record.command_path);
        if command.is_file() {
            return Ok(ManagedServerConfiguration {
                command: path_to_string(&command)?,
                args: vec!["--stdio".to_string()],
            });
        }
    }

    let release: GithubRelease =
        fetch_json("https://api.github.com/repos/Kotlin/kotlin-lsp/releases/latest")?;
    let version = extract_kotlin_release_version(&release.tag_name)
        .ok_or_else(|| format!("Unsupported Kotlin LSP release tag '{}'.", release.tag_name))?;
    let download_url = kotlin_lsp_download_url(&version)?;
    let install_dir =
        server_root(ManagedServerKind::KotlinLsp).join(format!("kotlin-lsp-{version}"));
    let archive_path = install_dir.with_extension("zip");
    download_to_file(&download_url, &archive_path)?;
    extract_zip_archive(&archive_path, &install_dir)?;
    let _ = fs::remove_file(&archive_path);

    let command = install_dir.join(match env::consts::OS {
        "windows" => "kotlin-lsp.cmd",
        _ => "kotlin-lsp.sh",
    });
    if !command.is_file() {
        return Err(format!(
            "Kotlin LSP installed to {}, but the launcher script was not found.",
            install_dir.display()
        ));
    }
    make_executable(&command)?;

    write_installed_server_record(
        ManagedServerKind::KotlinLsp,
        &InstalledServerRecord {
            version: version.clone(),
            install_dir: path_to_string(&install_dir)?,
            command_path: path_to_string(&command)?,
        },
    )?;
    cleanup_other_versions(
        ManagedServerKind::KotlinLsp,
        install_dir.file_name().and_then(|name| name.to_str()),
    )?;

    Ok(ManagedServerConfiguration {
        command: path_to_string(&command)?,
        args: vec!["--stdio".to_string()],
    })
}

fn install_jdtls(repo_root: &Path) -> Result<ManagedServerConfiguration, String> {
    let install_dir = ensure_jdtls_installed()?;

    let java = resolve_java_binary()?;
    let java_major_version = java_major_version(&java)?;
    if java_major_version < MINIMUM_JDTLS_JAVA_MAJOR_VERSION {
        return Err(format!(
            "Remiss downloaded JDTLS, but it still needs Java {MINIMUM_JDTLS_JAVA_MAJOR_VERSION}+ on PATH or JAVA_HOME. Found Java {java_major_version}."
        ));
    }

    let launcher_jar = find_jdtls_launcher_jar(&install_dir)?;
    let config_dir = install_dir.join(jdtls_config_dir_name());
    if !config_dir.is_dir() {
        return Err(format!(
            "JDTLS installed to {}, but {} is missing.",
            install_dir.display(),
            config_dir.display()
        ));
    }

    let data_dir = jdtls_workspace_data_dir(repo_root)?;
    fs::create_dir_all(&data_dir)
        .map_err(|error| format!("Failed to create JDTLS workspace data directory: {error}"))?;

    Ok(ManagedServerConfiguration {
        command: path_to_string(&java)?,
        args: vec![
            "-Declipse.application=org.eclipse.jdt.ls.core.id1".to_string(),
            "-Dosgi.bundles.defaultStartLevel=4".to_string(),
            "-Declipse.product=org.eclipse.jdt.ls.core.product".to_string(),
            "-Dosgi.checkConfiguration=true".to_string(),
            format!(
                "-Dosgi.sharedConfiguration.area={}",
                path_to_string(&config_dir)?
            ),
            "-Dosgi.sharedConfiguration.area.readOnly=true".to_string(),
            "-Dosgi.configuration.cascaded=true".to_string(),
            "-Djava.import.generatesMetadataFilesAtProjectRoot=false".to_string(),
            "-Xms1G".to_string(),
            "--add-modules=ALL-SYSTEM".to_string(),
            "--add-opens".to_string(),
            "java.base/java.util=ALL-UNNAMED".to_string(),
            "--add-opens".to_string(),
            "java.base/java.lang=ALL-UNNAMED".to_string(),
            "-jar".to_string(),
            path_to_string(&launcher_jar)?,
            "-data".to_string(),
            path_to_string(&data_dir)?,
        ],
    })
}

fn ensure_jdtls_installed() -> Result<PathBuf, String> {
    if let Some(record) = read_installed_server_record(ManagedServerKind::Jdtls)? {
        let install_dir = PathBuf::from(&record.install_dir);
        if install_dir.is_dir() && find_jdtls_launcher_jar(&install_dir).is_ok() {
            return Ok(install_dir);
        }
    }

    download_jdtls()
}

fn download_jdtls() -> Result<PathBuf, String> {
    let tags: Vec<GithubTag> =
        fetch_json("https://api.github.com/repos/eclipse-jdtls/eclipse.jdt.ls/tags?per_page=5")?;
    let mut last_error = None;

    for tag in tags {
        match download_jdtls_for_tag(&tag.name) {
            Ok(path) => return Ok(path),
            Err(error) => last_error = Some(error),
        }
    }

    Err(last_error.unwrap_or_else(|| "Failed to resolve a downloadable JDTLS build.".to_string()))
}

fn download_jdtls_for_tag(tag: &str) -> Result<PathBuf, String> {
    let build_name = fetch_text(format!(
        "https://download.eclipse.org/jdtls/milestones/{tag}/latest.txt"
    ))?;
    let build_name = build_name.trim();
    if build_name.is_empty() {
        return Err(format!("No JDTLS build is published for {tag}."));
    }

    let install_dir_name = build_name.trim_end_matches(".tar.gz");
    let install_dir = server_root(ManagedServerKind::Jdtls).join(install_dir_name);
    if install_dir.is_dir() && find_jdtls_launcher_jar(&install_dir).is_ok() {
        write_installed_server_record(
            ManagedServerKind::Jdtls,
            &InstalledServerRecord {
                version: tag.to_string(),
                install_dir: path_to_string(&install_dir)?,
                command_path: "java".to_string(),
            },
        )?;
        cleanup_other_versions(
            ManagedServerKind::Jdtls,
            install_dir.file_name().and_then(|name| name.to_str()),
        )?;
        return Ok(install_dir);
    }

    let archive_path = install_dir.with_extension("tar.gz");
    let download_url = format!(
        "https://www.eclipse.org/downloads/download.php?file=/jdtls/milestones/{tag}/{build_name}"
    );
    download_to_file(&download_url, &archive_path)?;
    extract_tar_gz_archive(&archive_path, &install_dir)?;
    let _ = fs::remove_file(&archive_path);

    if find_jdtls_launcher_jar(&install_dir).is_err() {
        return Err(format!(
            "Downloaded JDTLS from {download_url}, but the launcher jar was not found."
        ));
    }

    write_installed_server_record(
        ManagedServerKind::Jdtls,
        &InstalledServerRecord {
            version: tag.to_string(),
            install_dir: path_to_string(&install_dir)?,
            command_path: "java".to_string(),
        },
    )?;
    cleanup_other_versions(
        ManagedServerKind::Jdtls,
        install_dir.file_name().and_then(|name| name.to_str()),
    )?;
    Ok(install_dir)
}

fn install_roslyn() -> Result<ManagedServerConfiguration, String> {
    if let Some(record) = read_installed_server_record(ManagedServerKind::Roslyn)? {
        let command = PathBuf::from(&record.command_path);
        if command.is_file() {
            return build_roslyn_configuration(command);
        }
    }

    let rid = roslyn_rid();
    let package_id = format!("roslyn-language-server.{rid}").to_ascii_lowercase();
    let index_url = format!("https://api.nuget.org/v3-flatcontainer/{package_id}/index.json");
    let versions: NugetVersions = fetch_json(index_url)?;
    let version = versions
        .versions
        .last()
        .cloned()
        .ok_or_else(|| format!("No Roslyn package versions were found for {package_id}."))?;

    let install_dir =
        server_root(ManagedServerKind::Roslyn).join(format!("roslyn-language-server-{version}"));
    let package_path = install_dir.with_extension("nupkg");
    let download_url = format!(
        "https://api.nuget.org/v3-flatcontainer/{package_id}/{version}/{package_id}.{version}.nupkg"
    );
    download_to_file(&download_url, &package_path)?;
    extract_zip_archive(&package_path, &install_dir)?;
    let _ = fs::remove_file(&package_path);

    let command = resolve_roslyn_command_path(&install_dir, rid)?;
    if command.extension().and_then(|ext| ext.to_str()) != Some("dll") {
        make_executable(&command)?;
    }

    write_installed_server_record(
        ManagedServerKind::Roslyn,
        &InstalledServerRecord {
            version,
            install_dir: path_to_string(&install_dir)?,
            command_path: path_to_string(&command)?,
        },
    )?;
    cleanup_other_versions(
        ManagedServerKind::Roslyn,
        install_dir.file_name().and_then(|name| name.to_str()),
    )?;

    build_roslyn_configuration(command)
}

fn build_roslyn_configuration(command: PathBuf) -> Result<ManagedServerConfiguration, String> {
    if command.extension().and_then(|ext| ext.to_str()) == Some("dll") {
        let dotnet = resolve_binary_from_path("dotnet").ok_or_else(|| {
            "Roslyn was installed as a .NET assembly, but dotnet is not available in PATH."
                .to_string()
        })?;
        return Ok(ManagedServerConfiguration {
            command: path_to_string(&dotnet)?,
            args: vec![
                "exec".to_string(),
                path_to_string(&command)?,
                "--stdio".to_string(),
                "--autoLoadProjects".to_string(),
            ],
        });
    }

    Ok(ManagedServerConfiguration {
        command: path_to_string(&command)?,
        args: vec!["--stdio".to_string(), "--autoLoadProjects".to_string()],
    })
}

fn install_rust_analyzer() -> Result<ManagedServerConfiguration, String> {
    if let Some(record) = read_installed_server_record(ManagedServerKind::RustAnalyzer)? {
        let command = PathBuf::from(&record.command_path);
        if command.is_file() {
            return Ok(ManagedServerConfiguration {
                command: path_to_string(&command)?,
                args: Vec::new(),
            });
        }
    }

    let release: GithubRelease =
        fetch_json("https://api.github.com/repos/rust-lang/rust-analyzer/releases/latest")?;
    let asset_name = rust_analyzer_asset_name()?;
    let asset = release
        .assets
        .iter()
        .find(|asset| asset.name == asset_name)
        .ok_or_else(|| {
            format!(
                "rust-analyzer release '{}' does not include '{}'.",
                release.tag_name, asset_name
            )
        })?;
    let install_dir = server_root(ManagedServerKind::RustAnalyzer).join(format!(
        "rust-analyzer-{}",
        release.tag_name.trim_start_matches('v')
    ));
    let existing_command = install_dir.join(rust_analyzer_binary_name());
    if existing_command.is_file() {
        make_executable(&existing_command)?;
        write_installed_server_record(
            ManagedServerKind::RustAnalyzer,
            &InstalledServerRecord {
                version: release.tag_name,
                install_dir: path_to_string(&install_dir)?,
                command_path: path_to_string(&existing_command)?,
            },
        )?;
        cleanup_other_versions(
            ManagedServerKind::RustAnalyzer,
            install_dir.file_name().and_then(|name| name.to_str()),
        )?;
        return Ok(ManagedServerConfiguration {
            command: path_to_string(&existing_command)?,
            args: Vec::new(),
        });
    }

    let archive_path = install_dir.with_extension(rust_analyzer_archive_extension());
    download_to_file(&asset.browser_download_url, &archive_path)?;

    let command = if asset.name.ends_with(".zip") {
        extract_zip_archive(&archive_path, &install_dir)?;
        let command =
            find_file_in_tree(&install_dir, rust_analyzer_binary_name()).ok_or_else(|| {
                format!(
                    "Downloaded rust-analyzer from '{}', but '{}' was not found in '{}'.",
                    asset.browser_download_url,
                    rust_analyzer_binary_name(),
                    install_dir.display()
                )
            })?;
        if env::consts::OS != "windows" {
            make_executable(&command)?;
        }
        command
    } else {
        fs::create_dir_all(&install_dir).map_err(|error| {
            format!(
                "Failed to create rust-analyzer install directory '{}': {error}",
                install_dir.display()
            )
        })?;
        let command = install_dir.join(rust_analyzer_binary_name());
        extract_gzip_file(&archive_path, &command)?;
        make_executable(&command)?;
        command
    };
    let _ = fs::remove_file(&archive_path);

    write_installed_server_record(
        ManagedServerKind::RustAnalyzer,
        &InstalledServerRecord {
            version: release.tag_name,
            install_dir: path_to_string(&install_dir)?,
            command_path: path_to_string(&command)?,
        },
    )?;
    cleanup_other_versions(
        ManagedServerKind::RustAnalyzer,
        install_dir.file_name().and_then(|name| name.to_str()),
    )?;

    Ok(ManagedServerConfiguration {
        command: path_to_string(&command)?,
        args: Vec::new(),
    })
}

fn node_hosted_language_server_spec(kind: ManagedServerKind) -> NodeHostedLanguageServerSpec {
    match kind {
        ManagedServerKind::TypescriptLanguageServer => NodeHostedLanguageServerSpec {
            package_name: "typescript-language-server",
            entrypoint_relative_path: "node_modules/typescript-language-server/lib/cli.mjs",
            args: &["--stdio"],
            extra_packages: &["typescript@latest"],
        },
        ManagedServerKind::Pyright => NodeHostedLanguageServerSpec {
            package_name: "pyright",
            entrypoint_relative_path: "node_modules/pyright/langserver.index.js",
            args: &["--stdio"],
            extra_packages: &[],
        },
        ManagedServerKind::VscodeHtmlLanguageServer => NodeHostedLanguageServerSpec {
            package_name: "vscode-langservers-extracted",
            entrypoint_relative_path:
                "node_modules/vscode-langservers-extracted/bin/vscode-html-language-server",
            args: &["--stdio"],
            extra_packages: &[],
        },
        ManagedServerKind::VscodeCssLanguageServer => NodeHostedLanguageServerSpec {
            package_name: "vscode-langservers-extracted",
            entrypoint_relative_path:
                "node_modules/vscode-langservers-extracted/bin/vscode-css-language-server",
            args: &["--stdio"],
            extra_packages: &[],
        },
        ManagedServerKind::VscodeJsonLanguageServer => NodeHostedLanguageServerSpec {
            package_name: "vscode-langservers-extracted",
            entrypoint_relative_path:
                "node_modules/vscode-langservers-extracted/bin/vscode-json-language-server",
            args: &["--stdio"],
            extra_packages: &[],
        },
        ManagedServerKind::VscodeMarkdownLanguageServer => NodeHostedLanguageServerSpec {
            package_name: "vscode-langservers-extracted",
            entrypoint_relative_path:
                "node_modules/vscode-langservers-extracted/bin/vscode-markdown-language-server",
            args: &["--stdio"],
            extra_packages: &[],
        },
        ManagedServerKind::YamlLanguageServer => NodeHostedLanguageServerSpec {
            package_name: "yaml-language-server",
            entrypoint_relative_path: "node_modules/yaml-language-server/bin/yaml-language-server",
            args: &["--stdio"],
            extra_packages: &[],
        },
        ManagedServerKind::BashLanguageServer => NodeHostedLanguageServerSpec {
            package_name: "bash-language-server",
            entrypoint_relative_path: "node_modules/bash-language-server/out/cli.js",
            args: &["start"],
            extra_packages: &[],
        },
        ManagedServerKind::DockerfileLanguageServer => NodeHostedLanguageServerSpec {
            package_name: "dockerfile-language-server-nodejs",
            entrypoint_relative_path:
                "node_modules/dockerfile-language-server-nodejs/bin/docker-langserver",
            args: &["--stdio"],
            extra_packages: &[],
        },
        ManagedServerKind::Intelephense => NodeHostedLanguageServerSpec {
            package_name: "intelephense",
            entrypoint_relative_path: "node_modules/intelephense/lib/intelephense.js",
            args: &["--stdio"],
            extra_packages: &[],
        },
        ManagedServerKind::SvelteLanguageServer => NodeHostedLanguageServerSpec {
            package_name: "svelte-language-server",
            entrypoint_relative_path: "node_modules/svelte-language-server/bin/server.js",
            args: &["--stdio"],
            extra_packages: &["typescript@latest"],
        },
        ManagedServerKind::RustAnalyzer
        | ManagedServerKind::Gopls
        | ManagedServerKind::KotlinLsp
        | ManagedServerKind::Jdtls
        | ManagedServerKind::Roslyn => unreachable!("non-Node language server kind"),
    }
}

fn install_typescript_language_server() -> Result<ManagedServerConfiguration, String> {
    install_node_hosted_language_server(ManagedServerKind::TypescriptLanguageServer)
}

fn install_pyright() -> Result<ManagedServerConfiguration, String> {
    install_node_hosted_language_server(ManagedServerKind::Pyright)
}

fn install_node_hosted_language_server(
    kind: ManagedServerKind,
) -> Result<ManagedServerConfiguration, String> {
    let spec = node_hosted_language_server_spec(kind);
    let node_runtime = install_node_runtime()?;
    if let Some(record) = read_installed_server_record(kind)? {
        let entrypoint = PathBuf::from(&record.command_path);
        if entrypoint.is_file() {
            return build_node_hosted_server_configuration(
                &node_runtime.node,
                &entrypoint,
                spec.args,
            );
        }
    }

    let package: NpmPackageMetadata = fetch_json(format!(
        "https://registry.npmjs.org/{}/latest",
        spec.package_name
    ))?;
    let mut packages = vec![format!("{}@{}", spec.package_name, package.version)];
    packages.extend(
        spec.extra_packages
            .iter()
            .map(|package| (*package).to_string()),
    );
    let entrypoint = install_managed_node_package(
        kind,
        &package.version,
        packages,
        Path::new(spec.entrypoint_relative_path),
        &node_runtime,
    )?;

    build_node_hosted_server_configuration(&node_runtime.node, &entrypoint, spec.args)
}

fn install_gopls() -> Result<ManagedServerConfiguration, String> {
    if let Some(record) = read_installed_server_record(ManagedServerKind::Gopls)? {
        let command = PathBuf::from(&record.command_path);
        if command.is_file() {
            return Ok(ManagedServerConfiguration {
                command: path_to_string(&command)?,
                args: Vec::new(),
            });
        }
    }

    let go = resolve_binary_from_path("go").ok_or_else(|| {
        "Remiss can manage gopls installation automatically, but it still needs the Go toolchain on PATH."
            .to_string()
    })?;
    let module: GoModuleVersion =
        fetch_json("https://proxy.golang.org/golang.org/x/tools/gopls/@latest")?;
    let install_dir =
        server_root(ManagedServerKind::Gopls).join(format!("gopls-{}", module.version));
    fs::create_dir_all(&install_dir).map_err(|error| {
        format!(
            "Failed to create managed gopls install directory '{}': {error}",
            install_dir.display()
        )
    })?;

    let mut command = Command::new(&go);
    command
        .arg("install")
        .arg(format!("golang.org/x/tools/gopls@{}", module.version))
        .env("GOBIN", &install_dir);
    run_command(&mut command, "install managed gopls with the Go toolchain")?;

    let binary = install_dir.join(gopls_binary_name());
    if !binary.is_file() {
        return Err(format!(
            "Go installed gopls {}, but '{}' was not created.",
            module.version,
            binary.display()
        ));
    }

    write_installed_server_record(
        ManagedServerKind::Gopls,
        &InstalledServerRecord {
            version: module.version.clone(),
            install_dir: path_to_string(&install_dir)?,
            command_path: path_to_string(&binary)?,
        },
    )?;
    cleanup_other_versions(
        ManagedServerKind::Gopls,
        install_dir.file_name().and_then(|name| name.to_str()),
    )?;

    Ok(ManagedServerConfiguration {
        command: path_to_string(&binary)?,
        args: Vec::new(),
    })
}

fn install_node_runtime() -> Result<NodeRuntimePaths, String> {
    let record_path = node_runtime_record_path();
    if let Some(record) = read_installed_record(&record_path)? {
        let install_dir = PathBuf::from(&record.install_dir);
        if let Ok(paths) = node_runtime_paths_for_install_dir(&install_dir) {
            return Ok(paths);
        }
    }

    let release = latest_lts_node_release()?;
    let archive_name = node_archive_name(&release.version)?;
    let runtime_root = node_runtime_root();
    let install_dir = runtime_root.join(node_archive_root_dir_name(&release.version)?);
    if let Ok(paths) = node_runtime_paths_for_install_dir(&install_dir) {
        write_installed_record(
            &record_path,
            &InstalledServerRecord {
                version: release.version.clone(),
                install_dir: path_to_string(&install_dir)?,
                command_path: path_to_string(&paths.node)?,
            },
        )?;
        cleanup_directory_entries(
            &runtime_root,
            install_dir.file_name().and_then(|name| name.to_str()),
        )?;
        return Ok(paths);
    }

    fs::create_dir_all(&runtime_root).map_err(|error| {
        format!(
            "Failed to create managed Node runtime directory '{}': {error}",
            runtime_root.display()
        )
    })?;
    let archive_path = runtime_root.join(&archive_name);
    let download_url = format!(
        "https://nodejs.org/dist/{}/{}",
        release.version, archive_name
    );
    download_to_file(&download_url, &archive_path)?;
    if archive_name.ends_with(".zip") {
        extract_zip_archive(&archive_path, &runtime_root)?;
    } else {
        extract_tar_gz_archive(&archive_path, &runtime_root)?;
    }
    let _ = fs::remove_file(&archive_path);

    let paths = node_runtime_paths_for_install_dir(&install_dir)?;
    write_installed_record(
        &record_path,
        &InstalledServerRecord {
            version: release.version.clone(),
            install_dir: path_to_string(&install_dir)?,
            command_path: path_to_string(&paths.node)?,
        },
    )?;
    cleanup_directory_entries(
        &runtime_root,
        install_dir.file_name().and_then(|name| name.to_str()),
    )?;

    Ok(paths)
}

fn install_managed_node_package(
    kind: ManagedServerKind,
    version: &str,
    packages: Vec<String>,
    entrypoint_relative_path: &Path,
    node_runtime: &NodeRuntimePaths,
) -> Result<PathBuf, String> {
    let install_dir = server_root(kind).join(format!("{}-{version}", server_root_name(kind)));
    let entrypoint = install_dir.join(entrypoint_relative_path);
    if entrypoint.is_file() {
        write_installed_server_record(
            kind,
            &InstalledServerRecord {
                version: version.to_string(),
                install_dir: path_to_string(&install_dir)?,
                command_path: path_to_string(&entrypoint)?,
            },
        )?;
        cleanup_other_versions(kind, install_dir.file_name().and_then(|name| name.to_str()))?;
        return Ok(entrypoint);
    }

    fs::create_dir_all(&install_dir).map_err(|error| {
        format!(
            "Failed to create managed package directory '{}': {error}",
            install_dir.display()
        )
    })?;

    let mut command = Command::new(&node_runtime.npm);
    command
        .arg("install")
        .arg("--prefix")
        .arg(&install_dir)
        .arg("--no-save")
        .arg("--no-package-lock")
        .arg("--omit=dev");
    for package in packages {
        command.arg(package);
    }
    prepend_path_entry(
        &mut command,
        node_runtime
            .node
            .parent()
            .unwrap_or(&node_runtime.install_dir),
    )?;
    run_command(
        &mut command,
        &format!("install {}", managed_server_display_name(kind)),
    )?;

    if !entrypoint.is_file() {
        return Err(format!(
            "{} was installed into '{}', but '{}' is missing.",
            managed_server_display_name(kind),
            install_dir.display(),
            entrypoint.display()
        ));
    }

    write_installed_server_record(
        kind,
        &InstalledServerRecord {
            version: version.to_string(),
            install_dir: path_to_string(&install_dir)?,
            command_path: path_to_string(&entrypoint)?,
        },
    )?;
    cleanup_other_versions(kind, install_dir.file_name().and_then(|name| name.to_str()))?;

    Ok(entrypoint)
}

fn build_node_hosted_server_configuration(
    node_binary: &Path,
    entrypoint: &Path,
    args: &[&str],
) -> Result<ManagedServerConfiguration, String> {
    if !entrypoint.is_file() {
        return Err(format!(
            "Managed Node-hosted server entrypoint '{}' does not exist.",
            entrypoint.display()
        ));
    }

    let mut command_args = vec![path_to_string(entrypoint)?];
    command_args.extend(args.iter().map(|arg| (*arg).to_string()));
    Ok(ManagedServerConfiguration {
        command: path_to_string(node_binary)?,
        args: command_args,
    })
}

fn inspect_command_record(kind: ManagedServerKind) -> ManagedServerInstallStatus {
    match read_installed_server_record(kind) {
        Ok(Some(record)) => {
            let command = PathBuf::from(&record.command_path);
            if command.is_file() {
                installed_status(&record, "Installed and ready.".to_string())
            } else {
                broken_status(
                    &record,
                    "Managed install metadata exists, but the server executable is missing."
                        .to_string(),
                )
            }
        }
        Ok(None) => not_installed_status("Not downloaded yet.".to_string()),
        Err(error) => broken_status_without_record(error),
    }
}

fn inspect_node_hosted_record(
    kind: ManagedServerKind,
    entrypoint_relative_path: &Path,
) -> ManagedServerInstallStatus {
    match read_installed_server_record(kind) {
        Ok(Some(record)) => {
            let install_dir = PathBuf::from(&record.install_dir);
            let entrypoint = install_dir.join(entrypoint_relative_path);
            if !entrypoint.is_file() {
                return broken_status(
                    &record,
                    "Managed install metadata exists, but the server files are missing."
                        .to_string(),
                );
            }

            match read_installed_record(&node_runtime_record_path()) {
                Ok(Some(node_record)) => {
                    let node_install_dir = PathBuf::from(&node_record.install_dir);
                    match node_runtime_paths_for_install_dir(&node_install_dir) {
                        Ok(_) => installed_status(
                            &record,
                            "Installed and ready with the app-managed Node.js runtime."
                                .to_string(),
                        ),
                        Err(error) => broken_status(
                            &record,
                            format!("Managed server exists, but the shared Node.js runtime is broken: {error}"),
                        ),
                    }
                }
                Ok(None) => broken_status(
                    &record,
                    "Managed server exists, but the shared Node.js runtime is not installed."
                        .to_string(),
                ),
                Err(error) => broken_status(
                    &record,
                    format!(
                        "Managed server exists, but Node.js metadata could not be read: {error}"
                    ),
                ),
            }
        }
        Ok(None) => not_installed_status("Not downloaded yet.".to_string()),
        Err(error) => broken_status_without_record(error),
    }
}

fn inspect_jdtls_record() -> ManagedServerInstallStatus {
    match read_installed_server_record(ManagedServerKind::Jdtls) {
        Ok(Some(record)) => {
            let install_dir = PathBuf::from(&record.install_dir);
            if !install_dir.is_dir() {
                return broken_status(
                    &record,
                    "Managed install metadata exists, but the JDTLS files are missing.".to_string(),
                );
            }

            match find_jdtls_launcher_jar(&install_dir) {
                Ok(_) => installed_status(&record, "Installed and ready.".to_string()),
                Err(error) => broken_status(&record, error),
            }
        }
        Ok(None) => not_installed_status("Not downloaded yet.".to_string()),
        Err(error) => broken_status_without_record(error),
    }
}

fn not_installed_status(detail: String) -> ManagedServerInstallStatus {
    ManagedServerInstallStatus {
        state: ManagedServerInstallState::NotInstalled,
        version: None,
        detail,
    }
}

fn installed_status(record: &InstalledServerRecord, detail: String) -> ManagedServerInstallStatus {
    ManagedServerInstallStatus {
        state: ManagedServerInstallState::Installed,
        version: Some(record.version.clone()),
        detail: sanitize_managed_server_message(&detail),
    }
}

fn broken_status(record: &InstalledServerRecord, detail: String) -> ManagedServerInstallStatus {
    ManagedServerInstallStatus {
        state: ManagedServerInstallState::Broken,
        version: Some(record.version.clone()),
        detail: sanitize_managed_server_message(&detail),
    }
}

fn broken_status_without_record(detail: String) -> ManagedServerInstallStatus {
    ManagedServerInstallStatus {
        state: ManagedServerInstallState::Broken,
        version: None,
        detail: sanitize_managed_server_message(&detail),
    }
}

fn sanitize_managed_server_message(message: &str) -> String {
    let storage_root = app_storage::data_dir_root().display().to_string();
    if storage_root.is_empty() {
        return message.to_string();
    }

    let mut sanitized = String::with_capacity(message.len());
    let mut index = 0usize;

    while let Some(start_offset) = message[index..].find('\'') {
        let start = index + start_offset;
        let Some(end_offset) = message[start + 1..].find('\'') else {
            sanitized.push_str(&message[index..]);
            return sanitized.replace(&storage_root, "app storage");
        };
        let end = start + 1 + end_offset;
        let segment = &message[start + 1..end];

        sanitized.push_str(&message[index..start]);
        if segment.contains(&storage_root) {
            sanitized.push_str("'app storage'");
        } else {
            sanitized.push_str(&message[start..=end]);
        }
        index = end + 1;
    }

    sanitized.push_str(&message[index..]);
    sanitized.replace(&storage_root, "app storage")
}

fn resolve_roslyn_command_path(install_dir: &Path, rid: &str) -> Result<PathBuf, String> {
    let tools_dir = install_dir.join("tools");
    let tfm_dir = fs::read_dir(&tools_dir)
        .map_err(|error| {
            format!(
                "Failed to read Roslyn tools directory '{}': {error}",
                tools_dir.display()
            )
        })?
        .filter_map(Result::ok)
        .find_map(|entry| {
            entry
                .file_type()
                .ok()
                .filter(|file_type| file_type.is_dir())
                .map(|_| entry.path())
        })
        .ok_or_else(|| {
            format!(
                "No Roslyn runtime directory exists in '{}'.",
                tools_dir.display()
            )
        })?;

    let server_dir = tfm_dir.join(rid);
    let native_binary = server_dir.join(match env::consts::OS {
        "windows" => "Microsoft.CodeAnalysis.LanguageServer.exe",
        _ => "Microsoft.CodeAnalysis.LanguageServer",
    });
    if native_binary.is_file() {
        return Ok(native_binary);
    }

    let managed_binary = server_dir.join("Microsoft.CodeAnalysis.LanguageServer.dll");
    if managed_binary.is_file() {
        return Ok(managed_binary);
    }

    Err(format!(
        "Roslyn installed to '{}', but no runnable server binary was found.",
        server_dir.display()
    ))
}

fn extract_kotlin_release_version(tag_name: &str) -> Option<String> {
    tag_name
        .strip_prefix("kotlin-lsp/v")
        .or_else(|| tag_name.strip_prefix('v'))
        .map(|version| version.to_string())
}

fn kotlin_lsp_download_url(version: &str) -> Result<String, String> {
    let platform = match env::consts::OS {
        "macos" => "mac",
        "linux" => "linux",
        "windows" => "win",
        other => {
            return Err(format!(
                "Managed Kotlin LSP is not supported on '{other}' yet."
            ))
        }
    };
    let arch = match env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "aarch64",
        other => {
            return Err(format!(
                "Managed Kotlin LSP is not supported on architecture '{other}' yet."
            ))
        }
    };

    Ok(format!(
        "https://download-cdn.jetbrains.com/kotlin-lsp/{version}/kotlin-lsp-{version}-{platform}-{arch}.zip"
    ))
}

fn rust_analyzer_asset_name() -> Result<String, String> {
    let asset_name = match (env::consts::OS, env::consts::ARCH) {
        ("macos", "x86_64") => "rust-analyzer-x86_64-apple-darwin.gz",
        ("macos", "aarch64") => "rust-analyzer-aarch64-apple-darwin.gz",
        ("linux", "x86_64") => "rust-analyzer-x86_64-unknown-linux-gnu.gz",
        ("linux", "aarch64") => "rust-analyzer-aarch64-unknown-linux-gnu.gz",
        ("windows", "x86_64") => "rust-analyzer-x86_64-pc-windows-msvc.zip",
        ("windows", "aarch64") => "rust-analyzer-aarch64-pc-windows-msvc.zip",
        (os, arch) => {
            return Err(format!(
                "Managed rust-analyzer is not supported on '{os}/{arch}' yet."
            ))
        }
    };
    Ok(asset_name.to_string())
}

fn rust_analyzer_archive_extension() -> &'static str {
    if env::consts::OS == "windows" {
        "zip"
    } else {
        "gz"
    }
}

fn rust_analyzer_binary_name() -> &'static str {
    if env::consts::OS == "windows" {
        "rust-analyzer.exe"
    } else {
        "rust-analyzer"
    }
}

fn gopls_binary_name() -> &'static str {
    if env::consts::OS == "windows" {
        "gopls.exe"
    } else {
        "gopls"
    }
}

fn node_runtime_root() -> PathBuf {
    managed_servers_root().join("node-runtime")
}

fn node_runtime_record_path() -> PathBuf {
    node_runtime_root().join("current.json")
}

fn latest_lts_node_release() -> Result<NodeRelease, String> {
    let releases: Vec<NodeRelease> = fetch_json("https://nodejs.org/dist/index.json")?;
    let required_file = node_supported_file_marker()?;
    releases
        .into_iter()
        .find(|release| {
            node_release_is_lts(release) && release.files.iter().any(|file| file == required_file)
        })
        .ok_or_else(|| {
            format!(
                "Failed to find a Node.js LTS release that supports '{}'.",
                required_file
            )
        })
}

fn node_release_is_lts(release: &NodeRelease) -> bool {
    match &release.lts {
        Value::Bool(false) | Value::Null => false,
        Value::String(value) if value.is_empty() => false,
        _ => true,
    }
}

fn node_supported_file_marker() -> Result<&'static str, String> {
    match (env::consts::OS, env::consts::ARCH) {
        ("macos", "x86_64") => Ok("osx-x64-tar"),
        ("macos", "aarch64") => Ok("osx-arm64-tar"),
        ("linux", "x86_64") => Ok("linux-x64-tar"),
        ("linux", "aarch64") => Ok("linux-arm64-tar"),
        ("windows", "x86_64") => Ok("win-x64-zip"),
        ("windows", "aarch64") => Ok("win-arm64-zip"),
        (os, arch) => Err(format!(
            "Managed Node.js is not supported on '{os}/{arch}' yet."
        )),
    }
}

fn node_archive_platform_segment() -> Result<&'static str, String> {
    match (env::consts::OS, env::consts::ARCH) {
        ("macos", "x86_64") => Ok("darwin-x64"),
        ("macos", "aarch64") => Ok("darwin-arm64"),
        ("linux", "x86_64") => Ok("linux-x64"),
        ("linux", "aarch64") => Ok("linux-arm64"),
        ("windows", "x86_64") => Ok("win-x64"),
        ("windows", "aarch64") => Ok("win-arm64"),
        (os, arch) => Err(format!(
            "Managed Node.js is not supported on '{os}/{arch}' yet."
        )),
    }
}

fn node_archive_name(version: &str) -> Result<String, String> {
    let platform = node_archive_platform_segment()?;
    let extension = if env::consts::OS == "windows" {
        "zip"
    } else {
        "tar.gz"
    };
    Ok(format!("node-{version}-{platform}.{extension}"))
}

fn node_archive_root_dir_name(version: &str) -> Result<String, String> {
    Ok(format!(
        "node-{version}-{}",
        node_archive_platform_segment()?
    ))
}

fn node_runtime_paths_for_install_dir(install_dir: &Path) -> Result<NodeRuntimePaths, String> {
    let node = if env::consts::OS == "windows" {
        install_dir.join("node.exe")
    } else {
        install_dir.join("bin").join("node")
    };
    if !node.is_file() {
        return Err("Managed Node.js runtime is missing the node executable.".to_string());
    }

    let npm = if env::consts::OS == "windows" {
        install_dir.join("npm.cmd")
    } else {
        install_dir.join("bin").join("npm")
    };
    if !npm.is_file() {
        return Err("Managed Node.js runtime is missing the npm executable.".to_string());
    }

    Ok(NodeRuntimePaths {
        install_dir: install_dir.to_path_buf(),
        node,
        npm,
    })
}

fn prepend_path_entry(command: &mut Command, entry: &Path) -> Result<(), String> {
    let mut paths = vec![entry.to_path_buf()];
    if let Some(existing) = env::var_os("PATH") {
        paths.extend(env::split_paths(&existing));
    }
    let joined = env::join_paths(paths)
        .map_err(|error| format!("Failed to build PATH for managed server install: {error}"))?;
    command.env("PATH", joined);
    Ok(())
}

fn run_command(command: &mut Command, description: &str) -> Result<(), String> {
    let output = command
        .output()
        .map_err(|error| format!("Failed to {description}: {error}"))?;
    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let details = if !stderr.is_empty() {
        stderr
    } else if !stdout.is_empty() {
        stdout
    } else {
        format!("process exited with status {}", output.status)
    };

    Err(format!("Failed to {description}: {details}"))
}

fn find_file_in_tree(root: &Path, file_name: &str) -> Option<PathBuf> {
    let entries = fs::read_dir(root).ok()?;
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_file_in_tree(&path, file_name) {
                return Some(found);
            }
            continue;
        }

        if path.file_name().and_then(|name| name.to_str()) == Some(file_name) {
            return Some(path);
        }
    }
    None
}

fn find_jdtls_launcher_jar(install_dir: &Path) -> Result<PathBuf, String> {
    let plugins_dir = install_dir.join("plugins");
    if !plugins_dir.is_dir() {
        return Err("JDTLS is missing its plugins directory.".to_string());
    }

    let exact = plugins_dir.join("org.eclipse.equinox.launcher.jar");
    if exact.is_file() {
        return Ok(exact);
    }

    fs::read_dir(&plugins_dir)
        .map_err(|error| format!("Failed to read the JDTLS plugins directory: {error}"))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.is_file()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        name.starts_with("org.eclipse.equinox.launcher_") && name.ends_with(".jar")
                    })
        })
        .ok_or_else(|| "Could not find an Equinox launcher jar for JDTLS.".to_string())
}

fn jdtls_config_dir_name() -> &'static str {
    match env::consts::OS {
        "macos" => "config_mac",
        "linux" => "config_linux",
        "windows" => "config_win",
        _ => "config_linux",
    }
}

fn jdtls_workspace_data_dir(repo_root: &Path) -> Result<PathBuf, String> {
    let key = path_to_string(repo_root)?;
    let mut hasher = Sha1::new();
    hasher.update(key.as_bytes());
    let digest = hex_string(&hasher.finalize());
    Ok(managed_servers_root()
        .join("jdtls-workspaces")
        .join(format!("jdtls-{digest}")))
}

fn hex_string(bytes: &[u8]) -> String {
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        result.push_str(format!("{byte:02x}").as_str());
    }
    result
}

fn resolve_java_binary() -> Result<PathBuf, String> {
    if let Some(java_home) = env::var_os("JAVA_HOME") {
        let candidate =
            PathBuf::from(java_home)
                .join("bin")
                .join(if env::consts::OS == "windows" {
                    "java.exe"
                } else {
                    "java"
                });
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    resolve_binary_from_path("java").ok_or_else(|| {
        format!(
            "Remiss can download JDTLS automatically, but it still needs Java {MINIMUM_JDTLS_JAVA_MAJOR_VERSION}+ on PATH or JAVA_HOME."
        )
    })
}

fn java_major_version(java_binary: &Path) -> Result<u32, String> {
    let output = Command::new(java_binary)
        .arg("-version")
        .output()
        .map_err(|error| {
            format!(
                "Failed to run '{} -version': {error}",
                java_binary.display()
            )
        })?;
    let text = if output.stderr.is_empty() {
        String::from_utf8_lossy(&output.stdout).into_owned()
    } else {
        String::from_utf8_lossy(&output.stderr).into_owned()
    };
    parse_java_major_version(&text).ok_or_else(|| {
        format!(
            "Failed to parse the Java version from '{}'.",
            text.lines().next().unwrap_or_default()
        )
    })
}

fn parse_java_major_version(text: &str) -> Option<u32> {
    let version = text.split('"').nth(1)?;
    if let Some(rest) = version.strip_prefix("1.") {
        return rest.split('.').next()?.parse().ok();
    }
    version
        .split(|character: char| !character.is_ascii_digit())
        .find(|segment| !segment.is_empty())
        .and_then(|segment| segment.parse().ok())
}

fn roslyn_rid() -> &'static str {
    match (env::consts::OS, env::consts::ARCH) {
        ("windows", "x86_64") => "win-x64",
        ("windows", "aarch64") => "win-arm64",
        ("linux", "x86_64") => "linux-x64",
        ("linux", "aarch64") => "linux-arm64",
        ("macos", "x86_64") => "osx-x64",
        ("macos", "aarch64") => "osx-arm64",
        _ => "any",
    }
}

fn managed_servers_root() -> PathBuf {
    app_storage::managed_servers_root()
}

fn managed_install_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn server_root(kind: ManagedServerKind) -> PathBuf {
    managed_servers_root().join(server_root_name(kind))
}

fn server_root_name(kind: ManagedServerKind) -> &'static str {
    match kind {
        ManagedServerKind::RustAnalyzer => "rust-analyzer",
        ManagedServerKind::TypescriptLanguageServer => "typescript-language-server",
        ManagedServerKind::Pyright => "pyright",
        ManagedServerKind::Gopls => "gopls",
        ManagedServerKind::KotlinLsp => "kotlin-lsp",
        ManagedServerKind::Jdtls => "jdtls",
        ManagedServerKind::Roslyn => "roslyn",
        ManagedServerKind::VscodeHtmlLanguageServer => "html-language-server",
        ManagedServerKind::VscodeCssLanguageServer => "css-language-server",
        ManagedServerKind::VscodeJsonLanguageServer => "json-language-server",
        ManagedServerKind::VscodeMarkdownLanguageServer => "markdown-language-server",
        ManagedServerKind::YamlLanguageServer => "yaml-language-server",
        ManagedServerKind::BashLanguageServer => "bash-language-server",
        ManagedServerKind::DockerfileLanguageServer => "dockerfile-language-server",
        ManagedServerKind::Intelephense => "intelephense",
        ManagedServerKind::SvelteLanguageServer => "svelte-language-server",
    }
}

fn current_record_path(kind: ManagedServerKind) -> PathBuf {
    server_root(kind).join("current.json")
}

fn read_installed_server_record(
    kind: ManagedServerKind,
) -> Result<Option<InstalledServerRecord>, String> {
    read_installed_record(&current_record_path(kind))
}

fn read_installed_record(path: &Path) -> Result<Option<InstalledServerRecord>, String> {
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("Failed to read managed server metadata: {error}")),
    };

    let record: InstalledServerRecord = serde_json::from_slice(&bytes)
        .map_err(|error| format!("Failed to parse managed server metadata: {error}"))?;
    let relocated = relocate_installed_record_paths(&record);
    if relocated != record {
        write_installed_record(path, &relocated)?;
    }

    Ok(Some(relocated))
}

fn write_installed_server_record(
    kind: ManagedServerKind,
    record: &InstalledServerRecord,
) -> Result<(), String> {
    write_installed_record(&current_record_path(kind), record)
}

fn write_installed_record(path: &Path, record: &InstalledServerRecord) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!("Failed to create the managed server metadata directory: {error}")
        })?;
    }

    let json = serde_json::to_vec_pretty(record)
        .map_err(|error| format!("Failed to serialize managed server metadata: {error}"))?;
    fs::write(&path, json)
        .map_err(|error| format!("Failed to write managed server metadata: {error}"))
}

fn relocate_installed_record_paths(record: &InstalledServerRecord) -> InstalledServerRecord {
    InstalledServerRecord {
        version: record.version.clone(),
        install_dir: relocate_managed_storage_path(&record.install_dir)
            .unwrap_or_else(|| record.install_dir.clone()),
        command_path: relocate_managed_storage_path(&record.command_path)
            .unwrap_or_else(|| record.command_path.clone()),
    }
}

fn relocate_managed_storage_path(path: &str) -> Option<String> {
    let original = Path::new(path);
    if original.exists() {
        return None;
    }

    let rebased = rebase_managed_storage_path(original, &managed_servers_root())?;
    rebased
        .exists()
        .then(|| path_to_string(&rebased).ok())
        .flatten()
}

fn rebase_managed_storage_path(path: &Path, managed_root: &Path) -> Option<PathBuf> {
    let mut after_lsp_root = Vec::new();
    let mut found_lsp_root = false;

    for component in path.components() {
        let segment = component.as_os_str();
        if found_lsp_root {
            after_lsp_root.push(segment.to_os_string());
        } else if segment == std::ffi::OsStr::new(MANAGED_LSP_DIR) {
            found_lsp_root = true;
        }
    }

    if !found_lsp_root || after_lsp_root.is_empty() {
        return None;
    }

    let mut rebased = managed_root.to_path_buf();
    for segment in after_lsp_root {
        rebased.push(segment);
    }
    Some(rebased)
}

fn cleanup_other_versions(kind: ManagedServerKind, keep_name: Option<&str>) -> Result<(), String> {
    cleanup_directory_entries(&server_root(kind), keep_name)
}

fn cleanup_directory_entries(root: &Path, keep_name: Option<&str>) -> Result<(), String> {
    if !root.is_dir() {
        return Ok(());
    }

    for entry in fs::read_dir(&root)
        .map_err(|error| format!("Failed to read '{}': {error}", root.display()))?
    {
        let entry = entry.map_err(|error| {
            format!(
                "Failed to inspect a managed server entry in '{}': {error}",
                root.display()
            )
        })?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == "current.json" || keep_name.is_some_and(|keep| keep == name.as_ref()) {
            continue;
        }

        if path.is_dir() {
            let _ = fs::remove_dir_all(&path);
        } else {
            let _ = fs::remove_file(&path);
        }
    }

    Ok(())
}

fn download_to_file(url: &str, destination: &Path) -> Result<(), String> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Failed to create '{}': {error}", parent.display()))?;
    }

    let response = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .get(url)
        .set("User-Agent", USER_AGENT)
        .call()
        .map_err(|error| format!("Failed to download {url}: {error}"))?;

    let mut reader = BufReader::new(response.into_reader());
    let temp_path = destination.with_extension(format!(
        "{}.download",
        destination
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or_default()
    ));
    let mut file = File::create(&temp_path).map_err(|error| {
        format!(
            "Failed to create temporary download file '{}': {error}",
            temp_path.display()
        )
    })?;
    std::io::copy(&mut reader, &mut file)
        .map_err(|error| format!("Failed to write '{url}' to disk: {error}"))?;
    file.flush()
        .map_err(|error| format!("Failed to flush '{}': {error}", temp_path.display()))?;
    drop(file);

    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Failed to create '{}': {error}", parent.display()))?;
    }

    match fs::rename(&temp_path, destination) {
        Ok(()) => Ok(()),
        Err(rename_error) => {
            fs::copy(&temp_path, destination).map_err(|copy_error| {
                format!(
                    "Failed to move downloaded file into '{}': {rename_error}; fallback copy also failed: {copy_error}",
                    destination.display()
                )
            })?;
            let _ = fs::remove_file(&temp_path);
            Ok(())
        }
    }
}

fn extract_zip_archive(archive_path: &Path, destination: &Path) -> Result<(), String> {
    fs::create_dir_all(destination).map_err(|error| {
        format!(
            "Failed to create extraction directory '{}': {error}",
            destination.display()
        )
    })?;

    let file = File::open(archive_path)
        .map_err(|error| format!("Failed to open '{}': {error}", archive_path.display()))?;
    let mut archive = ZipArchive::new(file).map_err(|error| {
        format!(
            "Failed to read zip archive '{}': {error}",
            archive_path.display()
        )
    })?;

    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|error| {
            format!(
                "Failed to read entry #{index} from '{}': {error}",
                archive_path.display()
            )
        })?;
        let Some(relative_path) = entry.enclosed_name().map(|path| path.to_path_buf()) else {
            continue;
        };
        let target_path = destination.join(relative_path);
        if entry.is_dir() {
            fs::create_dir_all(&target_path).map_err(|error| {
                format!("Failed to create '{}': {error}", target_path.display())
            })?;
            continue;
        }

        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("Failed to create '{}': {error}", parent.display()))?;
        }

        let mut output = File::create(&target_path)
            .map_err(|error| format!("Failed to create '{}': {error}", target_path.display()))?;
        std::io::copy(&mut entry, &mut output).map_err(|error| {
            format!(
                "Failed to extract '{}' into '{}': {error}",
                archive_path.display(),
                target_path.display()
            )
        })?;
    }

    Ok(())
}

fn extract_tar_gz_archive(archive_path: &Path, destination: &Path) -> Result<(), String> {
    fs::create_dir_all(destination).map_err(|error| {
        format!(
            "Failed to create extraction directory '{}': {error}",
            destination.display()
        )
    })?;

    let file = File::open(archive_path)
        .map_err(|error| format!("Failed to open '{}': {error}", archive_path.display()))?;
    let decoder = GzDecoder::new(file);
    let mut archive = Archive::new(decoder);
    let entries = archive.entries().map_err(|error| {
        format!(
            "Failed to inspect tar archive '{}': {error}",
            archive_path.display()
        )
    })?;

    for entry in entries {
        let mut entry = entry.map_err(|error| {
            format!(
                "Failed to read an entry from '{}': {error}",
                archive_path.display()
            )
        })?;
        entry.unpack_in(destination).map_err(|error| {
            format!(
                "Failed to extract '{}' into '{}': {error}",
                archive_path.display(),
                destination.display()
            )
        })?;
    }

    Ok(())
}

fn extract_gzip_file(archive_path: &Path, destination: &Path) -> Result<(), String> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Failed to create '{}': {error}", parent.display()))?;
    }

    let input = File::open(archive_path)
        .map_err(|error| format!("Failed to open '{}': {error}", archive_path.display()))?;
    let mut decoder = GzDecoder::new(input);
    let mut output = File::create(destination)
        .map_err(|error| format!("Failed to create '{}': {error}", destination.display()))?;
    std::io::copy(&mut decoder, &mut output).map_err(|error| {
        format!(
            "Failed to extract '{}' into '{}': {error}",
            archive_path.display(),
            destination.display()
        )
    })?;
    output
        .flush()
        .map_err(|error| format!("Failed to flush '{}': {error}", destination.display()))
}

fn fetch_text(url: impl AsRef<str>) -> Result<String, String> {
    let url = url.as_ref();
    let response = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .get(url)
        .set("User-Agent", USER_AGENT)
        .call()
        .map_err(|error| format!("Failed to fetch {url}: {error}"))?;

    let mut body = String::new();
    response
        .into_reader()
        .read_to_string(&mut body)
        .map_err(|error| format!("Failed to read {url}: {error}"))?;
    Ok(body)
}

fn fetch_json<T>(url: impl AsRef<str>) -> Result<T, String>
where
    T: serde::de::DeserializeOwned,
{
    let body = fetch_text(url)?;
    serde_json::from_str(&body).map_err(|error| format!("Failed to parse server metadata: {error}"))
}

fn resolve_binary_from_path(binary: &str) -> Option<PathBuf> {
    let binary_path = Path::new(binary);
    if binary_path.components().count() > 1 {
        return binary_path.is_file().then(|| binary_path.to_path_buf());
    }

    env::var_os("PATH").and_then(|paths| {
        env::split_paths(&paths).find_map(|directory| {
            for candidate_name in executable_name_candidates(binary) {
                let candidate = directory.join(candidate_name);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
            None
        })
    })
}

fn executable_name_candidates(binary: &str) -> Vec<String> {
    if env::consts::OS == "windows" && Path::new(binary).extension().is_none() {
        vec![
            binary.to_string(),
            format!("{binary}.exe"),
            format!("{binary}.cmd"),
            format!("{binary}.bat"),
        ]
    } else {
        vec![binary.to_string()]
    }
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    let metadata = fs::metadata(path)
        .map_err(|error| format!("Failed to inspect '{}': {error}", path.display()))?;
    let mut permissions = metadata.permissions();
    permissions.set_mode(permissions.mode() | 0o755);
    fs::set_permissions(path, permissions)
        .map_err(|error| format!("Failed to mark '{}' executable: {error}", path.display()))
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<(), String> {
    Ok(())
}

fn path_to_string(path: &Path) -> Result<String, String> {
    path.to_str()
        .map(|value| value.to_string())
        .ok_or_else(|| format!("'{}' is not valid UTF-8.", path.display()))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        extract_kotlin_release_version, parse_java_major_version, rebase_managed_storage_path,
    };

    #[test]
    fn extracts_kotlin_release_version() {
        assert_eq!(
            extract_kotlin_release_version("kotlin-lsp/v262.2310.0"),
            Some("262.2310.0".to_string())
        );
    }

    #[test]
    fn parses_modern_java_version_output() {
        let output = r#"openjdk version "21.0.2" 2024-01-16"#;
        assert_eq!(parse_java_major_version(output), Some(21));
    }

    #[test]
    fn parses_legacy_java_version_output() {
        let output = r#"java version "1.8.0_432""#;
        assert_eq!(parse_java_major_version(output), Some(8));
    }

    #[test]
    fn rebases_migrated_lsp_storage_paths() {
        let original = Path::new(
            "/Users/example/Library/Application Support/gh-ui-tool/lsp-servers/rust-analyzer/rust-analyzer/rust-analyzer",
        );
        let current = Path::new("/Users/example/Library/Application Support/remiss/lsp-servers");

        assert_eq!(
            rebase_managed_storage_path(original, current),
            Some(current.join("rust-analyzer/rust-analyzer/rust-analyzer"))
        );
    }
}
