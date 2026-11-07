use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use notagent_agent::types::{
    AgentTool, AgentToolResult, AgentToolUpdateCallback, BoxFuture, ToolExecutionError,
};
use notagent_ai::types::{ConstrainedSampling, TextContent, TextOrImageContent};
use notagent_tui::components::text::Text;
use notagent_tui::tui::{ComponentRef, Container};
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use crate::core::experimental::get_experimental_tool_sampling;
use crate::core::mini_read::apply_minified_edit;
use crate::core::snapshots::{SnapshotStore, capture_before_mutation};
use crate::core::tools::edit_diff::{generate_diff_string, generate_unified_patch, strip_bom};
use crate::core::tools::file_lease::{LeaseCoordinator, LeaseGate};
use crate::core::tools::file_mutation_queue::{shares_queue, with_file_mutation_queues};
use crate::core::tools::path_utils::resolve_to_cwd;
use crate::core::tools::render_utils::{call_title, render_mutation_tool_path, str_arg};
use crate::core::tools::tool_definition::{
    ToolContext, ToolDefinition, ToolRenderContext, ToolRenderResult, ToolRenderResultOptions,
    tool_render_state, wrap_tool_definition,
};
use crate::modes::interactive::components::diff::{RenderDiffOptions, diff_info_line, render_diff};
use crate::modes::interactive::theme::theme::{Theme, block_style};

fn minified_edit_properties() -> Value {
    json!({
        "type": "object",
        "properties": {
            "old_string": { "type": "string", "description": "Text to find in the minified view of the file." },
            "new_string": { "type": "string", "description": "Replacement text." },
            "replace_all": {
                "type": "boolean",
                "description": "Replace every occurrence instead of requiring a unique match.",
            },
        },
        "required": ["old_string", "new_string"],
    })
}

fn patch_minified_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "Path to the file to edit (relative or absolute)" },
            "old_string": { "type": "string", "description": "Text to find in the minified view of the file." },
            "new_string": { "type": "string", "description": "Replacement text." },
            "replace_all": {
                "type": "boolean",
                "description": "Replace every occurrence instead of requiring a unique match.",
            },
            "keep_comments": {
                "type": "boolean",
                "description": "Whether comments participate in the minified view. Must match the view used to choose the edit.",
            },
        },
        "required": ["path", "old_string", "new_string"],
    })
}

fn multi_patch_minified_edits() -> Value {
    json!({
        "type": "array",
        "items": minified_edit_properties(),
        "description": "Edits applied in order. Each one operates on the result of the previous edit, against a freshly rebuilt minified view.",
    })
}

fn multi_patch_minified_keep_comments() -> Value {
    json!({
        "type": "boolean",
        "description": "Whether comments participate in the minified view. Must match the view used to choose the edits.",
    })
}

/// One file as a call names it: either the whole call (`path` and `edits`) or
/// one entry of `files`. Neither form is required on its own, so the schema
/// requires nothing at the top and `execute` checks that exactly one is given.
fn multi_patch_minified_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "Path to the file to edit (relative or absolute). For one file; use files to edit several." },
            "edits": multi_patch_minified_edits(),
            "keep_comments": multi_patch_minified_keep_comments(),
            "files": {
                "type": "array",
                "description": "Several files edited in one call, instead of path and edits. Preferred whenever more than one file changes: batch as many files per call as possible. All or nothing: if any edit in any file fails, no file is changed.",
                "items": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Path to the file to edit (relative or absolute)" },
                        "edits": multi_patch_minified_edits(),
                        "keep_comments": multi_patch_minified_keep_comments(),
                    },
                    "required": ["path", "edits"],
                },
            },
        },
    })
}

/// The `path` of every entry in a call's `files` argument. Shared with
/// everything that reasons about which files a call touches — permissions,
/// compaction summaries, the session tree — so none of them sees only the
/// first file of a multi-file call.
pub fn files_argument_paths(arguments: &Map<String, Value>) -> Vec<&str> {
    arguments
        .get("files")
        .and_then(Value::as_array)
        .map(|files| {
            files
                .iter()
                .filter_map(|file| {
                    file.get("path")
                        .or_else(|| file.get("file_path"))
                        .and_then(Value::as_str)
                })
                .filter(|path| !path.trim().is_empty())
                .collect()
        })
        .unwrap_or_default()
}

const SHARED_USAGE: &[&str] = &[
    "- When minified editing tools are attached, you must use them for edits to existing source files; this is mandatory, not a preference. Use patch_minified for one replacement in one file, and multi_patch_minified for everything else when attached: several replacements in one file, and edits to several files, which belong together in one call (files) rather than one call per file. Do not bypass them with plain patch, write, or shell commands.",
    "- If a minified edit fails because the source changed or the match is ambiguous, re-read with read and correct the match before considering a fallback. Plain patch is a fallback only when a concrete limitation or failure still prevents a safe minified edit; state the reason first.",
    "- Read the file with `read` first when attached; copying old_string from that output is the most reliable path.",
    "- old_string and new_string may be given in compact `read` form or in normal source formatting; if an exact match fails, the tool normalizes them into minified form and retries exact matching.",
    "- This normalization is NOT fuzzy matching: the final search must still match the rebuilt minified view exactly, and ambiguous matches still fail unless replace_all is set.",
    "- keep_comments controls whether comments participate in the minified view and must match the view you used to choose the edit.",
    "- The edit FAILS if the final search text is not unique; add surrounding context or use replace_all.",
    "- Comments hidden by minification that lie inside the replaced range are removed along with it — the result reports a warning when that happens.",
    "- Unsupported languages fall back to exact replacement on the raw file content.",
];

