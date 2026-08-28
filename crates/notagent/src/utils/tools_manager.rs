use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::config::{APP_NAME, get_bin_dir};
use crate::utils::management_http::{FetchRetryOptions, fetch_with_retry};

const NETWORK_TIMEOUT: Duration = Duration::from_secs(10);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagedTool {
    Fd,
    Rg,
}

impl ManagedTool {
    /// Display name, as used in the console messages.
    pub fn name(self) -> &'static str {
        match self {
            ManagedTool::Fd => "fd",
            ManagedTool::Rg => "ripgrep",
        }
    }

    fn repo(self) -> &'static str {
        match self {
            ManagedTool::Fd => "sharkdp/fd",
            ManagedTool::Rg => "BurntSushi/ripgrep",
        }
    }

    /// The binary name inside the archive.
    pub fn binary_name(self) -> &'static str {
        match self {
            ManagedTool::Fd => "fd",
            ManagedTool::Rg => "rg",
        }
    }

    /// System command names to try before downloading.
    fn system_binary_names(self) -> &'static [&'static str] {
        match self {
            ManagedTool::Fd => &["fd", "fdfind"],
            ManagedTool::Rg => &["rg"],
        }
    }

    fn tag_prefix(self) -> &'static str {
        match self {
            ManagedTool::Fd => "v",
            ManagedTool::Rg => "",
        }
    }

    fn termux_package(self) -> &'static str {
        match self {
            ManagedTool::Fd => "fd",
            ManagedTool::Rg => "ripgrep",
        }
    }

    fn asset_name(self, version: &str, platform: &str, architecture: &str) -> Option<String> {
        let arch_str = if architecture == "arm64" {
            "aarch64"
        } else {
            "x86_64"
        };
        match (self, platform) {
            (ManagedTool::Fd, "darwin") => {
                Some(format!("fd-v{version}-{arch_str}-apple-darwin.tar.gz"))
            }
            (ManagedTool::Fd, "linux") => {
                Some(format!("fd-v{version}-{arch_str}-unknown-linux-gnu.tar.gz"))
            }
            (ManagedTool::Fd, "win32") => {
                Some(format!("fd-v{version}-{arch_str}-pc-windows-msvc.zip"))
            }
            (ManagedTool::Rg, "darwin") => {
                Some(format!("ripgrep-{version}-{arch_str}-apple-darwin.tar.gz"))
            }
            (ManagedTool::Rg, "linux") if architecture == "arm64" => Some(format!(
                "ripgrep-{version}-aarch64-unknown-linux-gnu.tar.gz"
            )),
            (ManagedTool::Rg, "linux") => Some(format!(
                "ripgrep-{version}-x86_64-unknown-linux-musl.tar.gz"
            )),
            (ManagedTool::Rg, "win32") => {
                Some(format!("ripgrep-{version}-{arch_str}-pc-windows-msvc.zip"))
            }
            _ => None,
        }
    }
}

fn is_offline_mode_enabled() -> bool {
    let Ok(value) = std::env::var("NOTAGENT_OFFLINE") else {
        return false;
    };
    if value.is_empty() {
        return false;
    }
    value == "1" || value.eq_ignore_ascii_case("true") || value.eq_ignore_ascii_case("yes")
}

/// `process.platform`
fn platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "windows") {
        "win32"
    } else if cfg!(target_os = "android") {
        "android"
    } else {
        "linux"
    }
}

/// `os.arch()`
fn architecture() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "x64"
    }
}

/// True when running the command succeeds at all; ENOENT means "missing".
fn command_exists(command: &str) -> bool {
    std::process::Command::new(command)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
}

/// The path of a tool, either in the managed bin directory or on PATH.
pub fn get_tool_path(tool: ManagedTool) -> Option<String> {
    let extension = if platform() == "win32" { ".exe" } else { "" };
    let local_path = get_bin_dir().join(format!("{}{extension}", tool.binary_name()));
    if local_path.exists() {
        return Some(local_path.to_string_lossy().into_owned());
    }
    // A system binary is returned by name, because it is on PATH.
    tool.system_binary_names()
        .iter()
        .find(|name| command_exists(name))
        .map(|name| (*name).to_owned())
}

