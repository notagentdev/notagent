//! Port of `packages/coding-agent/src/core/tools/patch-minified.ts` (tool half).
//!
//! `patch_minified` and `multi_patch_minified`: the editing counterparts of
//! `read_minified`.
//!
//! Both route through the same single-edit path ([`apply_minified_replace`]), as
//! in the reference service. `multi_patch_minified` applies its edits
//! sequentially against the result of the previous one, re-minifying in
//! between, because every splice invalidates the previous source map. Nothing
//! is written until all edits succeed.

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
use crate::core::tools::file_mutation_queue::with_file_mutation_queue;
use crate::core::tools::path_utils::resolve_to_cwd;
use crate::core::tools::render_utils::{call_title, render_tool_path, str_arg};
use crate::core::tools::tool_definition::{
    ToolContext, ToolDefinition, ToolRenderContext, ToolRenderResult, ToolRenderResultOptions,
    tool_render_state, wrap_tool_definition,
};
use crate::modes::interactive::components::diff::{RenderDiffOptions, diff_info_line, render_diff};
use crate::modes::interactive::theme::theme::{BlockStyle, Theme, ThemeColor, block_style};

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

fn multi_patch_minified_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "Path to the file to edit (relative or absolute)" },
            "edits": {
                "type": "array",
                "items": minified_edit_properties(),
                "description": "Edits applied in order. Each one operates on the result of the previous edit, against a freshly rebuilt minified view.",
            },
            "keep_comments": {
                "type": "boolean",
                "description": "Whether comments participate in the minified view. Must match the view used to choose the edits.",
            },
        },
        "required": ["path", "edits"],
    })
}

