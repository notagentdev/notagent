use std::path::{Path, PathBuf};

// =============================================================================
// App config (package.json notagentConfig → Cargo metadata)
// =============================================================================

pub const PACKAGE_NAME: &str = "@notagent/coding-agent";
pub const APP_NAME: &str = "notagent";
pub const APP_TITLE: &str = APP_NAME;
pub const CONFIG_DIR_NAME: &str = ".notagent";
/// User-level state root below the home directory.
pub const USER_CONFIG_DIR_NAME: &str = ".notagent";
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
        _ => home_dir().join(USER_CONFIG_DIR_NAME).join("agent"),
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

/// Get path to the MCP OAuth credential file.
/// Kept apart from auth.json: those are the model providers' credentials, these
/// are third-party servers', and a session that reads one has no business
/// holding the other.
pub fn get_mcp_credentials_path() -> PathBuf {
    get_agent_dir().join("mcp-credentials.json")
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

/// `process.argv[1]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallEnv {
    pub package_dir: PathBuf,
    pub exec_path: PathBuf,
    pub entrypoint: Option<PathBuf>,
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

/// `{ root, prefix }` of an npm install inferred from the package path.
#[derive(Debug, Clone, PartialEq, Eq)]
struct InferredNpmInstall {
    root: PathBuf,
    prefix: PathBuf,
}

/// Node's `path.basename`; `windows` also splits on backslashes.
fn base_name(path: &str, windows: bool) -> &str {
    let separators: &[char] = if windows { &['/', '\\'] } else { &['/'] };
    let trimmed = path.trim_end_matches(separators);
    match trimmed.rfind(separators) {
        Some(index) => &trimmed[index + 1..],
        None => trimmed,
    }
}

/// Node's `path.dirname`; `windows` also splits on backslashes.
fn dir_name(path: &str, windows: bool) -> &str {
    let separators: &[char] = if windows { &['/', '\\'] } else { &['/'] };
    let trimmed = path.trim_end_matches(separators);
    match trimmed.rfind(separators) {
        Some(0) => &trimmed[..1],
        Some(index) => &trimmed[..index],
        None => ".",
    }
}

fn get_inferred_npm_install(package_dir: &Path) -> Option<InferredNpmInstall> {
    let package_dir = package_dir.to_string_lossy();
    let windows = cfg!(windows) || package_dir.contains('\\');
    let parent = dir_name(&package_dir, windows);
    let root = if base_name(parent, windows).starts_with('@')
        && base_name(dir_name(parent, windows), windows) == "node_modules"
    {
        dir_name(parent, windows)
    } else if base_name(parent, windows) == "node_modules" {
        parent
    } else {
        return None;
    };
    let root_parent = dir_name(root, windows);
    // Windows global npm prefixes use `<prefix>\node_modules`, which is
    // indistinguishable from local project installs by path shape alone. Do not
    // infer unsupported Windows custom prefixes without `npm root -g` evidence.
    if base_name(root_parent, windows) != "lib" {
        return None;
    }
    Some(InferredNpmInstall {
        root: PathBuf::from(root),
        prefix: PathBuf::from(dir_name(root_parent, windows)),
    })
}

fn read_command_output(
    command: &str,
    args: &[String],
    require_success: bool,
) -> Result<Option<String>, String> {
    let failure = |reason: String| {
        format!(
            "Failed to run {}: {reason}",
            std::iter::once(command.to_owned())
                .chain(args.iter().cloned())
                .collect::<Vec<_>>()
                .join(" ")
        )
    };
    let output = match std::process::Command::new(command)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
    {
        Ok(output) => output,
        Err(error) => {
            if require_success {
                return Err(failure(error.to_string()));
            }
            return Ok(None);
        }
    };
    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        return Ok((!stdout.is_empty()).then_some(stdout));
    }
    if require_success {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        let reason = if stderr.is_empty() {
            match output.status.code() {
                Some(code) => format!("exit code {code}"),
                None => "exit code unknown".to_owned(),
            }
        } else {
            stderr
        };
        return Err(failure(reason));
    }
    Ok(None)
}

static PNPM_GLOBAL_DIR: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"^(.*[\\/]global[\\/][^\\/]+)[\\/]\.pnpm[\\/]").expect("pnpm global regex")
});

/// The `--config.global-bin-dir` pnpm needs when `pnpm root -g` cannot answer.
fn pnpm_bin_dir_args(package_dir: &Path) -> Vec<String> {
    if read_command_output("pnpm", &["root".to_owned(), "-g".to_owned()], false)
        .ok()
        .flatten()
        .is_some()
    {
        return Vec::new();
    }
    let package_dir = package_dir.to_string_lossy();
    let Some(captures) = PNPM_GLOBAL_DIR.captures(&package_dir) else {
        return Vec::new();
    };
    let global_dir = &captures[1];
    let windows = cfg!(windows) || global_dir.contains('\\');
    let bin_dir = match std::env::var("PNPM_HOME") {
        Ok(home) if !home.is_empty() => home,
        _ => dir_name(dir_name(global_dir, windows), windows).to_owned(),
    };
    vec![format!("--config.global-bin-dir={bin_dir}")]
}

