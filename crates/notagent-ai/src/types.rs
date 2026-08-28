use std::collections::BTreeMap;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use notagent_telemetry::TelemetryContext;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use crate::utils::diagnostics::AssistantMessageDiagnostic;
use crate::utils::event_stream::AssistantMessageEventStream;
use crate::utils::fetch::FetchFunction;
use crate::utils::js_number;

// ---------------------------------------------------------------------------
//
// ---------------------------------------------------------------------------

pub type Api = String;
pub type ImagesApi = String;
pub type ProviderId = String;
pub type ImagesProviderId = String;

pub const KNOWN_APIS: [&str; 10] = [
    "openai-completions",
    "mistral-conversations",
    "openai-responses",
    "azure-openai-responses",
    "openai-codex-responses",
    "anthropic-messages",
    "bedrock-converse-stream",
    "google-generative-ai",
    "google-vertex",
    "pi-messages",
];

/// `KnownImagesApi`.
pub const KNOWN_IMAGES_APIS: [&str; 1] = ["openrouter-images"];

pub const KNOWN_PROVIDERS: [&str; 41] = [
    "amazon-bedrock",
    "ant-ling",
    "anthropic",
    "google",
    "google-vertex",
    "openai",
    "azure-openai-responses",
    "openai-codex",
    "radius",
    "nvidia",
    "deepseek",
    "github-copilot",
    "xai",
    "groq",
    "cerebras",
    "openrouter",
    "vercel-ai-gateway",
    "zai",
    "zai-coding-cn",
    "mistral",
    "minimax",
    "minimax-cn",
    "moonshotai",
    "moonshotai-cn",
    "huggingface",
    "fireworks",
    "together",
    "baseten",
    "opencode",
    "opencode-go",
    "kimi-coding",
    "cline-pass",
    "cloudflare-workers-ai",
    "cloudflare-ai-gateway",
    "qwen-token-plan",
    "qwen-token-plan-cn",
    "qwen-token-plan-individual",
    "xiaomi",
    "xiaomi-token-plan-cn",
    "xiaomi-token-plan-ams",
    "xiaomi-token-plan-sgp",
];

/// `KnownImagesProvider`.
pub const KNOWN_IMAGES_PROVIDERS: [&str; 1] = ["openrouter"];

// ---------------------------------------------------------------------------
// Thinking
// ---------------------------------------------------------------------------

/// `ThinkingLevel = "minimal" | "low" | "medium" | "high" | "xhigh" | "max"`
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingLevel {
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

/// `ModelThinkingLevel = "off" | ThinkingLevel`
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelThinkingLevel {
    Off,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl From<ThinkingLevel> for ModelThinkingLevel {
    fn from(level: ThinkingLevel) -> Self {
        match level {
            ThinkingLevel::Minimal => ModelThinkingLevel::Minimal,
            ThinkingLevel::Low => ModelThinkingLevel::Low,
            ThinkingLevel::Medium => ModelThinkingLevel::Medium,
            ThinkingLevel::High => ModelThinkingLevel::High,
            ThinkingLevel::Xhigh => ModelThinkingLevel::Xhigh,
            ThinkingLevel::Max => ModelThinkingLevel::Max,
        }
    }
}

impl ModelThinkingLevel {
    pub fn to_thinking_level(self) -> Option<ThinkingLevel> {
        match self {
            ModelThinkingLevel::Off => None,
            ModelThinkingLevel::Minimal => Some(ThinkingLevel::Minimal),
            ModelThinkingLevel::Low => Some(ThinkingLevel::Low),
            ModelThinkingLevel::Medium => Some(ThinkingLevel::Medium),
            ModelThinkingLevel::High => Some(ThinkingLevel::High),
            ModelThinkingLevel::Xhigh => Some(ThinkingLevel::Xhigh),
            ModelThinkingLevel::Max => Some(ThinkingLevel::Max),
        }
    }
}

/// `ThinkingLevelMap = Partial<Record<ModelThinkingLevel, string | null>>`
pub type ThinkingLevelMap = BTreeMap<ModelThinkingLevel, Option<String>>;

/// `$var`-Platzhalter in `chat_template_kwargs`/`chat_template_args`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatTemplateVar {
    #[serde(rename = "$var")]
    pub var: ChatTemplateVarName,
    #[serde(
        rename = "omitWhenOff",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub omit_when_off: Option<bool>,
}

/// `"thinking.enabled" | "thinking.effort"`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChatTemplateVarName {
    #[serde(rename = "thinking.enabled")]
    ThinkingEnabled,
    #[serde(rename = "thinking.effort")]
    ThinkingEffort,
}

