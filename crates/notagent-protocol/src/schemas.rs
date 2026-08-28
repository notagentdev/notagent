use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize, Serializer};

pub const PROTOCOL_VERSION: u64 = 1;

pub type JsonValue = serde_json::Value;

// --- Literal tags (Type.Literal) --------------------------------------------

macro_rules! literal_tag {
    ($(#[$meta:meta])* $name:ident, str $value:literal) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
        pub struct $name;

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str($value)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let value = String::deserialize(deserializer)?;
                if value == $value { Ok($name) } else { Err(de::Error::invalid_value(de::Unexpected::Str(&value), &$value)) }
            }
        }
    };
    ($(#[$meta:meta])* $name:ident, bool $value:literal) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
        pub struct $name;

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_bool($value)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let value = bool::deserialize(deserializer)?;
                if value == $value { Ok($name) } else { Err(de::Error::invalid_value(de::Unexpected::Bool(value), &stringify!($value))) }
            }
        }
    };
    ($(#[$meta:meta])* $name:ident, u64 $value:expr) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
        pub struct $name;

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_u64($value)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let value = u64::deserialize(deserializer)?;
                if value == $value { Ok($name) } else { Err(de::Error::invalid_value(de::Unexpected::Unsigned(value), &stringify!($value))) }
            }
        }
    };
}

literal_tag!(HelloTag, str "hello");
literal_tag!(HelloErrorTag, str "hello_error");
literal_tag!(RequestTag, str "request");
literal_tag!(ResponseTag, str "response");
literal_tag!(EventTag, str "event");
literal_tag!(TextTag, str "text");
literal_tag!(ThinkingTag, str "thinking");
literal_tag!(ImageTag, str "image");
literal_tag!(ToolCallTag, str "toolCall");
literal_tag!(UserTag, str "user");
literal_tag!(AssistantTag, str "assistant");
literal_tag!(ToolTag, str "tool");
literal_tag!(StreamingTag, str "streaming");
literal_tag!(CompleteTag, str "complete");
literal_tag!(ErrorTag, str "error");
literal_tag!(AbortedTag, str "aborted");
literal_tag!(RunningTag, str "running");
literal_tag!(ItemStartedTag, str "item_started");
literal_tag!(AssistantDeltaTag, str "assistant_delta");
literal_tag!(ItemUpdatedTag, str "item_updated");
literal_tag!(ItemFinishedTag, str "item_finished");
literal_tag!(ServerSnapshotTag, str "server_snapshot");
literal_tag!(SessionSnapshotTag, str "session_snapshot");
literal_tag!(SessionProgressTag, str "session_progress");
literal_tag!(SessionRemovedTag, str "session_removed");
literal_tag!(ListTag, str "list");
literal_tag!(CreateTag, str "create");
literal_tag!(AttachTag, str "attach");
literal_tag!(DetachTag, str "detach");
literal_tag!(PromptTag, str "prompt");
literal_tag!(SteerTag, str "steer");
literal_tag!(AbortTag, str "abort");
literal_tag!(SetModelTag, str "set_model");
literal_tag!(SetThinkingTag, str "set_thinking");
literal_tag!(TrueTag, bool true);
literal_tag!(FalseTag, bool false);
literal_tag!(
    /// `Type.Literal(PROTOCOL_VERSION)`.
    ProtocolVersionTag,
    u64 PROTOCOL_VERSION
);

// --- Constraint helpers ------------------------------------------------------

/// `Type.String({ minLength: 1 })` (identical to `IdSchema`).
fn de_non_empty_string<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let value = String::deserialize(deserializer)?;
    if value.is_empty() {
        return Err(de::Error::invalid_length(
            0,
            &"a string with at least 1 character",
        ));
    }
    Ok(value)
}

fn de_optional_non_empty_string<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    de_non_empty_string(deserializer).map(Some)
}