pub fn get_self_update_command_for_method(
    env: &InstallEnv,
    method: InstallMethod,
    installed_package_name: &str,
    target: &SelfUpdatePackageTarget,
    npm_command: Option<&[String]>,
) -> Option<SelfUpdateCommand> {
    let uninstall_needed = target.package_name != installed_package_name;
    match method {
        InstallMethod::Binary | InstallMethod::Unknown => None,
        InstallMethod::Pnpm => {
            let bin_dir_args = pnpm_bin_dir_args(&env.package_dir);
            let mut install_args = vec![
                "install",
                "-g",
                "--ignore-scripts",
                "--config.minimumReleaseAge=0",
            ];
            install_args.extend(bin_dir_args.iter().map(String::as_str));
            install_args.push(&target.install_spec);
            Some(make_command(
                make_step("pnpm", &install_args),
                uninstall_needed.then(|| {
                    let mut args = vec!["remove", "-g"];
                    args.extend(bin_dir_args.iter().map(String::as_str));
                    args.push(installed_package_name);
                    make_step("pnpm", &args)
                }),
            ))
        }
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
            let inferred = match npm_command {
                Some(npm_command) if !npm_command.is_empty() => None,
                _ => get_inferred_npm_install(&env.package_dir),
            };
            let prefix = inferred.map(|inferred| inferred.prefix.to_string_lossy().into_owned());
            let mut prefix_args: Vec<&str> = npm_args.iter().map(String::as_str).collect();
            if let Some(prefix) = &prefix {
                prefix_args.extend(["--prefix", prefix]);
            }
            let mut install_args = prefix_args.clone();
            install_args.extend([
                "install",
                "-g",
                "--ignore-scripts",
                "--min-release-age=0",
                &target.install_spec,
            ]);
            let install = make_step(&command, &install_args);
            let uninstall = uninstall_needed.then(|| {
                let mut args = prefix_args.clone();
                args.extend(["uninstall", "-g", installed_package_name]);
                make_step(&command, &args)
            });
            Some(make_command(install, uninstall))
        }
    }
}

fn get_global_package_roots(
    env: &InstallEnv,
    method: InstallMethod,
    npm_command: Option<&[String]>,
) -> Result<Vec<PathBuf>, String> {
    let bun_roots = |bun_bin: Option<String>| {
        let mut roots = vec![
            home_dir()
                .join(".bun")
                .join("install")
                .join("global")
                .join("node_modules"),
        ];
        if let Some(bun_bin) = bun_bin {
            let windows = cfg!(windows) || bun_bin.contains('\\');
            roots.push(
                PathBuf::from(dir_name(&bun_bin, windows))
                    .join("install")
                    .join("global")
                    .join("node_modules"),
            );
        }
        roots
    };
    match method {
        InstallMethod::Npm => {
            let configured = npm_command.is_some_and(|command| !command.is_empty());
            let (command, npm_args): (String, Vec<String>) = match npm_command {
                Some([first, rest @ ..]) => (first.clone(), rest.to_vec()),
                _ => ("npm".to_owned(), Vec::new()),
            };
            if configured && command == "bun" {
                let mut args = npm_args.clone();
                args.extend(["pm".to_owned(), "bin".to_owned(), "-g".to_owned()]);
                return Ok(bun_roots(read_command_output(&command, &args, true)?));
            }
            let mut args = npm_args;
            args.extend(["root".to_owned(), "-g".to_owned()]);
            let root = read_command_output(&command, &args, configured)?;
            let inferred = if configured {
                None
            } else {
                get_inferred_npm_install(&env.package_dir)
            };
            Ok(root
                .map(PathBuf::from)
                .into_iter()
                .chain(inferred.map(|inferred| inferred.root))
                .collect())
        }
        InstallMethod::Pnpm => {
            if let Some(root) =
                read_command_output("pnpm", &["root".to_owned(), "-g".to_owned()], false)?
            {
                let windows = cfg!(windows) || root.contains('\\');
                return Ok(vec![
                    PathBuf::from(&root),
                    PathBuf::from(dir_name(&root, windows)),
                ]);
            }
            let package_dir = env.package_dir.to_string_lossy().into_owned();
            Ok(PNPM_GLOBAL_DIR
                .captures(&package_dir)
                .map(|captures| vec![PathBuf::from(&captures[1])])
                .unwrap_or_default())
        }
        InstallMethod::Yarn => {
            let dir = read_command_output("yarn", &["global".to_owned(), "dir".to_owned()], false)?;
            Ok(dir
                .map(|dir| {
                    vec![
                        PathBuf::from(&dir),
                        PathBuf::from(&dir).join("node_modules"),
                    ]
                })
                .unwrap_or_default())
        }
        InstallMethod::Bun => Ok(bun_roots(read_command_output(
            "bun",
            &["pm".to_owned(), "bin".to_owned(), "-g".to_owned()],
            false,
        )?)),
        InstallMethod::Binary | InstallMethod::Unknown => Ok(Vec::new()),
    }
}

