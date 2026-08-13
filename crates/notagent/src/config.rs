//! Port of `packages/coding-agent/src/config.ts`.
//!
//! Deviation class 4 (distribution mechanics): the TS build ships as an npm
//! package and a Bun single binary and reads `package.json` at runtime; the
//! Rust build is a single binary and takes its identity from Cargo. The install
//! detection and the self-update command builder keep their shape because a
//! Rust binary can still be installed through a package manager wrapper.
//!
//! Deviation class 1: the detection functions read their inputs from an
//! explicit [`InstallEnv`] instead of `process.execPath`/`process.argv`, which
//! makes them testable without mutating global process state.

use std::path::{Path, PathBuf};

// =============================================================================
// App config (package.json notagentConfig → Cargo metadata)
// =============================================================================

pub const PACKAGE_NAME: &str = "@notagent/coding-agent";
pub const APP_NAME: &str = "notagent";
pub const APP_TITLE: &str = APP_NAME;
pub const CONFIG_DIR_NAME: &str = ".notagent";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// e.g. `NOTAGENT_CODING_AGENT_DIR`.
pub fn env_agent_dir() -> String {
    format!("{}_CODING_AGENT_DIR", APP_NAME.to_uppercase())
}

/// e.g. `NOTAGENT_CODING_AGENT_SESSION_DIR`.
pub fn env_session_dir() -> String {
    format!("{}_CODING_AGENT_SESSION_DIR", APP_NAME.to_uppercase())
}

const DEFAULT_SHARE_VIEWER_URL: &str = "https://notagent.dev/session/";

/// Get the share viewer URL for a gist ID.
pub fn get_share_viewer_url(gist_id: &str) -> String {
    let base_url = std::env::var("NOTAGENT_SHARE_VIEWER_URL")
        .unwrap_or_else(|_| DEFAULT_SHARE_VIEWER_URL.to_owned());
    format!("{base_url}#{gist_id}")
}

/// Port of `expandTildePath`/`normalizePath`: expands a leading `~`.
pub fn expand_tilde_path(path: &str) -> PathBuf {
    if path == "~" {
        return home_dir();
    }
    if let Some(rest) = path.strip_prefix("~/") {
        return home_dir().join(rest);
    }
    PathBuf::from(path)
}

fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

// =============================================================================
// User config paths (~/.notagent/agent/*)
// =============================================================================

/// Get the agent config directory (e.g. `~/.notagent/agent/`).
pub fn get_agent_dir() -> PathBuf {
    match std::env::var(env_agent_dir()) {
        Ok(dir) if !dir.is_empty() => expand_tilde_path(&dir),
        _ => home_dir().join(CONFIG_DIR_NAME).join("agent"),
    }
}

/// Get path to user's custom themes directory.
pub fn get_custom_themes_dir() -> PathBuf {
    get_agent_dir().join("themes")
}

/// Get path to models.json.
pub fn get_models_path() -> PathBuf {
    get_agent_dir().join("models.json")
}

/// Get path to auth.json.
pub fn get_auth_path() -> PathBuf {
    get_agent_dir().join("auth.json")
}

/// Get path to settings.json.
pub fn get_settings_path() -> PathBuf {
    get_agent_dir().join("settings.json")
}

/// Get path to tools directory.
pub fn get_tools_dir() -> PathBuf {
    get_agent_dir().join("tools")
}

/// Get path to managed binaries directory (fd, rg).
pub fn get_bin_dir() -> PathBuf {
    get_agent_dir().join("bin")
}

/// Get path to prompt templates directory.
pub fn get_prompts_dir() -> PathBuf {
    get_agent_dir().join("prompts")
}

/// Get path to sessions directory.
pub fn get_sessions_dir() -> PathBuf {
    get_agent_dir().join("sessions")
}

/// Get path to the background-task directory for one session.
///
/// Beside the sessions rather than inside them: a session is a single file, and
/// a task owns a record plus a growing log, which needs a directory of its own.
pub fn get_session_tasks_dir(session_id: &str) -> PathBuf {
    get_agent_dir().join("tasks").join(session_id)
}

/// Get path to debug log file.
pub fn get_debug_log_path() -> PathBuf {
    get_agent_dir().join(format!("{APP_NAME}-debug.log"))
}

// =============================================================================
// Package asset paths (shipped with the executable)
// =============================================================================

/// Get the base directory for resolving package assets.
///
/// The Rust binary embeds its assets, so this is the executable's directory
/// unless `NOTAGENT_PACKAGE_DIR` overrides it (kept for Nix/Guix, where store
/// paths tokenize poorly).
pub fn get_package_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("NOTAGENT_PACKAGE_DIR")
        && !dir.is_empty()
    {
        return expand_tilde_path(&dir);
    }
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn get_themes_dir() -> PathBuf {
    get_package_dir().join("theme")
}

