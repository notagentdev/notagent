use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use notagent_agent::types::{
    AgentTool, AgentToolResult, AgentToolUpdateCallback, BoxFuture, ToolExecutionError,
};
use notagent_ai::types::{ConstrainedSampling, TextContent, TextOrImageContent};
use notagent_tui::components::box_component::BoxComponent;
use notagent_tui::components::spacer::Spacer;
use notagent_tui::components::text::Text;
use notagent_tui::tui::{Component, ComponentRef, Container, Line, component_ref};
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use crate::core::experimental::get_experimental_tool_sampling;
use crate::core::snapshots::{SnapshotStore, capture_before_mutation};
use crate::core::tools::edit_diff::{
    DiffString, Edit, apply_edits_to_normalized_content, compute_edits_diff, detect_line_ending,
    generate_diff_string, generate_unified_patch, normalize_to_lf, restore_line_endings, strip_bom,
};
use crate::core::tools::file_lease::{LeaseCoordinator, LeaseGate};
use crate::core::tools::file_mutation_queue::with_file_mutation_queue;
use crate::core::tools::path_utils::resolve_to_cwd;
use crate::core::tools::render_utils::{render_mutation_tool_path, str_arg};
use crate::core::tools::tool_definition::{
    RenderFuture, RenderShell, SystemPromptContribution, ToolContext, ToolDefinition,
    ToolRenderContext, ToolRenderResult, ToolRenderResultOptions, tool_render_state,
    wrap_tool_definition,
};
use crate::modes::interactive::components::diff::{RenderDiffOptions, render_diff};
use crate::modes::interactive::theme::theme::{Theme, ThemeBg, ThemeColor, block_style};

pub const EDIT_TOOL_SYSTEM_PROMPT_CONTRIBUTION: SystemPromptContribution =
    SystemPromptContribution {
        snippet: "Fallback file edit with one exact text replacement",
        guidelines: &[
            "patch is a fallback only: use it when no minified editing tool is attached, or a concrete limitation or failure prevents the minified tools from safely making the requested edit. State the reason before falling back; convenience is not a reason. Supply path, old_string and new_string, with old_string taken from an exact read of the current file.",
            "Each patch call replaces one unique occurrence. Base subsequent changes on the file after the preceding patch.",
            "Keep old_string as small as possible while still being unique in the file. Do not pad with large unchanged regions.",
        ],
    };

const DESCRIPTION: &str = "Fallback for editing a single file using exact text replacement. When minified editing tools are attached, you must use them instead; use patch only when a concrete limitation or failure prevents them from safely making the requested edit, and state that reason first. If no minified editing tool is attached, patch is available for exact edits. Read the exact current source before falling back; never match against a minified view. Supply path, old_string and new_string directly. old_string must identify a unique region in the current file. new_string may be empty to delete that region. Do not include large unchanged regions just to connect distant changes.";

fn edit_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "Path to the file to edit (relative or absolute)" },
            "old_string": { "type": "string", "description": "Exact text to find. It must be non-empty and unique in the current file." },
            "new_string": { "type": "string", "description": "Replacement text. Use an empty string to delete the matched text." },
        },
        "required": ["path", "old_string", "new_string"],
        "additionalProperties": false,
    })
}

/// Pluggable operations, so editing can be delegated to a remote system.
pub trait EditOperations: Send + Sync {
    fn read_file<'a>(&'a self, absolute_path: &'a str) -> BoxFuture<'a, Result<Vec<u8>, String>>;
    fn write_file<'a>(
        &'a self,
        absolute_path: &'a str,
        content: &'a str,
    ) -> BoxFuture<'a, Result<(), String>>;
    /// `access(path, R_OK | W_OK)`.
    fn access<'a>(&'a self, absolute_path: &'a str) -> BoxFuture<'a, Result<(), String>>;
}

pub struct LocalEditOperations;

