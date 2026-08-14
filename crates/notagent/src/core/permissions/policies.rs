//! Port of `packages/coding-agent/src/core/permissions/policies.ts`.
//!
//! The self-contained policies: those that decide from the tool call and the
//! filesystem alone, without user configuration or session state.
//!
//! The user-authored allow/ask/deny rules and the session approval history are
//! deliberately not here. They need a settings shape and a per-session store,
//! and inventing either would fix decisions that belong to the user.

use std::sync::{Arc, LazyLock};

use regex::Regex;

use super::policy::{FnPolicy, PermissionContext, PermissionDecision, PermissionPolicy};
use crate::utils::paths::{is_absolute_path, main_separator, node_relative, node_resolve};

/// Tools that only read. They never need confirmation.
///
/// `task` is here because delegating changes nothing by itself: the subagent's
/// own tool calls come back through this same chain, so what it does is decided
/// where every other call is. Asking about the delegation as well would be a
/// prompt about an action that has no effect.
const READ_ONLY_TOOLS: [&str; 10] = [
    "read",
    "read_minified",
    "grep",
    "find",
    "ls",
    "skill",
    "task",
    // Listing background work and reading its output change nothing. `task_stop`
    // is deliberately absent: it ends a running process, which is the kind of
    // thing a prompt exists for.
    "task_list",
    "task_output",
    // Writing the checklist touches nothing but the checklist.
    "todo_write",
];

/// Tools whose argument names a path the policies reason about.
const PATH_ARGUMENTS: [&str; 2] = ["path", "file_path"];

/// Files worth a second look regardless of mode: secrets, credentials and
/// shell startup files that can execute code on the next login.
static SENSITIVE_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"(?i)(^|/)\.env(\.|$)",
        r"(?i)(^|/)\.netrc$",
        r"(?i)(^|/)\.npmrc$",
        r"(?i)(^|/)\.ssh/",
        r"(?i)(^|/)\.aws/",
        r"(?i)(^|/)\.gnupg/",
        r"(?i)(^|/)id_(rsa|dsa|ecdsa|ed25519)$",
        r"(?i)(^|/)\.(bash|zsh)(rc|_profile|env)$",
        r"(?i)(^|/)\.profile$",
        r"(?i)\.(pem|key|p12|pfx|keystore)$",
        r"(?i)(^|/)(credentials|secrets)(\.[a-z0-9]+)?$",
    ]
    .into_iter()
    .map(|pattern| Regex::new(pattern).expect("sensitive pattern"))
    .collect()
});

static GIT_CONTROL_PATH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(^|/)\.git(/|$)").expect("git control pattern"));

fn target_path(context: &PermissionContext) -> Option<&str> {
    PATH_ARGUMENTS
        .iter()
        .filter_map(|key| context.input.get(*key))
        .filter_map(|value| value.as_str())
        .find(|value| !value.trim().is_empty())
}

fn absolute_target(context: &PermissionContext) -> Option<String> {
    let raw = target_path(context)?;
    Some(if is_absolute_path(raw) {
        node_resolve(&[raw])
    } else {
        node_resolve(&[&context.cwd, raw])
    })
}

fn is_inside(parent: &str, child: &str) -> bool {
    let separator = main_separator();
    let relative = node_relative(&node_resolve(&[parent]), child);
    !relative.is_empty()
        && relative != ".."
        && !relative.starts_with(&format!("..{separator}"))
        && !is_absolute_path(&relative)
}

/// `target.split(sep).join("/")`: the patterns are written with forward slashes.
fn to_posix(path: &str) -> String {
    path.replace(main_separator(), "/")
}

/// Touching secrets asks even in a permissive mode. Placed ahead of yolo
/// approval so the most permissive stop stays bounded by it.
pub fn sensitive_file_access_ask() -> Arc<dyn PermissionPolicy> {
    Arc::new(FnPolicy::new(
        "sensitive-file-access-ask",
        |context: &PermissionContext| {
            let target = absolute_target(context)?;
            let normalized = to_posix(&target);
            if !SENSITIVE_PATTERNS
                .iter()
                .any(|pattern| pattern.is_match(&normalized))
            {
                return None;
            }
            Some(PermissionDecision::Ask {
                reason: Some(format!(
                    "{} looks like a credentials or shell startup file.",
                    target_path(context).unwrap_or_default()
                )),
            })
        },
    ))
}

/// Writing into the repository's own control directory can rewrite history or
/// hooks, which is not what an ordinary edit means.
pub fn git_control_path_access_ask() -> Arc<dyn PermissionPolicy> {
    Arc::new(FnPolicy::new(
        "git-control-path-access-ask",
        |context: &PermissionContext| {
            if READ_ONLY_TOOLS.contains(&context.tool_name.as_str()) {
                return None;
            }
            let target = absolute_target(context)?;
            if !GIT_CONTROL_PATH.is_match(&to_posix(&target)) {
                return None;
            }
            Some(PermissionDecision::Ask {
                reason: Some(format!(
                    "{} is inside the git control directory.",
                    target_path(context).unwrap_or_default()
                )),
            })
        },
    ))
}