/// `Type.Optional(...)`: a missing field is allowed, an explicit `null` is not.
fn de_optional<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}

/// `Type.Integer({ minimum: 1 })`.
fn de_positive_integer<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    let value = u64::deserialize(deserializer)?;
    if value < 1 {
        return Err(de::Error::invalid_value(
            de::Unexpected::Unsigned(value),
            &"an integer >= 1",
        ));
    }
    Ok(value)
}

/// `Type.Number({ minimum: 0 })`.
fn de_non_negative_number<'de, D: Deserializer<'de>>(deserializer: D) -> Result<f64, D::Error> {
    let value = f64::deserialize(deserializer)?;
    // `Type.Number({ minimum: 0 })`: NaN does not satisfy `value >= minimum`.
    if value < 0.0 || value.is_nan() {
        return Err(de::Error::invalid_value(
            de::Unexpected::Float(value),
            &"a number >= 0",
        ));
    }
    Ok(value)
}

/// `Type.Array(..., { minItems: 1 })`.
fn de_non_empty_vec<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Vec<T>, D::Error> {
    let value = Vec::<T>::deserialize(deserializer)?;
    if value.is_empty() {
        return Err(de::Error::invalid_length(
            0,
            &"an array with at least 1 item",
        ));
    }
    Ok(value)
}

// --- Base types --------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

/// Matches AgentHarnessPhase so adapters do not need a second phase vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionPhase {
    Idle,
    Turn,
    Compaction,
    BranchSummary,
    Retry,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelRef {
    #[serde(deserialize_with = "de_non_empty_string")]
    pub provider: String,
    #[serde(deserialize_with = "de_non_empty_string")]
    pub id: String,
}