const SHARED_USAGE: &[&str] = &[
    "- Read the file with `read_minified` first; copying old_string from that output is the most reliable path.",
    "- old_string and new_string may be given in compact `read_minified` form or in normal source formatting; if an exact match fails, the tool normalizes them into minified form and retries exact matching.",
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

/// TS reports `Error code: ${error.code}` for Node errors.
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
/// minifier (mirroring `read_minified`'s raw fallback).
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

/// The shared write path of `patch_minified` and `multi_patch_minified`.
///
/// Takes the tool it runs for rather than its pieces: it needs the cwd, the
/// operations, the lease coordinator and the tool name, which is the tool
/// itself.
async fn run_edits(
    tool: &PatchMinifiedToolDefinition,
    path: &str,
    edits: Vec<MinifiedEditRequest>,
    keep_comments: bool,
    signal: Option<CancellationToken>,
) -> Result<AgentToolResult, ToolExecutionError> {
    let cwd = tool.cwd.as_str();
    let operations = tool.operations.as_ref();
    let leases = &tool.leases;
    let snapshots = tool.snapshots.clone();
    let tool_name = tool.name();

    if edits.is_empty() {
        return Err(ToolExecutionError::new(
            "Patch tool input is invalid. edits must contain at least one replacement.",
        ));
    }
    let absolute_path = resolve_to_cwd(path, cwd);

    with_file_mutation_queue(&absolute_path, async {
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

        // Reserved inside the mutation queue, which already serializes this
        // process: a lease conflict therefore always names a foreign holder,
        // never a second call of our own. `None` when leases are off, and every
        // lease step below is then a no-op.
        let lease = leases
            .reserve(
                std::path::Path::new(&absolute_path),
                tool_name,
                PATCH_MINIFIED_LEASE_DURATION_MS,
            )
            .await
            .map_err(|error| ToolExecutionError::new(error.to_string()))?;

        let result = async {
            throw_if_aborted()?;

            if let Err(error) = operations.access(&absolute_path).await {
                throw_if_aborted()?;
                return Err(ToolExecutionError::new(format!(
                    "Could not edit file: {path}. {error}."
                )));
            }
            throw_if_aborted()?;

            let buffer = operations
                .read_file(&absolute_path)
                .await
                .map_err(ToolExecutionError::new)?;
            let raw_content = String::from_utf8_lossy(&buffer).into_owned();
            throw_if_aborted()?;

            // Strip the BOM before matching: the model will not include an invisible
            // BOM in old_string. It is restored verbatim on write.
            let (bom, original) = strip_bom(&raw_content);

            // Apply every edit sequentially against the result of the previous one,
            // re-minifying per edit because each splice invalidates the source map.
            let mut current = original.to_owned();
            let mut warnings: Vec<String> = Vec::new();
            for edit in &edits {
                let (content, edit_warnings) =
                    apply_minified_replace(path, &current, edit, keep_comments)
                        .map_err(ToolExecutionError::new)?;
                current = content;
                warnings.extend(edit_warnings);
                throw_if_aborted()?;
            }

            // Always: a patch only ever applies to a file that is already
            // there, so there is always an earlier state to return to.
            capture_before_mutation(snapshots.as_ref(), Path::new(&absolute_path))
                .await
                .map_err(ToolExecutionError::new)?;
            throw_if_aborted()?;

            // The pre-commit check runs immediately before the write: the lease
            // must still be ours and the file must still hold the content the edits
            // were minified against.
            let committed = match &lease {
                Some(lease) => Some(
                    leases
                        .prepare_commit(lease.clone())
                        .await
                        .map_err(|error| ToolExecutionError::new(error.to_string()))?,
                ),
                None => None,
            };

            operations
                .write_file(&absolute_path, &format!("{bom}{current}"))
                .await
                .map_err(ToolExecutionError::new)?;

            if let Some(committed) = &committed {
                leases
                    .release(committed)
                    .await
                    .map_err(|error| ToolExecutionError::new(error.to_string()))?;
            }
            throw_if_aborted()?;

            let diff_result = generate_diff_string(original, &current, 4);
            let patch = generate_unified_patch(path, original, &current, 4);
            let summary = format!(
                "Successfully applied {} minified edit(s) to {path}.",
                edits.len()
            );
            let text = if warnings.is_empty() {
                summary
            } else {
                format!(
                    "{summary}\n\nWarnings:\n{}",
                    warnings
                        .iter()
                        .map(|warning| format!("- {warning}"))
                        .collect::<Vec<_>>()
                        .join("\n")
                )
            };
            let mut details = Map::new();
            details.insert("diff".to_owned(), Value::from(diff_result.diff));
            details.insert("patch".to_owned(), Value::from(patch));
            if let Some(first_changed_line) = diff_result.first_changed_line {
                details.insert("firstChangedLine".to_owned(), json!(first_changed_line));
            }
            details.insert("warnings".to_owned(), json!(warnings));
            Ok(AgentToolResult {
                content: vec![TextOrImageContent::Text(TextContent::new(text))],
                details: Some(Value::Object(details)),
                usage: None,
                added_tool_names: None,
                terminate: None,
            })
        }
        .await;

        leases.release_on_error(lease.as_ref(), result).await
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
            "Performs exact string replacements in the minified view of a file — the precise editing counterpart of `read_minified`.",
            "",
            "Match old_string against the minified view produced by `read_minified` (compact indentation, comments omitted unless keep_comments=true). The tool reads the current file from disk, rebuilds the minified view and its source map, maps the matched minified range back to the original source, and splices the replacement into the real file. Untouched source stays identical, including comments and formatting outside the replaced range, while new_string is expanded back to the file's indentation style.",
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
            "Performs multiple sequential precise edits on a single file in the minified view — the multi-edit counterpart of `patch_minified`. Prefer this over several `patch_minified` calls on the same file.",
            "",
            "All rules of `patch_minified` apply to each edit. Each edit operates on the result of the previous one: the tool re-minifies between edits so every next match uses a fresh source map. The edits are atomic — if any edit fails, none are applied.",
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
///
/// The result is a `Text` when there is something to show and an empty
/// `Container` otherwise; see [`crate::core::tools::write`] for why the two
/// slots are separate here (deviation class 1).
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
    let path_display = render_tool_path(patch_path_arg(args).as_deref(), theme, cwd, None);
    format!("{}{path_display}", call_title(theme, label))
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
            "Apply several precise minified-view edits to one file"
        } else {
            "Edit code precisely through the compact minified view"
        })
    }

    fn prompt_guidelines(&self) -> Vec<String> {
        vec![
            if self.multi {
                "When changing multiple locations in one file through the minified view, use one multi_patch_minified call instead of several patch_minified calls."
            } else {
                "Use patch_minified to change code you found via read_minified; never paste a minified view into a plain edit call."
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
        let mut sections: Vec<String> = Vec::new();
        let diff = if context.is_error {
            None
        } else {
            result
                .details
                .and_then(|details| details.get("diff"))
                .and_then(Value::as_str)
                .filter(|diff| !diff.is_empty())
        };
        let badge_style = block_style() == BlockStyle::Badge;
        if let Some(diff) = diff {
            sections.push(render_diff(
                diff,
                &RenderDiffOptions {
                    file_path: raw_path,
                },
            ));
            // The badge style closes the diff with its summary; the standard
            // style stays the TS original, which the render oracle pins.
            if badge_style && let Some(info) = diff_info_line(diff) {
                sections.push(info);
            }
        }
        if let Some(warnings) = result
            .details
            .and_then(|details| details.get("warnings"))
            .and_then(Value::as_array)
        {
            for warning in warnings {
                let warning = warning.as_str().map_or_else(
                    || crate::core::tools::tool_definition::display_arg(warning),
                    str::to_string,
                );
                sections.push(theme.fg(ThemeColor::Warning, &format!("! {warning}")));
            }
        }

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
            let path = params
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let edits = if self.multi {
                params
                    .get("edits")
                    .and_then(Value::as_array)
                    .map(|edits| edits.iter().map(edit_request).collect())
                    .unwrap_or_default()
            } else {
                vec![edit_request(&params)]
            };
            let keep_comments = params
                .get("keep_comments")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            run_edits(self, &path, edits, keep_comments, signal).await
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
    //! Atomic leases (port addition, v0.1.19). The editing behaviour of these
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
