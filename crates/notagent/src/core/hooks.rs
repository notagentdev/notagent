//! Port of `packages/coding-agent/src/core/hooks/hooks.ts`.
//!
//! Hook declarations: what a user writes, and what survives validation.
//!
//! A hook is an event, an optional matcher, a shell command and an optional
//! timeout — the reference shape, so an existing hook file ports over
//! unchanged. Declarations are read from the user agent directory and the
//! project directory, the same two-level precedence modes already use.
//!
//! Everything rejected here is reported rather than dropped. A hook that
//! silently does not run is worse than one that never existed, because the
//! author believes it is in force.

pub mod dispatch;
pub mod events;
pub mod payload;
pub mod runner;
pub mod runtime;

use std::path::{Path, PathBuf};

use serde_json::Value;

use events::{BLOCKING_EVENT, HookEvent, hook_event_list};

/// File holding hook declarations, in the agent and project directories.
pub const HOOKS_FILE_NAME: &str = "hooks.json";

/// Default ceiling for a hook, so an omitted timeout is still bounded.
pub const DEFAULT_HOOK_TIMEOUT_MS: f64 = 30_000.0;

/// Upper bound a declaration may not exceed, however it is written.
pub const MAX_HOOK_TIMEOUT_MS: f64 = 300_000.0;

#[derive(Debug, Clone, PartialEq)]
pub struct Hook {
    pub event: HookEvent,
    /// Restricts a tool-scoped hook to matching tool names.
    pub matcher: Option<String>,
    pub command: String,
    pub timeout_ms: f64,
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
/// quotes what was written the way the TypeScript does.
fn describe_value(value: Option<&Value>) -> String {
    match value {
        None => "undefined".to_string(),
        Some(Value::String(value)) => value.clone(),
        Some(Value::Null) => "null".to_string(),
        Some(value) => value.to_string(),
    }
}

/// `Number(value)`: the coercion JavaScript applies before the range checks.
fn js_number(value: &Value) -> f64 {
    match value {
        Value::Number(number) => number.as_f64().unwrap_or(f64::NAN),
        Value::Bool(true) => 1.0,
        Value::Bool(false) | Value::Null => 0.0,
        Value::String(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                return 0.0;
            }
            trimmed.parse::<f64>().unwrap_or(f64::NAN)
        }
        _ => f64::NAN,
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

    let mut matcher: Option<String> = None;
    if let Some(declared) = entry.get("matcher") {
        match declared.as_str() {
            Some(value) if !value.trim().is_empty() => {
                if event.is_tool_scoped() {
                    matcher = Some(value.trim().to_string());
                } else {
                    // A matcher on an event that carries no tool name would never apply,
                    // so saying so beats letting the hook quietly never run.
                    diagnostics.push(HookDiagnostic {
                        source: source.to_string(),
                        message: format!(
                            "hook for {event} declares a matcher, but only tool events carry a tool name"
                        ),
                    });
                }
            }
            _ => diagnostics.push(HookDiagnostic {
                source: source.to_string(),
                message: format!("hook for {event} has an empty matcher"),
            }),
        }
    }

    let mut timeout_ms = DEFAULT_HOOK_TIMEOUT_MS;
    if let Some(declared) = entry.get("timeout") {
        let requested = js_number(declared);
        if !requested.is_finite() || requested <= 0.0 {
            diagnostics.push(HookDiagnostic {
                source: source.to_string(),
                message: format!("hook for {event} has an invalid timeout"),
            });
        } else if requested > MAX_HOOK_TIMEOUT_MS {
            diagnostics.push(HookDiagnostic {
                source: source.to_string(),
                message: format!(
                    "hook for {event} requests {requested}ms, capped at {MAX_HOOK_TIMEOUT_MS}ms"
                ),
            });
            timeout_ms = MAX_HOOK_TIMEOUT_MS;
        } else {
            timeout_ms = requested;
        }
    }

    if event.is_unemitted() {
        diagnostics.push(HookDiagnostic {
            source: source.to_string(),
            message: format!(
                "hook for {event} is accepted but never fires yet: the feature that raises it does not exist"
            ),
        });
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
    hook.event == BLOCKING_EVENT
}
