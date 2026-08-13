//! Port of `packages/coding-agent/src/utils/shell.ts`.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;
#[cfg(windows)]
use std::process::Stdio;
use std::sync::{LazyLock, Mutex};

use crate::config::get_bin_dir;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandTransport {
    Argv,
    Stdin,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellConfig {
    pub shell: String,
    pub args: Vec<String>,
    pub command_transport: Option<CommandTransport>,
}

/// Legacy WSL `bash.exe` takes its command on stdin, not in argv.
fn is_legacy_wsl_bash_path(path: &str) -> bool {
    let normalized = path.replace('/', "\\").to_lowercase();
    let Some(rest) = normalized.strip_suffix("\\bash.exe") else {
        return false;
    };
    let Some((drive, system)) = rest.split_once(":\\windows\\") else {
        return false;
    };
    drive.len() == 1
        && drive
            .chars()
            .all(|character| character.is_ascii_lowercase())
        && matches!(system, "system32" | "sysnative")
}

fn bash_shell_config(shell: &str) -> ShellConfig {
    if is_legacy_wsl_bash_path(shell) {
        ShellConfig {
            shell: shell.to_owned(),
            args: vec!["-s".to_owned()],
            command_transport: Some(CommandTransport::Stdin),
        }
    } else {
        ShellConfig {
            shell: shell.to_owned(),
            args: vec!["-c".to_owned()],
            command_transport: None,
        }
    }
}

/// Find the bash executable on PATH (cross-platform).
fn find_bash_on_path() -> Option<String> {
    #[cfg(windows)]
    {
        // Windows: `where` can return paths that do not exist, so verify.
        let output = Command::new("where").arg("bash.exe").output().ok()?;
        if !output.status.success() {
            return None;
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let first = stdout.trim().lines().next()?.trim().to_owned();
        if !first.is_empty() && Path::new(&first).exists() {
            Some(first)
        } else {
            None
        }
    }
    #[cfg(not(windows))]
    {
        // Unix: trust `which` (handles Termux and special filesystems).
        let output = Command::new("which").arg("bash").output().ok()?;
        if !output.status.success() {
            return None;
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let first = stdout.trim().lines().next()?.trim().to_owned();
        if first.is_empty() { None } else { Some(first) }
    }
}

/// Resolve shell configuration based on platform and an optional explicit shell path.
///
/// Resolution order: user-specified `shellPath`, then on Windows Git Bash in
/// known locations followed by bash on PATH, on Unix `/bin/bash`, bash on PATH
/// and finally `sh`.
pub fn get_shell_config(custom_shell_path: Option<&str>) -> Result<ShellConfig, String> {
    if let Some(custom) = custom_shell_path.filter(|path| !path.is_empty()) {
        let expanded = crate::config::expand_tilde_path(custom);
        if expanded.exists() {
            return Ok(bash_shell_config(&expanded.to_string_lossy()));
        }
        return Err(format!("Custom shell path not found: {custom}"));
    }

    #[cfg(windows)]
    {
        let mut paths: Vec<String> = Vec::new();
        if let Ok(program_files) = std::env::var("ProgramFiles") {
            paths.push(format!("{program_files}\\Git\\bin\\bash.exe"));
        }
        if let Ok(program_files_x86) = std::env::var("ProgramFiles(x86)") {
            paths.push(format!("{program_files_x86}\\Git\\bin\\bash.exe"));
        }
        for path in &paths {
            if Path::new(path).exists() {
                return Ok(bash_shell_config(path));
            }
        }
        if let Some(bash) = find_bash_on_path() {
            return Ok(bash_shell_config(&bash));
        }
        let searched = paths
            .iter()
            .map(|path| format!("  {path}"))
            .collect::<Vec<_>>()
            .join("\n");
        return Err(format!(
            "No bash shell found. Options:\n  1. Install Git for Windows: https://git-scm.com/download/win\n  2. Add your bash to PATH (Cygwin, MSYS2, etc.)\n  3. Set shellPath in settings.json\n\nSearched Git Bash in:\n{searched}"
        ));
    }

    #[cfg(not(windows))]
    {
        if Path::new("/bin/bash").exists() {
            return Ok(bash_shell_config("/bin/bash"));
        }
        if let Some(bash) = find_bash_on_path() {
            return Ok(bash_shell_config(&bash));
        }
        Ok(ShellConfig {
            shell: "sh".to_owned(),
            args: vec!["-c".to_owned()],
            command_transport: None,
        })
    }
}

/// The process environment with the managed binary directory on PATH.
pub fn get_shell_env() -> BTreeMap<String, String> {
    let mut env: BTreeMap<String, String> = std::env::vars().collect();
    let bin_dir = get_bin_dir().to_string_lossy().into_owned();
    let path_key = env
        .keys()
        .find(|key| key.eq_ignore_ascii_case("path"))
        .cloned()
        .unwrap_or_else(|| "PATH".to_owned());
    let current_path = env.get(&path_key).cloned().unwrap_or_default();
    let separator = if cfg!(windows) { ';' } else { ':' };
    let has_bin_dir = current_path.split(separator).any(|entry| entry == bin_dir);
    let updated_path = if has_bin_dir {
        current_path
    } else if current_path.is_empty() {
        bin_dir
    } else {
        format!("{bin_dir}{separator}{current_path}")
    };
    env.insert(path_key, updated_path);
    env
}

/// Sanitize binary output for display and storage.
///
/// Removes control characters (except tab, newline and carriage return) and the
/// Unicode format characters that crash width measurement. Lone surrogates
/// cannot occur in a Rust `str`.
pub fn sanitize_binary_output(value: &str) -> String {
    value
        .chars()
        .filter(|character| {
            let code = u32::from(*character);
            if matches!(code, 0x09 | 0x0a | 0x0d) {
                return true;
            }
            if code <= 0x1f {
                return false;
            }
            !(0xfff9..=0xfffb).contains(&code)
        })
        .collect()
}

/// Detached child processes must be tracked so they can be killed on parent
/// shutdown signals (SIGHUP/SIGTERM).
static TRACKED_DETACHED_CHILD_PIDS: LazyLock<Mutex<Vec<u32>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

pub fn track_detached_child_pid(pid: u32) {
    let mut tracked = TRACKED_DETACHED_CHILD_PIDS
        .lock()
        .expect("tracked pids mutex");
    if !tracked.contains(&pid) {
        tracked.push(pid);
    }
}

pub fn untrack_detached_child_pid(pid: u32) {
    TRACKED_DETACHED_CHILD_PIDS
        .lock()
        .expect("tracked pids mutex")
        .retain(|tracked| *tracked != pid);
}

pub fn kill_tracked_detached_children() {
    let pids: Vec<u32> = std::mem::take(
        &mut *TRACKED_DETACHED_CHILD_PIDS
            .lock()
            .expect("tracked pids mutex"),
    );
    for pid in pids {
        kill_process_tree(pid);
    }
}

/// Ask a process and all its children to stop (cross-platform).
///
/// The polite half of stopping: a shell that is told to terminate closes its
/// files, a dev server drops its socket, and a test runner writes its report.
/// Callers pair this with [`kill_process_tree`] after a grace window.
pub fn terminate_process_tree(pid: u32) {
    #[cfg(windows)]
    {
        // Without /F this asks the tree to close rather than forcing it.
        let _ = Command::new("taskkill")
            .args(["/T", "/PID", &pid.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
    #[cfg(not(windows))]
    {
        signal_process_tree(pid, libc::SIGTERM);
    }
}

/// Kill a process and all its children (cross-platform).
pub fn kill_process_tree(pid: u32) {
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
    #[cfg(not(windows))]
    {
        signal_process_tree(pid, libc::SIGKILL);
    }
}

#[cfg(not(windows))]
fn signal_process_tree(pid: u32, signal: i32) {
    // Signal the process group first, exactly like `process.kill(-pid, ...)`;
    // fall back to the single process when it has no group of its own.
    let group_result = unsafe { libc::kill(-(pid as i32), signal) };
    if group_result != 0 {
        unsafe {
            libc::kill(pid as i32, signal);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_legacy_wsl_bash_paths() {
        assert!(is_legacy_wsl_bash_path("C:\\Windows\\System32\\bash.exe"));
        assert!(is_legacy_wsl_bash_path("c:/windows/sysnative/bash.exe"));
        assert!(!is_legacy_wsl_bash_path(
            "C:\\Program Files\\Git\\bin\\bash.exe"
        ));
        assert!(!is_legacy_wsl_bash_path("/bin/bash"));
    }

    #[test]
    fn wsl_bash_takes_its_command_on_stdin() {
        let config = bash_shell_config("C:\\Windows\\System32\\bash.exe");
        assert_eq!(config.args, vec!["-s".to_owned()]);
        assert_eq!(config.command_transport, Some(CommandTransport::Stdin));
        let config = bash_shell_config("/bin/bash");
        assert_eq!(config.args, vec!["-c".to_owned()]);
        assert_eq!(config.command_transport, None);
    }

    #[test]
    fn sanitizes_control_and_format_characters() {
        assert_eq!(sanitize_binary_output("a\u{0}b\u{7}c"), "abc");
        assert_eq!(
            sanitize_binary_output("keep\tthese\nlines\r"),
            "keep\tthese\nlines\r"
        );
        assert_eq!(sanitize_binary_output("x\u{fff9}y\u{fffb}z"), "xyz");
        assert_eq!(sanitize_binary_output("emoji 🎉 stays"), "emoji 🎉 stays");
    }

    #[test]
    fn shell_env_puts_the_managed_bin_directory_first() {
        let env = get_shell_env();
        let path_key = env
            .keys()
            .find(|key| key.eq_ignore_ascii_case("path"))
            .expect("PATH");
        let separator = if cfg!(windows) { ';' } else { ':' };
        let bin_dir = get_bin_dir().to_string_lossy().into_owned();
        assert!(
            env[path_key].split(separator).any(|entry| entry == bin_dir),
            "{}",
            env[path_key]
        );
    }

    #[test]
    fn resolves_a_usable_shell() {
        let config = get_shell_config(None).expect("a shell");
        assert!(!config.shell.is_empty());
        assert!(!config.args.is_empty());
    }

    #[test]
    fn rejects_a_missing_custom_shell_path() {
        let error = get_shell_config(Some("/definitely/not/a/shell")).expect_err("rejects");
        assert!(error.contains("Custom shell path not found"), "{error}");
    }
}