/// `ChatTemplateKwargValue = string | number | boolean | null | { $var, omitWhenOff? }`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ChatTemplateKwargValue {
    Var(ChatTemplateVar),
    String(String),
    Number(f64),
    Boolean(bool),
    Null,
}

/// `ThinkingBudgets { minimal?, low?, medium?, high? }` — Token-Budgets je Stufe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ThinkingBudgets {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimal: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub low: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub medium: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub high: Option<u64>,
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------

/// `CacheRetention = "none" | "short" | "long"`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CacheRetention {
    None,
    Short,
    Long,
}

/// `Transport = "sse" | "websocket" | "websocket-cached" | "auto"`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Transport {
    Sse,
    Websocket,
    WebsocketCached,
    Auto,
}

pub type ProviderEnv = BTreeMap<String, String>;

pub type ProviderHeaders = BTreeMap<String, Option<String>>;

/// `SessionAffinityFormat = "openai" | "openai-nosession" | "openrouter"`
/// bank on a header of its own, and without it the server falls back to
/// inferring the session from a prompt prefix scan on every request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionAffinityFormat {
    Openai,
    OpenaiNosession,
    Openrouter,
    Mtplx,
}

/// `ProviderResponse { status, headers }`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderResponse {
    pub status: u16,
    pub headers: BTreeMap<String, String>,
}

/// `onPayload?: (payload, model) => unknown | undefined | Promise<...>`
pub type OnPayload<M> = Arc<
    dyn for<'a> Fn(Value, &'a M) -> Pin<Box<dyn Future<Output = Option<Value>> + Send>>
        + Send
        + Sync,
>;

/// `onResponse?: (response, model) => void | Promise<void>`
pub type OnResponse<M> = Arc<
    dyn for<'a> Fn(ProviderResponse, &'a M) -> Pin<Box<dyn Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

pub struct ProviderRequestOptions<M = Model> {
    pub signal: Option<CancellationToken>,
    pub telemetry_context: Option<Arc<dyn TelemetryContext>>,
    pub api_key: Option<String>,
    pub fetch: Option<FetchFunction>,
    pub env: Option<ProviderEnv>,
    pub on_payload: Option<OnPayload<M>>,
    pub on_response: Option<OnResponse<M>>,
    pub headers: Option<ProviderHeaders>,
    pub timeout_ms: Option<u64>,
    pub max_retries: Option<u32>,
    pub max_retry_delay_ms: Option<u64>,
}

impl<M> Default for ProviderRequestOptions<M> {
    fn default() -> Self {
        ProviderRequestOptions {
            signal: None,
            telemetry_context: None,
            api_key: None,
            fetch: None,
            env: None,
            on_payload: None,
            on_response: None,
            headers: None,
            timeout_ms: None,
            max_retries: None,
            max_retry_delay_ms: None,
        }
    }
}

impl<M> Clone for ProviderRequestOptions<M> {
    fn clone(&self) -> Self {
        ProviderRequestOptions {
            signal: self.signal.clone(),
            telemetry_context: self.telemetry_context.clone(),
            api_key: self.api_key.clone(),
            fetch: self.fetch.clone(),
            env: self.env.clone(),
            on_payload: self.on_payload.clone(),
            on_response: self.on_response.clone(),
            headers: self.headers.clone(),
            timeout_ms: self.timeout_ms,
            max_retries: self.max_retries,
            max_retry_delay_ms: self.max_retry_delay_ms,
        }
    }
}