pub fn get_export_template_dir() -> PathBuf {
    get_package_dir().join("export-html")
}

pub fn get_readme_path() -> PathBuf {
    get_package_dir().join("README.md")
}

pub fn get_docs_path() -> PathBuf {
    get_package_dir().join("docs")
}

pub fn get_examples_path() -> PathBuf {
    get_package_dir().join("examples")
}

pub fn get_changelog_path() -> PathBuf {
    get_package_dir().join("CHANGELOG.md")
}

pub fn get_interactive_assets_dir() -> PathBuf {
    get_package_dir().join("assets")
}

pub fn get_bundled_interactive_asset_path(name: &str) -> PathBuf {
    get_interactive_assets_dir().join(name)
}

// =============================================================================
// Install method detection
// =============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InstallMethod {
    /// TS: `bun-binary` — a standalone executable, updated from the releases page.
    Binary,
    Npm,
    Pnpm,
    Yarn,
    Bun,
    Unknown,
}

impl InstallMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Binary => "binary",
            Self::Npm => "npm",
            Self::Pnpm => "pnpm",
            Self::Yarn => "yarn",
            Self::Bun => "bun",
            Self::Unknown => "unknown",
        }
    }
}

/// The process facts the detection reads (TS: `__dirname`, `process.execPath`,
/// `process.argv[1]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallEnv {
    pub package_dir: PathBuf,
    pub exec_path: PathBuf,
    pub entrypoint: Option<PathBuf>,
    /// True when the executable is a standalone binary (TS: `isBunBinary`).
    pub standalone_binary: bool,
}

impl InstallEnv {
    pub fn current() -> Self {
        let exec_path = std::env::current_exe().unwrap_or_default();
        Self {
            package_dir: get_package_dir(),
            exec_path: exec_path.clone(),
            entrypoint: Some(exec_path),
            // A Rust build is always a standalone binary unless it sits inside a
            // package manager root, which `detect_install_method` checks first.
            standalone_binary: true,
        }
    }
}

fn lowercase_slashes(path: &Path) -> String {
    path.to_string_lossy().to_lowercase().replace('\\', "/")
}

pub fn detect_install_method(env: &InstallEnv) -> InstallMethod {
    let resolved = format!(
        "{}\0{}",
        lowercase_slashes(&env.package_dir),
        lowercase_slashes(&env.exec_path)
    );
    if resolved.contains("/pnpm/") || resolved.contains("/.pnpm/") {
        return InstallMethod::Pnpm;
    }
    if resolved.contains("/yarn/") || resolved.contains("/.yarn/") {
        return InstallMethod::Yarn;
    }
    if resolved.contains("/install/global/node_modules/") {
        return InstallMethod::Bun;
    }
    if resolved.contains("/npm/") || resolved.contains("/node_modules/") {
        return InstallMethod::Npm;
    }
    if env.standalone_binary {
        return InstallMethod::Binary;
    }
    InstallMethod::Unknown
}

// =============================================================================
// Self-update commands
// =============================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfUpdateCommandStep {
    pub command: String,
    pub args: Vec<String>,
    pub display: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfUpdateCommand {
    pub command: String,
    pub args: Vec<String>,
    pub display: String,
    pub steps: Option<Vec<SelfUpdateCommandStep>>,
}

/// `SelfUpdatePackageTarget = string | { packageName, installSpec? }`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfUpdatePackageTarget {
    pub package_name: String,
    pub install_spec: String,
}

impl SelfUpdatePackageTarget {
    pub fn new(package_name: impl Into<String>) -> Self {
        let package_name = package_name.into();
        Self {
            install_spec: package_name.clone(),
            package_name,
        }
    }

    pub fn with_spec(package_name: impl Into<String>, install_spec: impl Into<String>) -> Self {
        Self {
            package_name: package_name.into(),
            install_spec: install_spec.into(),
        }
    }
}