impl ModelRef {
    pub fn new(provider: impl Into<String>, id: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            id: id.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ModelCost {
    #[serde(deserialize_with = "de_non_negative_number")]
    pub input: f64,
    #[serde(deserialize_with = "de_non_negative_number")]
    pub output: f64,
    #[serde(deserialize_with = "de_non_negative_number")]
    pub cache_read: f64,
    #[serde(deserialize_with = "de_non_negative_number")]
    pub cache_write: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelInput {
    Text,
    Image,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ModelMetadata {
    #[serde(deserialize_with = "de_non_empty_string")]
    pub provider: String,
    #[serde(deserialize_with = "de_non_empty_string")]
    pub id: String,
    #[serde(deserialize_with = "de_non_empty_string")]
    pub name: String,
    #[serde(deserialize_with = "de_non_empty_string")]
    pub api: String,
    pub reasoning: bool,
    pub input: Vec<ModelInput>,
    #[serde(deserialize_with = "de_positive_integer")]
    pub context_window: u64,
    #[serde(deserialize_with = "de_positive_integer")]
    pub max_tokens: u64,
    pub cost: ModelCost,
    #[serde(deserialize_with = "de_non_empty_vec")]
    pub supported_thinking_levels: Vec<ThinkingLevel>,
    pub authenticated: bool,
}

// --- Content -----------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextContent {
    #[serde(rename = "type")]
    pub kind: TextTag,
    pub text: String,
}

impl TextContent {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            kind: TextTag,
            text: text.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThinkingContent {
    #[serde(rename = "type")]
    pub kind: ThinkingTag,
    pub thinking: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_optional"
    )]
    pub redacted: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ImageContent {
    #[serde(rename = "type")]
    pub kind: ImageTag,
    pub data: String,
    #[serde(deserialize_with = "de_non_empty_string")]
    pub mime_type: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ToolCallContent {
    #[serde(rename = "type")]
    pub kind: ToolCallTag,
    #[serde(deserialize_with = "de_non_empty_string")]
    pub tool_call_id: String,
    #[serde(deserialize_with = "de_non_empty_string")]
    pub tool_name: String,
    pub input: JsonValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum UserContent {
    Text(TextContent),
    Image(ImageContent),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AssistantContent {
    Text(TextContent),
    Thinking(ThinkingContent),
    ToolCall(ToolCallContent),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolContent {
    Text(TextContent),
    Image(ImageContent),
}

// --- Usage -------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct UsageCost {
    #[serde(deserialize_with = "de_non_negative_number")]
    pub input: f64,
    #[serde(deserialize_with = "de_non_negative_number")]
    pub output: f64,
    #[serde(deserialize_with = "de_non_negative_number")]
    pub cache_read: f64,
    #[serde(deserialize_with = "de_non_negative_number")]
    pub cache_write: f64,
    #[serde(deserialize_with = "de_non_negative_number")]
    pub total: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_optional"
    )]
    pub reasoning: Option<u64>,
    pub total_tokens: u64,
    pub cost: UsageCost,
}

// --- Transcript --------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserTranscriptItem {
    #[serde(deserialize_with = "de_non_empty_string")]
    pub id: String,
    pub role: UserTag,
    pub content: Vec<UserContent>,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AssistantStopReason {
    Stop,
    Length,
    ToolUse,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct StreamingAssistantTranscriptItem {
    #[serde(deserialize_with = "de_non_empty_string")]
    pub id: String,
    pub role: AssistantTag,
    pub content: Vec<AssistantContent>,
    pub model: ModelRef,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_optional_non_empty_string"
    )]
    pub response_model: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_optional"
    )]
    pub usage: Option<Usage>,
    pub timestamp: u64,
    pub status: StreamingTag,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CompleteAssistantTranscriptItem {
    #[serde(deserialize_with = "de_non_empty_string")]
    pub id: String,
    pub role: AssistantTag,
    pub content: Vec<AssistantContent>,
    pub model: ModelRef,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_optional_non_empty_string"
    )]
    pub response_model: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_optional"
    )]
    pub usage: Option<Usage>,
    pub timestamp: u64,
    pub status: CompleteTag,
    pub stop_reason: AssistantStopReason,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ErrorAssistantTranscriptItem {
    #[serde(deserialize_with = "de_non_empty_string")]
    pub id: String,
    pub role: AssistantTag,
    pub content: Vec<AssistantContent>,
    pub model: ModelRef,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_optional_non_empty_string"
    )]
    pub response_model: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_optional"
    )]
    pub usage: Option<Usage>,
    pub timestamp: u64,
    pub status: ErrorTag,
    pub stop_reason: ErrorTag,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_optional_non_empty_string"
    )]
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AbortedAssistantTranscriptItem {
    #[serde(deserialize_with = "de_non_empty_string")]
    pub id: String,
    pub role: AssistantTag,
    pub content: Vec<AssistantContent>,
    pub model: ModelRef,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_optional_non_empty_string"
    )]
    pub response_model: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_optional"
    )]
    pub usage: Option<Usage>,
    pub timestamp: u64,
    pub status: AbortedTag,
    pub stop_reason: AbortedTag,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_optional"
    )]
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AssistantTranscriptItem {
    Streaming(StreamingAssistantTranscriptItem),
    Complete(CompleteAssistantTranscriptItem),
    Error(ErrorAssistantTranscriptItem),
    Aborted(AbortedAssistantTranscriptItem),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunningToolTranscriptItem {
    #[serde(deserialize_with = "de_non_empty_string")]
    pub id: String,
    pub role: ToolTag,
    #[serde(deserialize_with = "de_non_empty_string")]
    pub tool_call_id: String,
    #[serde(deserialize_with = "de_non_empty_string")]
    pub tool_name: String,
    pub input: JsonValue,
    pub content: Vec<ToolContent>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_optional"
    )]
    pub details: Option<JsonValue>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_optional"
    )]
    pub usage: Option<Usage>,
    pub timestamp: u64,
    pub status: RunningTag,
    pub is_error: FalseTag,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CompleteToolTranscriptItem {
    #[serde(deserialize_with = "de_non_empty_string")]
    pub id: String,
    pub role: ToolTag,
    #[serde(deserialize_with = "de_non_empty_string")]
    pub tool_call_id: String,
    #[serde(deserialize_with = "de_non_empty_string")]
    pub tool_name: String,
    pub input: JsonValue,
    pub content: Vec<ToolContent>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_optional"
    )]
    pub details: Option<JsonValue>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_optional"
    )]
    pub usage: Option<Usage>,
    pub timestamp: u64,
    pub status: CompleteTag,
    pub is_error: FalseTag,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ErrorToolTranscriptItem {
    #[serde(deserialize_with = "de_non_empty_string")]
    pub id: String,
    pub role: ToolTag,
    #[serde(deserialize_with = "de_non_empty_string")]
    pub tool_call_id: String,
    #[serde(deserialize_with = "de_non_empty_string")]
    pub tool_name: String,
    pub input: JsonValue,
    pub content: Vec<ToolContent>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_optional"
    )]
    pub details: Option<JsonValue>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_optional"
    )]
    pub usage: Option<Usage>,
    pub timestamp: u64,
    pub status: ErrorTag,
    pub is_error: TrueTag,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolTranscriptItem {
    Running(RunningToolTranscriptItem),
    Complete(CompleteToolTranscriptItem),
    Error(ErrorToolTranscriptItem),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TranscriptItem {
    User(UserTranscriptItem),
    Assistant(AssistantTranscriptItem),
    Tool(ToolTranscriptItem),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum UpdatedTranscriptItem {
    Assistant(AssistantTranscriptItem),
    Tool(ToolTranscriptItem),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FinishedTranscriptItem {
    CompleteAssistant(CompleteAssistantTranscriptItem),
    ErrorAssistant(ErrorAssistantTranscriptItem),
    AbortedAssistant(AbortedAssistantTranscriptItem),
    CompleteTool(CompleteToolTranscriptItem),
    ErrorTool(ErrorToolTranscriptItem),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AssistantDeltaKind {
    Text,
    Thinking,
    ToolCall,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemStartedProgress {
    #[serde(rename = "type")]
    pub kind: ItemStartedTag,
    pub item: TranscriptItem,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AssistantDeltaProgress {
    #[serde(rename = "type")]
    pub kind: AssistantDeltaTag,
    #[serde(deserialize_with = "de_non_empty_string")]
    pub message_id: String,
    pub content_index: u64,
    #[serde(rename = "kind")]
    pub delta_kind: AssistantDeltaKind,
    pub delta: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemUpdatedProgress {
    #[serde(rename = "type")]
    pub kind: ItemUpdatedTag,
    pub item: UpdatedTranscriptItem,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemFinishedProgress {
    #[serde(rename = "type")]
    pub kind: ItemFinishedTag,
    pub item: FinishedTranscriptItem,
}

/// Normalized incremental activity. Snapshots remain authoritative.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TranscriptProgress {
    ItemStarted(ItemStartedProgress),
    AssistantDelta(AssistantDeltaProgress),
    ItemUpdated(ItemUpdatedProgress),
    ItemFinished(ItemFinishedProgress),
}

// --- Sessions ----------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionMetadata {
    #[serde(deserialize_with = "de_non_empty_string")]
    pub id: String,
    pub created_at: u64,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_optional"
    )]
    pub updated_at: Option<u64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_optional_non_empty_string"
    )]
    pub parent_session_id: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_optional"
    )]
    pub session_name: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_optional_non_empty_string"
    )]
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionSnapshot {
    #[serde(deserialize_with = "de_non_empty_string")]
    pub id: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_optional"
    )]
    pub name: Option<String>,
    #[serde(deserialize_with = "de_non_empty_string")]
    pub cwd: String,
    pub created_at: u64,
    pub updated_at: u64,
    pub phase: SessionPhase,
    pub model: ModelRef,
    pub thinking_level: ThinkingLevel,
    pub attached: bool,
    pub locked: bool,
    pub revision: u64,
    pub transcript: Vec<TranscriptItem>,
    pub queued_steer: Vec<UserTranscriptItem>,
    pub queued_steer_count: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ServerSnapshot {
    #[serde(deserialize_with = "de_non_empty_string")]
    pub server_id: String,
    pub protocol_version: ProtocolVersionTag,
    pub revision: u64,
    pub sessions: Vec<SessionMetadata>,
    pub models: Vec<ModelMetadata>,
}

