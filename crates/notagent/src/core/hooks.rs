pub mod dispatch;
pub mod events;
pub mod payload;
pub mod runner;
pub mod runtime;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use events::{HookEvent, hook_event_list};

/// File holding hook declarations, in the agent and project directories.
pub const HOOKS_FILE_NAME: &str = "hooks.json";

/// Default ceiling for a hook, so an omitted timeout is still bounded.
pub const DEFAULT_HOOK_TIMEOUT_MS: u64 = 30_000;

/// Upper bound a declaration may not exceed, however it is written.
pub const MAX_HOOK_TIMEOUT_MS: u64 = 300_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hook {
    pub event: HookEvent,
    /// Restricts a tool-scoped hook to matching tool names.
    pub matcher: Option<String>,
    pub command: String,
    pub timeout_ms: u64,
    /// File the declaration came from, for reporting.
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookDiagnostic {
    pub source: String,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct LoadHooksResult {
    pub hooks: Vec<Hook>,
    pub diagnostics: Vec<HookDiagnostic>,
}

/// `String(value)` for the values a declaration can carry, so a diagnostic
fn describe_value(value: Option<&Value>) -> String {
    match value {
        None => "undefined".to_string(),
        Some(Value::String(value)) => value.clone(),
        Some(Value::Null) => "null".to_string(),
        Some(value) => value.to_string(),
    }
}

fn validate_entry(
    raw: &Value,
    source: &str,
    diagnostics: &mut Vec<HookDiagnostic>,
) -> Option<Hook> {
    let Some(entry) = raw.as_object() else {
        diagnostics.push(HookDiagnostic {
            source: source.to_string(),
            message: "hook entry must be an object".to_string(),
        });
        return None;
    };

    let mut unknown = entry
        .keys()
        .filter(|key| !matches!(key.as_str(), "event" | "matcher" | "command" | "timeout_ms"))
        .cloned()
        .collect::<Vec<_>>();
    unknown.sort();
    if !unknown.is_empty() {
        let message = if unknown.iter().any(|key| key == "timeout") {
            "hook field \"timeout\" is not supported; use \"timeout_ms\"".to_string()
        } else {
            format!("hook has unknown field(s): {}", unknown.join(", "))
        };
        diagnostics.push(HookDiagnostic {
            source: source.to_string(),
            message,
        });
        return None;
    }

    let Some(event) = entry
        .get("event")
        .and_then(Value::as_str)
        .and_then(HookEvent::parse)
    else {
        diagnostics.push(HookDiagnostic {
            source: source.to_string(),
            message: format!(
                "unknown event \"{}\". Valid events: {}",
                describe_value(entry.get("event")),
                hook_event_list()
            ),
        });
        return None;
    };

    let command = entry.get("command").and_then(Value::as_str).unwrap_or("");
    if command.trim().is_empty() {
        diagnostics.push(HookDiagnostic {
            source: source.to_string(),
            message: format!("hook for {event} has no command"),
        });
        return None;
    }

    let matcher = match entry.get("matcher") {
        None => None,
        Some(declared) => match declared.as_str() {
            Some(value) if !value.trim().is_empty() && event.is_tool_scoped() => {
                Some(value.trim().to_string())
            }
            Some(value) if !value.trim().is_empty() => {
                diagnostics.push(HookDiagnostic {
                    source: source.to_string(),
                    message: format!(
                        "hook for {event} declares a matcher, but only tool events carry a tool name"
                    ),
                });
                return None;
            }
            _ => {
                diagnostics.push(HookDiagnostic {
                    source: source.to_string(),
                    message: format!("hook for {event} has an empty matcher"),
                });
                return None;
            }
        },
    };

    let mut timeout_ms = DEFAULT_HOOK_TIMEOUT_MS;
    if let Some(declared) = entry.get("timeout_ms") {
        let Some(requested) = declared.as_u64() else {
            diagnostics.push(HookDiagnostic {
                source: source.to_string(),
                message: format!("hook for {event} has an invalid timeout_ms"),
            });
            return None;
        };
        if requested == 0 || requested > MAX_HOOK_TIMEOUT_MS {
            diagnostics.push(HookDiagnostic {
                source: source.to_string(),
                message: format!(
                    "hook for {event} has timeout_ms {requested}; expected an integer from 1 through {MAX_HOOK_TIMEOUT_MS}"
                ),
            });
            return None;
        }
        timeout_ms = requested;
    }

    Some(Hook {
        event,
        matcher,
        command: command.trim().to_string(),
        timeout_ms,
        source: source.to_string(),
    })
}

fn load_file(file_path: &Path, diagnostics: &mut Vec<HookDiagnostic>) -> Vec<Hook> {
    let source = file_path.to_string_lossy().into_owned();
    let raw = match std::fs::read_to_string(file_path) {
        Ok(raw) => raw,
        Err(error) => {
            diagnostics.push(HookDiagnostic {
                source,
                message: format!("cannot read: {error}"),
            });
            return Vec::new();
        }
    };
    let parsed: Value = match serde_json::from_str(&raw) {
        Ok(parsed) => parsed,
        Err(error) => {
            diagnostics.push(HookDiagnostic {
                source,
                message: format!("invalid JSON: {error}"),
            });
            return Vec::new();
        }
    };

    let entries = match &parsed {
        Value::Array(entries) => Some(entries),
        Value::Object(object) => object.get("hooks").and_then(Value::as_array),
        _ => None,
    };
    let Some(entries) = entries else {
        diagnostics.push(HookDiagnostic {
            source,
            message: "expected an array of hooks, or an object with a hooks array".to_string(),
        });
        return Vec::new();
    };

    entries
        .iter()
        .filter_map(|entry| validate_entry(entry, &source, diagnostics))
        .collect()
}

/// Loads hooks from the given directories, in ascending precedence. Later
/// declarations are appended rather than replacing earlier ones: several hooks
/// on one event all run, which is what makes a project able to add to a user's
/// without taking it over.
pub fn load_hooks(dirs: &[PathBuf]) -> LoadHooksResult {
    let mut hooks: Vec<Hook> = Vec::new();
    let mut diagnostics: Vec<HookDiagnostic> = Vec::new();
    for dir in dirs {
        let file_path = dir.join(HOOKS_FILE_NAME);
        if !file_path.exists() {
            continue;
        }
        hooks.extend(load_file(&file_path, &mut diagnostics));
    }
    let mut first_source: HashMap<(HookEvent, Option<String>, String, u64), String> =
        HashMap::new();
    for hook in &hooks {
        let key = (
            hook.event,
            hook.matcher.clone(),
            hook.command.clone(),
            hook.timeout_ms,
        );
        if let Some(first) = first_source.get(&key) {
            diagnostics.push(HookDiagnostic {
                source: hook.source.clone(),
                message: format!(
                    "duplicate hook for {} also declared in {first}; both declarations will run",
                    hook.event
                ),
            });
        } else {
            first_source.insert(key, hook.source.clone());
        }
    }
    LoadHooksResult { hooks, diagnostics }
}

/// Hooks that apply to an event, narrowed by tool name where one is given.
pub fn select_hooks<'a>(
    hooks: &'a [Hook],
    event: HookEvent,
    tool_name: Option<&str>,
) -> Vec<&'a Hook> {
    hooks
        .iter()
        .filter(|hook| {
            if hook.event != event {
                return false;
            }
            match &hook.matcher {
                None => true,
                Some(matcher) => tool_name.is_some_and(|name| matches_tool(matcher, name)),
            }
        })
        .collect()
}

/// A matcher is a comma-separated list of tool names, or `*` for all of them.
/// Deliberately not a regular expression: a matcher is written once and read
/// often, and an accidental pattern that matches too much would widen what a
/// hook touches without anyone noticing.
pub fn matches_tool(matcher: &str, tool_name: &str) -> bool {
    matcher
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .any(|part| part == "*" || part == tool_name)
}

/// Whether a hook may refuse the call it is called for.
pub fn is_blocking_hook(hook: &Hook) -> bool {
    hook.event.is_blocking()
}