fn make_step(command: &str, args: &[&str]) -> SelfUpdateCommandStep {
    let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
    let display = std::iter::once(command.to_owned())
        .chain(args.iter().cloned())
        .map(|arg| {
            if arg.chars().any(char::is_whitespace) {
                format!("\"{arg}\"")
            } else {
                arg
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    SelfUpdateCommandStep {
        command: command.to_owned(),
        args,
        display,
    }
}

fn make_command(
    install: SelfUpdateCommandStep,
    uninstall: Option<SelfUpdateCommandStep>,
) -> SelfUpdateCommand {
    match uninstall {
        None => SelfUpdateCommand {
            command: install.command.clone(),
            args: install.args.clone(),
            display: install.display,
            steps: None,
        },
        Some(uninstall) => SelfUpdateCommand {
            command: install.command.clone(),
            args: install.args.clone(),
            display: format!("{} && {}", uninstall.display, install.display),
            steps: Some(vec![uninstall, install]),
        },
    }
}

pub fn get_self_update_command_for_method(
    method: InstallMethod,
    installed_package_name: &str,
    target: &SelfUpdatePackageTarget,
    npm_command: Option<&[String]>,
) -> Option<SelfUpdateCommand> {
    let uninstall_needed = target.package_name != installed_package_name;
    match method {
        InstallMethod::Binary | InstallMethod::Unknown => None,
        InstallMethod::Pnpm => Some(make_command(
            make_step(
                "pnpm",
                &[
                    "install",
                    "-g",
                    "--ignore-scripts",
                    "--config.minimumReleaseAge=0",
                    &target.install_spec,
                ],
            ),
            uninstall_needed.then(|| make_step("pnpm", &["remove", "-g", installed_package_name])),
        )),
        InstallMethod::Yarn => Some(make_command(
            make_step(
                "yarn",
                &["global", "add", "--ignore-scripts", &target.install_spec],
            ),
            uninstall_needed
                .then(|| make_step("yarn", &["global", "remove", installed_package_name])),
        )),
        InstallMethod::Bun => Some(make_command(
            make_step(
                "bun",
                &[
                    "install",
                    "-g",
                    "--ignore-scripts",
                    "--minimum-release-age=0",
                    &target.install_spec,
                ],
            ),
            uninstall_needed
                .then(|| make_step("bun", &["uninstall", "-g", installed_package_name])),
        )),
        InstallMethod::Npm => {
            let (command, npm_args): (String, Vec<String>) = match npm_command {
                Some([first, rest @ ..]) => (first.clone(), rest.to_vec()),
                _ => ("npm".to_owned(), Vec::new()),
            };
            let mut install_args: Vec<&str> = npm_args.iter().map(String::as_str).collect();
            install_args.extend([
                "install",
                "-g",
                "--ignore-scripts",
                "--min-release-age=0",
                &target.install_spec,
            ]);
            let install = make_step(&command, &install_args);
            let uninstall = uninstall_needed.then(|| {
                let mut args: Vec<&str> = npm_args.iter().map(String::as_str).collect();
                args.extend(["uninstall", "-g", installed_package_name]);
                make_step(&command, &args)
            });
            Some(make_command(install, uninstall))
        }
    }
}

/// TS additionally verifies that a global package manager manages the install
/// path and that the path is writable. The Rust binary carries no package
/// directory to verify, so only the writability check remains.
pub fn get_self_update_command(
    env: &InstallEnv,
    package_name: &str,
    npm_command: Option<&[String]>,
    target: &SelfUpdatePackageTarget,
) -> Option<SelfUpdateCommand> {
    let method = detect_install_method(env);
    let command = get_self_update_command_for_method(method, package_name, target, npm_command)?;
    if !is_self_update_path_writable(env) {
        return None;
    }
    Some(command)
}

fn is_self_update_path_writable(env: &InstallEnv) -> bool {
    let writable = |path: &Path| {
        std::fs::metadata(path)
            .map(|metadata| !metadata.permissions().readonly())
            .unwrap_or(false)
    };
    writable(&env.package_dir) && env.package_dir.parent().is_some_and(writable)
}

pub fn get_self_update_unavailable_instruction(
    env: &InstallEnv,
    package_name: &str,
    npm_command: Option<&[String]>,
    target: &SelfUpdatePackageTarget,
) -> String {
    let method = detect_install_method(env);
    if method == InstallMethod::Binary {
        return "Download from: https://github.com/notagentdev/notagent/releases/latest".to_owned();
    }
    match get_self_update_command_for_method(method, package_name, target, npm_command) {
        Some(command) => {
            if !is_self_update_path_writable(env) {
                return format!(
                    "This installation is managed by a global {} install, but the install path is not writable. Update it yourself with: {}",
                    method.as_str(),
                    command.display
                );
            }
            format!(
                "This installation is not managed by a global {} install. Update it with the package manager, wrapper, or source checkout that provides it.",
                method.as_str()
            )
        }
        None => format!(
            "Update {} using the package manager, wrapper, or source checkout that provides this installation.",
            target.install_spec
        ),
    }
}

pub fn get_update_instruction(env: &InstallEnv, package_name: &str) -> String {
    let method = detect_install_method(env);
    let target = SelfUpdatePackageTarget::new(package_name);
    match get_self_update_command_for_method(method, package_name, &target, None) {
        Some(command) => format!("Run: {}", command.display),
        None => get_self_update_unavailable_instruction(env, package_name, None, &target),
    }
}