fn description(lines: &[&str]) -> String {
    lines
        .iter()
        .copied()
        .chain(SHARED_USAGE.iter().copied())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Pluggable operations, mirroring `EditOperations` in `edit.rs`.
pub trait PatchMinifiedOperations: Send + Sync {
    fn read_file<'a>(&'a self, absolute_path: &'a str) -> BoxFuture<'a, Result<Vec<u8>, String>>;
    fn write_file<'a>(
        &'a self,
        absolute_path: &'a str,
        content: &'a str,
    ) -> BoxFuture<'a, Result<(), String>>;
    /// `access(path, R_OK | W_OK)`.
    fn access<'a>(&'a self, absolute_path: &'a str) -> BoxFuture<'a, Result<(), String>>;
}

pub struct LocalPatchMinifiedOperations;

impl PatchMinifiedOperations for LocalPatchMinifiedOperations {
    fn read_file<'a>(&'a self, absolute_path: &'a str) -> BoxFuture<'a, Result<Vec<u8>, String>> {
        Box::pin(async move {
            tokio::fs::read(absolute_path)
                .await
                .map_err(|error| error.to_string())
        })
    }

    fn write_file<'a>(
        &'a self,
        absolute_path: &'a str,
        content: &'a str,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            tokio::fs::write(absolute_path, content)
                .await
                .map_err(|error| error.to_string())
        })
    }

    fn access<'a>(&'a self, absolute_path: &'a str) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let metadata = tokio::fs::metadata(absolute_path)
                .await
                .map_err(|error| error_code(&error))?;
            if metadata.permissions().readonly() {
                return Err("Error code: EACCES".to_owned());
            }
            tokio::fs::File::open(absolute_path)
                .await
                .map(|_| ())
                .map_err(|error| error_code(&error))
        })
    }
}

fn error_code(error: &std::io::Error) -> String {
    let code = match error.kind() {
        std::io::ErrorKind::NotFound => "ENOENT",
        std::io::ErrorKind::PermissionDenied => "EACCES",
        std::io::ErrorKind::IsADirectory => "EISDIR",
        _ => return error.to_string(),
    };
    format!("Error code: {code}")
}

#[derive(Clone, Default)]
pub struct PatchMinifiedToolOptions {
    pub operations: Option<Arc<dyn PatchMinifiedOperations>>,
    /// Whether atomic file leases are enabled; absent means disabled.
    pub leases: Option<LeaseGate>,
    /// Where a copy of the file goes before it is patched, so `undo` can put it
    /// back. Absent means no snapshot is taken.
    pub snapshots: Option<SnapshotStore>,
}

/// How long a minified patch expects to hold its lease (the reference's
/// `fs_patch`, of which these tools are the minified-view counterpart).
const PATCH_MINIFIED_LEASE_DURATION_MS: u64 = 30_000;

#[derive(Debug, Clone, PartialEq, Eq)]
struct MinifiedEditRequest {
    old_string: String,
    new_string: String,
    replace_all: bool,
}

/// Plain exact replacement on the raw content, used when the language is not
/// supported by the minifier. Mirrors the reference fallback, which routes an
/// unsupported language through the ordinary Replace/ReplaceAll operation.
fn apply_raw_replace(
    content: &str,
    search: &str,
    replacement: &str,
    replace_all: bool,
) -> Result<String, String> {
    if search.is_empty() {
        return Err("search text must not be empty".to_owned());
    }
    let occurrences: Vec<usize> = content.match_indices(search).map(|(at, _)| at).collect();
    if occurrences.is_empty() {
        return Err(format!(
            "Could not find match for search text: '{search}' in the file."
        ));
    }
    if occurrences.len() > 1 && !replace_all {
        return Err(format!(
            "Multiple matches found for search text: '{search}'. Either provide a more specific search pattern or use replace_all to replace all occurrences."
        ));
    }
    let mut updated = content.to_owned();
    for at in occurrences.iter().rev() {
        updated.replace_range(*at..at + search.len(), replacement);
    }
    Ok(updated)
}

/// Applies a single replace expressed in minified space, falling back to a plain
/// exact replace on the raw content when the language is unsupported by the
/// minifier (mirroring `read`'s raw fallback).
fn apply_minified_replace(
    path: &str,
    current_content: &str,
    edit: &MinifiedEditRequest,
    keep_comments: bool,
) -> Result<(String, Vec<String>), String> {
    match apply_minified_edit(
        std::path::Path::new(path),
        current_content,
        &edit.old_string,
        &edit.new_string,
        edit.replace_all,
        keep_comments,
    ) {
        Some(result) => result.map(|edit| (edit.content, edit.warnings)),
        None => apply_raw_replace(
            current_content,
            &edit.old_string,
            &edit.new_string,
            edit.replace_all,
        )
        .map(|content| (content, Vec::new())),
    }
}

/// One file's part of a call.
struct FileEdits {
    path: String,
    edits: Vec<MinifiedEditRequest>,
    keep_comments: bool,
}