impl<M> fmt::Debug for ProviderRequestOptions<M> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderRequestOptions")
            .field("signal", &self.signal.is_some())
            .field("telemetry_context", &self.telemetry_context.is_some())
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("fetch", &self.fetch.is_some())
            .field("env", &self.env)
            .field("on_payload", &self.on_payload.is_some())
            .field("on_response", &self.on_response.is_some())
            .field("headers", &self.headers)
            .field("timeout_ms", &self.timeout_ms)
            .field("max_retries", &self.max_retries)
            .field("max_retry_delay_ms", &self.max_retry_delay_ms)
            .finish()
    }
}

/// `StreamOptions extends ProviderRequestOptions<Model<Api>>`
#[derive(Debug, Clone, Default)]
pub struct StreamOptions {
    pub base: ProviderRequestOptions<Model>,
    pub temperature: Option<f64>,
    pub sampling_params: Option<Map<String, Value>>,
    /// Token counts share one width across model, context and request options.
    pub max_tokens: Option<u64>,
    pub transport: Option<Transport>,
    /// Default: `"short"`.
    pub cache_retention: Option<CacheRetention>,
    pub session_id: Option<String>,
    pub websocket_connect_timeout_ms: Option<u64>,
    pub metadata: Option<Map<String, Value>>,
}

/// `deferred?: boolean | { window?: "15m" | "1h" | "24h" }`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DeferredRequest {
    Enabled(bool),
    Window {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window: Option<DeferredWindow>,
    },
}

/// `"15m" | "1h" | "24h"`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeferredWindow {
    #[serde(rename = "15m")]
    FifteenMinutes,
    #[serde(rename = "1h")]
    OneHour,
    #[serde(rename = "24h")]
    TwentyFourHours,
}

/// `SimpleStreamOptions extends StreamOptions`
#[derive(Debug, Clone, Default)]
pub struct SimpleStreamOptions {
    pub base: StreamOptions,
    pub reasoning: Option<ThinkingLevel>,
    pub deferred: Option<DeferredRequest>,
    pub thinking_budgets: Option<ThinkingBudgets>,
}

/// `DeferredFetchOptions extends ProviderRequestOptions<Model<Api>>`
#[derive(Debug, Clone, Default)]
pub struct DeferredFetchOptions {
    pub base: ProviderRequestOptions<Model>,
    pub wait: Option<u64>,
}

/// `DeferredCancelOptions = ProviderRequestOptions<Model<Api>>`
pub type DeferredCancelOptions = ProviderRequestOptions<Model>;

/// `ImagesOptions extends ProviderRequestOptions<ImagesModel<ImagesApi>>`
#[derive(Debug, Clone, Default)]
pub struct ImagesOptions {
    pub base: ProviderRequestOptions<ImagesModel>,
    pub metadata: Option<Map<String, Value>>,
}

pub trait ProviderStreams: Send + Sync {
    fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: Option<StreamOptions>,
    ) -> AssistantMessageEventStream;
    fn stream_simple(
        &self,
        model: &Model,
        context: &Context,
        options: Option<SimpleStreamOptions>,
    ) -> AssistantMessageEventStream;
    fn fetch_deferred(
        &self,
        _model: &Model,
        _handle: &DeferredHandle,
        _options: Option<DeferredFetchOptions>,
    ) -> Option<AssistantMessageEventStream> {
        None
    }
    fn cancel_deferred(
        &self,
        _model: &Model,
        _handle: &DeferredHandle,
        _options: Option<DeferredCancelOptions>,
    ) -> Option<Pin<Box<dyn Future<Output = ()> + Send>>> {
        None
    }

    /// Whether [`ProviderStreams::fetch_deferred`] is implemented. `createProvider`
    /// method, and calling it to find out would run its side effects, so the
    /// implementation declares it.
    fn supports_fetch_deferred(&self) -> bool {
        false
    }

    /// The same for [`ProviderStreams::cancel_deferred`].
    fn supports_cancel_deferred(&self) -> bool {
        false
    }
}

