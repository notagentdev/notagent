use std::process::Stdio;
use std::time::{Duration, Instant};

use serde_json::{Map, Value};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use super::Hook;
use super::events::BLOCKING_EVENT;

/// Grace period between asking a timed-out child to stop and killing it.
const KILL_GRACE: Duration = Duration::from_millis(2_000);

/// Output kept from a hook, enough to explain itself without flooding.
const MAX_CAPTURED_CHARS: usize = 4_000;

#[derive(Debug, Clone, PartialEq)]
pub struct HookRunResult {
    pub hook: Hook,
    /// False for a non-zero exit, a timeout, or a spawn failure.
    pub ok: bool,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub duration_ms: u64,
}

/// What a PreToolUse hook decided about one tool call.
/// Three-valued rather than block-or-not, because the permission chain has
/// three user-authored slots and a hook is how they are filled. Most hooks
/// abstain: a formatter or a logger observes the call and has no opinion about
/// whether it should happen.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum HookVerdict {
    #[default]
    Abstain,
    Allow {
        reason: Option<String>,
    },
    Ask {
        reason: Option<String>,
    },
    Deny {
        reason: String,
    },
}

/// A verdict together with the hook that produced it.
#[derive(Debug, Clone, PartialEq)]
pub struct HookVerdictOutcome {
    pub verdict: HookVerdict,
    /// The deciding hook. Absent when every hook abstained.
    pub hook: Option<Hook>,
    pub results: Vec<HookRunResult>,
}

fn truncate(text: &str) -> String {
    let units: Vec<u16> = text.encode_utf16().collect();
    if units.len() <= MAX_CAPTURED_CHARS {
        return text.to_string();
    }
    format!(
        "{}…",
        String::from_utf16_lossy(&units[..MAX_CAPTURED_CHARS])
    )
}

/// `spawn(command, { shell: true })`: the platform's default shell, not the
/// user's configured one — a hook file is written against `sh`.
fn shell_command(command: &str) -> Command {
    #[cfg(windows)]
    {
        let shell = std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".to_string());
        let mut spawned = Command::new(shell);
        spawned.args(["/d", "/s", "/c", command]);
        spawned
    }
    #[cfg(not(windows))]
    {
        let mut spawned = Command::new("/bin/sh");
        spawned.args(["-c", command]);
        spawned
    }
}

/// SIGTERM to the child, as `child.kill("SIGTERM")` sends it.
fn terminate(pid: u32) {
    #[cfg(unix)]
    // SAFETY: `kill` with a valid pid; the worst case of a reused pid is the
    // same one Node's `child.kill` has.
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGTERM);
    }
    #[cfg(not(unix))]
    let _ = pid;
}

/// Runs one hook to completion. Never fails: a hook that cannot start is a
/// failed hook, not an error for the caller to handle, because the caller
/// is in the middle of a tool call.
pub async fn run_hook(
    hook: &Hook,
    payload: &Value,
    signal: Option<&CancellationToken>,
) -> HookRunResult {
    let started_at = Instant::now();
    let finish = |ok_code: Option<i32>,
                  stdout: String,
                  stderr: String,
                  timed_out: bool,
                  started_at: Instant| HookRunResult {
        hook: hook.clone(),
        ok: !timed_out && ok_code == Some(0),
        exit_code: ok_code,
        stdout: truncate(&stdout),
        stderr: truncate(&stderr),
        timed_out,
        duration_ms: started_at.elapsed().as_millis() as u64,
    };

    let mut command = shell_command(&hook.command);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return finish(None, String::new(), error.to_string(), false, started_at),
    };

    // A hook that ignores its input must not wedge on a full pipe, so write
    // and close regardless of whether anything reads.
    if let Some(mut stdin) = child.stdin.take() {
        let line = format!("{payload}\n");
        tokio::spawn(async move {
            let _ = stdin.write_all(line.as_bytes()).await;
            let _ = stdin.shutdown().await;
        });
    }

    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();
    let read_stdout = tokio::spawn(read_to_string(stdout_pipe));
    let read_stderr = tokio::spawn(read_to_string(stderr_pipe));

    let pid = child.id();
    let mut timed_out = false;
    let deadline = Duration::from_secs_f64(hook.timeout_ms.max(0.0) / 1000.0);

    let status = loop {
        let stop = async {
            match signal {
                Some(token) => token.cancelled().await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            status = child.wait() => break status.ok().and_then(|status| status.code()),
            () = tokio::time::sleep(deadline), if !timed_out => {
                timed_out = true;
                if let Some(pid) = pid { terminate(pid); }
            }
            () = stop, if !timed_out => {
                timed_out = true;
                if let Some(pid) = pid { terminate(pid); }
            }
            // The grace period only starts once the child has been asked to stop.
            () = tokio::time::sleep(KILL_GRACE), if timed_out => {
                let _ = child.kill().await;
            }
        }
    };

    let stdout = read_stdout.await.unwrap_or_default();
    let stderr = read_stderr.await.unwrap_or_default();
    finish(status, stdout, stderr, timed_out, started_at)
}

