//! Native replacement for `packages/coding-agent/src/core/extensions/types.ts`
//! (`ToolDefinition`) and `core/tools/tool-definition-wrapper.ts`.
//!
//! Deviation (class 2): the extension system is removed, so `ExtensionContext`
//! collapses to [`ToolContext`] with exactly the fields the built-in tools read
//! at runtime (`plans/facts/extension-boundary.md` §2.4: only bash uses it, for
//! `NOTAGENT_SESSION_ID`, the session file and `NOTAGENT_REASONING_LEVEL`).
//!
//! `renderCall`/`renderResult` belong to the interactive mode (task 13) and are
//! declared here as default methods, together with the render context the TS
//! version defines in `extensions/types.ts:410-497`.

use std::any::Any;
use std::cell::{RefCell, RefMut};
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use notagent_agent::types::{
    AgentTool, AgentToolResult, AgentToolUpdateCallback, BoxFuture, ToolExecutionError,
    ToolExecutionMode,
};
use notagent_ai::types::{ConstrainedSampling, TextOrImageContent};
use notagent_tui::components::text::Text;
use notagent_tui::tui::ComponentRef;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::modes::interactive::theme::theme::{BlockStyle, Theme, block_style};

/// The runtime context handed to a tool call.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ToolContext {
    pub session_id: Option<String>,
    pub session_file: Option<String>,
    pub thinking_level: Option<String>,
    /// The model of the current request; `read` uses it to warn about images.
    pub model: Option<notagent_ai::types::Model>,
}

/// `renderShell` — whether the standard tool shell frames the output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RenderShell {
    #[default]
    Default,
    /// The tool renders its own framing.
    SelfManaged,
}

/// One line for the "Available tools" section plus guideline bullets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemPromptContribution {
    pub snippet: &'static str,
    pub guidelines: &'static [&'static str],
}

// ============================================================================
// Rendering (`extensions/types.ts:410-497`)
// ============================================================================

/// `ToolRenderResultOptions` — how the result view is currently displayed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ToolRenderResultOptions {
    /// Whether the result view is expanded.
    pub expanded: bool,
    /// Whether this is a partial/streaming result.
    pub is_partial: bool,
}

/// The result half handed to [`ToolDefinition::render_result`].
///
/// `renderResult` receives `{ content, details }` only; whether the result is an
/// error travels in the context, exactly as in TypeScript
/// (`components/tool-execution.ts:296-301`).
#[derive(Debug, Clone, Copy)]
pub struct ToolRenderResult<'a> {
    pub content: &'a [TextOrImageContent],
    pub details: Option<&'a Value>,
}

/// The shared render state of one tool row (`rendererState: any = {}`).
///
/// Deviation (class 1): TypeScript hands every renderer the same untyped object
/// and each tool writes the fields it wants. The port keeps one boxed value per
/// row and lets each tool claim it with its own state type through
/// [`tool_render_state`]; a row only ever runs the renderers of a single tool,
/// so the slot has exactly one occupant, as in TS.
pub type ToolRenderStateRef = Rc<RefCell<Option<Box<dyn Any>>>>;

/// A fresh, still unclaimed render state — the `{}` of the TS version.
pub fn new_tool_render_state() -> ToolRenderStateRef {
    Rc::new(RefCell::new(None))
}

/// The row state as `T`, initialised on first use.
///
/// The `!is::<T>()` branch is the equivalent of reading a field the previous
/// occupant never wrote: it cannot happen while a row belongs to one tool, and
/// it starts from the default rather than panicking if it ever does.
pub fn tool_render_state<T: Default + Any>(state: &ToolRenderStateRef) -> RefMut<'_, T> {
    let mut slot = state.borrow_mut();
    if !slot.as_ref().is_some_and(|value| value.is::<T>()) {
        *slot = Some(Box::new(T::default()));
    }
    RefMut::map(slot, |value| {
        value
            .as_mut()
            .and_then(|value| value.downcast_mut::<T>())
            .expect("render state initialised above")
    })
}