impl EditOperations for LocalEditOperations {
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
pub struct EditToolOptions {
    pub operations: Option<Arc<dyn EditOperations>>,
    /// Whether atomic file leases are enabled; absent means disabled.
    pub leases: Option<LeaseGate>,
    /// Where a copy of the file goes before it is patched, so `undo` can put it
    /// back. Absent means no snapshot is taken.
    pub snapshots: Option<SnapshotStore>,
}

pub struct EditToolDefinition {
    cwd: String,
    snapshots: Option<SnapshotStore>,
    operations: Arc<dyn EditOperations>,
    leases: LeaseCoordinator,
    parameters: Value,
    constrained_sampling: Option<ConstrainedSampling>,
}

/// How long an `edit` expects to hold its lease (the reference's `fs_patch`).
const EDIT_LEASE_DURATION_MS: u64 = 30_000;

pub fn create_edit_tool_definition(
    cwd: &str,
    options: Option<EditToolOptions>,
) -> EditToolDefinition {
    let options = options.unwrap_or_default();
    EditToolDefinition {
        cwd: cwd.to_owned(),
        operations: options
            .operations
            .unwrap_or_else(|| Arc::new(LocalEditOperations)),
        leases: LeaseCoordinator::new(cwd, options.leases),
        snapshots: options.snapshots,
        parameters: edit_schema(),
        constrained_sampling: get_experimental_tool_sampling(),
    }
}

fn validate_edit_input(input: &Value) -> Result<(String, Vec<Edit>), ToolExecutionError> {
    if input.get("edits").is_some() {
        return Err(ToolExecutionError::new(
            "Patch accepts one replacement. Supply path, old_string and new_string directly, not edits.",
        ));
    }
    let string_arg = |name: &str| {
        input.get(name).and_then(Value::as_str).ok_or_else(|| {
            ToolExecutionError::new(format!("Patch requires {name} to be a string."))
        })
    };
    let path = string_arg("path")?;
    let old_text = string_arg("old_string")?;
    let new_text = string_arg("new_string")?;
    if path.is_empty() || old_text.is_empty() {
        return Err(ToolExecutionError::new(
            "Patch requires non-empty path and old_string.",
        ));
    }
    Ok((
        path.to_owned(),
        vec![Edit {
            old_text: old_text.to_owned(),
            new_text: new_text.to_owned(),
        }],
    ))
}

// ============================================================================
// Rendering
// ============================================================================

/// The preview shown before the edit runs: either its diff or why it failed.
type EditPreview = Result<DiffString, String>;

/// The header of an `edit` call, with the live preview of its diff.
struct EditCallComponent {
    header: BoxComponent,
    preview: Option<EditPreview>,
    preview_args_key: Option<String>,
    preview_pending: bool,
    settled_error: bool,
}

impl EditCallComponent {
    fn new() -> Self {
        Self {
            header: BoxComponent::new(1, 1, Some(Rc::new(|text: &str| text.to_string()))),
            preview: None,
            preview_args_key: None,
            preview_pending: false,
            settled_error: false,
        }
    }
}

impl Component for EditCallComponent {
    fn render(&mut self, width: usize) -> Vec<Line> {
        self.header.render(width)
    }

