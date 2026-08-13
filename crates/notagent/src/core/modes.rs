//! Port of `packages/coding-agent/src/core/modes/modes.ts`.
//!
//! Mode discovery and loading.
//!
//! A mode is a directory of skill files. The directory name is the mode id; the
//! files inside supply the behaviour that gets injected when the mode becomes
//! active. Frontmatter carries the hard attributes the runtime enforces (shell,
//! tool delta); the body is guidance for the model.
//!
//! Two levels are searched, matching the precedence compaction settings already
//! use: the user agent directory, then the project directory. A project mode of
//! the same name replaces the user one entirely, so a project can fully own a
//! mode rather than partially merging with it.

pub mod cycle;
pub mod indicator;
pub mod shells;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde_yaml_ng::Value;

use crate::config::get_package_dir;
use crate::core::tools::ToolName;
use crate::utils::frontmatter::{FrontmatterError, parse_frontmatter};
use shells::{ApprovalLevel, DEFAULT_APPROVAL_LEVEL, ShellId, apply_tool_delta};

/// Directory name holding mode folders under the agent and project dirs.
pub const MODES_DIR_NAME: &str = "modes";

/// The four shipped modes, embedded in the binary.
///
/// Deviation (class 4, distribution): TypeScript ships them as `.md` files next
/// to the compiled sources and reads them from disk; a Rust binary carries its
/// assets inside. They still travel the same loader — same frontmatter rules,
/// same diagnostics, same reported paths — so the defaults remain worked
/// examples rather than a second code path.
const BUILTIN_MODES: &[(&str, &[(&str, &str)])] = &[
    (
        "auto",
        &[("10-auto.md", include_str!("modes/builtin/auto/10-auto.md"))],
    ),
    (
        "manual",
        &[(
            "10-manual.md",
            include_str!("modes/builtin/manual/10-manual.md"),
        )],
    ),
    (
        "plan",
        &[("10-plan.md", include_str!("modes/builtin/plan/10-plan.md"))],
    ),
    (
        "yolo",
        &[("10-yolo.md", include_str!("modes/builtin/yolo/10-yolo.md"))],
    ),
];

/// Directory holding the shipped modes. They are ordinary mode folders loaded
/// through the same path as user-authored ones, so the defaults double as
/// worked examples.
pub fn get_builtin_modes_dir() -> PathBuf {
    get_package_dir().join(MODES_DIR_NAME).join("builtin")
}