/// A future the render loop drives on the TUI thread; see
/// [`ToolDefinition::pump_render`].
pub type RenderFuture<'a> = Pin<Box<dyn std::future::Future<Output = ()> + 'a>>;

/// `ToolRenderContext` — everything the renderers of one tool row read.
#[derive(Clone)]
pub struct ToolRenderContext {
    /// Current tool call arguments, shared by the call and result renderers.
    pub args: Value,
    /// Unique id of this tool execution, stable across both renderers.
    pub tool_call_id: String,
    /// Redraw just this tool row.
    pub invalidate: Rc<dyn Fn()>,
    /// The component the same renderer returned last time, if any.
    ///
    /// The built-in tools keep their reusable components in [`Self::state`]
    /// instead, because a `dyn Component` cannot be downcast back to the
    /// concrete type the TS renderers cast to (deviation class 1, same
    /// observable result: one component per slot for the lifetime of the row).
    pub last_component: Option<ComponentRef>,
    /// Shared renderer state for this tool row.
    pub state: ToolRenderStateRef,
    /// Working directory of this tool execution.
    pub cwd: String,
    /// Whether the tool execution has started.
    pub execution_started: bool,
    /// Whether the tool call arguments are complete.
    pub args_complete: bool,
    /// Whether the tool result is partial/streaming.
    pub is_partial: bool,
    /// Whether the result view is expanded.
    pub expanded: bool,
    /// Whether inline images are currently shown in the TUI.
    pub show_images: bool,
    /// Whether the current result is an error.
    pub is_error: bool,
}

impl ToolRenderContext {
    /// A context with the defaults of a freshly created tool row.
    pub fn new(tool_call_id: impl Into<String>, args: Value, cwd: impl Into<String>) -> Self {
        Self {
            args,
            tool_call_id: tool_call_id.into(),
            invalidate: Rc::new(|| {}),
            last_component: None,
            state: new_tool_render_state(),
            cwd: cwd.into(),
            execution_started: false,
            args_complete: false,
            is_partial: true,
            expanded: false,
            show_images: true,
            is_error: false,
        }
    }
}

/// The two `Text` components of a tool row that renders plain text.
///
/// TypeScript reuses `context.lastComponent` and falls back to `new Text("", 0, 0)`;
/// the port keeps the same two components in the row state, one per renderer.
#[derive(Default)]
pub struct TextRenderSlots {
    pub call: Option<Rc<RefCell<Text>>>,
    pub result: Option<Rc<RefCell<Text>>>,
}

fn set_slot_text(slot: &mut Option<Rc<RefCell<Text>>>, text: &str) -> ComponentRef {
    let component = slot.get_or_insert_with(|| Rc::new(RefCell::new(Text::new("", 0, 0))));
    component.borrow_mut().set_text(text);
    Rc::clone(component) as ComponentRef
}

/// `renderCall` of every tool whose call display is a single `Text`.
pub fn render_text_call(context: &ToolRenderContext, text: &str) -> ComponentRef {
    let mut state = tool_render_state::<TextRenderSlots>(&context.state);
    set_slot_text(&mut state.call, text)
}

/// `renderResult` of every tool whose result display is a single `Text`.
///
/// The renderers prefix their result with `\n`, which the standard style's
/// filled box needs as a separator row. The badge style stacks the result
/// directly under the badge line, so the leading blank is dropped here for
/// every text-result tool at once.
pub fn render_text_result(context: &ToolRenderContext, text: &str) -> ComponentRef {
    // The renderers sometimes colour the whole result, putting the `\n`
    // inside the ANSI sequence — so the check is "is the first line visibly
    // empty", not "does the text start with a newline".
    let stripped;
    let text = if block_style() == BlockStyle::Badge {
        match text.split_once('\n') {
            Some((first, rest)) if crate::utils::ansi::strip_ansi(first).trim().is_empty() => {
                stripped = format!("{first}{rest}");
                &stripped
            }
            _ => text,
        }
    } else {
        text
    };
    let mut state = tool_render_state::<TextRenderSlots>(&context.state);
    set_slot_text(&mut state.result, text)
}