pub trait ProviderImages: Send + Sync {
    fn generate_images(
        &self,
        model: &ImagesModel,
        context: &ImagesContext,
        options: Option<ImagesOptions>,
    ) -> Pin<Box<dyn Future<Output = AssistantImages> + Send>>;
}

pub type StreamFunction = Arc<
    dyn Fn(&Model, &Context, Option<StreamOptions>) -> AssistantMessageEventStream + Send + Sync,
>;

/// `ImagesFunction`
pub type ImagesFunction = Arc<
    dyn Fn(
            &ImagesModel,
            &ImagesContext,
            Option<ImagesOptions>,
        ) -> Pin<Box<dyn Future<Output = AssistantImages> + Send>>
        + Send
        + Sync,
>;

// ---------------------------------------------------------------------------
// Content blocks.
// ---------------------------------------------------------------------------

/// `TextSignatureV1 { v: 1, id, phase? }`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextSignatureV1 {
    pub v: u8,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<TextSignaturePhase>,
}

/// `phase?: "commentary" | "final_answer"`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextSignaturePhase {
    Commentary,
    FinalAnswer,
}

/// `TextContent { type: "text", text, textSignature? }`
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextContent {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_signature: Option<String>,
    /// Streaming-State (Master-Plan, Architektur).
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl TextContent {
    pub fn new(text: impl Into<String>) -> Self {
        TextContent {
            text: text.into(),
            text_signature: None,
            extra: Map::new(),
        }
    }
}

/// `ThinkingContent { type: "thinking", thinking, thinkingSignature?, redacted? }`
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingContent {
    pub thinking: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_signature: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redacted: Option<bool>,
    /// Streaming-State (Master-Plan, Architektur).
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// `ImageContent { type: "image", data (base64), mimeType }`
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageContent {
    /// base64-kodierte Bilddaten.
    pub data: String,
    /// z. B. `"image/jpeg"`, `"image/png"`.
    pub mime_type: String,
}

/// `ToolCall { type: "toolCall", id, name, arguments, thoughtSignature?, namespace? }`
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thought_signature: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    /// Streaming-State (Master-Plan, Architektur).
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

pub mod tagged_tool_call {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use super::ToolCall;

    #[derive(Serialize)]
    struct Tagged<'a> {
        #[serde(rename = "type")]
        kind: &'static str,
        #[serde(flatten)]
        tool_call: &'a ToolCall,
    }

    pub fn serialize<S: Serializer>(
        tool_call: &ToolCall,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        Tagged {
            kind: "toolCall",
            tool_call,
        }
        .serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<ToolCall, D::Error> {
        ToolCall::deserialize(deserializer)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AssistantContent {
    #[serde(rename = "text")]
    Text(TextContent),
    #[serde(rename = "thinking")]
    Thinking(ThinkingContent),
    #[serde(rename = "toolCall")]
    ToolCall(ToolCall),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum TextOrImageContent {
    #[serde(rename = "text")]
    Text(TextContent),
    #[serde(rename = "image")]
    Image(ImageContent),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum UserContent {
    Text(String),
    Blocks(Vec<TextOrImageContent>),
}

impl Default for UserContent {
    fn default() -> Self {
        UserContent::Blocks(Vec::new())
    }
}

// ---------------------------------------------------------------------------
// Usage, StopReason, Deferred
// ---------------------------------------------------------------------------

/// `cost` innerhalb von [`Usage`].
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageCost {
    #[serde(with = "js_number")]
    pub input: f64,
    #[serde(with = "js_number")]
    pub output: f64,
    #[serde(with = "js_number")]
    pub cache_read: f64,
    #[serde(with = "js_number")]
    pub cache_write: f64,
    #[serde(with = "js_number")]
    pub total: f64,
}

/// `Usage { input, output, cacheRead, cacheWrite, cacheWrite1h?, reasoning?, totalTokens, cost }`
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write1h: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    pub cost: UsageCost,
}

impl Default for Usage {
    fn default() -> Self {
        Usage {
            input: 0,
            output: 0,
            cache_read: 0,
            cache_write: 0,
            cache_write1h: None,
            reasoning: None,
            total_tokens: Some(0),
            cost: UsageCost::default(),
        }
    }
}

/// `StopReason = "pending" | "stop" | "length" | "toolUse" | "error" | "aborted" | "deferred"`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StopReason {
    Pending,
    Stop,
    Length,
    ToolUse,
    Error,
    Aborted,
    Deferred,
}

pub type JsonValue = Value;

/// `DeferredHandle { provider, modelId, api, id, expiresAt?, pollAfterMs?, data? }`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeferredHandle {
    pub provider: String,
    pub model_id: String,
    pub api: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poll_after_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<JsonValue>,
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------

/// `transformMessages` normalizes `null`/missing `content` to an empty array before every
/// provider request, because hand-built histories, custom tools and old session files
/// violate the type (issues #6259, #6276). The Rust types make that state
/// unrepresentable, so the same leniency lives at deserialization: the message ends up in
fn lax_content<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

/// `UserMessage { role: "user", content, timestamp }`
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct UserMessage {
    #[serde(default, deserialize_with = "lax_content")]
    pub content: UserContent,
    /// Unix-Zeitstempel in Millisekunden.
    pub timestamp: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantMessage {
    #[serde(default, deserialize_with = "lax_content")]
    pub content: Vec<AssistantContent>,
    pub api: Api,
    pub provider: ProviderId,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<Vec<AssistantMessageDiagnostic>>,
    pub usage: Usage,
    pub stop_reason: StopReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deferred: Option<DeferredHandle>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_stop_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_turn: Option<bool>,
    /// Unix-Zeitstempel in Millisekunden.
    pub timestamp: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResultMessage {
    pub tool_call_id: String,
    pub tool_name: String,
    #[serde(default, deserialize_with = "lax_content")]
    pub content: Vec<TextOrImageContent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added_tool_names: Option<Vec<String>>,
    pub is_error: bool,
    /// Unix-Zeitstempel in Millisekunden.
    pub timestamp: i64,
}

/// `Message = UserMessage | AssistantMessage | ToolResultMessage`
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "camelCase")]
pub enum Message {
    User(UserMessage),
    Assistant(AssistantMessage),
    ToolResult(ToolResultMessage),
}

impl Message {
    pub fn timestamp(&self) -> i64 {
        match self {
            Message::User(message) => message.timestamp,
            Message::Assistant(message) => message.timestamp,
            Message::ToolResult(message) => message.timestamp,
        }
    }
}

// ---------------------------------------------------------------------------
// Bildgenerierung
// ---------------------------------------------------------------------------

pub type ImagesInputContent = TextOrImageContent;
pub type ImagesOutputContent = TextOrImageContent;

/// `ImagesContext { input }`
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ImagesContext {
    pub input: Vec<ImagesInputContent>,
}

/// `ImagesStopReason = "stop" | "error" | "aborted"`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ImagesStopReason {
    Stop,
    Error,
    Aborted,
}

/// `AssistantImages`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantImages {
    pub api: ImagesApi,
    pub provider: ImagesProviderId,
    pub model: String,
    pub output: Vec<ImagesOutputContent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    pub stop_reason: ImagesStopReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    /// Unix-Zeitstempel in Millisekunden.
    pub timestamp: i64,
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------

/// `GrammarFormat = "openai_lark" | "openai_regex"`
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrammarFormat {
    OpenaiLark,
    OpenaiRegex,
}

/// `GrammarVariants = Partial<Record<GrammarFormat, string>>`
pub type GrammarVariants = BTreeMap<GrammarFormat, String>;

/// `strict: "prefer" | "require"`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StrictMode {
    Prefer,
    Require,
}

/// `ConstrainedSamplingConfig = { type: "json_schema", strict } | { type: "grammar", variants }`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConstrainedSamplingConfig {
    JsonSchema { strict: StrictMode },
    Grammar { variants: GrammarVariants },
}

/// `constrainedSampling?: false | ConstrainedSamplingConfig`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ConstrainedSampling {
    Disabled(bool),
    Config(ConstrainedSamplingConfig),
}

