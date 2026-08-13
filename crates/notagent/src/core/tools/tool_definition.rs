//! Native replacement for `packages/coding-agent/src/core/extensions/types.ts`
//! (`ToolDefinition`) and `core/tools/tool-definition-wrapper.ts`.
//!
//! Deviation (class 2): the extension system is removed, so `ExtensionContext`
//! collapses to [`ToolContext`] with exactly the fields the built-in tools read
//! at runtime (`plans/facts/extension-boundary.md` §2.4: only bash uses it, for
//! `NOTAGENT_SESSION_ID`, the session file and `NOTAGENT_REASONING_LEVEL`).
//!
//! `renderCall`/`renderResult` are not part of this trait: they return TUI
//! components and need the theme, so they are wired in task 13 together with the
//! interactive mode.

use std::sync::Arc;

use notagent_agent::types::{
    AgentTool, AgentToolResult, AgentToolUpdateCallback, BoxFuture, ToolExecutionError,
    ToolExecutionMode,
};
use notagent_ai::types::ConstrainedSampling;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

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