/// A raw argument value in a rendered header, spelled as JavaScript spells it.
///
/// The renderers interpolate arguments straight into template literals
/// (`` `limit ${limit}` ``), and they run on incomplete, unvalidated arguments
/// while the model is still streaming them — so anything JSON can hold has to
/// print the way `String(value)` prints it.
pub fn display_arg(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => "null".to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Number(number) => number.as_f64().map_or_else(
            || number.to_string(),
            notagent_ai::utils::js_number::to_js_string,
        ),
        Value::Array(entries) => entries
            .iter()
            // `String([null])` is `""`: array elements print empty when nullish.
            .map(|entry| match entry {
                Value::Null => String::new(),
                entry => display_arg(entry),
            })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".to_string(),
    }
}

/// `ToolDefinition` — the coding agent's view of a tool.
pub trait ToolDefinition: Send + Sync {
    /// Tool name, used in LLM tool calls.
    fn name(&self) -> &str;
    /// Human-readable label for the UI.
    fn label(&self) -> &str;
    /// Description for the LLM.
    ///
    /// Deviation (class 1): TS uses a getter so a tool can change what it
    /// advertises without being rebuilt. A Rust tool does the same by returning
    /// one of the descriptions it owns, so the borrow stays valid.
    fn description(&self) -> &str;
    /// One-line snippet for the "Available tools" section of the system prompt.
    fn prompt_snippet(&self) -> Option<&str> {
        None
    }
    /// Guideline bullets appended to the system prompt while this tool is active.
    fn prompt_guidelines(&self) -> Vec<String> {
        Vec::new()
    }
    /// JSON schema of the parameters; owned by the tool for the same reason.
    fn parameters(&self) -> &Value;
    fn constrained_sampling(&self) -> Option<&ConstrainedSampling> {
        None
    }
    fn render_shell(&self) -> RenderShell {
        RenderShell::Default
    }
    /// Compatibility shim applied to raw arguments before schema validation.
    fn prepare_arguments(&self, args: Value) -> Value {
        args
    }
    fn execution_mode(&self) -> Option<ToolExecutionMode> {
        None
    }
    fn execute<'a>(
        &'a self,
        tool_call_id: &'a str,
        params: Value,
        signal: Option<CancellationToken>,
        on_update: Option<AgentToolUpdateCallback>,
        context: Option<ToolContext>,
    ) -> BoxFuture<'a, Result<AgentToolResult, ToolExecutionError>>;

    /// `renderCall` — the tool call display. `None` renders the fallback header.
    fn render_call(
        &self,
        args: &Value,
        theme: &Theme,
        context: &ToolRenderContext,
    ) -> Option<ComponentRef> {
        let _ = (args, theme, context);
        None
    }

    /// `renderResult` — the tool result display. `None` renders the plain text
    /// output of the result.
    fn render_result(
        &self,
        result: ToolRenderResult<'_>,
        options: ToolRenderResultOptions,
        theme: &Theme,
        context: &ToolRenderContext,
    ) -> Option<ComponentRef> {
        let _ = (result, options, theme, context);
        None
    }

    /// When this row needs to be rendered again without any input.
    ///
    /// Deviation (class 1): the TS renderers keep the row moving with
    /// `setInterval`/`setTimeout` handles they store in the render state. A
    /// callback would need `&mut` access to the row, so the port reports the
    /// deadline and the render loop redraws — the same seam the TUI already uses
    /// (`Loader::next_frame_deadline`, `Editor::autocomplete_deadline`).
    fn render_deadline(&self, context: &ToolRenderContext) -> Option<Instant> {
        let _ = context;
        None
    }

    /// Run the render-side work this row started, if any.
    ///
    /// Deviation (class 1): where TS continues a promise inside the renderer
    /// (`edit` computes its diff preview), the port hands the work back to the
    /// render loop, which awaits it on the TUI thread and redraws afterwards —
    /// the same seam as `Editor::pump_autocomplete`; `None` means there is
    /// nothing to await.
    fn pump_render<'a>(&'a self, context: &'a ToolRenderContext) -> Option<RenderFuture<'a>> {
        let _ = context;
        None
    }
}