/// `Tool { name, description, parameters, constrainedSampling? }`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub parameters: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub constrained_sampling: Option<ConstrainedSampling>,
}

/// `Context { systemPrompt?, messages, tools? }`
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Context {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    pub messages: Vec<Message>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Tool>>,
}

// ---------------------------------------------------------------------------
// Stream-Events
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DoneReason {
    Stop,
    Length,
    ToolUse,
    Deferred,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ErrorReason {
    Aborted,
    Error,
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all_fields = "camelCase")]
pub enum AssistantMessageEvent {
    #[serde(rename = "start")]
    Start { partial: AssistantMessage },
    #[serde(rename = "text_start")]
    TextStart {
        content_index: usize,
        partial: AssistantMessage,
    },
    #[serde(rename = "text_delta")]
    TextDelta {
        content_index: usize,
        delta: String,
        partial: AssistantMessage,
    },
    #[serde(rename = "text_end")]
    TextEnd {
        content_index: usize,
        content: String,
        partial: AssistantMessage,
    },
    #[serde(rename = "thinking_start")]
    ThinkingStart {
        content_index: usize,
        partial: AssistantMessage,
    },
    #[serde(rename = "thinking_delta")]
    ThinkingDelta {
        content_index: usize,
        delta: String,
        partial: AssistantMessage,
    },
    #[serde(rename = "thinking_end")]
    ThinkingEnd {
        content_index: usize,
        content: String,
        partial: AssistantMessage,
    },
    #[serde(rename = "toolcall_start")]
    ToolcallStart {
        content_index: usize,
        partial: AssistantMessage,
    },
    #[serde(rename = "toolcall_delta")]
    ToolcallDelta {
        content_index: usize,
        delta: String,
        partial: AssistantMessage,
    },
    #[serde(rename = "toolcall_end")]
    ToolcallEnd {
        content_index: usize,
        #[serde(with = "tagged_tool_call")]
        tool_call: ToolCall,
        partial: AssistantMessage,
    },
    #[serde(rename = "done")]
    Done {
        reason: DoneReason,
        message: AssistantMessage,
    },
    #[serde(rename = "error")]
    Error {
        reason: ErrorReason,
        error: AssistantMessage,
    },
}