// --- Errors ------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtocolErrorCode {
    Version,
    Busy,
    SessionLocked,
    NotFound,
    InvalidRequest,
    NotImplemented,
    InternalError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProtocolError {
    pub code: ProtocolErrorCode,
    pub message: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_optional"
    )]
    pub details: Option<JsonValue>,
}

// --- Commands ----------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListCommand {
    pub command: ListTag,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CreateCommand {
    pub command: CreateTag,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_optional_non_empty_string"
    )]
    pub cwd: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_optional"
    )]
    pub name: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_optional"
    )]
    pub model: Option<ModelRef>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_optional"
    )]
    pub thinking_level: Option<ThinkingLevel>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AttachCommand {
    pub command: AttachTag,
    #[serde(deserialize_with = "de_non_empty_string")]
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DetachCommand {
    pub command: DetachTag,
    #[serde(deserialize_with = "de_non_empty_string")]
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PromptCommand {
    pub command: PromptTag,
    #[serde(deserialize_with = "de_non_empty_string")]
    pub session_id: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SteerCommand {
    pub command: SteerTag,
    #[serde(deserialize_with = "de_non_empty_string")]
    pub session_id: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AbortCommand {
    pub command: AbortTag,
    #[serde(deserialize_with = "de_non_empty_string")]
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SetModelCommand {
    pub command: SetModelTag,
    #[serde(deserialize_with = "de_non_empty_string")]
    pub session_id: String,
    pub model: ModelRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SetThinkingCommand {
    pub command: SetThinkingTag,
    #[serde(deserialize_with = "de_non_empty_string")]
    pub session_id: String,
    pub thinking_level: ThinkingLevel,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Command {
    List(ListCommand),
    Create(CreateCommand),
    Attach(AttachCommand),
    Detach(DetachCommand),
    Prompt(PromptCommand),
    Steer(SteerCommand),
    Abort(AbortCommand),
    SetModel(SetModelCommand),
    SetThinking(SetThinkingCommand),
}

/// `Command["command"]` — the command name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandName {
    List,
    Create,
    Attach,
    Detach,
    Prompt,
    Steer,
    Abort,
    SetModel,
    SetThinking,
}

impl Command {
    pub fn name(&self) -> CommandName {
        match self {
            Self::List(_) => CommandName::List,
            Self::Create(_) => CommandName::Create,
            Self::Attach(_) => CommandName::Attach,
            Self::Detach(_) => CommandName::Detach,
            Self::Prompt(_) => CommandName::Prompt,
            Self::Steer(_) => CommandName::Steer,
            Self::Abort(_) => CommandName::Abort,
            Self::SetModel(_) => CommandName::SetModel,
            Self::SetThinking(_) => CommandName::SetThinking,
        }
    }
}

// --- Results -----------------------------------------------------------------

macro_rules! session_result {
    ($name:ident, $tag:ident) => {
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
        #[serde(deny_unknown_fields)]
        pub struct $name {
            pub command: $tag,
            pub session: SessionSnapshot,
        }
    };
}

session_result!(CreateResult, CreateTag);
session_result!(AttachResult, AttachTag);
session_result!(PromptResult, PromptTag);
session_result!(SteerResult, SteerTag);
session_result!(AbortResult, AbortTag);
session_result!(SetModelResult, SetModelTag);
session_result!(SetThinkingResult, SetThinkingTag);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListResult {
    pub command: ListTag,
    pub sessions: Vec<SessionMetadata>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DetachResult {
    pub command: DetachTag,
    #[serde(deserialize_with = "de_non_empty_string")]
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CommandResult {
    List(ListResult),
    Create(CreateResult),
    Attach(AttachResult),
    Detach(DetachResult),
    Prompt(PromptResult),
    Steer(SteerResult),
    Abort(AbortResult),
    SetModel(SetModelResult),
    SetThinking(SetThinkingResult),
}

impl CommandResult {
    pub fn command(&self) -> CommandName {
        match self {
            Self::List(_) => CommandName::List,
            Self::Create(_) => CommandName::Create,
            Self::Attach(_) => CommandName::Attach,
            Self::Detach(_) => CommandName::Detach,
            Self::Prompt(_) => CommandName::Prompt,
            Self::Steer(_) => CommandName::Steer,
            Self::Abort(_) => CommandName::Abort,
            Self::SetModel(_) => CommandName::SetModel,
            Self::SetThinking(_) => CommandName::SetThinking,
        }
    }
}

// --- Client messages ---------------------------------------------------------

/// Must be the first frame sent by a client. Version is intentionally an
/// integer, not a coercible string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientHello {
    #[serde(rename = "type")]
    pub kind: HelloTag,
    pub version: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestEnvelope {
    #[serde(rename = "type")]
    pub kind: RequestTag,
    #[serde(deserialize_with = "de_non_empty_string")]
    pub id: String,
    pub request: Command,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ClientMessage {
    Hello(ClientHello),
    Request(RequestEnvelope),
}

// --- Server messages ---------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerSnapshotEvent {
    #[serde(rename = "type")]
    pub kind: ServerSnapshotTag,
    pub snapshot: ServerSnapshot,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionSnapshotEvent {
    #[serde(rename = "type")]
    pub kind: SessionSnapshotTag,
    pub snapshot: SessionSnapshot,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionProgressEvent {
    #[serde(rename = "type")]
    pub kind: SessionProgressTag,
    #[serde(deserialize_with = "de_non_empty_string")]
    pub session_id: String,
    pub progress: TranscriptProgress,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionRemovedEvent {
    #[serde(rename = "type")]
    pub kind: SessionRemovedTag,
    #[serde(deserialize_with = "de_non_empty_string")]
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ServerEvent {
    ServerSnapshot(ServerSnapshotEvent),
    SessionSnapshot(SessionSnapshotEvent),
    SessionProgress(SessionProgressEvent),
    SessionRemoved(SessionRemovedEvent),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ServerHello {
    #[serde(rename = "type")]
    pub kind: HelloTag,
    pub version: ProtocolVersionTag,
    #[serde(deserialize_with = "de_non_empty_string")]
    pub connection_id: String,
    pub snapshot: ServerSnapshot,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerHelloError {
    #[serde(rename = "type")]
    pub kind: HelloErrorTag,
    pub error: ProtocolError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OkResponseEnvelope {
    #[serde(rename = "type")]
    pub kind: ResponseTag,
    #[serde(deserialize_with = "de_non_empty_string")]
    pub id: String,
    pub ok: TrueTag,
    pub result: CommandResult,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorResponseEnvelope {
    #[serde(rename = "type")]
    pub kind: ResponseTag,
    #[serde(deserialize_with = "de_non_empty_string")]
    pub id: String,
    pub ok: FalseTag,
    pub error: ProtocolError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ResponseEnvelope {
    Ok(OkResponseEnvelope),
    Error(ErrorResponseEnvelope),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventEnvelope {
    #[serde(rename = "type")]
    pub kind: EventTag,
    pub event: ServerEvent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ServerMessage {
    Hello(ServerHello),
    HelloError(ServerHelloError),
    Response(ResponseEnvelope),
    Event(EventEnvelope),
}