/// Wrap a [`ToolDefinition`] into an [`AgentTool`] for the agent loop.
///
/// `description` and `parameters` are forwarded rather than copied, so a tool
/// whose capabilities depend on session state can change what it advertises
/// without being rebuilt.
pub struct WrappedToolDefinition {
    definition: Arc<dyn ToolDefinition>,
    context_factory: Option<Arc<dyn Fn() -> ToolContext + Send + Sync>>,
}

impl std::fmt::Debug for WrappedToolDefinition {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WrappedToolDefinition")
            .field("name", &self.definition.name())
            .finish()
    }
}

pub fn wrap_tool_definition(
    definition: Arc<dyn ToolDefinition>,
    context_factory: Option<Arc<dyn Fn() -> ToolContext + Send + Sync>>,
) -> Arc<dyn AgentTool> {
    Arc::new(WrappedToolDefinition {
        definition,
        context_factory,
    })
}

pub fn wrap_tool_definitions(
    definitions: Vec<Arc<dyn ToolDefinition>>,
    context_factory: Option<Arc<dyn Fn() -> ToolContext + Send + Sync>>,
) -> Vec<Arc<dyn AgentTool>> {
    definitions
        .into_iter()
        .map(|definition| wrap_tool_definition(definition, context_factory.clone()))
        .collect()
}

impl AgentTool for WrappedToolDefinition {
    fn name(&self) -> &str {
        self.definition.name()
    }

    fn label(&self) -> &str {
        self.definition.label()
    }

    fn description(&self) -> &str {
        self.definition.description()
    }

    fn parameters(&self) -> &Value {
        self.definition.parameters()
    }

    fn constrained_sampling(&self) -> Option<&ConstrainedSampling> {
        self.definition.constrained_sampling()
    }

    fn prepare_arguments(&self, args: Value) -> Value {
        self.definition.prepare_arguments(args)
    }

    fn execution_mode(&self) -> Option<ToolExecutionMode> {
        self.definition.execution_mode()
    }

    fn execute<'a>(
        &'a self,
        tool_call_id: &'a str,
        params: Value,
        signal: Option<CancellationToken>,
        on_update: Option<AgentToolUpdateCallback>,
    ) -> BoxFuture<'a, Result<AgentToolResult, ToolExecutionError>> {
        let context = self.context_factory.as_ref().map(|factory| factory());
        self.definition
            .execute(tool_call_id, params, signal, on_update, context)
    }
}

/// Port of `createToolDefinitionFromAgentTool` from
/// `packages/coding-agent/src/core/tools/tool-definition-wrapper.ts`.
///
/// The other direction of [`wrap_tool_definition`]: a plain tool becomes a
/// definition so the session can keep a definition-first registry even when a
/// caller supplies tools rather than definitions.
pub struct AgentToolDefinition {
    tool: Arc<dyn AgentTool>,
}

pub fn create_tool_definition_from_agent_tool(tool: Arc<dyn AgentTool>) -> Arc<dyn ToolDefinition> {
    Arc::new(AgentToolDefinition { tool })
}

impl ToolDefinition for AgentToolDefinition {
    fn name(&self) -> &str {
        self.tool.name()
    }

    fn label(&self) -> &str {
        self.tool.label()
    }

    fn description(&self) -> &str {
        self.tool.description()
    }

    fn parameters(&self) -> &Value {
        self.tool.parameters()
    }

    fn constrained_sampling(&self) -> Option<&ConstrainedSampling> {
        self.tool.constrained_sampling()
    }

    fn prepare_arguments(&self, args: Value) -> Value {
        self.tool.prepare_arguments(args)
    }

    fn execution_mode(&self) -> Option<ToolExecutionMode> {
        self.tool.execution_mode()
    }

    fn execute<'a>(
        &'a self,
        tool_call_id: &'a str,
        params: Value,
        signal: Option<CancellationToken>,
        on_update: Option<AgentToolUpdateCallback>,
        _context: Option<ToolContext>,
    ) -> BoxFuture<'a, Result<AgentToolResult, ToolExecutionError>> {
        self.tool.execute(tool_call_id, params, signal, on_update)
    }
}