/// Reading never needs confirmation.
pub fn default_tool_approve() -> Arc<dyn PermissionPolicy> {
    Arc::new(FnPolicy::new(
        "default-tool-approve",
        |context: &PermissionContext| {
            READ_ONLY_TOOLS
                .contains(&context.tool_name.as_str())
                .then_some(PermissionDecision::Approve)
        },
    ))
}

/// Writes inside the working directory are the ordinary case and are approved.
/// A write outside it is not, and falls through to the trailing ask.
pub fn git_cwd_write_approve() -> Arc<dyn PermissionPolicy> {
    Arc::new(FnPolicy::new(
        "git-cwd-write-approve",
        |context: &PermissionContext| {
            let target = absolute_target(context)?;
            is_inside(&context.cwd, &target).then_some(PermissionDecision::Approve)
        },
    ))
}

/// Commands whose effect cannot be taken back, or that reach outside the
/// workspace in a way no amount of reading beforehand would reveal.
///
/// A regular expression rather than a parse, because the alternative is a shell
/// grammar and the answer only has to be "worth a second look". A false
/// positive costs one prompt; a false negative costs the thing the pattern was
/// written for.
static DESTRUCTIVE_COMMAND_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"(?i)\brm\s+(-[a-z]*[rf][a-z]*\s+)+",
        r"(?i)\brmdir\s+",
        r"(?i)\bgit\s+push\b[^\n]*(--force\b|--force-with-lease\b|\s-f\b)",
        r"(?i)\bgit\s+reset\s+--hard\b",
        r"(?i)\bgit\s+clean\b[^\n]*-[a-z]*f",
        r"(?i)\bgit\s+checkout\s+--\s",
        r"(?i)\bgit\s+branch\s+-D\b",
        r"(?i)\bdd\s+[^\n]*\bof=",
        r"(?i)\bmkfs(\.[a-z0-9]+)?\b",
        r"(?i)\b(shutdown|reboot|halt)\b",
        r"(?i)\bchmod\s+(-[a-z]+\s+)*(777|[0-7]*7[0-7]{2})\b",
        r"(?i)\b(drop|truncate)\s+(table|database|schema)\b",
        r"(?i)\bkill(all)?\s+-9\b",
        r"(?i)\bnpm\s+publish\b",
        r"(?i)\b(curl|wget)\b[^\n|]*\|\s*(sudo\s+)?(ba|z|k)?sh\b",
        r"(?i)(^|\s)>\s*/dev/(sd|nvme|disk)",
    ]
    .into_iter()
    .map(|pattern| Regex::new(pattern).expect("destructive pattern"))
    .collect()
});

/// Tool arguments that carry a shell command rather than a path.
const COMMAND_ARGUMENTS: [&str; 2] = ["command", "cmd"];

fn command_text(context: &PermissionContext) -> Option<&str> {
    COMMAND_ARGUMENTS
        .iter()
        .filter_map(|key| context.input.get(*key))
        .filter_map(|value| value.as_str())
        .find(|value| !value.trim().is_empty())
}

fn evaluate_destructive(context: &PermissionContext) -> Option<PermissionDecision> {
    let command = command_text(context)?;
    if !DESTRUCTIVE_COMMAND_PATTERNS
        .iter()
        .any(|pattern| pattern.is_match(command))
    {
        return None;
    }
    Some(PermissionDecision::Ask {
        reason: Some(
            "This command cannot be undone, so it is confirmed even in modes that approve automatically."
                .to_string(),
        ),
    })
}

/// Irreversible commands ask in every mode, including yolo.
///
/// This sits ahead of every approving policy on purpose. Without it the guards
/// are blind exactly where the damage is: they all read a path argument, and a
/// shell command has none — so `rm -rf` reached auto-approval untouched while
/// a write to the same directory would have been stopped. The mode gradient is
/// about supervision, not about whether a mistake can be undone.
///
/// A destructive command is also never remembered for the session: "allow this
/// once" is the most it can ever mean.
pub fn destructive_command_ask() -> Arc<dyn PermissionPolicy> {
    Arc::new(FnPolicy::new(
        "destructive-command-ask",
        evaluate_destructive,
    ))
}

/// Whether a call must be confirmed every time, never remembered.
pub fn is_destructive_call(context: &PermissionContext) -> bool {
    evaluate_destructive(context).is_some()
}

/// Closes the chain. Everything that reached this point is unrecognised, and
/// unrecognised means ask rather than allow.
pub fn fallback_ask() -> Arc<dyn PermissionPolicy> {
    Arc::new(FnPolicy::new(
        "fallback-ask",
        |_context: &PermissionContext| {
            Some(PermissionDecision::Ask {
                reason: Some("No rule covers this call.".to_string()),
            })
        },
    ))
}

/// The policies implemented here, keyed by their slot name.
pub fn self_contained_policies() -> Vec<(&'static str, Arc<dyn PermissionPolicy>)> {
    vec![
        ("destructive-command-ask", destructive_command_ask()),
        ("sensitive-file-access-ask", sensitive_file_access_ask()),
        ("git-control-path-access-ask", git_control_path_access_ask()),
        ("default-tool-approve", default_tool_approve()),
        ("git-cwd-write-approve", git_cwd_write_approve()),
        ("fallback-ask", fallback_ask()),
    ]
}