async fn get_latest_version(repo: &str) -> Result<String, String> {
    let client = reqwest::Client::new();
    let url = format!("https://api.github.com/repos/{repo}/releases/latest");
    let response = fetch_with_retry(
        &client,
        || {
            client
                .get(&url)
                .header("User-Agent", format!("{APP_NAME}-coding-agent"))
        },
        FetchRetryOptions {
            timeout: Some(NETWORK_TIMEOUT),
            ..FetchRetryOptions::default()
        },
    )
    .await?;
    if !response.status().is_success() {
        return Err(format!("GitHub API error: {}", response.status().as_u16()));
    }
    let body: serde_json::Value = response.json().await.map_err(|error| error.to_string())?;
    let tag_name = body
        .get("tag_name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    Ok(tag_name.strip_prefix('v').unwrap_or(tag_name).to_owned())
}

async fn download_file(url: &str, destination: &Path) -> Result<(), String> {
    let client = reqwest::Client::new();
    let response = fetch_with_retry(
        &client,
        || client.get(url),
        FetchRetryOptions {
            timeout: Some(DOWNLOAD_TIMEOUT),
            ..FetchRetryOptions::default()
        },
    )
    .await?;
    if !response.status().is_success() {
        return Err(format!(
            "Failed to download: {}",
            response.status().as_u16()
        ));
    }
    let bytes = response.bytes().await.map_err(|error| error.to_string())?;
    tokio::fs::write(destination, &bytes)
        .await
        .map_err(|error| error.to_string())
}

fn find_binary_recursively(root: &Path, binary_file_name: &str) -> Option<PathBuf> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_file() && entry.file_name() == binary_file_name {
                return Some(path);
            }
            if file_type.is_dir() {
                stack.push(path);
            }
        }
    }
    None
}

fn run_extraction_command(command: &str, args: &[&str]) -> Option<String> {
    match std::process::Command::new(command).args(args).output() {
        Ok(output) if output.status.success() => None,
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            let reason = if !stderr.is_empty() {
                stderr
            } else if !stdout.is_empty() {
                stdout
            } else {
                match output.status.code() {
                    Some(code) => format!("exit status {code}"),
                    None => "exit status unknown".to_owned(),
                }
            };
            Some(format!("{command}: {reason}"))
        }
        Err(error) => Some(format!("{command}: {error}")),
    }
}

fn extract_archive(
    archive_path: &Path,
    extract_dir: &Path,
    asset_name: &str,
) -> Result<(), String> {
    let archive = archive_path.to_string_lossy().into_owned();
    let destination = extract_dir.to_string_lossy().into_owned();
    if asset_name.ends_with(".tar.gz") {
        return match run_extraction_command("tar", &["xzf", &archive, "-C", &destination]) {
            None => Ok(()),
            Some(failure) => Err(format!("Failed to extract {asset_name}: {failure}")),
        };
    }
    if !asset_name.ends_with(".zip") {
        return Err(format!("Unsupported archive format: {asset_name}"));
    }

    let mut failures: Vec<String> = Vec::new();
    #[cfg(windows)]
    {
        // Windows ships bsdtar as tar.exe, which handles zip; prefer the System32
        // binary over Git Bash's GNU tar, which does not.
        let system_tar = std::env::var("SystemRoot")
            .or_else(|_| std::env::var("WINDIR"))
            .ok()
            .map(|root| Path::new(&root).join("System32").join("tar.exe"))
            .filter(|path| path.exists())
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_else(|| "tar.exe".to_owned());
        match run_extraction_command(&system_tar, &["xf", &archive, "-C", &destination]) {
            None => return Ok(()),
            Some(failure) => failures.push(failure),
        }
        let script = "& { param($archive, $destination) $ErrorActionPreference = 'Stop'; Expand-Archive -LiteralPath $archive -DestinationPath $destination -Force }";
        match run_extraction_command(
            "powershell.exe",
            &[
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                script,
                &archive,
                &destination,
            ],
        ) {
            None => return Ok(()),
            Some(failure) => failures.push(failure),
        }
    }
    #[cfg(not(windows))]
    {
        match run_extraction_command("unzip", &["-q", &archive, "-d", &destination]) {
            None => return Ok(()),
            Some(failure) => failures.push(failure),
        }
        match run_extraction_command("tar", &["xf", &archive, "-C", &destination]) {
            None => return Ok(()),
            Some(failure) => failures.push(failure),
        }
    }
    Err(format!(
        "Failed to extract {asset_name}: {}",
        failures.join("; ")
    ))
}