/// One skill file inside a mode directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeSkill {
    /// File name, used for the deterministic alphabetical ordering.
    pub file_name: String,
    pub file_path: PathBuf,
    /// Body without frontmatter — this is what gets injected.
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mode {
    pub id: String,
    pub shell: ShellId,
    /// How much runs without the user once a tool is permitted by the shell.
    pub approval: ApprovalLevel,
    /// Effective tool allowlist after applying the mode's tool delta.
    pub tools: Vec<ToolName>,
    /// Modes this one may delegate to. `None` means any of them.
    ///
    /// A separate axis from the shell, and a narrower one. The shell already
    /// stops a read-only mode from delegating work that writes; this is for the
    /// case where every candidate is permitted but only some are appropriate — a
    /// review mode that should reach for an explorer and not for a builder.
    pub subagents: Option<Vec<String>>,
    pub skills: Vec<ModeSkill>,
    /// Directory the mode was loaded from; later sources win.
    pub source_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeDiagnostic {
    pub mode_id: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LoadModesResult {
    pub modes: Vec<Mode>,
    pub diagnostics: Vec<ModeDiagnostic>,
}

/// Frontmatter that cannot be parsed stops mode loading.
///
/// Deviation (class 1): `parseFrontmatter` throws and `loadModeDir` does not
/// catch, so a malformed mode file fails the load in TypeScript as well. The
/// throw becomes a `Result` rather than a panic, and it names the file.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{path}: {source}")]
pub struct ModeLoadError {
    pub path: PathBuf,
    #[source]
    pub source: FrontmatterError,
}

/// `String.prototype.localeCompare(a, b, "en")` for the ASCII file and mode
/// names in play: case-insensitive, with lowercase winning a tie.
fn locale_compare(a: &str, b: &str) -> std::cmp::Ordering {
    let folded = a.to_lowercase().cmp(&b.to_lowercase());
    if folded != std::cmp::Ordering::Equal {
        return folded;
    }
    b.cmp(a)
}

/// `Array.isArray(value) ? strings : typeof value === "string" ? [value] : undefined`.
fn read_optional_string_array(value: Option<&Value>) -> Option<Vec<String>> {
    match value? {
        Value::Sequence(entries) => Some(
            entries
                .iter()
                .filter_map(|entry| entry.as_str().map(str::to_string))
                .collect(),
        ),
        Value::String(value) => Some(vec![value.clone()]),
        _ => None,
    }
}

/// What a frontmatter value looked like, for the diagnostics that quote it.
///
/// `String(value)` in TypeScript: a string prints as itself, everything else in
/// its JavaScript spelling.
fn describe_value(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Null => "null".to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::Sequence(entries) => entries
            .iter()
            .map(describe_value)
            .collect::<Vec<_>>()
            .join(","),
        _ => "[object Object]".to_string(),
    }
}

/// Loads a single mode directory from files already read into memory. Skill
/// files are sorted by file name so numeric prefixes give authors explicit
/// control over injection order.
fn load_mode_entries(
    dir: &Path,
    mut entries: Vec<(String, Result<String, String>)>,
    known_tool_names: &HashSet<String>,
    diagnostics: &mut Vec<ModeDiagnostic>,
) -> Result<Option<Mode>, ModeLoadError> {
    let id = dir
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    if entries.is_empty() {
        diagnostics.push(ModeDiagnostic {
            mode_id: id,
            message: "mode directory contains no .md skill files".to_string(),
        });
        return Ok(None);
    }
    entries.sort_by(|(a, _), (b, _)| locale_compare(a, b));

    let mut skills: Vec<ModeSkill> = Vec::new();
    let mut shell: Option<ShellId> = None;
    let mut approval: Option<ApprovalLevel> = None;
    let mut subagents: Option<Vec<String>> = None;
    let mut tool_delta: Option<Vec<String>> = None;

    for (file_name, content) in entries {
        let file_path = dir.join(&file_name);
        let raw = match content {
            Ok(raw) => raw,
            Err(error) => {
                diagnostics.push(ModeDiagnostic {
                    mode_id: id.clone(),
                    message: format!("cannot read {file_name}: {error}"),
                });
                continue;
            }
        };
        let parsed = parse_frontmatter(&raw).map_err(|source| ModeLoadError {
            path: file_path.clone(),
            source,
        })?;

        // The first file that declares a shell wins; a second, different
        // declaration is a conflict the author must resolve.
        if let Some(declared) = parsed.get("shell") {
            match declared.as_str().and_then(ShellId::parse) {
                None => diagnostics.push(ModeDiagnostic {
                    mode_id: id.clone(),
                    message: format!("{file_name}: unknown shell \"{}\"", describe_value(declared)),
                }),
                Some(declared) => match shell {
                    None => shell = Some(declared),
                    Some(existing) if existing != declared => diagnostics.push(ModeDiagnostic {
                        mode_id: id.clone(),
                        message: format!(
                            "{file_name}: conflicting shell \"{declared}\", mode already declared \"{existing}\""
                        ),
                    }),
                    Some(_) => {}
                },
            }
        }

        // Same first-wins rule as the shell: a second, differing declaration is a
        // conflict the author must resolve rather than something to merge.
        if let Some(declared) = parsed.get("approval") {
            match declared.as_str().and_then(ApprovalLevel::parse) {
                None => diagnostics.push(ModeDiagnostic {
                    mode_id: id.clone(),
                    message: format!(
                        "{file_name}: unknown approval level \"{}\"",
                        describe_value(declared)
                    ),
                }),
                Some(declared) => match approval {
                    None => approval = Some(declared),
                    Some(existing) if existing != declared => diagnostics.push(ModeDiagnostic {
                        mode_id: id.clone(),
                        message: format!(
                            "{file_name}: conflicting approval \"{declared}\", mode already declared \"{existing}\""
                        ),
                    }),
                    Some(_) => {}
                },
            }
        }

        if let Some(delta) = read_optional_string_array(parsed.get("tools")) {
            tool_delta.get_or_insert_with(Vec::new).extend(delta);
        }

        // `*` means "any", spelled explicitly so a mode can state it rather than
        // leaving the reader to infer it from the field's absence.
        if let Some(declared) = read_optional_string_array(parsed.get("subagents"))
            && !(declared.len() == 1 && declared[0] == "*")
        {
            subagents.get_or_insert_with(Vec::new).extend(declared);
        }

        skills.push(ModeSkill {
            file_name,
            file_path,
            body: parsed.body.trim().to_string(),
        });
    }

    if skills.is_empty() {
        return Ok(None);
    }

    // A mode without an explicit shell is treated as read-only: an undefined
    // permission state must never default to the permissive side.
    let shell = shell.unwrap_or_else(|| {
        diagnostics.push(ModeDiagnostic {
            mode_id: id.clone(),
            message: "no shell declared, defaulting to read-only".to_string(),
        });
        ShellId::ReadOnly
    });

    let delta = apply_tool_delta(shell, tool_delta.as_deref(), known_tool_names);
    for problem in delta.problems {
        diagnostics.push(ModeDiagnostic {
            mode_id: id.clone(),
            message: problem,
        });
    }

    Ok(Some(Mode {
        id,
        shell,
        approval: approval.unwrap_or(DEFAULT_APPROVAL_LEVEL),
        tools: delta.tools,
        subagents: subagents.map(deduplicate),
        skills,
        source_dir: dir.to_path_buf(),
    }))
}

/// `[...new Set(values)]`.
fn deduplicate(values: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    values
        .into_iter()
        .filter(|value| seen.insert(value.clone()))
        .collect()
}

fn read_mode_dir(
    dir: &Path,
    known_tool_names: &HashSet<String>,
    diagnostics: &mut Vec<ModeDiagnostic>,
) -> Result<Option<Mode>, ModeLoadError> {
    let id = dir
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let read_dir = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => {
            diagnostics.push(ModeDiagnostic {
                mode_id: id,
                message: format!("cannot read mode directory: {error}"),
            });
            return Ok(None);
        }
    };

    let mut entries: Vec<(String, Result<String, String>)> = Vec::new();
    for entry in read_dir.flatten() {
        let file_name = entry.file_name().to_string_lossy().into_owned();
        if !file_name.to_lowercase().ends_with(".md") {
            continue;
        }
        let content = std::fs::read_to_string(entry.path()).map_err(|error| error.to_string());
        entries.push((file_name, content));
    }

    load_mode_entries(dir, entries, known_tool_names, diagnostics)
}

