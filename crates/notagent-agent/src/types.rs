use std::collections::BTreeSet;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use notagent_ai::types::{
    AssistantMessage, AssistantMessageEvent, Context, ImageContent, Message, Model,
    SimpleStreamOptions, TextContent, TextOrImageContent, Tool, ToolCall, ToolResultMessage, Usage,
};
use notagent_ai::utils::event_stream::AssistantMessageEventStream;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::harness::messages::{
    BashExecutionMessage, BranchSummaryMessage, CompactionSummaryMessage, CustomMessage,
};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Streaming function used by the agent loop.
/// `AssistantMessage` (`stopReason` `error`/`aborted` plus `errorMessage`) kodiert.
pub type StreamFn = Arc<
    dyn Fn(
            Model,
            Context,
            Option<SimpleStreamOptions>,
        ) -> BoxFuture<'static, AssistantMessageEventStream>
        + Send
        + Sync,
>;

/// `ToolExecutionMode = "sequential" | "parallel"`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolExecutionMode {
    Sequential,
    Parallel,
}

/// `QueueMode = "all" | "one-at-a-time"`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum QueueMode {
    All,
    OneAtATime,
}

pub type AgentToolCall = ToolCall;

/// `BeforeToolCallResult { block?, reason?, terminate? }`
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct BeforeToolCallResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminate: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct AfterToolCallResult {
    pub content: Option<Vec<TextOrImageContent>>,
    pub details: Option<Value>,
    pub is_error: Option<bool>,
    pub usage: Option<Usage>,
    pub terminate: Option<bool>,
}

/// `BeforeToolCallContext`
#[derive(Debug, Clone)]
pub struct BeforeToolCallContext {
    pub assistant_message: AssistantMessage,
    pub tool_call: AgentToolCall,
    pub args: Value,
    pub context: AgentContext,
}

/// `AfterToolCallContext`
#[derive(Debug, Clone)]
pub struct AfterToolCallContext {
    pub assistant_message: AssistantMessage,
    pub tool_call: AgentToolCall,
    pub args: Value,
    pub result: AgentToolResult,
    pub is_error: bool,
    pub context: AgentContext,
}

/// `ShouldStopAfterTurnContext`
#[derive(Debug, Clone)]
pub struct ShouldStopAfterTurnContext {
    pub message: AssistantMessage,
    pub tool_results: Vec<ToolResultMessage>,
    pub context: AgentContext,
    pub new_messages: Vec<AgentMessage>,
}

/// `PrepareNextTurnContext extends ShouldStopAfterTurnContext`
pub type PrepareNextTurnContext = ShouldStopAfterTurnContext;

#[derive(Debug, Clone, Default)]
pub struct AgentLoopTurnUpdate {
    pub context: Option<AgentContext>,
    pub model: Option<Model>,
    pub thinking_level: Option<ThinkingLevel>,
}

pub type ConvertToLlmFn =
    Arc<dyn Fn(Vec<AgentMessage>) -> BoxFuture<'static, Vec<Message>> + Send + Sync>;

/// `transformContext?`
pub type TransformContextFn = Arc<
    dyn Fn(Vec<AgentMessage>, Option<CancellationToken>) -> BoxFuture<'static, Vec<AgentMessage>>
        + Send
        + Sync,
>;

/// Prepares the authoritative history before each request, after tool results
/// and queued input have been recorded. Failure prevents the provider call.
pub type PrepareContextFn = Arc<
    dyn Fn(
            Vec<AgentMessage>,
            Option<CancellationToken>,
        ) -> BoxFuture<'static, Result<Vec<AgentMessage>, crate::agent::AgentError>>
        + Send
        + Sync,
>;

/// Resolves the API key for each turn so short-lived credentials can refresh.
pub type GetApiKeyFn = Arc<dyn Fn(String) -> BoxFuture<'static, Option<String>> + Send + Sync>;