// ---------------------------------------------------------------------------
// Compat-Einstellungen
// ---------------------------------------------------------------------------

/// `maxTokensField?: "max_completion_tokens" | "max_tokens"`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaxTokensField {
    MaxCompletionTokens,
    MaxTokens,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThinkingFormat {
    Openai,
    Openrouter,
    Deepseek,
    Together,
    Baseten,
    Zai,
    Qwen,
    ChatTemplate,
    QwenChatTemplate,
    StringThinking,
    AntLing,
}

/// `cacheControlFormat?: "anthropic"`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CacheControlFormat {
    Anthropic,
}

/// `deferredToolsMode?: "kimi"`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeferredToolsMode {
    Kimi,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenAICompletionsCompat {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_store: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_developer_role: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_reasoning_effort: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_usage_in_streaming: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_finish_reason: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens_field: Option<MaxTokensField>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_tool_result_name: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_assistant_after_tool_result: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_thinking_as_text: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_reasoning_content_on_assistant_messages: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_format: Option<ThinkingFormat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_template_kwargs: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_template_args: Option<Map<String, Value>>,
    #[serde(
        rename = "openRouterRouting",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub open_router_routing: Option<OpenRouterRouting>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vercel_gateway_routing: Option<VercelGatewayRouting>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zai_tool_stream: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_thinking_token_budget: Option<bool>,
    #[serde(
        rename = "supportsOpenAIGrammarTools",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub supports_openai_grammar_tools: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_strict_mode: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_control_format: Option<CacheControlFormat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send_session_affinity_headers: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deferred_tools_mode: Option<DeferredToolsMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_affinity_format: Option<SessionAffinityFormat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_long_cache_retention: Option<bool>,
    /// interface does not declare (for example `supportsReasoningEffort` on an
    /// preserved here instead of being dropped.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenAIResponsesCompat {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_developer_role: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_affinity_format: Option<SessionAffinityFormat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_long_cache_retention: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_strict_mode: Option<bool>,
    #[serde(
        rename = "supportsOpenAIGrammarTools",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub supports_openai_grammar_tools: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_additional_tools: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_tool_search: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_explicit_prompt_cache_mode: Option<bool>,
    /// interface does not declare (for example `supportsReasoningEffort` on an
    /// preserved here instead of being dropped.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// `AnthropicMessagesCompat`
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnthropicMessagesCompat {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_eager_tool_input_streaming: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_long_cache_retention: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send_session_affinity_headers: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_cache_control_on_tools: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_temperature: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub force_adaptive_thinking: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_empty_signature: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_strict_tools: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_tool_references: Option<bool>,
    /// interface does not declare (for example `supportsReasoningEffort` on an
    /// preserved here instead of being dropped.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// `BedrockCompat`
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BedrockCompat {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_strict_mode: Option<bool>,
    /// interface does not declare (for example `supportsReasoningEffort` on an
    /// preserved here instead of being dropped.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum NumberOrString {
    Number(f64),
    String(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct OpenRouterPercentiles {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p50: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p75: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p90: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p99: Option<f64>,
}

/// `number | { p50?, p75?, p90?, p99? }`
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum NumberOrPercentiles {
    Number(f64),
    Percentiles(OpenRouterPercentiles),
}

/// `sort?: string | { by?, partition? }`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OpenRouterSort {
    Name(String),
    Object {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        by: Option<String>,
        #[serde(
            default,
            deserialize_with = "deserialize_some",
            skip_serializing_if = "Option::is_none"
        )]
        partition: Option<Option<String>>,
    },
}

fn deserialize_some<'de, T, D>(deserializer: D) -> Result<Option<T>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    T::deserialize(deserializer).map(Some)
}