/// A file whose edits applied in memory and that waits for the write.
struct PreparedFile {
    path: String,
    absolute_path: String,
    bom: String,
    original: String,
    updated: String,
    warnings: Vec<String>,
    edit_count: usize,
}

/// The diff, patch and first changed line of one prepared file, as details.
fn file_details(file: &PreparedFile) -> Map<String, Value> {
    let diff_result = generate_diff_string(&file.original, &file.updated, 4);
    let patch = generate_unified_patch(&file.path, &file.original, &file.updated, 4);
    let mut details = Map::new();
    details.insert("diff".to_owned(), Value::from(diff_result.diff));
    details.insert("patch".to_owned(), Value::from(patch));
    if let Some(first_changed_line) = diff_result.first_changed_line {
        details.insert("firstChangedLine".to_owned(), json!(first_changed_line));
    }
    details
}

fn with_warnings(summary: String, warnings: &[String]) -> String {
    if warnings.is_empty() {
        return summary;
    }
    format!(
        "{summary}\n\nWarnings:\n{}",
        warnings
            .iter()
            .map(|warning| format!("- {warning}"))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

/// The result of a call. One file keeps the shape single-file calls always
/// had; several files carry one details entry each under `files`.
fn edits_result(prepared: &[PreparedFile]) -> AgentToolResult {
    let (text, details) = match prepared {
        [file] => {
            let mut details = file_details(file);
            details.insert("warnings".to_owned(), json!(file.warnings));
            let summary = format!(
                "Successfully applied {} minified edit(s) to {}.",
                file.edit_count, file.path
            );
            (with_warnings(summary, &file.warnings), details)
        }
        _ => {
            let warnings: Vec<String> = prepared
                .iter()
                .flat_map(|file| {
                    file.warnings
                        .iter()
                        .map(move |warning| format!("{}: {warning}", file.path))
                })
                .collect();
            let summary = format!(
                "Successfully applied {} minified edit(s) to {} files: {}.",
                prepared.iter().map(|file| file.edit_count).sum::<usize>(),
                prepared.len(),
                prepared
                    .iter()
                    .map(|file| file.path.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            let files = prepared
                .iter()
                .map(|file| {
                    let mut entry = file_details(file);
                    entry.insert("path".to_owned(), Value::from(file.path.clone()));
                    entry.insert("warnings".to_owned(), json!(file.warnings));
                    Value::Object(entry)
                })
                .collect::<Vec<_>>();
            let mut details = Map::new();
            details.insert("files".to_owned(), Value::Array(files));
            details.insert("warnings".to_owned(), json!(warnings));
            (with_warnings(summary, &warnings), details)
        }
    };
    AgentToolResult {
        content: vec![TextOrImageContent::Text(TextContent::new(text))],
        details: Some(Value::Object(details)),
        usage: None,
        added_tool_names: None,
        terminate: None,
    }
}

/// The shared write path of `patch_minified` and `multi_patch_minified`, for
/// one file or several.
/// Several files are all or nothing. Every file is locked, read and edited in
/// memory before the first write, so a match that fails anywhere changes
/// nothing. A write that fails part way puts the files already written back to
/// what they held; a file system offers no transaction across files, and this
/// is the closest it gets.
/// Takes the tool it runs for rather than its pieces: it needs the cwd, the
/// operations, the lease coordinator and the tool name, which is the tool
/// itself.
async fn run_edits(
    tool: &PatchMinifiedToolDefinition,
    files: Vec<FileEdits>,
    signal: Option<CancellationToken>,
) -> Result<AgentToolResult, ToolExecutionError> {
    let cwd = tool.cwd.as_str();
    let operations = tool.operations.as_ref();
    let leases = &tool.leases;
    let snapshots = tool.snapshots.clone();
    let tool_name = tool.name();
    let several = files.len() > 1;

    if files.is_empty() {
        return Err(ToolExecutionError::new(
            "Patch tool input is invalid. files must contain at least one file.",
        ));
    }
    if let Some(file) = files.iter().find(|file| file.edits.is_empty()) {
        return Err(ToolExecutionError::new(if several {
            format!(
                "Patch tool input is invalid. edits for {} must contain at least one replacement.",
                file.path
            )
        } else {
            "Patch tool input is invalid. edits must contain at least one replacement.".to_owned()
        }));
    }
    let absolute_paths: Vec<String> = files
        .iter()
        .map(|file| resolve_to_cwd(&file.path, cwd))
        .collect();
    for (index, absolute) in absolute_paths.iter().enumerate() {
        if let Some(offset) = absolute_paths[index + 1..]
            .iter()
            .position(|other| shares_queue(absolute, other))
        {
            return Err(ToolExecutionError::new(format!(
                "Patch tool input is invalid. {} and {} are the same file; put all of its edits into one entry of files.",
                files[index].path,
                files[index + 1 + offset].path
            )));
        }
    }

    with_file_mutation_queues(&absolute_paths, async {
        // Do not abort from a listener: that would release the mutation queue
        // while a filesystem call is still in flight.
        let throw_if_aborted = || -> Result<(), ToolExecutionError> {
            match &signal {
                Some(signal) if signal.is_cancelled() => {
                    Err(ToolExecutionError::new("Operation aborted"))
                }
                _ => Ok(()),
            }
        };
        throw_if_aborted()?;

        // Every lease taken so far, released again if anything fails.
        let mut reserved: Vec<crate::core::tools::file_lease::FileLease> = Vec::new();
        let result = async {
            let mut prepared: Vec<PreparedFile> = Vec::with_capacity(files.len());
            for (file, absolute_path) in files.iter().zip(&absolute_paths) {
                let path = file.path.as_str();
                // Reserved inside the mutation queue, which already serializes
                // this process: a lease conflict therefore always names a
                // foreign holder, never a second call of our own. `None` when
                // leases are off, and every lease step below is then a no-op.
                if let Some(lease) = leases
                    .reserve(Path::new(absolute_path), tool_name, PATCH_MINIFIED_LEASE_DURATION_MS)
                    .await
                    .map_err(|error| ToolExecutionError::new(error.to_string()))?
                {
                    reserved.push(lease);
                }
                throw_if_aborted()?;

                if let Err(error) = operations.access(absolute_path).await {
                    throw_if_aborted()?;
                    return Err(ToolExecutionError::new(format!(
                        "Could not edit file: {path}. {error}."
                    )));
                }
                throw_if_aborted()?;

                let buffer = operations
                    .read_file(absolute_path)
                    .await
                    .map_err(ToolExecutionError::new)?;
                // The whole file is written back, so a lossy decode would
                // replace every invalid byte in it, not only the ones the edit
                // touched.
                let raw_content = String::from_utf8(buffer).map_err(|_| {
                    ToolExecutionError::new(format!(
                        "Could not edit file: {path}. The file is not valid UTF-8 \
                         (binary or unsupported encoding); editing it would corrupt \
                         bytes outside the edited range."
                    ))
                })?;
                throw_if_aborted()?;

                // Strip the BOM before matching: the model will not include an
                // invisible BOM in old_string. It is restored verbatim on write.
                let (bom, original) = strip_bom(&raw_content);

                // Apply every edit sequentially against the result of the
                // previous one, re-minifying per edit because each splice
                // invalidates the source map.
                let mut current = original.to_owned();
                let mut warnings: Vec<String> = Vec::new();
                for edit in &file.edits {
                    let (content, edit_warnings) =
                        apply_minified_replace(path, &current, edit, file.keep_comments)
                            .map_err(|error| {
                                ToolExecutionError::new(if several {
                                    format!("{path}: {error}")
                                } else {
                                    error
                                })
                            })?;
                    current = content;
                    warnings.extend(edit_warnings);
                    throw_if_aborted()?;
                }
                prepared.push(PreparedFile {
                    path: file.path.clone(),
                    absolute_path: absolute_path.clone(),
                    bom: bom.to_owned(),
                    original: original.to_owned(),
                    updated: current,
                    warnings,
                    edit_count: file.edits.len(),
                });
            }

            // Always: a patch only ever applies to a file that is already
            // there, so there is always an earlier state to return to.
            for file in &prepared {
                capture_before_mutation(snapshots.as_ref(), Path::new(&file.absolute_path))
                    .await
                    .map_err(ToolExecutionError::new)?;
            }
            throw_if_aborted()?;

            // The pre-commit check runs immediately before the writes: each
            // lease must still be ours and each file must still hold the
            // content its edits were minified against.
            let mut committed = Vec::with_capacity(reserved.len());
            for lease in &reserved {
                committed.push(
                    leases
                        .prepare_commit(lease.clone())
                        .await
                        .map_err(|error| ToolExecutionError::new(error.to_string()))?,
                );
            }

            for (written, file) in prepared.iter().enumerate() {
                let write = operations
                    .write_file(
                        &file.absolute_path,
                        &format!("{}{}", file.bom, file.updated),
                    )
                    .await;
                if let Err(error) = write {
                    let mut message = format!("Could not write {}: {error}", file.path);
                    let mut unrestored = Vec::new();
                    for done in &prepared[..written] {
                        let restored = operations
                            .write_file(
                                &done.absolute_path,
                                &format!("{}{}", done.bom, done.original),
                            )
                            .await;
                        if let Err(restore_error) = restored {
                            unrestored.push(format!(
                                "Could not put {} back after that failure: {restore_error}. It holds the edited content.",
                                done.path
                            ));
                        }
                    }
                    if written > 0 && unrestored.is_empty() {
                        message.push_str(
                            "\nThe files already written were put back; no file is changed.",
                        );
                    }
                    for line in unrestored {
                        message.push('\n');
                        message.push_str(&line);
                    }
                    return Err(ToolExecutionError::new(message));
                }
            }

            for lease in &committed {
                leases
                    .release(lease)
                    .await
                    .map_err(|error| ToolExecutionError::new(error.to_string()))?;
            }
            throw_if_aborted()?;

            Ok(edits_result(&prepared))
        }
        .await;

        if result.is_err() {
            for lease in &reserved {
                let _ = leases.release(lease).await;
            }
        }
        result
    })
    .await
}

fn edit_request(value: &Value) -> MinifiedEditRequest {
    MinifiedEditRequest {
        old_string: value
            .get("old_string")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        new_string: value
            .get("new_string")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        replace_all: value
            .get("replace_all")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }
}

fn string_param(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn bool_param(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn edits_param(value: &Value) -> Vec<MinifiedEditRequest> {
    value
        .get("edits")
        .and_then(Value::as_array)
        .map(|edits| edits.iter().map(edit_request).collect())
        .unwrap_or_default()
}

/// The files a `multi_patch_minified` call names: the entries of `files`, or
/// the call itself when it uses the one-file form. A `keep_comments` on the
/// call applies to every entry that does not set its own.
fn multi_file_edits(params: &Value) -> Result<Vec<FileEdits>, ToolExecutionError> {
    let Some(entries) = params.get("files").filter(|files| !files.is_null()) else {
        return Ok(vec![FileEdits {
            path: string_param(params, "path"),
            edits: edits_param(params),
            keep_comments: bool_param(params, "keep_comments"),
        }]);
    };
    let Some(entries) = entries.as_array() else {
        return Err(ToolExecutionError::new(
            "Patch tool input is invalid. files must be an array of {path, edits}.",
        ));
    };
    if !string_param(params, "path").is_empty() || params.get("edits").is_some() {
        return Err(ToolExecutionError::new(
            "Patch tool input is invalid. Give either files, or path with edits, not both.",
        ));
    }
    let default_keep_comments = bool_param(params, "keep_comments");
    Ok(entries
        .iter()
        .map(|entry| FileEdits {
            path: string_param(entry, "path"),
            edits: edits_param(entry),
            keep_comments: entry
                .get("keep_comments")
                .and_then(Value::as_bool)
                .unwrap_or(default_keep_comments),
        })
        .collect())
}

pub struct PatchMinifiedToolDefinition {
    cwd: String,
    operations: Arc<dyn PatchMinifiedOperations>,
    leases: LeaseCoordinator,
    snapshots: Option<SnapshotStore>,
    multi: bool,
    description: String,
    parameters: Value,
    constrained_sampling: Option<ConstrainedSampling>,
}

pub fn create_patch_minified_tool_definition(
    cwd: &str,
    options: Option<PatchMinifiedToolOptions>,
) -> PatchMinifiedToolDefinition {
    let options = options.unwrap_or_default();
    PatchMinifiedToolDefinition {
        cwd: cwd.to_owned(),
        operations: options
            .operations
            .unwrap_or_else(|| Arc::new(LocalPatchMinifiedOperations)),
        leases: LeaseCoordinator::new(cwd, options.leases),
        snapshots: options.snapshots,
        multi: false,
        description: description(&[
            "Performs exact string replacements in the minified view of a file — the precise editing counterpart of `read`.",
            "",
            "Match old_string against the minified view produced by `read` (compact indentation, comments omitted unless keep_comments=true). The tool reads the current file from disk, rebuilds the minified view and its source map, maps the matched minified range back to the original source, and splices the replacement into the real file. Untouched source stays identical, including comments and formatting outside the replaced range, while new_string is expanded back to the file's indentation style.",
            "",
            "Usage:",
        ]),
        parameters: patch_minified_schema(),
        constrained_sampling: get_experimental_tool_sampling(),
    }
}

pub fn create_multi_patch_minified_tool_definition(
    cwd: &str,
    options: Option<PatchMinifiedToolOptions>,
) -> PatchMinifiedToolDefinition {
    let options = options.unwrap_or_default();
    PatchMinifiedToolDefinition {
        cwd: cwd.to_owned(),
        operations: options
            .operations
            .unwrap_or_else(|| Arc::new(LocalPatchMinifiedOperations)),
        leases: LeaseCoordinator::new(cwd, options.leases),
        snapshots: options.snapshots,
        multi: true,
        description: description(&[
            "Performs multiple sequential precise edits in the minified view — the multi-edit counterpart of `patch_minified`. Prefer this over several `patch_minified` calls on the same file.",
            "",
            "Edit as many files as possible per call. Whenever you have edits for more than one file ready, put them all into one call with files: [{path, edits, keep_comments?}] instead of making one call per file — every extra call costs a round trip and splits one change into pieces that can half-succeed. Use path and edits only when a single file is all you are changing. Each file appears once in files; put all of its edits into that entry.",
            "",
            "All rules of `patch_minified` apply to each edit. Each edit operates on the result of the previous one in the same file: the tool re-minifies between edits so every next match uses a fresh source map. The call is atomic — if any edit in any file fails, no file is changed.",
            "",
            "Usage:",
        ]),
        parameters: multi_patch_minified_schema(),
        constrained_sampling: get_experimental_tool_sampling(),
    }
}

// ============================================================================
// Rendering
// ============================================================================

/// The row state of a patch call.
/// The result is a `Text` when there is something to show and an empty
/// `Container` otherwise; see [`crate::core::tools::write`] for why the two
#[derive(Default)]
struct PatchRenderState {
    call: Option<Rc<RefCell<Text>>>,
    result_text: Option<Rc<RefCell<Text>>>,
    result_container: Option<Rc<RefCell<Container>>>,
}

/// `str(args?.file_path ?? args?.path)`
fn patch_path_arg(args: &Value) -> Option<String> {
    let raw = args
        .get("file_path")
        .filter(|value| !value.is_null())
        .or_else(|| args.get("path"));
    str_arg(raw)
}

fn format_patch_call(label: &str, args: &Value, theme: &Theme, cwd: &str) -> String {
    let files = args
        .as_object()
        .map(files_argument_paths)
        .unwrap_or_default();
    let path_display = if files.is_empty() {
        render_mutation_tool_path(patch_path_arg(args).as_deref(), theme, cwd, None)
    } else {
        files
            .iter()
            .map(|path| render_mutation_tool_path(Some(path), theme, cwd, None))
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!("{}{path_display}", call_title(theme, label))
}

/// The diff sections of a result: one per file, each under its path when the
/// call edited several.
fn diff_sections(
    details: Option<&Value>,
    raw_path: Option<String>,
    theme: &Theme,
    cwd: &str,
) -> Vec<String> {
    fn push_diff(sections: &mut Vec<String>, diff: &str, file_path: Option<String>) {
        sections.push(render_diff(diff, &RenderDiffOptions { file_path }));
        // The badge style closes the diff with its summary; the standard
        if block_style().is_compact()
            && let Some(info) = diff_info_line(diff)
        {
            sections.push(info);
        }
    }
    fn non_empty_diff(value: &Value) -> Option<&str> {
        value
            .get("diff")
            .and_then(Value::as_str)
            .filter(|diff| !diff.is_empty())
    }

    let mut sections = Vec::new();
    let Some(details) = details else {
        return sections;
    };
    match details.get("files").and_then(Value::as_array) {
        Some(files) => {
            for file in files {
                let path = file.get("path").and_then(Value::as_str).map(str::to_owned);
                let Some(diff) = non_empty_diff(file) else {
                    continue;
                };
                sections.push(render_mutation_tool_path(path.as_deref(), theme, cwd, None));
                push_diff(&mut sections, diff, path);
            }
        }
        None => {
            if let Some(diff) = non_empty_diff(details) {
                push_diff(&mut sections, diff, raw_path);
            }
        }
    }
    sections
}

impl ToolDefinition for PatchMinifiedToolDefinition {
    fn name(&self) -> &str {
        if self.multi {
            "multi_patch_minified"
        } else {
            "patch_minified"
        }
    }

    fn label(&self) -> &str {
        if self.multi {
            "multi patch minified"
        } else {
            "patch minified"
        }
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn prompt_snippet(&self) -> Option<&str> {
        Some(if self.multi {
            "Apply several precise minified-view edits to one file or several files at once"
        } else {
            "Edit code precisely through the compact minified view"
        })
    }

    fn prompt_guidelines(&self) -> Vec<String> {
        vec![
            if self.multi {
                "When multi_patch_minified is attached, you must use it for multiple replacements in one source file. Edit as many files as possible per call: when edits for several files are ready, send them together through its files parameter, not one call per file. Minified editing tools are mandatory, not a preference; do not bypass them with plain patch, write, or shell commands."
            } else {
                "When patch_minified is attached, you must use it for single replacements in existing source files. Minified editing tools are mandatory, not a preference; do not bypass them with plain patch, write, or shell commands. Read with read first when attached; never paste a minified view into a plain patch call."
            }
            .to_owned(),
        ]
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    /// These tools use the default shell, so the renderers return plain text
    /// content (matching read/write/grep) rather than framing themselves.
    fn render_call(
        &self,
        args: &Value,
        theme: &Theme,
        context: &ToolRenderContext,
    ) -> Option<ComponentRef> {
        let mut state = tool_render_state::<PatchRenderState>(&context.state);
        let text = format_patch_call(self.label(), args, theme, &context.cwd);
        let component = state
            .call
            .get_or_insert_with(|| Rc::new(RefCell::new(Text::new("", 0, 0))));
        component.borrow_mut().set_text(text);
        Some(Rc::clone(component) as ComponentRef)
    }

    fn render_result(
        &self,
        result: ToolRenderResult<'_>,
        _options: ToolRenderResultOptions,
        theme: &Theme,
        context: &ToolRenderContext,
    ) -> Option<ComponentRef> {
        let raw_path = patch_path_arg(&context.args);
        let details = if context.is_error {
            None
        } else {
            result.details
        };
        let sections = diff_sections(details, raw_path, theme, &context.cwd);
        let badge_style = block_style().is_compact();
        let mut state = tool_render_state::<PatchRenderState>(&context.state);
        if sections.is_empty() {
            let component = state
                .result_container
                .get_or_insert_with(|| Rc::new(RefCell::new(Container::new())));
            component.borrow_mut().clear();
            return Some(Rc::clone(component) as ComponentRef);
        }
        let component = state
            .result_text
            .get_or_insert_with(|| Rc::new(RefCell::new(Text::new("", 0, 0))));
        // The leading blank row separates the result in the standard style's
        // box; the badge style stacks it directly under the badge line.
        let lead = if badge_style { "" } else { "\n" };
        component
            .borrow_mut()
            .set_text(format!("{lead}{}", sections.join("\n")));
        Some(Rc::clone(component) as ComponentRef)
    }

    fn constrained_sampling(&self) -> Option<&ConstrainedSampling> {
        self.constrained_sampling.as_ref()
    }

    fn execute<'a>(
        &'a self,
        _tool_call_id: &'a str,
        params: Value,
        signal: Option<CancellationToken>,
        _on_update: Option<AgentToolUpdateCallback>,
        _context: Option<ToolContext>,
    ) -> BoxFuture<'a, Result<AgentToolResult, ToolExecutionError>> {
        Box::pin(async move {
            let files = if self.multi {
                multi_file_edits(&params)?
            } else {
                vec![FileEdits {
                    path: string_param(&params, "path"),
                    edits: vec![edit_request(&params)],
                    keep_comments: bool_param(&params, "keep_comments"),
                }]
            };
            run_edits(self, files, signal).await
        })
    }
}

/// `createCreatePatchMinifiedTool` — the tool as the agent loop takes it.
pub fn create_patch_minified_tool(
    cwd: &str,
    options: Option<PatchMinifiedToolOptions>,
) -> Arc<dyn AgentTool> {
    wrap_tool_definition(
        Arc::new(create_patch_minified_tool_definition(cwd, options)),
        None,
    )
}

/// `createCreateMultiPatchMinifiedTool` — the tool as the agent loop takes it.
pub fn create_multi_patch_minified_tool(
    cwd: &str,
    options: Option<PatchMinifiedToolOptions>,
) -> Arc<dyn AgentTool> {
    wrap_tool_definition(
        Arc::new(create_multi_patch_minified_tool_definition(cwd, options)),
        None,
    )
}

#[cfg(test)]
mod tests {
    //! two tools is covered by `tests/minified_tools.rs`; what is checked here
    //! is that they take, release and respect a lease like `write` and `edit`.

    use super::*;
    use crate::core::tools::file_lease::{FileLease, FileLeaseStore};

    struct TempDir {
        path: std::path::PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            let directory = tempfile::Builder::new()
                .prefix("notagent-patch-minified-lease-")
                .tempdir()
                .expect("temp dir");
            let path = directory.path().to_path_buf();
            let _ = directory.keep();
            Self { path }
        }

        fn cwd(&self) -> String {
            self.path.to_string_lossy().into_owned()
        }

        fn write(&self, name: &str, contents: &str) {
            std::fs::write(self.path.join(name), contents).expect("write");
        }

        fn read(&self, name: &str) -> String {
            std::fs::read_to_string(self.path.join(name)).expect("read")
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn with_leases(enabled: bool) -> Option<PatchMinifiedToolOptions> {
        Some(PatchMinifiedToolOptions {
            leases: Some(Arc::new(move || enabled)),
            ..PatchMinifiedToolOptions::default()
        })
    }

    fn one_patch() -> Value {
        json!({ "path": "file.rs", "old_string": "let a = 1;", "new_string": "let a = 2;" })
    }

    /// A lease held by a live process (this one), so it blocks like a foreign
    /// agent's would.
    async fn plant_foreign_lease(directory: &TempDir, target: &std::path::Path) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_millis() as u64;
        FileLeaseStore::for_workspace(&directory.cwd())
            .try_acquire(
                FileLease::new("foreign-lease", target.to_path_buf())
                    .agent_id(format!("pid:{}", std::process::id()))
                    .tool_name("patch_minified")
                    .acquired_at_ms(now)
                    .expected_duration_ms(60_000)
                    .lease_until_ms(now + 60_000),
            )
            .await
            .expect("plant");
    }

    #[tokio::test]
    async fn refuses_a_file_that_is_not_utf8_and_leaves_its_bytes_alone() {
        let directory = TempDir::new();
        let original: &[u8] = b"fn main() {\n    let a = 1; // gr\xfc\xdfe\n}\n";
        std::fs::write(directory.path.join("file.rs"), original).expect("write");
        let tool = create_patch_minified_tool_definition(&directory.cwd(), with_leases(false));
        let error = tool
            .execute("call-1", one_patch(), None, None, None)
            .await
            .expect_err("not utf-8");

        assert!(
            error.message.contains("not valid UTF-8"),
            "{}",
            error.message
        );
        assert_eq!(
            std::fs::read(directory.path.join("file.rs")).expect("read"),
            original,
            "a Latin-1 file keeps every byte when the patch is refused"
        );
    }

    #[tokio::test]
    async fn patches_under_a_lease_and_releases_it_again() {
        let directory = TempDir::new();
        directory.write("file.rs", "fn main() {\n    let a = 1;\n}\n");
        let tool = create_patch_minified_tool_definition(&directory.cwd(), with_leases(true));
        tool.execute("call-1", one_patch(), None, None, None)
            .await
            .expect("patch");

        assert!(directory.read("file.rs").contains("let a = 2;"));
        let leases = FileLeaseStore::for_workspace(&directory.cwd());
        assert_eq!(
            leases
                .get_by_path(&directory.path.join("file.rs"))
                .await
                .expect("get"),
            None,
            "a successful patch releases its lease"
        );
    }

    #[tokio::test]
    async fn refuses_a_file_another_holder_has_leased() {
        let directory = TempDir::new();
        directory.write("file.rs", "fn main() {\n    let a = 1;\n}\n");
        plant_foreign_lease(&directory, &directory.path.join("file.rs")).await;

        let tool = create_patch_minified_tool_definition(&directory.cwd(), with_leases(true));
        let error = tool
            .execute("call-1", one_patch(), None, None, None)
            .await
            .expect_err("leased");

        assert!(
            error.message.contains("file is currently leased"),
            "{}",
            error.message
        );
        assert!(
            directory.read("file.rs").contains("let a = 1;"),
            "a blocked patch leaves the file untouched"
        );
    }

    #[tokio::test]
    async fn the_multi_variant_takes_a_lease_too() {
        let directory = TempDir::new();
        directory.write("file.rs", "fn main() {\n    let a = 1;\n}\n");
        plant_foreign_lease(&directory, &directory.path.join("file.rs")).await;

        let tool = create_multi_patch_minified_tool_definition(&directory.cwd(), with_leases(true));
        let error = tool
            .execute(
                "call-1",
                json!({
                    "path": "file.rs",
                    "edits": [{ "old_string": "let a = 1;", "new_string": "let a = 2;" }],
                }),
                None,
                None,
                None,
            )
            .await
            .expect_err("leased");

        assert!(
            error.message.contains("file is currently leased"),
            "{}",
            error.message
        );
    }

    #[tokio::test]
    async fn a_foreign_lease_is_ignored_while_leases_are_off() {
        let directory = TempDir::new();
        directory.write("file.rs", "fn main() {\n    let a = 1;\n}\n");
        plant_foreign_lease(&directory, &directory.path.join("file.rs")).await;

        let tool = create_patch_minified_tool_definition(&directory.cwd(), with_leases(false));
        tool.execute("call-1", one_patch(), None, None, None)
            .await
            .expect("patch");
        assert!(directory.read("file.rs").contains("let a = 2;"));
    }

    /// Local file access whose writes to one named file fail.
    struct FailingWrite {
        file_name: &'static str,
    }

    impl PatchMinifiedOperations for FailingWrite {
        fn read_file<'a>(
            &'a self,
            absolute_path: &'a str,
        ) -> BoxFuture<'a, Result<Vec<u8>, String>> {
            LocalPatchMinifiedOperations.read_file(absolute_path)
        }

        fn write_file<'a>(
            &'a self,
            absolute_path: &'a str,
            content: &'a str,
        ) -> BoxFuture<'a, Result<(), String>> {
            if absolute_path.ends_with(self.file_name) {
                return Box::pin(async { Err("disk full".to_owned()) });
            }
            LocalPatchMinifiedOperations.write_file(absolute_path, content)
        }

        fn access<'a>(&'a self, absolute_path: &'a str) -> BoxFuture<'a, Result<(), String>> {
            LocalPatchMinifiedOperations.access(absolute_path)
        }
    }

    fn two_files() -> Value {
        json!({
            "files": [
                { "path": "a.rs", "edits": [{ "old_string": "let a = 1;", "new_string": "let a = 2;" }] },
                { "path": "b.rs", "edits": [{ "old_string": "let a = 1;", "new_string": "let a = 3;" }] },
            ],
        })
    }

    #[tokio::test]
    async fn a_write_that_fails_part_way_puts_the_written_files_back() {
        let directory = TempDir::new();
        let original = "fn main() {\n    let a = 1;\n}\n";
        directory.write("a.rs", original);
        directory.write("b.rs", original);
        let tool = create_multi_patch_minified_tool_definition(
            &directory.cwd(),
            Some(PatchMinifiedToolOptions {
                operations: Some(Arc::new(FailingWrite { file_name: "b.rs" })),
                ..PatchMinifiedToolOptions::default()
            }),
        );

        let error = tool
            .execute("call-1", two_files(), None, None, None)
            .await
            .expect_err("the second write fails");

        assert!(error.message.contains("disk full"), "{}", error.message);
        assert!(
            error.message.contains("put back"),
            "the error says the first file was restored: {}",
            error.message
        );
        assert_eq!(
            directory.read("a.rs"),
            original,
            "a.rs was written before b.rs failed and must hold its old content again"
        );
        assert_eq!(directory.read("b.rs"), original);
    }

    #[tokio::test]
    async fn a_foreign_lease_on_any_file_blocks_every_file() {
        let directory = TempDir::new();
        let original = "fn main() {\n    let a = 1;\n}\n";
        directory.write("a.rs", original);
        directory.write("b.rs", original);
        plant_foreign_lease(&directory, &directory.path.join("b.rs")).await;

        let tool = create_multi_patch_minified_tool_definition(&directory.cwd(), with_leases(true));
        let error = tool
            .execute("call-1", two_files(), None, None, None)
            .await
            .expect_err("b.rs is leased");

        assert!(
            error.message.contains("file is currently leased"),
            "{}",
            error.message
        );
        assert_eq!(directory.read("a.rs"), original);
        let leases = FileLeaseStore::for_workspace(&directory.cwd());
        assert_eq!(
            leases
                .get_by_path(&directory.path.join("a.rs"))
                .await
                .expect("get"),
            None,
            "the lease taken on a.rs before b.rs was refused is released again"
        );
    }

    #[tokio::test]
    async fn a_failed_patch_releases_its_lease() {
        let directory = TempDir::new();
        directory.write("file.rs", "fn main() {\n    let a = 1;\n}\n");
        let tool = create_patch_minified_tool_definition(&directory.cwd(), with_leases(true));

        tool.execute(
            "call-1",
            json!({ "path": "file.rs", "old_string": "not in the file", "new_string": "x" }),
            None,
            None,
            None,
        )
        .await
        .expect_err("no match");

        let leases = FileLeaseStore::for_workspace(&directory.cwd());
        assert_eq!(
            leases
                .get_by_path(&directory.path.join("file.rs"))
                .await
                .expect("get"),
            None,
            "the file must not stay blocked until the lease's TTL runs out"
        );
    }
}