/// `shouldStopAfterTurn?`
pub type ShouldStopAfterTurnFn =
    Arc<dyn Fn(ShouldStopAfterTurnContext) -> BoxFuture<'static, bool> + Send + Sync>;

/// `prepareNextTurn?`
pub type PrepareNextTurnFn = Arc<
    dyn Fn(PrepareNextTurnContext) -> BoxFuture<'static, Option<AgentLoopTurnUpdate>> + Send + Sync,
>;

/// `getSteeringMessages?` / `getFollowUpMessages?`
pub type GetQueuedMessagesFn = Arc<dyn Fn() -> BoxFuture<'static, Vec<AgentMessage>> + Send + Sync>;

/// `beforeToolCall?`
pub type BeforeToolCallFn = Arc<
    dyn Fn(
            BeforeToolCallContext,
            Option<CancellationToken>,
        ) -> BoxFuture<'static, Option<BeforeToolCallResult>>
        + Send
        + Sync,
>;

/// `afterToolCall?`
pub type AfterToolCallFn = Arc<
    dyn Fn(
            AfterToolCallContext,
            Option<CancellationToken>,
        ) -> BoxFuture<'static, Option<AfterToolCallResult>>
        + Send
        + Sync,
>;

/// `AgentLoopConfig extends SimpleStreamOptions`.
#[derive(Clone)]
pub struct AgentLoopConfig {
    pub base: SimpleStreamOptions,
    pub model: Model,
    pub convert_to_llm: ConvertToLlmFn,
    pub transform_context: Option<TransformContextFn>,
    pub prepare_context: Option<PrepareContextFn>,
    pub get_api_key: Option<GetApiKeyFn>,
    pub should_stop_after_turn: Option<ShouldStopAfterTurnFn>,
    pub prepare_next_turn: Option<PrepareNextTurnFn>,
    pub get_steering_messages: Option<GetQueuedMessagesFn>,
    pub get_follow_up_messages: Option<GetQueuedMessagesFn>,
    /// Default: `Parallel`.
    pub tool_execution: Option<ToolExecutionMode>,
    pub before_tool_call: Option<BeforeToolCallFn>,
    pub after_tool_call: Option<AfterToolCallFn>,
}

impl fmt::Debug for AgentLoopConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentLoopConfig")
            .field("model", &self.model.id)
            .field("transform_context", &self.transform_context.is_some())
            .field("prepare_context", &self.prepare_context.is_some())
            .field("get_api_key", &self.get_api_key.is_some())
            .field(
                "should_stop_after_turn",
                &self.should_stop_after_turn.is_some(),
            )
            .field("prepare_next_turn", &self.prepare_next_turn.is_some())
            .field(
                "get_steering_messages",
                &self.get_steering_messages.is_some(),
            )
            .field(
                "get_follow_up_messages",
                &self.get_follow_up_messages.is_some(),
            )
            .field("tool_execution", &self.tool_execution)
            .field("before_tool_call", &self.before_tool_call.is_some())
            .field("after_tool_call", &self.after_tool_call.is_some())
            .finish()
    }
}