/// `max_price?: { prompt?, completion?, image?, audio?, request? }`
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct OpenRouterMaxPrice {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<NumberOrString>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion: Option<NumberOrString>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<NumberOrString>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<NumberOrString>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<NumberOrString>,
}

/// `data_collection?: "deny" | "allow"`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OpenRouterDataCollection {
    Deny,
    Allow,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct OpenRouterRouting {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_fallbacks: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub require_parameters: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_collection: Option<OpenRouterDataCollection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zdr: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enforce_distillable_text: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub only: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ignore: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantizations: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort: Option<OpenRouterSort>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_price: Option<OpenRouterMaxPrice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_min_throughput: Option<NumberOrPercentiles>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_max_latency: Option<NumberOrPercentiles>,
}

/// `VercelGatewayRouting`
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct VercelGatewayRouting {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub only: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<Vec<String>>,
}

// ---------------------------------------------------------------------------
// Modelle
// ---------------------------------------------------------------------------

/// `ModelCostRates { input, output, cacheRead, cacheWrite }` — $/Millionen Tokens.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCostRates {
    #[serde(with = "js_number")]
    pub input: f64,
    #[serde(with = "js_number")]
    pub output: f64,
    #[serde(with = "js_number")]
    pub cache_read: f64,
    #[serde(with = "js_number")]
    pub cache_write: f64,
}

/// `ModelCostTier extends ModelCostRates { inputTokensAbove }`
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCostTier {
    #[serde(with = "js_number")]
    pub input: f64,
    #[serde(with = "js_number")]
    pub output: f64,
    #[serde(with = "js_number")]
    pub cache_read: f64,
    #[serde(with = "js_number")]
    pub cache_write: f64,
    pub input_tokens_above: u64,
}

