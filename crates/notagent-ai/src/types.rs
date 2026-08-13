//! Kerntypen des LLM-Layers.
//!
//! 1:1-Port von `packages/ai/src/types.ts` (830 LOC). Die JSON-Repräsentation ist
//! identisch zum TS-Original: camelCase, optionale Felder werden weggelassen statt
//! auf `null` gesetzt (Session-Dateien und Wire-Formate hängen daran).

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
// API- und Provider-Kennungen
//
// Abweichung Klasse 1: TS modelliert `Api = KnownApi | (string & {})` — zur Laufzeit
// ein String mit Autovervollständigung. In Rust bleibt der String die Repräsentation;
// die bekannten Werte stehen als Konstanten daneben.
// ---------------------------------------------------------------------------

pub type Api = String;
pub type ImagesApi = String;
pub type ProviderId = String;
pub type ImagesProviderId = String;

/// Die zehn `KnownApi`-Werte aus `types.ts:19-28`.
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

/// Die 40 `KnownProvider`-Werte aus `types.ts:35-74`.
pub const KNOWN_PROVIDERS: [&str; 40] = [
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
    /// `"off"` hat kein Gegenstück in [`ThinkingLevel`].
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
///
/// `null` markiert eine Stufe explizit als nicht unterstützt und wird — anders als
/// weggelassene Schlüssel — serialisiert.
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
    pub minimal: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub low: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub medium: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub high: Option<u32>,
}

// ---------------------------------------------------------------------------
// Transport- und Request-Optionen
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

/// `ProviderEnv = Record<string, string>` — hat Vorrang vor `process.env`.
pub type ProviderEnv = BTreeMap<String, String>;

/// `ProviderHeaders = Record<string, string | null>` — `null` löscht einen Default-Header.
pub type ProviderHeaders = BTreeMap<String, Option<String>>;

/// `SessionAffinityFormat = "openai" | "openai-nosession" | "openrouter"`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionAffinityFormat {
    Openai,
    OpenaiNosession,
    Openrouter,
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

/// `ProviderRequestOptions<TModel>` — Auth, HTTP-Transport und Lifecycle-Callbacks.
pub struct ProviderRequestOptions<M = Model> {
    /// TS `signal?: AbortSignal` (Master-Substitution: `CancellationToken`).
    pub signal: Option<CancellationToken>,
    /// Expliziter Parent-Kontext für die Telemetrie dieses Requests.
    pub telemetry_context: Option<Arc<dyn TelemetryContext>>,
    pub api_key: Option<String>,
    /// Optionale HTTP-Implementierung; Default ist der reqwest-basierte Client.
    pub fetch: Option<FetchFunction>,
    /// Provider-bezogene Umgebungswerte; haben Vorrang vor `process.env`.
    pub env: Option<ProviderEnv>,
    pub on_payload: Option<OnPayload<M>>,
    pub on_response: Option<OnResponse<M>>,
    /// Zusammengeführt mit den Provider-Defaults; `None`-Werte unterdrücken einen Default.
    pub headers: Option<ProviderHeaders>,
    pub timeout_ms: Option<u64>,
    pub max_retries: Option<u32>,
    /// Default 60 000 ms; `0` deaktiviert die Obergrenze.
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
    /// Wird nach den benannten Feldern in den Request-Body gemischt und überschreibt sie
    /// dadurch; nur OpenAI-kompatible Adapter werten sie aus.
    pub sampling_params: Option<Map<String, Value>>,
    pub max_tokens: Option<u32>,
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
    /// Bittet fähige Provider um ein dauerhaftes Handle und asynchrone Fortsetzung.
    pub deferred: Option<DeferredRequest>,
    /// Eigene Token-Budgets je Thinking-Stufe (nur token-basierte Provider).
    pub thinking_budgets: Option<ThinkingBudgets>,
}

/// `DeferredFetchOptions extends ProviderRequestOptions<Model<Api>>`
#[derive(Debug, Clone, Default)]
pub struct DeferredFetchOptions {
    pub base: ProviderRequestOptions<Model>,
    /// Maximale Long-Poll-Dauer in ms; Default 0 = eine Statusabfrage.
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

/// `ProviderStreams` — einheitlicher Vertrag jedes API-Moduls unter `src/api/`.
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
}

