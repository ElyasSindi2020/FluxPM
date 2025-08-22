// src/main.rs

use clap::{Parser, Subcommand};
use futures_util::stream::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process;
use thiserror::Error;
use tokio::fs::{self, File};
use tokio::io::{self, AsyncReadExt, AsyncWriteExt};
use url::Url;

// --- Custom Error Types ---
#[derive(Debug, Error)]
enum FluxError {
    #[error("Package '{0}' not found in the repository.")]
    PackageNotFound(String),
    #[error("Checksum mismatch for {package_name}! Expected: {expected}, Found: {found}")]
    ChecksumMismatch {
        package_name: String,
        expected: String,
        found: String,
    },
    #[error("Cannot remove '{package_name}'. It is a dependency for: {dependents:?}")]
    DependencyInUse {
        package_name: String,
        dependents: Vec<String>,
    },
    #[error("I/O Error: {0}")]
    Io(#[from] io::Error),
    #[error("Network request failed: {0}")]
    Network(#[from] reqwest::Error),
    #[error("Failed to parse YAML: {0}")]
    YamlParse(#[from] serde_yaml::Error),
    #[error("Failed to parse JSON: {0}")]
    JsonParse(#[from] serde_json::Error),
    #[error("Archive extraction failed: {0}")]
    Archive(String),
    #[error("Post-install script failed for '{package_name}': {message}")]
    PostInstallScriptFailed {
        package_name: String,
        message: String,
    },
    #[error("System hook failed for '{package_name}' with hook '{hook_script}': {message}")]
    HookFailed {
        package_name: String,
        hook_script: String,
        message: String,
    },
    #[error("Invalid URL in config: {0}")]
    InvalidUrl(#[from] url::ParseError),
    #[error("Configuration Error: {0}")]
    Config(String),
}

// --- Metadata Structures ---
#[derive(Debug, Serialize, Deserialize, Clone)]
struct PackageIndex {
    packages: Vec<PackageInfo>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum PackageType {
    System,
    App,
}

impl Default for PackageType {
    fn default() -> Self {
        PackageType::System
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
enum InstallReason {
    Explicit,
    Dependency,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct PackageInfo {
    name: String,
    #[serde(default)]
    #[serde(rename = "type")]
    package_type: PackageType,
    version: String,
    url: String,
    checksum: String,
    dependencies: Option<Vec<String>>,
    description: String,
    icon_url: String,
    changelog_url: String,
    post_install: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct InstalledPackageInfo {
    name: String,
    version: String,
    package_type: PackageType,
    install_reason: InstallReason,
    files: Vec<PathBuf>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct FluxConfig {
    repository_url: String,
    hooks: Option<HashMap<String, String>>,
}

// --- Application Context ---
struct AppContext {
    host_cache_path: PathBuf,
    target_root: PathBuf,
    target_apps_root: PathBuf,
    target_db_path: PathBuf,
    config: FluxConfig,
    package_index: HashMap<String, PackageInfo>,
}

impl AppContext {
    async fn new(root: PathBuf) -> Result<Self, FluxError> {
        let home_dir = dirs::home_dir().ok_or_else(|| FluxError::Config("Could not find home directory".to_string()))?;
        let host_cache_dir = home_dir.join(".cache/flux");
        fs::create_dir_all(&host_cache_dir).await?;
        let host_cache_path = host_cache_dir.join("repo.yaml");

        let target_apps_root = root.join("flux/apps");
        let target_db_dir = root.join("var/lib/flux");
        let target_db_path = target_db_dir.join("db.json");

        let config_content = fs::read_to_string("flux.conf").await.map_err(|_| FluxError::Config("Could not read flux.conf".to_string()))?;
        let config: FluxConfig = serde_yaml::from_str(&config_content)?;

        if !host_cache_path.exists() {
            println!("No local repository cache found. Please run 'flux update' to fetch it.");
        }

        let package_index = if host_cache_path.exists() {
            let index_content = fs::read_to_string(&host_cache_path).await?;
            let index: PackageIndex = serde_yaml::from_str(&index_content)?;
            index.packages.into_iter().map(|p| (p.name.clone(), p)).collect()
        } else {
            HashMap::new()
        };

        Ok(Self {
            host_cache_path,
            target_root: root,
            target_apps_root,
            target_db_path,
            config,
            package_index,
        })
    }

    fn get_install_path(&self, info: &PackageInfo) -> PathBuf {
        match info.package_type {
            PackageType::System => self.target_root.clone(),
            PackageType::App => self.target_apps_root.join(format!("{}-{}", info.name, info.version)),
        }
    }

    async fn get_installed_packages(&self) -> Result<Vec<InstalledPackageInfo>, FluxError> {
        if !self.target_db_path.exists() { return Ok(Vec::new()); }
        let content = fs::read_to_string(&self.target_db_path).await?;
        if content.trim().is_empty() { return Ok(Vec::new()); }
        Ok(serde_json::from_str(&content)?)
    }

    async fn write_installed_packages(&self, packages: &[InstalledPackageInfo]) -> Result<(), FluxError> {
        fs::create_dir_all(&self.target_db_path.parent().unwrap()).await?;
        let content = serde_json::to_string_pretty(packages)?;
        fs::write(&self.target_db_path, content).await?;
        Ok(())
    }
}

// --- CLI Structure ---
#[derive(Parser)]
#[command(author, version, about = "A fast and reliable package manager for your system.", long_about = None)]
struct Cli {
    #[arg(long, global = true, default_value = "/")]
    root: PathBuf,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Install { package: String },
    Remove { package: String },
    Update,
    Upgrade,
    List,
    Autoremove,
}

// --- Core Logic ---

async fn download_file(url: &Url, dest_path: &Path) -> Result<(), FluxError> {
    if url.scheme() == "file" {
        let source_path = url.to_file_path().map_err(|_| FluxError::Config(format!("Invalid file path in URL: {}", url)))?;
        fs::copy(&source_path, dest_path).await?;
    } else {
        let response = reqwest::get(url.clone()).await?.error_for_status()?;
        let mut stream = response.bytes_stream();
        let mut dest_file = File::create(dest_path).await?;
        while let Some(chunk) = stream.next().await {
            dest_file.write_all(&chunk?).await?;
        }
    }
    Ok(())
}

async fn verify_checksum(info: &PackageInfo, file_path: &Path) -> Result<(), FluxError> {
    println!("Verifying checksum for {}...", info.name);
    let mut file = File::open(file_path).await?;
    let mut hasher = Sha256::new();
    let mut buffer = [0; 1024];
    loop {
        let n = file.read(&mut buffer).await?;
        if n == 0 { break; }
        hasher.update(&buffer[..n]);
    }
    let hash = hasher.finalize();
    let calculated_checksum = format!("{:x}", hash);

    if calculated_checksum == info.checksum {
        println!("Checksum verified.");
        Ok(())
    } else {
        Err(FluxError::ChecksumMismatch {
            package_name: info.name.clone(),
            expected: info.checksum.clone(),
            found: calculated_checksum,
        })
    }
}

async fn extract_package(archive_path: &Path, extract_to: &Path) -> Result<Vec<PathBuf>, FluxError> {
    println!("Decompressing and extracting to {}...", extract_to.display());
    let compressed_bytes = fs::read(archive_path).await?;
    let extract_to_owned = extract_to.to_owned();

    let extracted_files = tokio::task::spawn_blocking(move || -> Result<Vec<PathBuf>, FluxError> {
        let cursor = std::io::Cursor::new(&compressed_bytes);
        let decoder = zstd::stream::read::Decoder::new(cursor).map_err(|e| FluxError::Archive(e.to_string()))?;
        let mut archive = tar::Archive::new(decoder);

        let mut files = Vec::new();
        for entry in archive.entries().map_err(|e| FluxError::Archive(e.to_string()))? {
            let mut entry = entry.map_err(|e| FluxError::Archive(e.to_string()))?;
            let path = entry.path()?.into_owned();
            entry.unpack_in(&extract_to_owned).map_err(|e| FluxError::Archive(e.to_string()))?;
            files.push(path);
        }
        Ok(files)
    }).await.unwrap()?;

    println!("Extraction complete.");
    Ok(extracted_files)
}

fn run_script(script_path: &Path, package_name: &str, error_type: fn(String, String, String) -> FluxError) -> Result<(), FluxError> {
    let output = process::Command::new("sh").arg(script_path).output().map_err(|e| error_type(package_name.to_string(), script_path.to_string_lossy().to_string(), e.to_string()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(error_type(package_name.to_string(), script_path.to_string_lossy().to_string(), stderr.to_string()));
    }
    Ok(())
}

async fn handle_install(package_name: &str, ctx: &AppContext) -> Result<(), FluxError> {
    let mut to_install_names = HashSet::new();
    resolve_dependencies(package_name, ctx, &mut to_install_names)?;

    let installed_packages = ctx.get_installed_packages().await?;
    let installed_names: HashSet<_> = installed_packages.iter().map(|p| p.name.as_str()).collect();

    let packages_to_process: Vec<_> = to_install_names.iter()
        .filter(|name| !installed_names.contains(name.as_str()))
        .map(|name| ctx.package_index.get(name).unwrap().clone())
        .collect();

    if packages_to_process.is_empty() {
        println!("Package '{}' and all its dependencies are already installed.", package_name);
        return Ok(());
    }

    let mut new_install_records = Vec::new();

    for info in &packages_to_process {
        let install_path = ctx.get_install_path(info);
        fs::create_dir_all(&install_path).await?;

        let archive_name = format!("{}-{}.tar.zst", &info.name, &info.version);
        let archive_path = ctx.host_cache_path.parent().unwrap().join(&archive_name);

        let is_placeholder = info.checksum.starts_with("some_") || info.checksum.starts_with("a_real_");
        let mut extracted_files = Vec::new();

        if is_placeholder {
            println!("Skipping download and extraction for {} due to placeholder checksum.", info.name);
        } else {
            println!("Downloading {} from {}", info.name, info.url);
            download_file(&Url::parse(&info.url)?, &archive_path).await?;
            verify_checksum(info, &archive_path).await?;
            extracted_files = extract_package(&archive_path, &install_path).await?;
            fs::remove_file(&archive_path).await?;
        }

        if let Some(script_name) = &info.post_install {
            let script_path = install_path.join(script_name);
            if script_path.exists() {
                run_script(&script_path, &info.name, |pkg, _, msg| FluxError::PostInstallScriptFailed { package_name: pkg, message: msg })?;
            }
        }

        if let Some(hooks) = &ctx.config.hooks {
            for (pattern, hook_script) in hooks {
                if info.name.starts_with(&pattern.replace('*', "")) {
                    let full_hook_path = ctx.target_root.join(hook_script.strip_prefix('/').unwrap_or(hook_script));
                    run_script(&full_hook_path, &info.name, |pkg, hook, msg| FluxError::HookFailed { package_name: pkg, hook_script: hook, message: msg })?;
                }
            }
        }

        let reason = if info.name == package_name {
            InstallReason::Explicit
        } else {
            InstallReason::Dependency
        };
        new_install_records.push(InstalledPackageInfo {
            name: info.name.clone(),
            version: info.version.clone(),
            package_type: info.package_type.clone(),
            install_reason: reason,
            files: extracted_files,
        });
    }

    let mut all_installed = installed_packages;
    all_installed.extend(new_install_records);

    ctx.write_installed_packages(&all_installed).await?;
    println!("Package database updated.");
    Ok(())
}

fn resolve_dependencies<'a>(pkg_name: &'a str, ctx: &'a AppContext, resolved: &mut HashSet<String>) -> Result<(), FluxError> {
    if resolved.contains(pkg_name) { return Ok(()); }
    let info = ctx.package_index.get(pkg_name).ok_or_else(|| FluxError::PackageNotFound(pkg_name.to_string()))?;
    if let Some(deps) = &info.dependencies {
        for dep in deps { resolve_dependencies(dep, ctx, resolved)?; }
    }
    resolved.insert(pkg_name.to_string());
    Ok(())
}

async fn handle_remove(package_name: &str, ctx: &AppContext) -> Result<(), FluxError> {
    let mut installed = ctx.get_installed_packages().await?;

    let mut dependents = Vec::new();
    for pkg in &installed {
        if pkg.name == package_name { continue; }
        if let Some(info) = ctx.package_index.get(&pkg.name) {
            if let Some(deps) = &info.dependencies {
                if deps.contains(&package_name.to_string()) {
                    dependents.push(pkg.name.clone());
                }
            }
        }
    }

    if !dependents.is_empty() {
        return Err(FluxError::DependencyInUse { package_name: package_name.to_string(), dependents });
    }

    if let Some(index) = installed.iter().position(|p| p.name == package_name) {
        let pkg_to_remove = installed.remove(index);

        println!("Removing package: {}", pkg_to_remove.name);
        if pkg_to_remove.package_type == PackageType::App {
            let info_from_repo = ctx.package_index.get(&pkg_to_remove.name).ok_or_else(|| FluxError::PackageNotFound(pkg_to_remove.name.clone()))?;
            let install_path = ctx.get_install_path(info_from_repo);
            if install_path.exists() {
                fs::remove_dir_all(&install_path).await?;
                println!("Removed directory: {}", install_path.display());
            }
        } else { // System package
            println!("Removing files for system package {}...", pkg_to_remove.name);
            for file_path in pkg_to_remove.files.iter().rev() {
                let full_path = ctx.target_root.join(file_path);
                if full_path.exists() {
                    if full_path.is_dir() {
                        if fs::read_dir(&full_path).await?.next_entry().await?.is_none() {
                            println!("Removing empty directory: {}", full_path.display());
                            fs::remove_dir(&full_path).await?;
                        }
                    } else {
                        println!("Removing file: {}", full_path.display());
                        fs::remove_file(&full_path).await?;
                    }
                }
            }
        }

        ctx.write_installed_packages(&installed).await?;
        println!("Successfully removed '{}'.", pkg_to_remove.name);
    } else {
        return Err(FluxError::PackageNotFound(format!("{} (not installed)", package_name)));
    }

    Ok(())
}

async fn handle_list(ctx: &AppContext) -> Result<(), FluxError> {
    println!("Listing installed packages...");
    let installed = ctx.get_installed_packages().await?;

    if installed.is_empty() {
        println!("No packages are currently installed.");
        return Ok(());
    }

    for pkg in installed {
        println!("- {} (version: {}, type: {:?}, reason: {:?})", pkg.name, pkg.version, pkg.package_type, pkg.install_reason);
    }
    Ok(())
}

async fn handle_update(ctx: &mut AppContext) -> Result<(), FluxError> {
    println!("Updating repository index from {}...", ctx.config.repository_url);

    let url = if ctx.config.repository_url.starts_with("file://./") {
        let current_dir = std::env::current_dir()?;
        let file_path = ctx.config.repository_url.strip_prefix("file://./").unwrap();
        Url::from_file_path(current_dir.join(file_path)).map_err(|_| FluxError::Config("Could not create absolute file URL".to_string()))?
    } else {
        Url::parse(&ctx.config.repository_url)?
    };

    download_file(&url, &ctx.host_cache_path).await?;
    println!("Repository index updated successfully.");

    let index_content = fs::read_to_string(&ctx.host_cache_path).await?;
    let index: PackageIndex = serde_yaml::from_str(&index_content)?;
    ctx.package_index = index.packages.into_iter().map(|p| (p.name.clone(), p)).collect();

    Ok(())
}

async fn handle_upgrade(ctx: &AppContext) -> Result<(), FluxError> {
    let installed = ctx.get_installed_packages().await?;
    let mut packages_to_update = Vec::new();

    for pkg in &installed {
        if let Some(repo_pkg) = ctx.package_index.get(&pkg.name) {
            if repo_pkg.version != pkg.version {
                println!("- {} (Installed: {}, Available: {})", pkg.name, pkg.version, repo_pkg.version);
                packages_to_update.push(pkg.name.clone());
            }
        }
    }

    if packages_to_update.is_empty() {
        println!("All packages are up to date.");
        return Ok(());
    }

    println!("\nStarting upgrade...");
    for package_name in packages_to_update {
        println!("\nUpgrading {}...", package_name);
        handle_remove(&package_name, ctx).await?;
        handle_install(&package_name, ctx).await?;
    }

    println!("\nUpgrade complete.");
    Ok(())
}

async fn handle_autoremove(ctx: &AppContext) -> Result<(), FluxError> {
    println!("Checking for unused dependencies...");
    let installed = ctx.get_installed_packages().await?;
    let mut required_deps = HashSet::new();

    for pkg in &installed {
        if let Some(info) = ctx.package_index.get(&pkg.name) {
            if let Some(deps) = &info.dependencies {
                for dep in deps {
                    required_deps.insert(dep.clone());
                }
            }
        }
    }

    let mut orphans_to_remove = Vec::new();
    for pkg in &installed {
        if pkg.install_reason == InstallReason::Dependency && !required_deps.contains(&pkg.name) {
            orphans_to_remove.push(pkg.name.clone());
        }
    }

    if orphans_to_remove.is_empty() {
        println!("No unused dependencies to remove.");
        return Ok(());
    }

    println!("\nThe following packages are no longer required and will be removed:");
    for orphan in &orphans_to_remove {
        println!("- {}", orphan);
    }

    println!("\nRemoving unused dependencies...");
    for package_name in orphans_to_remove {
        handle_remove(&package_name, ctx).await?;
    }

    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let mut ctx = AppContext::new(cli.root).await?;

    let result = match cli.command {
        Commands::Install { package } => handle_install(&package, &ctx).await,
        Commands::Remove { package } => handle_remove(&package, &ctx).await,
        Commands::List => handle_list(&ctx).await,
        Commands::Update => handle_update(&mut ctx).await,
        Commands::Upgrade => handle_upgrade(&ctx).await,
        Commands::Autoremove => handle_autoremove(&ctx).await,
    };

    if let Err(e) = result {
        eprintln!("Error: {}", e);
        process::exit(1);
    }

    Ok(())
}