/// Loads modes from the given directories, each of which directly contains mode
/// folders. Precedence is ascending: a mode found in a later directory replaces
/// one of the same id from an earlier one.
pub fn load_modes(
    roots: &[PathBuf],
    known_tool_names: &HashSet<String>,
) -> Result<LoadModesResult, ModeLoadError> {
    let builtin_dir = get_builtin_modes_dir();
    // Insertion-ordered, like the `Map` this mirrors: a replacement keeps the
    // position of the entry it replaces.
    let mut by_id: Vec<(String, Mode)> = Vec::new();
    let mut diagnostics: Vec<ModeDiagnostic> = Vec::new();

    let record = |mode: Mode, by_id: &mut Vec<(String, Mode)>| match by_id
        .iter_mut()
        .find(|(id, _)| *id == mode.id)
    {
        Some((_, existing)) => *existing = mode,
        None => by_id.push((mode.id.clone(), mode)),
    };

    for modes_dir in roots {
        if *modes_dir == builtin_dir {
            for (id, files) in BUILTIN_MODES {
                let entries = files
                    .iter()
                    .map(|(name, content)| ((*name).to_string(), Ok((*content).to_string())))
                    .collect();
                if let Some(mode) = load_mode_entries(
                    &builtin_dir.join(id),
                    entries,
                    known_tool_names,
                    &mut diagnostics,
                )? {
                    record(mode, &mut by_id);
                }
            }
            continue;
        }

        if !modes_dir.exists() {
            continue;
        }
        let read_dir = match std::fs::read_dir(modes_dir) {
            Ok(entries) => entries,
            Err(error) => {
                diagnostics.push(ModeDiagnostic {
                    mode_id: "*".to_string(),
                    message: format!("cannot read {}: {error}", modes_dir.display()),
                });
                continue;
            }
        };
        let mut names: Vec<String> = read_dir
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort_by(|a, b| locale_compare(a, b));

        for name in names {
            let dir = modes_dir.join(name);
            if !dir.is_dir() {
                continue;
            }
            if let Some(mode) = read_mode_dir(&dir, known_tool_names, &mut diagnostics)? {
                record(mode, &mut by_id);
            }
        }
    }

    let mut modes: Vec<Mode> = by_id.into_iter().map(|(_, mode)| mode).collect();
    modes.sort_by(|a, b| locale_compare(&a.id, &b.id));
    Ok(LoadModesResult { modes, diagnostics })
}

/// Concatenates a mode's skill bodies into the text injected on activation.
pub fn render_mode_injection(mode: &Mode) -> String {
    mode.skills
        .iter()
        .map(|skill| skill.body.as_str())
        .filter(|body| !body.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// The delimited block that activates a mode.
///
/// One function for both callers: the main agent prepends it to the next user
/// message on a switch, and a delegated child receives it ahead of its task.
/// Two renderings would mean a child could be told something subtly different
/// from what the same mode tells the agent that spawned it.
///
/// `note` carries anything that has to be said about the transition rather than
/// about the mode itself. Returns `None` when there would be nothing inside
/// the wrapper, since an empty block is noise the model has to parse for no
/// gain.
pub fn render_mode_block(mode: &Mode, note: Option<&str>) -> Option<String> {
    let body = render_mode_injection(mode);
    let prefix = note.map_or_else(String::new, |note| format!("{note}\n\n"));
    if body.trim().is_empty() && prefix.is_empty() {
        return None;
    }
    Some(format!(
        "<mode name=\"{}\" shell=\"{}\">\n{prefix}{body}\n</mode>",
        mode.id, mode.shell
    ))
}