/// `ModelCost extends ModelCostRates { tiers? }`
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCost {
    #[serde(with = "js_number")]
    pub input: f64,
    #[serde(with = "js_number")]
    pub output: f64,
    #[serde(with = "js_number")]
    pub cache_read: f64,
    #[serde(with = "js_number")]
    pub cache_write: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tiers: Option<Vec<ModelCostTier>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Modality {
    Text,
    Image,
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum ModelCompat {
    OpenAICompletions(OpenAICompletionsCompat),
    OpenAIResponses(OpenAIResponsesCompat),
    AnthropicMessages(AnthropicMessagesCompat),
    Bedrock(BedrockCompat),
    Other(Value),
}

impl ModelCompat {
    pub fn from_api_value(api: &str, value: Value) -> Result<Self, serde_json::Error> {
        Ok(match api {
            "openai-completions" => ModelCompat::OpenAICompletions(serde_json::from_value(value)?),
            "openai-responses" | "azure-openai-responses" | "openai-codex-responses" => {
                ModelCompat::OpenAIResponses(serde_json::from_value(value)?)
            }
            "anthropic-messages" => ModelCompat::AnthropicMessages(serde_json::from_value(value)?),
            "bedrock-converse-stream" => ModelCompat::Bedrock(serde_json::from_value(value)?),
            _ => ModelCompat::Other(value),
        })
    }

    pub fn as_openai_completions(&self) -> Option<&OpenAICompletionsCompat> {
        match self {
            ModelCompat::OpenAICompletions(compat) => Some(compat),
            _ => None,
        }
    }

    pub fn as_openai_responses(&self) -> Option<&OpenAIResponsesCompat> {
        match self {
            ModelCompat::OpenAIResponses(compat) => Some(compat),
            _ => None,
        }
    }

    pub fn as_anthropic_messages(&self) -> Option<&AnthropicMessagesCompat> {
        match self {
            ModelCompat::AnthropicMessages(compat) => Some(compat),
            _ => None,
        }
    }

    pub fn as_bedrock(&self) -> Option<&BedrockCompat> {
        match self {
            ModelCompat::Bedrock(compat) => Some(compat),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    pub id: String,
    pub name: String,
    pub api: Api,
    pub provider: ProviderId,
    pub base_url: String,
    pub reasoning: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_level_map: Option<ThinkingLevelMap>,
    pub input: Vec<Modality>,
    pub cost: ModelCost,
    pub context_window: u64,
    pub max_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sampling_params: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compat: Option<ModelCompat>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelRaw {
    id: String,
    name: String,
    api: Api,
    provider: ProviderId,
    base_url: String,
    reasoning: bool,
    #[serde(default)]
    thinking_level_map: Option<ThinkingLevelMap>,
    input: Vec<Modality>,
    cost: ModelCost,
    context_window: u64,
    max_tokens: u64,
    #[serde(default)]
    sampling_params: Option<Map<String, Value>>,
    #[serde(default)]
    headers: Option<BTreeMap<String, String>>,
    #[serde(default)]
    compat: Option<Value>,
}

impl<'de> Deserialize<'de> for Model {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = ModelRaw::deserialize(deserializer)?;
        let compat = match raw.compat {
            Some(value) => Some(
                ModelCompat::from_api_value(&raw.api, value).map_err(serde::de::Error::custom)?,
            ),
            None => None,
        };
        Ok(Model {
            id: raw.id,
            name: raw.name,
            api: raw.api,
            provider: raw.provider,
            base_url: raw.base_url,
            reasoning: raw.reasoning,
            thinking_level_map: raw.thinking_level_map,
            input: raw.input,
            cost: raw.cost,
            context_window: raw.context_window,
            max_tokens: raw.max_tokens,
            sampling_params: raw.sampling_params,
            headers: raw.headers,
            compat,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImagesModel {
    pub id: String,
    pub name: String,
    pub api: ImagesApi,
    pub provider: ImagesProviderId,
    pub base_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_level_map: Option<ThinkingLevelMap>,
    pub input: Vec<Modality>,
    pub output: Vec<Modality>,
    pub cost: ModelCost,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sampling_params: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<BTreeMap<String, String>>,
}

fn _assert_thinking_level_map_is_serializable(map: &ThinkingLevelMap) -> String {
    serde_json::to_string(map).unwrap_or_default()
}