async fn read_to_string<R: tokio::io::AsyncRead + Unpin>(pipe: Option<R>) -> String {
    let Some(mut pipe) = pipe else {
        return String::new();
    };
    let mut buffer = Vec::new();
    let _ = tokio::io::AsyncReadExt::read_to_end(&mut pipe, &mut buffer).await;
    String::from_utf8_lossy(&buffer).into_owned()
}

/// Runs hooks in declaration order, one after another. Sequential on purpose:
/// hooks touch the workspace, and two formatters racing on the same file is a
/// corruption the user cannot debug.
pub async fn run_hooks(
    hooks: &[&Hook],
    payload: &Value,
    signal: Option<&CancellationToken>,
) -> Vec<HookRunResult> {
    let mut results = Vec::new();
    for hook in hooks {
        results.push(run_hook(hook, payload, signal).await);
    }
    results
}

const DECISION_KEYS: [&str; 3] = ["permission", "permissionDecision", "decision"];
const REASON_KEYS: [&str; 2] = ["reason", "permissionDecisionReason"];

fn parse_json_object(text: &str) -> Option<Map<String, Value>> {
    let attempt = |candidate: &str| -> Option<Map<String, Value>> {
        if !candidate.starts_with('{') {
            return None;
        }
        match serde_json::from_str::<Value>(candidate) {
            Ok(Value::Object(object)) => Some(object),
            _ => None,
        }
    };
    if let Some(whole) = attempt(text.trim()) {
        return Some(whole);
    }
    // A hook that prints progress before its decision is common enough that
    // ignoring the decision would look like the hook was never consulted, which
    // is the failure a user cannot see. So the last line gets a second chance.
    let last = text.split('\n').rfind(|line| !line.trim().is_empty())?;
    attempt(last.trim())
}

/// The exit status a hook uses to refuse. Any other non-zero exit is a fault.
pub const REFUSAL_EXIT_CODE: i32 = 2;

/// Reads what a hook decided.
/// A hook states a decision by printing JSON: `{"permission": "allow" | "ask" |
/// "deny", "reason": "..."}`. The reference spellings `permissionDecision` and
/// `decision` with `approve`/`block` are accepted too, so a hook written against
/// it works here unchanged.
/// Without such output the exit status decides, and only exit 2 refuses. Every
/// other non-zero exit is a broken hook, not a decision: a command that does not
/// exist leaves 127, and a hook with a typo in its name would otherwise refuse
/// every tool call in the session while reporting nothing more useful than the
/// number. The reference draws the same line, so a hook written for it keeps its
/// meaning here.
/// A timeout still refuses, deliberately unlike the reference. A hook that hangs
/// has decided nothing, and treating silence as consent is the one direction a
/// blocking hook must never fail in.
/// Exit zero without a stated decision abstains rather than approves: an
/// ordinary hook that happens to succeed must not silently widen what is
/// permitted.
pub fn read_verdict(result: &HookRunResult) -> HookVerdict {
    let stated = parse_json_object(&result.stdout);
    let raw = stated.as_ref().and_then(|stated| {
        DECISION_KEYS
            .iter()
            .filter_map(|key| stated.get(*key))
            .find_map(Value::as_str)
    });
    if let Some(raw) = raw {
        let spoken = raw.trim().to_lowercase();
        let reason = stated
            .as_ref()
            .and_then(|stated| {
                REASON_KEYS
                    .iter()
                    .filter_map(|key| stated.get(*key))
                    .find_map(Value::as_str)
            })
            .filter(|reason| !reason.trim().is_empty())
            .map(|reason| reason.trim().to_string());
        if spoken == "allow" || spoken == "approve" {
            return HookVerdict::Allow { reason };
        }
        if spoken == "ask" || spoken == "confirm" {
            return HookVerdict::Ask { reason };
        }
        if spoken == "deny" || spoken == "block" {
            return HookVerdict::Deny {
                reason: reason.unwrap_or_else(|| describe_refusal(result)),
            };
        }
    }
    if result.timed_out || result.exit_code == Some(REFUSAL_EXIT_CODE) {
        return HookVerdict::Deny {
            reason: describe_refusal(result),
        };
    }
    HookVerdict::Abstain
}