    fn invalidate(&mut self) {
        self.header.invalidate();
    }
}

/// The row state of an `edit` call.
#[derive(Default)]
struct EditRenderState {
    call: Option<Rc<RefCell<EditCallComponent>>>,
    result: Option<Rc<RefCell<Container>>>,
    /// A preview the render loop still has to compute; see `pump_render`.
    pending: Option<PendingPreview>,
}

/// One outstanding preview computation.
struct PendingPreview {
    path: String,
    edits: Vec<Edit>,
    cwd: String,
    args_key: String,
}

/// The arguments a preview can be computed from, or `None` while they are still
/// incomplete.
struct RenderablePreviewInput {
    path: String,
    edits: Vec<Edit>,
}

fn get_renderable_preview_input(args: &Value) -> Option<RenderablePreviewInput> {
    // `args.path` first here, unlike the header, which prefers `file_path`.
    let path = args
        .get("path")
        .and_then(Value::as_str)
        .or_else(|| args.get("file_path").and_then(Value::as_str))
        .filter(|path| !path.is_empty())?
        .to_string();

    if let Some(entries) = args.get("edits").and_then(Value::as_array)
        && !entries.is_empty()
        && entries.iter().all(|edit| {
            edit.get("old_string").and_then(Value::as_str).is_some()
                && edit.get("new_string").and_then(Value::as_str).is_some()
        })
    {
        return Some(RenderablePreviewInput {
            path,
            edits: entries
                .iter()
                .map(|edit| Edit {
                    old_text: edit["old_string"].as_str().unwrap_or_default().to_string(),
                    new_text: edit["new_string"].as_str().unwrap_or_default().to_string(),
                })
                .collect(),
        });
    }

    let old_text = args.get("old_string").and_then(Value::as_str)?;
    let new_text = args.get("new_string").and_then(Value::as_str)?;
    Some(RenderablePreviewInput {
        path,
        edits: vec![Edit {
            old_text: old_text.to_string(),
            new_text: new_text.to_string(),
        }],
    })
}

/// `JSON.stringify({ path, edits })` — the identity of one preview request.
fn preview_args_key(input: &RenderablePreviewInput) -> String {
    json!({
        "path": input.path,
        "edits": input.edits.iter().map(|edit| json!({
            "old_string": edit.old_text,
            "new_string": edit.new_text,
        })).collect::<Vec<_>>(),
    })
    .to_string()
}

/// `str(args?.file_path ?? args?.path)`
fn edit_path_arg(args: &Value) -> Option<String> {
    let raw = args
        .get("file_path")
        .filter(|value| !value.is_null())
        .or_else(|| args.get("path"));
    str_arg(raw)
}

fn format_edit_call(args: &Value, theme: &Theme, cwd: &str) -> String {
    let path_display = render_mutation_tool_path(edit_path_arg(args).as_deref(), theme, cwd, None);
    format!(
        "{}{path_display}",
        crate::core::tools::render_utils::call_title(theme, "patch")
    )
}

/// The header colour says where the edit stands: previewed, failed or pending.
fn edit_header_bg(
    preview: Option<&EditPreview>,
    settled_error: bool,
    theme: &Theme,
) -> notagent_tui::components::text::BackgroundFn {
    let background = match (preview, settled_error) {
        (Some(Err(_)), _) => ThemeBg::ToolErrorBg,
        (Some(Ok(_)), _) => ThemeBg::ToolSuccessBg,
        (None, true) => ThemeBg::ToolErrorBg,
        (None, false) => ThemeBg::ToolPendingBg,
    };
    let theme = theme.clone();
    Rc::new(move |text: &str| theme.bg(background, text))
}

fn build_edit_call_component(
    component: &Rc<RefCell<EditCallComponent>>,
    args: &Value,
    theme: &Theme,
    cwd: &str,
) {
    let header_text = format_edit_call(args, theme, cwd);
    let mut component = component.borrow_mut();
    // In the badge style the wash and padding rows go; the badge the tool
    // block prepends pairs with the header as the first line (reference
    // `build_patch_call_component`).
    let badge_style = crate::modes::interactive::theme::theme::block_style().is_compact();
    if badge_style {
        component.header.set_padding(0, 0);
        component.header.set_bg_fn(None);
    } else {
        component.header.set_padding(1, 1);
        let background = edit_header_bg(component.preview.as_ref(), component.settled_error, theme);
        component.header.set_bg_fn(Some(background));
    }
    component.header.clear();
    component
        .header
        .add_child(component_ref(Text::new(header_text, 0, 0)));

    let Some(preview) = &component.preview else {
        return;
    };
    let body = match preview {
        Err(error) => theme.fg(ThemeColor::Error, error),
        // The path decides the syntax colours of the preview, the same way it
        // does for the settled result below.
        Ok(diff) => render_diff(
            &diff.diff,
            &RenderDiffOptions {
                file_path: edit_path_arg(args),
            },
        ),
    };
    // The badge style stacks the diff directly under the header row.
    if !badge_style {
        component.header.add_child(component_ref(Spacer::new(1)));
    }
    component
        .header
        .add_child(component_ref(Text::new(body, 0, 0)));
}

/// Store a preview and report whether it differs from the one shown.
fn set_edit_preview(
    component: &mut EditCallComponent,
    preview: EditPreview,
    args_key: Option<String>,
) -> bool {
    let changed = match (&component.preview, &preview) {
        (None, _) => true,
        (Some(Err(current)), Err(next)) => current != next,
        (Some(Ok(current)), Ok(next)) => {
            current.diff != next.diff || current.first_changed_line != next.first_changed_line
        }
        _ => true,
    };
    component.preview = Some(preview);
    component.preview_args_key = args_key;
    component.preview_pending = false;
    changed
}

/// What the result adds below the header: an error the preview did not already
/// show, or a diff that differs from the previewed one.
fn format_edit_result(
    args: &Value,
    preview: Option<&EditPreview>,
    result: ToolRenderResult<'_>,
    theme: &Theme,
    is_error: bool,
) -> Option<String> {
    let raw_path = edit_path_arg(args);
    let preview_diff = match preview {
        Some(Ok(preview)) => Some(preview.diff.as_str()),
        _ => None,
    };
    let preview_error = match preview {
        Some(Err(error)) => Some(error.as_str()),
        _ => None,
    };
    if is_error {
        let error_text = result
            .content
            .iter()
            .filter_map(|block| match block {
                TextOrImageContent::Text(text) => Some(text.text.clone()),
                TextOrImageContent::Image(_) => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        if error_text.is_empty() || Some(error_text.as_str()) == preview_error {
            return None;
        }
        return Some(theme.fg(ThemeColor::Error, &error_text));
    }

    let result_diff = result
        .details
        .and_then(|details| details.get("diff"))
        .and_then(Value::as_str)
        .filter(|diff| !diff.is_empty())?;
    if Some(result_diff) == preview_diff {
        return None;
    }
    Some(render_diff(
        result_diff,
        &RenderDiffOptions {
            file_path: raw_path,
        },
    ))
}

impl ToolDefinition for EditToolDefinition {
    fn name(&self) -> &str {
        "patch"
    }

    fn label(&self) -> &str {
        "patch"
    }

    fn description(&self) -> &str {
        DESCRIPTION
    }

    fn prompt_snippet(&self) -> Option<&str> {
        Some(EDIT_TOOL_SYSTEM_PROMPT_CONTRIBUTION.snippet)
    }

    fn prompt_guidelines(&self) -> Vec<String> {
        EDIT_TOOL_SYSTEM_PROMPT_CONTRIBUTION
            .guidelines
            .iter()
            .map(|line| (*line).to_owned())
            .collect()
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    fn constrained_sampling(&self) -> Option<&ConstrainedSampling> {
        self.constrained_sampling.as_ref()
    }

    /// The edit tool frames its own diff output.
    fn render_shell(&self) -> RenderShell {
        RenderShell::SelfManaged
    }

    fn render_call(
        &self,
        args: &Value,
        theme: &Theme,
        context: &ToolRenderContext,
    ) -> Option<ComponentRef> {
        let mut state = tool_render_state::<EditRenderState>(&context.state);
        let component = state
            .call
            .get_or_insert_with(|| Rc::new(RefCell::new(EditCallComponent::new())))
            .clone();
        let preview_input = get_renderable_preview_input(args);
        let args_key = preview_input.as_ref().map(preview_args_key);

        {
            let mut borrowed = component.borrow_mut();
            // Changed arguments invalidate everything the old ones produced.
            if borrowed.preview_args_key != args_key {
                borrowed.preview = None;
                borrowed.preview_args_key = args_key.clone();
                borrowed.preview_pending = false;
                borrowed.settled_error = false;
            }

            if context.args_complete
                && let Some(preview_input) = &preview_input
                && borrowed.preview.is_none()
                && !borrowed.preview_pending
            {
                borrowed.preview_pending = true;
                state.pending = Some(PendingPreview {
                    path: preview_input.path.clone(),
                    edits: preview_input.edits.clone(),
                    cwd: context.cwd.clone(),
                    args_key: args_key.clone().unwrap_or_default(),
                });
            }
        }
        drop(state);

        build_edit_call_component(&component, args, theme, &context.cwd);
        Some(component as ComponentRef)
    }

    fn render_result(
        &self,
        result: ToolRenderResult<'_>,
        _options: ToolRenderResultOptions,
        theme: &Theme,
        context: &ToolRenderContext,
    ) -> Option<ComponentRef> {
        let mut state = tool_render_state::<EditRenderState>(&context.state);
        let call_component = state.call.clone();
        let container = state
            .result
            .get_or_insert_with(|| Rc::new(RefCell::new(Container::new())))
            .clone();
        drop(state);

        let preview_input = get_renderable_preview_input(&context.args);
        let args_key = preview_input.as_ref().map(preview_args_key);
        let result_diff = if context.is_error {
            None
        } else {
            result
                .details
                .and_then(|details| details.get("diff"))
                .and_then(Value::as_str)
        };

        if let Some(call_component) = &call_component {
            let mut changed = false;
            {
                let mut borrowed = call_component.borrow_mut();
                if let Some(result_diff) = result_diff {
                    let first_changed_line = result
                        .details
                        .and_then(|details| details.get("firstChangedLine"))
                        .and_then(Value::as_u64)
                        .map(|line| line as usize);
                    changed = set_edit_preview(
                        &mut borrowed,
                        Ok(DiffString {
                            diff: result_diff.to_string(),
                            first_changed_line,
                        }),
                        args_key,
                    ) || changed;
                }
                if borrowed.settled_error != context.is_error {
                    borrowed.settled_error = context.is_error;
                    changed = true;
                }
            }
            if changed {
                build_edit_call_component(call_component, &context.args, theme, &context.cwd);
            }
        }

        let preview = call_component
            .as_ref()
            .and_then(|component| component.borrow().preview.clone());
        let output = format_edit_result(
            &context.args,
            preview.as_ref(),
            result,
            theme,
            context.is_error,
        );
        container.borrow_mut().clear();
        let Some(output) = output else {
            return Some(container as ComponentRef);
        };
        // The blank row separates the result in the standard style's box; the
        // badge style stacks it directly under the badge line.
        if !block_style().is_compact() {
            container
                .borrow_mut()
                .add_child(component_ref(Spacer::new(1)));
        }
        container
            .borrow_mut()
            .add_child(component_ref(Text::new(output, 1, 0)));
        Some(container as ComponentRef)
    }

    /// A pending preview is due at once; the loop calls `pump_render` for it.
    fn render_deadline(&self, context: &ToolRenderContext) -> Option<Instant> {
        tool_render_state::<EditRenderState>(&context.state)
            .pending
            .is_some()
            .then(Instant::now)
    }

    /// Compute the preview the last `render_call` asked for.
    /// render loop, which awaits it on the TUI thread. The result is dropped if
    /// the arguments moved on in the meantime, exactly as the `previewArgsKey`
    /// check does there.
    fn pump_render<'a>(&'a self, context: &'a ToolRenderContext) -> Option<RenderFuture<'a>> {
        let pending = tool_render_state::<EditRenderState>(&context.state)
            .pending
            .take()?;
        Some(Box::pin(async move {
            let preview = compute_edits_diff(&pending.path, &pending.edits, &pending.cwd).await;
            let call = tool_render_state::<EditRenderState>(&context.state)
                .call
                .clone();
            let Some(call) = call else { return };
            let matches =
                call.borrow().preview_args_key.as_deref() == Some(pending.args_key.as_str());
            if !matches {
                return;
            }
            set_edit_preview(&mut call.borrow_mut(), preview, Some(pending.args_key));
            (context.invalidate)();
        }))
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
            let (path, edits) = validate_edit_input(&params)?;
            let absolute_path = resolve_to_cwd(&path, &self.cwd);

            with_file_mutation_queue(&absolute_path, async {
                // Do not abort from a listener: that would release the mutation
                // queue while a filesystem call is still in flight.
                let throw_if_aborted = || -> Result<(), ToolExecutionError> {
                    match &signal {
                        Some(signal) if signal.is_cancelled() => {
                            Err(ToolExecutionError::new("Operation aborted"))
                        }
                        _ => Ok(()),
                    }
                };

                throw_if_aborted()?;
                // Reserved inside the mutation queue, which already serializes
                // this process: a lease conflict therefore always names a
                // foreign holder, never a second call of our own. `None` when
                // leases are off, and every lease step below is then a no-op.
                let lease = self
                    .leases
                    .reserve(
                        std::path::Path::new(&absolute_path),
                        self.name(),
                        EDIT_LEASE_DURATION_MS,
                    )
                    .await
                    .map_err(|error| ToolExecutionError::new(error.to_string()))?;

                let result = async {
                    throw_if_aborted()?;
                    if let Err(error) = self.operations.access(&absolute_path).await {
                        throw_if_aborted()?;
                        return Err(ToolExecutionError::new(format!(
                            "Could not edit file: {path}. {error}."
                        )));
                    }
                    throw_if_aborted()?;

                    let bytes = self
                        .operations
                        .read_file(&absolute_path)
                        .await
                        .map_err(ToolExecutionError::new)?;
                    // writes the U+FFFD replacements back, permanently corrupting
                    // bytes the edit never touched. Refusing is the only safe
                    // answer a byte-exact port can give.
                    let raw_content = String::from_utf8(bytes).map_err(|_| {
                        ToolExecutionError::new(format!(
                            "Could not edit file: {path}. The file is not valid UTF-8 \
                             (binary or unsupported encoding); editing it would corrupt \
                             bytes outside the edited range."
                        ))
                    })?;
                    throw_if_aborted()?;

                    // The model never includes an invisible BOM in old_string.
                    let (bom, content) = strip_bom(&raw_content);
                    let original_ending = detect_line_ending(content);
                    let normalized_content = normalize_to_lf(content);
                    let applied =
                        apply_edits_to_normalized_content(&normalized_content, &edits, &path)
                            .map_err(ToolExecutionError::new)?;
                    throw_if_aborted()?;

                    let final_content = format!(
                        "{bom}{}",
                        restore_line_endings(&applied.new_content, original_ending)
                    );

                    // Always: this tool only ever patches a file that is
                    // already there, so there is always an earlier state.
                    capture_before_mutation(self.snapshots.as_ref(), Path::new(&absolute_path))
                        .await
                        .map_err(ToolExecutionError::new)?;
                    throw_if_aborted()?;

                    // The pre-commit check runs immediately before the write: the
                    // lease must still be ours and the file must still hold the
                    // content we read the edits against.
                    let committed = match &lease {
                        Some(lease) => Some(
                            self.leases
                                .prepare_commit(lease.clone())
                                .await
                                .map_err(|error| ToolExecutionError::new(error.to_string()))?,
                        ),
                        None => None,
                    };

                    self.operations
                        .write_file(&absolute_path, &final_content)
                        .await
                        .map_err(ToolExecutionError::new)?;

                    if let Some(committed) = &committed {
                        self.leases
                            .release(committed)
                            .await
                            .map_err(|error| ToolExecutionError::new(error.to_string()))?;
                    }
                    throw_if_aborted()?;

                    let diff = generate_diff_string(&applied.base_content, &applied.new_content, 4);
                    let patch = generate_unified_patch(
                        &path,
                        &applied.base_content,
                        &applied.new_content,
                        4,
                    );
                    let mut details = Map::new();
                    details.insert("diff".to_owned(), Value::from(diff.diff));
                    details.insert("patch".to_owned(), Value::from(patch));
                    if let Some(first_changed_line) = diff.first_changed_line {
                        details.insert("firstChangedLine".to_owned(), json!(first_changed_line));
                    }
                    Ok(AgentToolResult {
                        content: vec![TextOrImageContent::Text(TextContent::new(format!(
                            "Successfully replaced {} block(s) in {path}.",
                            edits.len()
                        )))],
                        details: Some(Value::Object(details)),
                        usage: None,
                        added_tool_names: None,
                        terminate: None,
                    })
                }
                .await;

                self.leases.release_on_error(lease.as_ref(), result).await
            })
            .await
        })
    }
}

/// `createCreateEditTool` — the tool as the agent loop takes it.
pub fn create_edit_tool(cwd: &str, options: Option<EditToolOptions>) -> Arc<dyn AgentTool> {
    wrap_tool_definition(Arc::new(create_edit_tool_definition(cwd, options)), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir {
        path: std::path::PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            let directory = tempfile::Builder::new()
                .prefix("notagent-edit-tool-")
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

    fn text_of(result: &AgentToolResult) -> String {
        result
            .content
            .iter()
            .filter_map(|block| match block {
                TextOrImageContent::Text(text) => Some(text.text.clone()),
                TextOrImageContent::Image(_) => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[tokio::test]
    async fn replaces_text_in_a_file() {
        let directory = TempDir::new();
        directory.write("edit-test.txt", "Hello, world!");
        let tool = create_edit_tool_definition(&directory.cwd(), None);
        let result = tool
            .execute(
                "call-1",
                json!({
                    "path": "edit-test.txt",
                    "old_string": "world", "new_string": "testing",
                }),
                None,
                None,
                None,
            )
            .await
            .expect("edit");
        assert_eq!(
            text_of(&result),
            "Successfully replaced 1 block(s) in edit-test.txt."
        );
        assert_eq!(directory.read("edit-test.txt"), "Hello, testing!");
        let details = result.details.expect("details");
        assert!(details["diff"].as_str().expect("diff").contains("testing"));
        let patch = details["patch"].as_str().expect("patch");
        assert!(patch.contains("--- edit-test.txt"));
        assert!(patch.contains("-Hello, world!"));
        assert!(patch.contains("+Hello, testing!"));
        assert_eq!(details["firstChangedLine"], json!(1));
    }

    #[tokio::test]
    async fn refuses_a_file_that_is_not_valid_utf8() {
        let directory = TempDir::new();
        // A Latin-1 "café" — the 0xE9 byte is invalid UTF-8.
        std::fs::write(
            directory.path.join("latin1.txt"),
            [0x63, 0x61, 0x66, 0xE9, 0x0A],
        )
        .expect("write");
        let tool = create_edit_tool_definition(&directory.cwd(), None);
        let error = tool
            .execute(
                "call-1",
                json!({
                    "path": "latin1.txt",
                    "old_string": "caf", "new_string": "bar",
                }),
                None,
                None,
                None,
            )
            .await
            .expect_err("must refuse instead of corrupting");
        assert!(error.to_string().contains("not valid UTF-8"), "{error}");
        // The file is untouched — no U+FFFD replacement bytes were written.
        assert_eq!(
            std::fs::read(directory.path.join("latin1.txt")).expect("read"),
            [0x63, 0x61, 0x66, 0xE9, 0x0A]
        );
    }

    #[tokio::test]
    async fn successive_calls_match_the_result_of_the_preceding_patch() {
        let directory = TempDir::new();
        directory.write("file.txt", "alpha\nbeta\n");
        let tool = create_edit_tool_definition(&directory.cwd(), None);
        for (old, new) in [("alpha", "ALPHA"), ("ALPHA", "changed")] {
            tool.execute(
                "patch",
                json!({ "path": "file.txt", "old_string": old, "new_string": new }),
                None,
                None,
                None,
            )
            .await
            .expect("patch the current file");
        }
        assert_eq!(
            directory.read("file.txt"),
            "changed\nbeta\n",
            "later patches must match the updated file"
        );
    }

    #[tokio::test]
    async fn preserves_crlf_line_endings_and_the_byte_order_mark() {
        let directory = TempDir::new();
        directory.write("crlf.txt", "\u{FEFF}one\r\ntwo\r\nthree\r\n");
        let tool = create_edit_tool_definition(&directory.cwd(), None);
        tool.execute(
            "call-1",
            json!({ "path": "crlf.txt", "old_string": "two", "new_string": "TWO" }),
            None,
            None,
            None,
        )
        .await
        .expect("edit");
        assert_eq!(
            directory.read("crlf.txt"),
            "\u{FEFF}one\r\nTWO\r\nthree\r\n"
        );
    }

    #[tokio::test]
    async fn matches_fuzzily_while_keeping_untouched_bytes() {
        let directory = TempDir::new();
        directory.write(
            "fuzzy.txt",
            "keep \u{2018}this\u{2019}   \nchange \u{2018}that\u{2019}\n",
        );
        let tool = create_edit_tool_definition(&directory.cwd(), None);
        tool.execute(
            "call-1",
            json!({
                "path": "fuzzy.txt",
                "old_string": "change 'that'", "new_string": "changed",
            }),
            None,
            None,
            None,
        )
        .await
        .expect("edit");
        assert_eq!(
            directory.read("fuzzy.txt"),
            "keep \u{2018}this\u{2019}   \nchanged\n"
        );
    }

    #[tokio::test]
    async fn reports_missing_files_and_invalid_input() {
        let directory = TempDir::new();
        let tool = create_edit_tool_definition(&directory.cwd(), None);

        let error = tool
            .execute(
                "call-1",
                json!({ "path": "missing.txt", "old_string": "a", "new_string": "b" }),
                None,
                None,
                None,
            )
            .await
            .expect_err("missing");
        assert_eq!(
            error.message,
            "Could not edit file: missing.txt. Error code: ENOENT."
        );

        let error = tool
            .execute(
                "call-1",
                json!({ "path": "file.txt", "old_string": "content" }),
                None,
                None,
                None,
            )
            .await
            .expect_err("invalid");
        assert_eq!(error.message, "Patch requires new_string to be a string.");
    }

    #[tokio::test]
    async fn does_not_write_when_an_edit_does_not_match() {
        let directory = TempDir::new();
        directory.write("file.txt", "content");
        let tool = create_edit_tool_definition(&directory.cwd(), None);
        let error = tool
            .execute(
                "call-1",
                json!({ "path": "file.txt", "old_string": "zzz", "new_string": "b" }),
                None,
                None,
                None,
            )
            .await
            .expect_err("no match");
        assert!(
            error
                .message
                .starts_with("Could not find the exact text in file.txt.")
        );
        assert_eq!(directory.read("file.txt"), "content");
    }

    #[tokio::test]
    async fn rejects_incomplete_or_nested_arguments_without_changing_the_file() {
        let directory = TempDir::new();
        directory.write("file.txt", "content");
        let tool = create_edit_tool_definition(&directory.cwd(), None);
        for args in [
            json!({ "path": "file.txt", "old_string": "content" }),
            json!({ "path": "file.txt", "old_string": "content", "new_string": null }),
            json!({ "path": "file.txt", "old_string": 1, "new_string": "x" }),
            json!({ "path": "file.txt", "old_string": "", "new_string": "x" }),
            json!({ "path": "", "old_string": "content", "new_string": "x" }),
            json!({ "old_string": "content", "new_string": "x" }),
            json!({ "path": "file.txt", "edits": [{ "old_string": "content", "new_string": "x" }] }),
            json!({ "path": "file.txt", "old_string": "content", "new_string": "x", "edits": [] }),
        ] {
            let result = tool
                .execute("invalid", args.clone(), None, None, None)
                .await;
            assert!(
                result.is_err(),
                "invalid patch arguments must be rejected: {args}"
            );
            assert_eq!(
                directory.read("file.txt"),
                "content",
                "invalid input must not mutate the file: {args}"
            );
        }
    }

    #[tokio::test]
    async fn an_empty_replacement_deletes_the_matched_text() {
        let directory = TempDir::new();
        directory.write("file.txt", "before remove after");
        let tool = create_edit_tool_definition(&directory.cwd(), None);
        tool.execute(
            "delete",
            json!({ "path": "file.txt", "old_string": "remove ", "new_string": "" }),
            None,
            None,
            None,
        )
        .await
        .expect("empty replacement is valid");
        assert_eq!(
            directory.read("file.txt"),
            "before after",
            "an explicit empty replacement must delete only the match"
        );
    }

    #[tokio::test]
    async fn an_ambiguous_match_does_not_change_the_file() {
        let directory = TempDir::new();
        directory.write("file.txt", "same same");
        let tool = create_edit_tool_definition(&directory.cwd(), None);
        let result = tool
            .execute(
                "ambiguous",
                json!({ "path": "file.txt", "old_string": "same", "new_string": "different" }),
                None,
                None,
                None,
            )
            .await;
        assert!(result.is_err(), "patch must require a unique match");
        assert_eq!(
            directory.read("file.txt"),
            "same same",
            "an ambiguous patch must leave the file unchanged"
        );
    }

    #[tokio::test]
    async fn stops_on_an_aborted_signal() {
        let directory = TempDir::new();
        directory.write("file.txt", "content");
        let tool = create_edit_tool_definition(&directory.cwd(), None);
        let signal = CancellationToken::new();
        signal.cancel();
        let error = tool
            .execute(
                "call-1",
                json!({ "path": "file.txt", "old_string": "content", "new_string": "x" }),
                Some(signal),
                None,
                None,
            )
            .await
            .expect_err("aborted");
        assert_eq!(error.message, "Operation aborted");
        assert_eq!(directory.read("file.txt"), "content");
    }

    use crate::core::tools::file_lease::{FileLease, FileLeaseStore};

    fn with_leases(enabled: bool) -> Option<EditToolOptions> {
        Some(EditToolOptions {
            leases: Some(Arc::new(move || enabled)),
            ..EditToolOptions::default()
        })
    }

    fn one_edit() -> Value {
        json!({
            "path": "file.txt",
            "old_string": "content", "new_string": "changed",
        })
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
                    .tool_name("edit")
                    .acquired_at_ms(now)
                    .expected_duration_ms(60_000)
                    .lease_until_ms(now + 60_000),
            )
            .await
            .expect("plant");
    }

    #[tokio::test]
    async fn edits_under_a_lease_and_releases_it_again() {
        let directory = TempDir::new();
        directory.write("file.txt", "content");
        let tool = create_edit_tool_definition(&directory.cwd(), with_leases(true));
        tool.execute("call-1", one_edit(), None, None, None)
            .await
            .expect("edit");

        assert_eq!(directory.read("file.txt"), "changed");
        let leases = FileLeaseStore::for_workspace(&directory.cwd());
        assert_eq!(
            leases
                .get_by_path(&directory.path.join("file.txt"))
                .await
                .expect("get"),
            None,
            "a successful edit releases its lease"
        );
    }

    #[tokio::test]
    async fn refuses_a_file_another_holder_has_leased() {
        let directory = TempDir::new();
        directory.write("file.txt", "content");
        plant_foreign_lease(&directory, &directory.path.join("file.txt")).await;

        let tool = create_edit_tool_definition(&directory.cwd(), with_leases(true));
        let error = tool
            .execute("call-1", one_edit(), None, None, None)
            .await
            .expect_err("leased");

        assert!(
            error.message.contains("file is currently leased"),
            "{}",
            error.message
        );
        assert_eq!(
            directory.read("file.txt"),
            "content",
            "a blocked edit leaves the file untouched"
        );
    }

    #[tokio::test]
    async fn a_foreign_lease_is_ignored_while_leases_are_off() {
        let directory = TempDir::new();
        directory.write("file.txt", "content");
        plant_foreign_lease(&directory, &directory.path.join("file.txt")).await;

        let tool = create_edit_tool_definition(&directory.cwd(), with_leases(false));
        tool.execute("call-1", one_edit(), None, None, None)
            .await
            .expect("edit");
        assert_eq!(directory.read("file.txt"), "changed");
    }

    #[tokio::test]
    async fn a_failed_edit_releases_its_lease() {
        let directory = TempDir::new();
        directory.write("file.txt", "content");
        let tool = create_edit_tool_definition(&directory.cwd(), with_leases(true));

        tool.execute(
            "call-1",
            json!({
                "path": "file.txt",
                "old_string": "not in the file", "new_string": "x",
            }),
            None,
            None,
            None,
        )
        .await
        .expect_err("no match");

        let leases = FileLeaseStore::for_workspace(&directory.cwd());
        assert_eq!(
            leases
                .get_by_path(&directory.path.join("file.txt"))
                .await
                .expect("get"),
            None,
            "the file must not stay blocked until the lease's TTL runs out"
        );
    }

    #[test]
    fn the_agent_schema_accepts_flat_replacements_and_rejects_nested_calls() {
        use notagent_ai::types::ToolCall;
        use notagent_ai::utils::validation::{check, validate_tool_arguments};

        let tool = create_edit_tool("/tmp", None).to_tool();
        for (args, valid) in [
            (
                json!({ "path": "file.txt", "old_string": "before", "new_string": "after" }),
                true,
            ),
            (
                json!({ "path": "file.txt", "old_string": "before", "new_string": "" }),
                true,
            ),
            (json!({ "path": "file.txt", "old_string": "before" }), false),
            (
                json!({ "path": "file.txt", "old_string": "before", "new_string": null }),
                false,
            ),
            (
                json!({ "path": "file.txt", "edits": [{ "old_string": "before", "new_string": "after" }] }),
                false,
            ),
            (
                json!({ "path": "file.txt", "old_string": "before", "new_string": "after", "edits": [] }),
                false,
            ),
        ] {
            let call = ToolCall {
                name: "patch".to_owned(),
                arguments: args.as_object().expect("object arguments").clone(),
                ..ToolCall::default()
            };
            assert_eq!(
                check(&tool.parameters, &args),
                valid,
                "the model-facing schema must enforce the flat patch contract: {args}"
            );
            if valid {
                assert_eq!(
                    validate_tool_arguments(&tool, &call).expect("valid patch arguments"),
                    args,
                    "valid flat arguments must pass through central validation unchanged"
                );
            }
        }
    }

    #[test]
    fn advertises_its_schema_and_prompt_contribution() {
        let tool = create_edit_tool_definition("/tmp", None);
        assert_eq!(tool.name(), "patch");
        assert!(
            tool.description()
                .starts_with("Fallback for editing a single file using exact text replacement."),
            "patch must describe itself as a fallback: {}",
            tool.description()
        );
        assert_eq!(
            tool.render_shell(),
            crate::core::tools::tool_definition::RenderShell::SelfManaged
        );
        assert_eq!(tool.prompt_guidelines().len(), 3);
        assert_eq!(
            tool.parameters()["required"],
            json!(["path", "old_string", "new_string"]),
            "patch must expose the same flat replacement fields as patch_minified"
        );
        let properties = tool.parameters()["properties"]
            .as_object()
            .expect("properties");
        assert_eq!(
            properties.len(),
            3,
            "patch needs only path, old_string and new_string"
        );
        assert!(
            properties.values().all(|field| field["type"] == "string"),
            "patch arguments must be flat strings"
        );
        let args = json!({ "path": "file.txt", "old_string": "before", "new_string": "after" });
        assert_eq!(
            tool.prepare_arguments(args.clone()),
            args,
            "flat arguments must reach execution unchanged"
        );
    }
}