async fn download_tool(tool: ManagedTool) -> Result<String, String> {
    let platform = platform();
    let architecture = architecture();

    let mut version = get_latest_version(tool.repo()).await?;
    if tool == ManagedTool::Fd && platform == "darwin" && architecture == "x64" {
        version = "10.3.0".to_owned();
    }
    let Some(asset_name) = tool.asset_name(&version, platform, architecture) else {
        return Err(format!("Unsupported platform: {platform}/{architecture}"));
    };

    let tools_dir = get_bin_dir();
    std::fs::create_dir_all(&tools_dir).map_err(|error| error.to_string())?;
    let download_url = format!(
        "https://github.com/{}/releases/download/{}{version}/{asset_name}",
        tool.repo(),
        tool.tag_prefix()
    );
    let archive_path = tools_dir.join(&asset_name);
    let binary_extension = if platform == "win32" { ".exe" } else { "" };
    let binary_file_name = format!("{}{binary_extension}", tool.binary_name());
    let binary_path = tools_dir.join(&binary_file_name);

    download_file(&download_url, &archive_path).await?;

    // fd and rg can download concurrently at startup, so the extraction directory
    // must be unique per run.
    let extract_dir = tools_dir.join(format!(
        "extract_tmp_{}_{}_{}",
        tool.binary_name(),
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&extract_dir).map_err(|error| error.to_string())?;

    let result = (|| -> Result<(), String> {
        extract_archive(&archive_path, &extract_dir, &asset_name)?;

        // Some archives keep the binary at the root, others below a versioned directory.
        let stripped = asset_name
            .strip_suffix(".tar.gz")
            .or_else(|| asset_name.strip_suffix(".zip"))
            .unwrap_or(&asset_name);
        let candidates = [
            extract_dir.join(stripped).join(&binary_file_name),
            extract_dir.join(&binary_file_name),
        ];
        let extracted = candidates
            .into_iter()
            .find(|candidate| candidate.exists())
            .or_else(|| find_binary_recursively(&extract_dir, &binary_file_name));
        let Some(extracted) = extracted else {
            return Err(format!(
                "Binary not found in archive: expected {binary_file_name} under {}",
                extract_dir.to_string_lossy()
            ));
        };
        std::fs::rename(&extracted, &binary_path).map_err(|error| error.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&binary_path, std::fs::Permissions::from_mode(0o755))
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    })();

    let _ = std::fs::remove_file(&archive_path);
    let _ = std::fs::remove_dir_all(&extract_dir);
    result?;
    Ok(binary_path.to_string_lossy().into_owned())
}

/// Ensure a tool is available, downloading it if needed.
pub async fn ensure_tool(tool: ManagedTool, silent: bool) -> Option<String> {
    if let Some(existing_path) = get_tool_path(tool) {
        return Some(existing_path);
    }
    if is_offline_mode_enabled() {
        if !silent {
            println!(
                "{} not found. Offline mode enabled, skipping download.",
                tool.name()
            );
        }
        return None;
    }
    // Linux binaries do not run on Android/Termux (Bionic libc), so the user has
    // to install the package.
    if platform() == "android" {
        if !silent {
            println!(
                "{} not found. Install with: pkg install {}",
                tool.name(),
                tool.termux_package()
            );
        }
        return None;
    }
    if !silent {
        println!("{} not found. Downloading...", tool.name());
    }
    match download_tool(tool).await {
        Ok(path) => {
            if !silent {
                println!("{} installed to {path}", tool.name());
            }
            Some(path)
        }
        Err(error) => {
            if !silent {
                println!("Failed to download {}: {error}", tool.name());
            }
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_the_release_asset_names() {
        assert_eq!(
            ManagedTool::Fd
                .asset_name("10.3.0", "darwin", "arm64")
                .as_deref(),
            Some("fd-v10.3.0-aarch64-apple-darwin.tar.gz")
        );
        assert_eq!(
            ManagedTool::Fd
                .asset_name("10.3.0", "linux", "x64")
                .as_deref(),
            Some("fd-v10.3.0-x86_64-unknown-linux-gnu.tar.gz")
        );
        assert_eq!(
            ManagedTool::Fd
                .asset_name("10.3.0", "win32", "x64")
                .as_deref(),
            Some("fd-v10.3.0-x86_64-pc-windows-msvc.zip")
        );
        // ripgrep uses musl on x86 Linux and gnu on arm64.
        assert_eq!(
            ManagedTool::Rg
                .asset_name("14.1.1", "linux", "x64")
                .as_deref(),
            Some("ripgrep-14.1.1-x86_64-unknown-linux-musl.tar.gz")
        );
        assert_eq!(
            ManagedTool::Rg
                .asset_name("14.1.1", "linux", "arm64")
                .as_deref(),
            Some("ripgrep-14.1.1-aarch64-unknown-linux-gnu.tar.gz")
        );
        assert_eq!(ManagedTool::Rg.asset_name("14.1.1", "freebsd", "x64"), None);
    }

    #[test]
    fn reports_the_tool_metadata() {
        assert_eq!(ManagedTool::Fd.name(), "fd");
        assert_eq!(ManagedTool::Fd.binary_name(), "fd");
        assert_eq!(ManagedTool::Fd.system_binary_names(), &["fd", "fdfind"]);
        assert_eq!(ManagedTool::Fd.tag_prefix(), "v");
        assert_eq!(ManagedTool::Rg.name(), "ripgrep");
        assert_eq!(ManagedTool::Rg.binary_name(), "rg");
        assert_eq!(ManagedTool::Rg.tag_prefix(), "");
        assert_eq!(ManagedTool::Rg.termux_package(), "ripgrep");
    }

    #[test]
    fn finds_a_binary_below_a_versioned_directory() {
        let directory = tempfile::Builder::new()
            .prefix("notagent-tools-manager-")
            .tempdir()
            .expect("temp dir");
        let nested = directory.path().join("ripgrep-14.1.1-x86_64").join("bin");
        std::fs::create_dir_all(&nested).expect("dirs");
        std::fs::write(nested.join("rg"), "x").expect("binary");
        let found = find_binary_recursively(directory.path(), "rg").expect("found");
        assert_eq!(found, nested.join("rg"));
        assert_eq!(find_binary_recursively(directory.path(), "fd"), None);
    }

    #[test]
    fn offline_mode_is_read_from_the_environment() {
        // SAFETY: the variable is restored before the test returns.
        let original = std::env::var("NOTAGENT_OFFLINE").ok();
        for (value, expected) in [
            ("1", true),
            ("true", true),
            ("YES", true),
            ("0", false),
            ("", false),
        ] {
            unsafe { std::env::set_var("NOTAGENT_OFFLINE", value) };
            assert_eq!(is_offline_mode_enabled(), expected, "{value}");
        }
        unsafe {
            match original {
                Some(value) => std::env::set_var("NOTAGENT_OFFLINE", value),
                None => std::env::remove_var("NOTAGENT_OFFLINE"),
            }
        }
    }

    #[tokio::test]
    async fn offline_mode_skips_the_download() {
        let original = std::env::var("NOTAGENT_OFFLINE").ok();
        unsafe { std::env::set_var("NOTAGENT_OFFLINE", "1") };
        // Only meaningful when the tool is not installed on this machine.
        if get_tool_path(ManagedTool::Fd).is_none() {
            assert_eq!(ensure_tool(ManagedTool::Fd, true).await, None);
        }
        unsafe {
            match original {
                Some(value) => std::env::set_var("NOTAGENT_OFFLINE", value),
                None => std::env::remove_var("NOTAGENT_OFFLINE"),
            }
        }
    }
}