/// `ThinkingLevel = "off" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max"`
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingLevel {
    Off,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl From<notagent_ai::types::ModelThinkingLevel> for ThinkingLevel {
    fn from(level: notagent_ai::types::ModelThinkingLevel) -> Self {
        use notagent_ai::types::ModelThinkingLevel;
        match level {
            ModelThinkingLevel::Off => ThinkingLevel::Off,
            ModelThinkingLevel::Minimal => ThinkingLevel::Minimal,
            ModelThinkingLevel::Low => ThinkingLevel::Low,
            ModelThinkingLevel::Medium => ThinkingLevel::Medium,
            ModelThinkingLevel::High => ThinkingLevel::High,
            ModelThinkingLevel::Xhigh => ThinkingLevel::Xhigh,
            ModelThinkingLevel::Max => ThinkingLevel::Max,
        }
    }
}

impl ThinkingLevel {
    /// `thinkingLevel "off"` → `reasoning: undefined` (Faktenbericht B4).
    pub fn to_reasoning(self) -> Option<notagent_ai::types::ThinkingLevel> {
        use notagent_ai::types::ThinkingLevel as Reasoning;
        match self {
            ThinkingLevel::Off => None,
            ThinkingLevel::Minimal => Some(Reasoning::Minimal),
            ThinkingLevel::Low => Some(Reasoning::Low),
            ThinkingLevel::Medium => Some(Reasoning::Medium),
            ThinkingLevel::High => Some(Reasoning::High),
            ThinkingLevel::Xhigh => Some(Reasoning::Xhigh),
            ThinkingLevel::Max => Some(Reasoning::Max),
        }
    }
}

/// `AgentMessage = Message | CustomAgentMessages[keyof CustomAgentMessages]`
/// Agent message variants understood by the loop.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "camelCase")]
pub enum AgentMessage {
    User(notagent_ai::types::UserMessage),
    Assistant(AssistantMessage),
    ToolResult(ToolResultMessage),
    BashExecution(BashExecutionMessage),
    Custom(CustomMessage),
    BranchSummary(BranchSummaryMessage),
    CompactionSummary(CompactionSummaryMessage),
}

impl From<Message> for AgentMessage {
    fn from(message: Message) -> Self {
        match message {
            Message::User(message) => AgentMessage::User(message),
            Message::Assistant(message) => AgentMessage::Assistant(message),
            Message::ToolResult(message) => AgentMessage::ToolResult(message),
        }
    }
}

impl AgentMessage {
    pub fn as_llm_message(&self) -> Option<Message> {
        match self {
            AgentMessage::User(message) => Some(Message::User(message.clone())),
            AgentMessage::Assistant(message) => Some(Message::Assistant(message.clone())),
            AgentMessage::ToolResult(message) => Some(Message::ToolResult(message.clone())),
            _ => None,
        }
    }

    pub fn timestamp(&self) -> i64 {
        match self {
            AgentMessage::User(message) => message.timestamp,
            AgentMessage::Assistant(message) => message.timestamp,
            AgentMessage::ToolResult(message) => message.timestamp,
            AgentMessage::BashExecution(message) => message.timestamp,
            AgentMessage::Custom(message) => message.timestamp,
            AgentMessage::BranchSummary(message) => message.timestamp,
            AgentMessage::CompactionSummary(message) => message.timestamp,
        }
    }
}

/// Public agent state.
#[derive(Clone)]
pub struct AgentState {
    pub system_prompt: String,
    pub model: Model,
    pub thinking_level: ThinkingLevel,
    pub tools: Vec<Arc<dyn AgentTool>>,
    pub messages: Vec<AgentMessage>,
    pub is_streaming: bool,
    pub streaming_message: Option<AgentMessage>,
    pub pending_tool_calls: BTreeSet<String>,
    pub error_message: Option<String>,
}

impl fmt::Debug for AgentState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentState")
            .field("system_prompt", &self.system_prompt)
            .field("model", &self.model.id)
            .field("thinking_level", &self.thinking_level)
            .field(
                "tools",
                &self
                    .tools
                    .iter()
                    .map(|tool| tool.name().to_string())
                    .collect::<Vec<_>>(),
            )
            .field("messages", &self.messages.len())
            .field("is_streaming", &self.is_streaming)
            .field("streaming_message", &self.streaming_message)
            .field("pending_tool_calls", &self.pending_tool_calls)
            .field("error_message", &self.error_message)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct AgentToolResult {
    pub content: Vec<TextOrImageContent>,
    pub details: Option<Value>,
    pub usage: Option<Usage>,
    pub added_tool_names: Option<Vec<String>>,
    pub terminate: Option<bool>,
}

impl AgentToolResult {
    pub fn text(text: impl Into<String>) -> Self {
        AgentToolResult {
            content: vec![TextOrImageContent::Text(TextContent::new(text))],
            details: Some(Value::Object(Default::default())),
            ..Default::default()
        }
    }

    pub fn image(data: impl Into<String>, mime_type: impl Into<String>) -> TextOrImageContent {
        TextOrImageContent::Image(ImageContent {
            data: data.into(),
            mime_type: mime_type.into(),
        })
    }
}

pub type AgentToolUpdateCallback = Arc<dyn Fn(AgentToolResult) + Send + Sync>;

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct ToolExecutionError {
    pub message: String,
}

impl ToolExecutionError {
    pub fn new(message: impl Into<String>) -> Self {
        ToolExecutionError {
            message: message.into(),
        }
    }
}

pub trait AgentTool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> &Value;
    fn constrained_sampling(&self) -> Option<&notagent_ai::types::ConstrainedSampling> {
        None
    }
    fn label(&self) -> &str;
    fn prepare_arguments(&self, args: Value) -> Value {
        args
    }
    fn execute<'a>(
        &'a self,
        tool_call_id: &'a str,
        params: Value,
        signal: Option<CancellationToken>,
        on_update: Option<AgentToolUpdateCallback>,
    ) -> BoxFuture<'a, Result<AgentToolResult, ToolExecutionError>>;
    fn execution_mode(&self) -> Option<ToolExecutionMode> {
        None
    }
    fn to_tool(&self) -> Tool {
        Tool {
            name: self.name().to_string(),
            description: self.description().to_string(),
            parameters: required_first(self.parameters()),
            constrained_sampling: self.constrained_sampling().cloned(),
        }
    }
}