/// `ProviderImages` — Vertrag der Bild-API-Module.
pub trait ProviderImages: Send + Sync {
    fn generate_images(
        &self,
        model: &ImagesModel,
        context: &ImagesContext,
        options: Option<ImagesOptions>,
    ) -> Pin<Box<dyn Future<Output = AssistantImages> + Send>>;
}

/// `StreamFunction` — Vertrag: nach dem Aufruf wird nicht mehr geworfen; Fehler
/// erscheinen als `error`-Event mit `stopReason` `error`/`aborted` im Strom.
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
// Content-Blöcke
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
    /// z. B. bei OpenAI Responses: Message-Metadaten (Legacy-ID-String oder `TextSignatureV1`-JSON).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_signature: Option<String>,
    /// JS-Objekte sind offen: bricht ein Stream vor `content_block_stop` ab, bleiben
    /// Scratch-Felder (`partialJson`, `index`) am Block hängen und landen so in der
    /// Session-Datei (bug-compat, belegt in `packages/coding-agent/test/fixtures`).
    /// Neu erzeugte Blöcke haben diese Felder nicht — der Scratch-Zustand lebt im
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
    /// True, wenn Sicherheitsfilter den Inhalt redigiert haben; die verschlüsselte
    /// Nutzlast liegt dann in `thinking_signature`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redacted: Option<bool>,
    /// JS-Objekte sind offen: bricht ein Stream vor `content_block_stop` ab, bleiben
    /// Scratch-Felder (`partialJson`, `index`) am Block hängen und landen so in der
    /// Session-Datei (bug-compat, belegt in `packages/coding-agent/test/fixtures`).
    /// Neu erzeugte Blöcke haben diese Felder nicht — der Scratch-Zustand lebt im
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
    /// Google-spezifisch: opake Signatur zum Wiederverwenden des Thought-Kontexts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thought_signature: Option<String>,
    /// OpenAI-Responses-Namespace für dynamisch geladene bzw. genamespacete Tools.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    /// JS-Objekte sind offen: bricht ein Stream vor `content_block_stop` ab, bleiben
    /// Scratch-Felder (`partialJson`, `index`) am Block hängen und landen so in der
    /// Session-Datei (bug-compat, belegt in `packages/coding-agent/test/fixtures`).
    /// Neu erzeugte Blöcke haben diese Felder nicht — der Scratch-Zustand lebt im
    /// Streaming-State (Master-Plan, Architektur).
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Serialisiert eine [`ToolCall`] eigenständig — also mit dem Diskriminator
/// `"type": "toolCall"`, den sie in TS als Teil des Interface trägt. Innerhalb von
/// [`AssistantContent`] liefert bereits die Enum-Auszeichnung dieses Feld.
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
        // Der Diskriminator wird als unbekanntes Feld ignoriert.
        ToolCall::deserialize(deserializer)
    }
}

/// Inhalt einer `AssistantMessage`: `TextContent | ThinkingContent | ToolCall`.
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

/// Inhalt von User- und ToolResult-Nachrichten: `TextContent | ImageContent`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum TextOrImageContent {
    #[serde(rename = "text")]
    Text(TextContent),
    #[serde(rename = "image")]
    Image(ImageContent),
}

/// `content: string | (TextContent | ImageContent)[]` einer `UserMessage`.
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
    /// Teilmenge von `cache_write` mit 1h-Retention. Nur Anthropic meldet diese Aufteilung.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write1h: Option<u64>,
    /// Reasoning-Tokens, sofern der Provider sie meldet — Teilmenge von `output`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<u64>,
    /// TS deklariert das Feld als Pflicht, historische Session-Dateien der TS-App
    /// enthalten es aber nicht (abgebrochene Streams älterer Versionen). Es bleibt
    /// optional, damit ein Roundtrip solcher Dateien verlustfrei ist; `estimate.ts`
    /// behandelt `undefined` und `0` ohnehin gleich (`usage.totalTokens || summe`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    pub cost: UsageCost,
}