/// Whether a run failed in a way that is a fault rather than a decision.
pub fn is_hook_fault(result: &HookRunResult) -> bool {
    if result.ok {
        return false;
    }
    if result.timed_out {
        return true;
    }
    result.exit_code != Some(REFUSAL_EXIT_CODE)
}

/// Runs the PreToolUse hooks and combines what they decided.
/// A denial ends the run: the call is not happening, so later hooks would react
/// to an action that never occurs. Anything else keeps going, because a hook
/// that allows must not be able to suppress a later one that refuses — order
/// would otherwise decide safety, and the order of a hook file is not something
/// a user thinks of as a security setting.
/// Among what remains, the strictest wins: asking beats allowing, allowing beats
/// abstaining.
pub async fn decide_tool_call(
    hooks: &[&Hook],
    payload: &Value,
    signal: Option<&CancellationToken>,
) -> HookVerdictOutcome {
    let mut results: Vec<HookRunResult> = Vec::new();
    let mut verdict = HookVerdict::Abstain;
    let mut decided_by: Option<Hook> = None;

    for hook in hooks {
        if hook.event != BLOCKING_EVENT {
            continue;
        }
        let result = run_hook(hook, payload, signal).await;
        let stated = read_verdict(&result);
        results.push(result);
        if matches!(stated, HookVerdict::Deny { .. }) {
            return HookVerdictOutcome {
                verdict: stated,
                hook: Some((*hook).clone()),
                results,
            };
        }
        if matches!(stated, HookVerdict::Abstain) {
            continue;
        }
        if matches!(verdict, HookVerdict::Ask { .. }) {
            continue;
        }
        verdict = stated;
        decided_by = Some((*hook).clone());
    }

    HookVerdictOutcome {
        verdict,
        hook: decided_by,
        results,
    }
}

/// The message a user sees for a refusal. What the hook wrote comes first;
/// failing that, the outcome is stated plainly, because "blocked" with no
/// explanation is the worst possible prompt.
pub fn describe_refusal(result: &HookRunResult) -> String {
    let written = first_line(&result.stderr, &result.stdout);
    if let Some(written) = written {
        return format!("{written} (hook: {})", result.hook.command);
    }
    if result.timed_out {
        return format!(
            "Hook timed out after {}ms (hook: {})",
            result.hook.timeout_ms, result.hook.command
        );
    }
    format!(
        "Hook exited with {} (hook: {})",
        result
            .exit_code
            .map_or_else(|| "no status".to_string(), |code| code.to_string()),
        result.hook.command
    )
}

/// `(stderr.trim() || stdout.trim()).split("\n")[0]?.trim()`, empty means none.
fn first_line<'a>(stderr: &'a str, stdout: &'a str) -> Option<&'a str> {
    let text = if stderr.trim().is_empty() {
        stdout.trim()
    } else {
        stderr.trim()
    };
    let first = text.split('\n').next().unwrap_or("").trim();
    (!first.is_empty()).then_some(first)
}
