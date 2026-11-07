use std::path::PathBuf;
use std::sync::{Arc, LazyLock};

use regex::Regex;

use super::policy::{FnPolicy, PermissionContext, PermissionDecision, PermissionPolicy};
use crate::config::{get_auth_path, get_mcp_credentials_path};
use crate::core::modes::shells::MUTATING_TOOLS;
use crate::core::tools::ToolName;
use crate::core::tools::patch_minified::files_argument_paths;
use crate::core::tools::path_utils::{resolve_read_path, resolve_to_cwd};
use crate::utils::paths::{is_absolute_path, main_separator, node_relative, node_resolve};

/// Tools that only read. They never need confirmation.
/// `task` is here because delegating changes nothing by itself: the subagent's
/// own tool calls come back through this same chain, so what it does is decided
/// where every other call is. Asking about the delegation as well would be a
/// prompt about an action that has no effect.
const READ_ONLY_TOOLS: [&str; 13] = [
    "read",
    "read_minified",
    "grep",
    "find_filesystem",
    "find_codebase",
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
    // Folding rewrites what the model reads next, from the session's own
    // history; no file is touched.
    "context_fold",
    // A question changes nothing, and a permission prompt in front of it would
    // ask the user whether they may be asked.
    "ask_user_question",
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

/// Case-insensitive because the default file systems on macOS and Windows are:
/// `.GIT/config` is `.git/config` there, and git runs what that file names.
static GIT_CONTROL_PATH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(^|/)\.git(/|$)").expect("git control pattern"));

/// Every path a call names: its `path` (or `file_path`) and the `path` of each
/// entry in `files`. A guard that looked only at the first would let a
/// multi-file call carry `.git/config` in its second entry past it.
fn target_paths(context: &PermissionContext) -> Vec<&str> {
    let single = PATH_ARGUMENTS
        .iter()
        .filter_map(|key| context.input.get(*key))
        .filter_map(|value| value.as_str())
        .find(|value| !value.trim().is_empty());
    single
        .into_iter()
        .chain(files_argument_paths(&context.input))
        .collect()
}

fn absolute(context: &PermissionContext, raw: &str) -> String {
    if matches!(context.tool_name.as_str(), "read" | "read_minified") {
        resolve_read_path(raw, &context.cwd)
    } else {
        resolve_to_cwd(raw, &context.cwd)
    }
}

/// The first path, as written, for which `matches` holds on its absolute form.
fn first_target_where(context: &PermissionContext, matches: impl Fn(&str) -> bool) -> Option<&str> {
    target_paths(context)
        .into_iter()
        .find(|raw| matches(&absolute(context, raw)))
}

/// Symlinks are followed by every tool that opens a file, so a guard that only
/// reads the path as written can be walked around: `notes -> ~/.ssh` makes
/// `notes/authorized_keys` look like an ordinary file in the workspace.
const MAX_LINK_HOPS: usize = 40;

/// `path` with the symlinks among its existing components resolved. The part
/// that does not exist yet, a file about to be created, is appended unchanged.
fn resolve_links(path: &str) -> String {
    let mut current = PathBuf::from(path);
    let mut missing: Vec<std::ffi::OsString> = Vec::new();
    for _ in 0..MAX_LINK_HOPS {
        if let Ok(mut real) = std::fs::canonicalize(&current) {
            real.extend(missing.iter().rev());
            return real.to_string_lossy().into_owned();
        }
        // A dangling link still decides where a write lands.
        if let Ok(target) = std::fs::read_link(&current) {
            current = match current.parent() {
                Some(parent) => parent.join(target),
                None => target,
            };
            continue;
        }
        let (Some(parent), Some(name)) = (current.parent(), current.file_name()) else {
            break;
        };
        missing.push(name.to_owned());
        current = parent.to_path_buf();
    }
    path.to_string()
}

/// The path as written and the path it leads to; a guard that matches either
/// one applies.
fn guarded_paths(target: &str) -> [String; 2] {
    [to_posix(target), to_posix(&resolve_links(target))]
}

/// This agent's own credential stores. Their names are generic, so no pattern
/// recognises them, and reading one hands every provider key to the model.
fn is_agent_credential_file(target: &str) -> bool {
    let real = resolve_links(target).to_lowercase();
    [get_auth_path(), get_mcp_credentials_path()]
        .iter()
        .any(|path| resolve_links(&path.to_string_lossy()).to_lowercase() == real)
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

/// Touching secrets asks even in auto mode. Yolo approves before this guard is
/// reached.
pub fn sensitive_file_access_ask() -> Arc<dyn PermissionPolicy> {
    Arc::new(FnPolicy::new(
        "sensitive-file-access-ask",
        |context: &PermissionContext| {
            let sensitive = first_target_where(context, |target| {
                guarded_paths(target).iter().any(|path| {
                    SENSITIVE_PATTERNS
                        .iter()
                        .any(|pattern| pattern.is_match(path))
                }) || is_agent_credential_file(target)
            })?;
            Some(PermissionDecision::Ask {
                reason: Some(format!(
                    "{sensitive} looks like a credentials or shell startup file."
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
            let control = first_target_where(context, |target| {
                guarded_paths(target)
                    .iter()
                    .any(|path| GIT_CONTROL_PATH.is_match(path))
            })?;
            Some(PermissionDecision::Ask {
                reason: Some(format!("{control} is inside the git control directory.")),
            })
        },
    ))
}

/// Taking a change back never needs confirmation.
/// The only state undo can produce is one the workspace already had, on a path
/// this agent already changed — so a prompt here asks about the remedy rather
/// than about the damage, and the damage was already confirmed when it was
/// done. The reference draws the same line, listing undo among the tools that
/// skip permission checks entirely (`catalog.rs:1500`).
/// It sits *after* the sensitive-file and git-directory guards in the chain, so
/// the two cases where even a restore deserves a question still reach one.
pub fn undo_approve() -> Arc<dyn PermissionPolicy> {
    Arc::new(FnPolicy::new(
        "undo-approve",
        |context: &PermissionContext| {
            (context.tool_name == "undo").then_some(PermissionDecision::Approve)
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
/// Only the built-in editing tools qualify. An MCP tool may take a `path` and
/// do anything with it, delete included, and it is active even in a read-only
/// mode; its argument names are no evidence of what it does.
pub fn git_cwd_write_approve() -> Arc<dyn PermissionPolicy> {
    Arc::new(FnPolicy::new(
        "git-cwd-write-approve",
        |context: &PermissionContext| {
            let tool = ToolName::parse(&context.tool_name)?;
            if !MUTATING_TOOLS.contains(&tool) {
                return None;
            }
            let targets = target_paths(context);
            if targets.is_empty() {
                return None;
            }
            let real_cwd = resolve_links(&context.cwd);
            // Every file must be inside; one outside sends the whole call to
            // the trailing ask.
            let inside = targets.iter().all(|raw| {
                let target = absolute(context, raw);
                is_inside(&context.cwd, &target) && is_inside(&real_cwd, &resolve_links(&target))
            });
            inside.then_some(PermissionDecision::Approve)
        },
    ))
}

/// Commands whose effect cannot be taken back, or that reach outside the
/// workspace in a way no amount of reading beforehand would reveal.
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

/// Irreversible commands ask in every mode except yolo, whose whole promise is
/// that nothing is confirmed.
/// This sits ahead of every other approving policy on purpose. Without it the guards
/// are blind exactly where the damage is: they all read a path argument, and a
/// shell command has none — so `rm -rf` reached auto-approval untouched while
/// a write to the same directory would have been stopped. The mode gradient is
/// about supervision, not about whether a mistake can be undone.
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
        ("undo-approve", undo_approve()),
        ("default-tool-approve", default_tool_approve()),
        ("git-cwd-write-approve", git_cwd_write_approve()),
        ("fallback-ask", fallback_ask()),
    ]
}