impl Default for Usage {
    /// Entspricht `EMPTY_USAGE` aus `packages/agent/src/agent.ts:39-46`.
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

/// `JsonValue` aus `types.ts:392`.
pub type JsonValue = Value;

/// `DeferredHandle { provider, modelId, api, id, expiresAt?, pollAfterMs?, data? }`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeferredHandle {
    pub provider: String,
    pub model_id: String,
    pub api: String,
    /// Provider-Token, etwa eine Response- oder Batch-ID plus Zeilen-ID.
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poll_after_ms: Option<u64>,
    /// Provider-Konvertierungsdaten zum Rekonstruieren der finalen Assistant-Nachricht.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<JsonValue>,
}

// ---------------------------------------------------------------------------
// Nachrichten
// ---------------------------------------------------------------------------

/// `UserMessage { role: "user", content, timestamp }`
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct UserMessage {
    pub content: UserContent,
    /// Unix-Zeitstempel in Millisekunden.
    pub timestamp: i64,
}

/// `AssistantMessage` — alle Felder aus `types.ts:409-435`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantMessage {
    pub content: Vec<AssistantContent>,
    pub api: Api,
    pub provider: ProviderId,
    pub model: String,
    /// Konkretes `chunk.model`, wenn es vom angefragten `model` abweicht.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_model: Option<String>,
    /// Provider-spezifische Response-/Message-ID, sofern die API eine liefert.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_id: Option<String>,
    /// Redigierte Provider-/Laufzeit-Diagnosen zu Fehlern und Recoveries.
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
    /// Provider-Hinweis, ob das Modell den Zug bewusst beendet hat. Nur für Debugging.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_turn: Option<bool>,
    /// Unix-Zeitstempel in Millisekunden.
    pub timestamp: i64,
}

/// `ToolResultMessage<TDetails>` — `details` bleibt als JSON erhalten.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResultMessage {
    pub tool_call_id: String,
    pub tool_name: String,
    pub content: Vec<TextOrImageContent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
    /// Usage der Tool-Ausführung selbst; zählt nicht zum LLM-Kontext.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// Namen aus `Context.tools`, die nach diesem Ergebnis verfügbar wurden.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added_tool_names: Option<Vec<String>>,
    pub is_error: bool,
    /// Unix-Zeitstempel in Millisekunden.
    pub timestamp: i64,
}

/// `Message = UserMessage | AssistantMessage | ToolResultMessage`
// Boxen der Varianten würde die öffentliche Form gegenüber dem TS-Original ändern
// (CONVENTIONS.md §9): Nachrichten werden überall direkt konstruiert und gematcht.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "camelCase")]
pub enum Message {
    User(UserMessage),
    Assistant(AssistantMessage),
    ToolResult(ToolResultMessage),
}

impl Message {
    /// TS: `message.timestamp` — auf allen drei Varianten vorhanden.
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
// Tools und Kontext
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
    /// TS: das Literal `false` deaktiviert Constrained Sampling für dieses Tool.
    Disabled(bool),
    Config(ConstrainedSamplingConfig),
}

/// `Tool { name, description, parameters, constrainedSampling? }`
///
/// Substitution Klasse 3: TypeBox-`TSchema` → statischer `serde_json`-JSON-Schema-Wert.
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

/// `reason` eines `done`-Events: `Extract<StopReason, "stop" | "length" | "toolUse" | "deferred">`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DoneReason {
    Stop,
    Length,
    ToolUse,
    Deferred,
}

/// `reason` eines `error`-Events: `Extract<StopReason, "aborted" | "error">`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ErrorReason {
    Aborted,
    Error,
}

/// `AssistantMessageEvent` — 12 Varianten (`types.ts:523-539`).
///
/// In TS ist `partial` über den gesamten Strom dieselbe, in-place mutierte Objektreferenz.
/// In Rust trägt jedes Event einen Snapshot (Master-Plan, Architektur-Entscheidung).
// Jedes Event trägt den `partial`-Snapshot direkt, wie im TS-Original (CONVENTIONS.md §9).
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