fn normalize_existing_path_for_comparison(path: &Path, resolve_symlinks: bool) -> Option<String> {
    let resolved = crate::utils::paths::resolve_path_default(
        &path.to_string_lossy(),
        &std::env::current_dir()
            .unwrap_or_default()
            .to_string_lossy(),
    )
    .ok()?;
    if !Path::new(&resolved).exists() {
        return None;
    }
    let normalized = if resolve_symlinks {
        let canonical = crate::utils::paths::canonicalize_path(&resolved);
        if !Path::new(&canonical).exists() {
            return None;
        }
        canonical
    } else {
        resolved
    };
    Some(if cfg!(windows) {
        normalized.to_lowercase()
    } else {
        normalized
    })
}

fn path_comparison_candidates(path: &Path) -> Vec<String> {
    let mut candidates: Vec<String> = Vec::new();
    for resolve_symlinks in [false, true] {
        if let Some(candidate) = normalize_existing_path_for_comparison(path, resolve_symlinks)
            && !candidates.contains(&candidate)
        {
            candidates.push(candidate);
        }
    }
    candidates
}

fn get_entrypoint_package_dir(entrypoint: Option<&Path>) -> Option<PathBuf> {
    let entrypoint = entrypoint?;
    let mut directory = entrypoint.parent()?.to_path_buf();
    loop {
        if directory.join("package.json").exists() {
            return Some(directory);
        }
        let parent = directory.parent()?.to_path_buf();
        if parent == directory {
            return None;
        }
        directory = parent;
    }
}

fn is_managed_by_global_package_manager(
    env: &InstallEnv,
    method: InstallMethod,
    npm_command: Option<&[String]>,
) -> Result<bool, String> {
    let package_dirs: Vec<PathBuf> = std::iter::once(env.package_dir.clone())
        .chain(get_entrypoint_package_dir(env.entrypoint.as_deref()))
        .collect();
    let package_dir_candidates: Vec<String> = package_dirs
        .iter()
        .flat_map(|dir| path_comparison_candidates(dir))
        .collect();
    let separator = if cfg!(windows) { '\\' } else { '/' };
    Ok(get_global_package_roots(env, method, npm_command)?
        .iter()
        .any(|root| {
            path_comparison_candidates(root).iter().any(|root| {
                let root_prefix = if root.ends_with(separator) {
                    root.clone()
                } else {
                    format!("{root}{separator}")
                };
                package_dir_candidates
                    .iter()
                    .any(|package_dir| package_dir.starts_with(&root_prefix))
            })
        }))
}

pub fn get_self_update_command(
    env: &InstallEnv,
    package_name: &str,
    npm_command: Option<&[String]>,
    target: &SelfUpdatePackageTarget,
) -> Result<Option<SelfUpdateCommand>, String> {
    let method = detect_install_method(env);
    let Some(command) =
        get_self_update_command_for_method(env, method, package_name, target, npm_command)
    else {
        return Ok(None);
    };
    if !is_managed_by_global_package_manager(env, method, npm_command)?
        || !is_self_update_path_writable(env)
    {
        return Ok(None);
    }
    Ok(Some(command))
}

/// `accessSync(path, W_OK)` for the package directory and its parent.
fn is_self_update_path_writable(env: &InstallEnv) -> bool {
    let writable = |path: &Path| {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let Ok(path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
                return false;
            };
            unsafe { libc::access(path.as_ptr(), libc::W_OK) == 0 }
        }
        #[cfg(not(unix))]
        {
            std::fs::metadata(path)
                .map(|metadata| !metadata.permissions().readonly())
                .unwrap_or(false)
        }
    };
    writable(&env.package_dir) && env.package_dir.parent().is_some_and(writable)
}

pub fn get_self_update_unavailable_instruction(
    env: &InstallEnv,
    package_name: &str,
    npm_command: Option<&[String]>,
    target: &SelfUpdatePackageTarget,
) -> Result<String, String> {
    let method = detect_install_method(env);
    if method == InstallMethod::Binary {
        return Ok(
            "Download from: https://github.com/notagentdev/notagent/releases/latest".to_owned(),
        );
    }
    match get_self_update_command_for_method(env, method, package_name, target, npm_command) {
        Some(command) => {
            if is_managed_by_global_package_manager(env, method, npm_command)?
                && !is_self_update_path_writable(env)
            {
                return Ok(format!(
                    "This installation is managed by a global {} install, but the install path is not writable. Update it yourself with: {}",
                    method.as_str(),
                    command.display
                ));
            }
            Ok(format!(
                "This installation is not managed by a global {} install. Update it with the package manager, wrapper, or source checkout that provides it.",
                method.as_str()
            ))
        }
        None => Ok(format!(
            "Update {} using the package manager, wrapper, or source checkout that provides this installation.",
            target.install_spec
        )),
    }
}

pub fn get_update_instruction(env: &InstallEnv, package_name: &str) -> Result<String, String> {
    let method = detect_install_method(env);
    let target = SelfUpdatePackageTarget::new(package_name);
    match get_self_update_command_for_method(env, method, package_name, &target, None) {
        Some(command) => Ok(format!("Run: {}", command.display)),
        None => get_self_update_unavailable_instruction(env, package_name, None, &target),
    }
}