/// Moves `required` ahead of `properties` at every level of a JSON schema.
/// A model reads the schema in the order it arrives and commits to the shape of
/// the call early; a `required` list it reaches only after the whole property
/// block has gone by arrives too late to steer what it is already writing.
/// Serialisation here preserves insertion order, so the order in the map is the
/// order on the wire.
/// Applied at the one place every tool passes through rather than in each
/// schema, so a tool added later cannot forget it and a schema that arrives
/// from an MCP server — which nobody here wrote — is covered too.
pub fn required_first(schema: &Value) -> Value {
    let Value::Object(fields) = schema else {
        return schema.clone();
    };
    let mut reordered = serde_json::Map::with_capacity(fields.len());
    // `type` stays in front: it says what the rest of the object even is.
    if let Some(kind) = fields.get("type") {
        reordered.insert("type".to_string(), kind.clone());
    }
    if let Some(required) = fields.get("required") {
        reordered.insert("required".to_string(), required.clone());
    }
    for (key, value) in fields {
        if key == "type" || key == "required" {
            continue;
        }
        let value = match (key.as_str(), value) {
            // The nested schemas a model still has to read in order: one per
            // property, the element schema of an array, and the branches of a
            // union.
            ("properties", Value::Object(properties)) => Value::Object(
                properties
                    .iter()
                    .map(|(name, property)| (name.clone(), required_first(property)))
                    .collect(),
            ),
            ("items", value) => required_first(value),
            ("anyOf" | "oneOf" | "allOf", Value::Array(branches)) => {
                Value::Array(branches.iter().map(required_first).collect())
            }
            _ => value.clone(),
        };
        reordered.insert(key.clone(), value);
    }
    Value::Object(reordered)
}

#[derive(Clone, Default)]
pub struct AgentContext {
    pub system_prompt: String,
    pub messages: Vec<AgentMessage>,
    pub tools: Option<Vec<Arc<dyn AgentTool>>>,
}