/// Die elf `thinkingFormat`-Varianten aus `types.ts:566-578`.
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

/// `OpenAICompletionsCompat` — Kompatibilitätsschalter für OpenAI-kompatible Completions-APIs.
///
/// `chat_template_kwargs`/`chat_template_args` bleiben als `serde_json::Map`, damit die
/// Schlüsselreihenfolge des Katalogs im Request-Body erhalten bleibt (byte-identische
/// Payloads); die Werte entsprechen [`ChatTemplateKwargValue`].
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
}

/// `OpenAIResponsesCompat` — für die drei Responses-APIs.
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
}

/// `BedrockCompat`
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BedrockCompat {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_strict_mode: Option<bool>,
}

/// `number | string` in den OpenRouter-Preisgrenzen.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum NumberOrString {
    Number(f64),
    String(String),
}

/// Perzentil-Schwellen für Durchsatz bzw. Latenz.
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
        /// TS: `string | null` — fehlender Schlüssel und `null` sind unterscheidbar.
        #[serde(
            default,
            deserialize_with = "deserialize_some",
            skip_serializing_if = "Option::is_none"
        )]
        partition: Option<Option<String>>,
    },
}

/// Unterscheidet einen fehlenden Schlüssel (`None`) von explizitem `null` (`Some(None)`).
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

/// `OpenRouterRouting` — wird als `provider`-Feld im Request-Body gesendet.
/// Die Feldnamen sind bereits im TS-Original snake_case und bleiben unverändert.
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
    /// Diese Stufe gilt für Anfragen, deren Gesamt-Input diese Tokenzahl übersteigt.
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
    /// Request-weite Preisstufen; die höchste passende Schwelle gilt für die ganze Anfrage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tiers: Option<Vec<ModelCostTier>>,
}

/// `input: ("text" | "image")[]` bzw. `output` bei Bildmodellen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Modality {
    Text,
    Image,
}

/// `compat` von [`Model`] — die konkrete Variante folgt aus `api` (TS: bedingter Typ).
// Wie bei `Message`: Boxen würde die 1:1-Form der Compat-Objekte verstecken.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum ModelCompat {
    OpenAICompletions(OpenAICompletionsCompat),
    OpenAIResponses(OpenAIResponsesCompat),
    AnthropicMessages(AnthropicMessagesCompat),
    Bedrock(BedrockCompat),
    /// APIs ohne Compat-Zuordnung (TS-Typ `never`): der Rohwert bleibt erhalten,
    /// damit ein Roundtrip verlustfrei ist.
    Other(Value),
}

impl ModelCompat {
    /// Parst den `compat`-Wert passend zur `api` des Modells.
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

/// `Model<TApi>` — Eintrag des Modellkatalogs.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    pub id: String,
    pub name: String,
    pub api: Api,
    pub provider: ProviderId,
    pub base_url: String,
    pub reasoning: bool,
    /// Bildet notagent-Thinking-Stufen auf provider-spezifische Werte ab;
    /// `null` markiert eine Stufe als nicht unterstützt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_level_map: Option<ThinkingLevelMap>,
    pub input: Vec<Modality>,
    pub cost: ModelCost,
    pub context_window: u64,
    pub max_tokens: u64,
    /// Default-Sampling-Parameter; Werte aus `StreamOptions.sampling_params` überschreiben sie.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sampling_params: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<BTreeMap<String, String>>,
    /// Kompatibilitäts-Overrides; ohne Angabe wird aus `base_url` abgeleitet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compat: Option<ModelCompat>,
}

/// Rohform für die api-abhängige Deserialisierung von `compat`.
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

/// `ImagesModel<TApi>` — `Model` ohne `api`/`provider`/`reasoning`/`contextWindow`/
/// `maxTokens`/`compat`, dafür mit `output`.
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

/// Serialisierungshelfer: `ThinkingLevelMap`-Schlüssel sind Enum-Werte und damit
/// als JSON-Objektschlüssel darstellbar.
fn _assert_thinking_level_map_is_serializable(map: &ThinkingLevelMap) -> String {
    serde_json::to_string(map).unwrap_or_default()
}