impl fmt::Debug for AgentContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentContext")
            .field("system_prompt", &self.system_prompt)
            .field("messages", &self.messages.len())
            .field(
                "tools",
                &self.tools.as_ref().map(|tools| {
                    tools
                        .iter()
                        .map(|tool| tool.name().to_string())
                        .collect::<Vec<_>>()
                }),
            )
            .finish()
    }
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum AgentEvent {
    AgentStart,
    AgentEnd {
        messages: Vec<AgentMessage>,
    },
    TurnStart,
    TurnEnd {
        message: AgentMessage,
        tool_results: Vec<ToolResultMessage>,
    },
    MessageStart {
        message: AgentMessage,
    },
    MessageUpdate {
        message: AgentMessage,
        assistant_message_event: AssistantMessageEvent,
    },
    MessageEnd {
        message: AgentMessage,
    },
    ToolExecutionStart {
        tool_call_id: String,
        tool_name: String,
        args: Value,
    },
    ToolExecutionUpdate {
        tool_call_id: String,
        tool_name: String,
        args: Value,
        partial_result: AgentToolResult,
    },
    ToolExecutionEnd {
        tool_call_id: String,
        tool_name: String,
        result: AgentToolResult,
        is_error: bool,
    },
}

impl AgentEvent {
    pub fn type_name(&self) -> &'static str {
        match self {
            AgentEvent::AgentStart => "agent_start",
            AgentEvent::AgentEnd { .. } => "agent_end",
            AgentEvent::TurnStart => "turn_start",
            AgentEvent::TurnEnd { .. } => "turn_end",
            AgentEvent::MessageStart { .. } => "message_start",
            AgentEvent::MessageUpdate { .. } => "message_update",
            AgentEvent::MessageEnd { .. } => "message_end",
            AgentEvent::ToolExecutionStart { .. } => "tool_execution_start",
            AgentEvent::ToolExecutionUpdate { .. } => "tool_execution_update",
            AgentEvent::ToolExecutionEnd { .. } => "tool_execution_end",
        }
    }
}

#[cfg(test)]
mod required_first_tests {
    use super::required_first;
    use serde_json::json;

    /// The order in the map is the order on the wire, so the check is on the
    /// serialised form rather than on equality of values.
    fn keys(schema: &serde_json::Value) -> Vec<String> {
        schema
            .as_object()
            .expect("object")
            .keys()
            .cloned()
            .collect()
    }

    #[test]
    fn required_arrives_before_the_properties_it_constrains() {
        let reordered = required_first(&json!({
            "type": "object",
            "properties": { "path": { "type": "string" } },
            "required": ["path"],
        }));
        assert_eq!(keys(&reordered), ["type", "required", "properties"]);
        assert_eq!(reordered["required"], json!(["path"]));
        assert_eq!(reordered["properties"]["path"]["type"], json!("string"));
    }

    #[test]
    fn every_nested_schema_is_reordered_too() {
        // The shape `edit` uses: an array of objects, each with its own
        // required list.
        let reordered = required_first(&json!({
            "type": "object",
            "properties": {
                "edits": {
                    "type": "array",
                    "items": {
                        "properties": {
                            "old_string": { "type": "string" },
                            "new_string": { "type": "string" },
                        },
                        "type": "object",
                        "required": ["old_string", "new_string"],
                    },
                },
            },
            "required": ["path", "edits"],
        }));
        assert_eq!(
            keys(&reordered["properties"]["edits"]["items"]),
            ["type", "required", "properties"]
        );
    }

    #[test]
    fn a_schema_with_nothing_to_move_keeps_its_order() {
        let schema = json!({ "type": "object", "properties": {} });
        assert_eq!(keys(&required_first(&schema)), ["type", "properties"]);
        // And a schema that is not an object at all is handed back untouched.
        assert_eq!(required_first(&json!("string")), json!("string"));
    }

    #[test]
    fn union_branches_are_reordered() {
        let reordered = required_first(&json!({
            "anyOf": [
                { "properties": { "a": {} }, "type": "object", "required": ["a"] },
            ],
        }));
        assert_eq!(
            keys(&reordered["anyOf"][0]),
            ["type", "required", "properties"]
        );
    }
}
