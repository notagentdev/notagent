use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

use crate::types::{
    AssistantContent, AssistantMessage, AssistantMessageEvent, CacheRetention, Context, DoneReason,
    ErrorReason, Model, ProviderEnv, ProviderHeaders, ProviderRequestOptions, SimpleStreamOptions,
    StopReason, TextContent, ThinkingContent, ThinkingLevel, ToolCall, Usage,
};
use crate::utils::diagnostics::{
    DiagnosticErrorInfo, append_assistant_message_diagnostic, create_assistant_message_diagnostic,
};
use crate::utils::event_stream::{
    AssistantMessageEventStream, create_assistant_message_event_stream,
};
use crate::utils::fetch::{FetchBody, FetchRequest, ReqwestFetch};
use crate::utils::headers::provider_headers_to_record;
use crate::utils::json_parse::parse_streaming_json;
use crate::utils::provider_env::get_provider_env_value;
use crate::utils::utf8_stream::Utf8StreamDecoder;

const MAX_DIAGNOSTIC_STRING_LENGTH: usize = 8192;

/// `toolChoice?: "auto" | "none" | "required" | { type: "function", … }`
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PiMessagesToolChoice {
    Auto,
    None,
    Required,
    Function(String),
}

impl PiMessagesToolChoice {
    fn to_value(&self) -> Value {
        match self {
            PiMessagesToolChoice::Auto => json!("auto"),
            PiMessagesToolChoice::None => json!("none"),
            PiMessagesToolChoice::Required => json!("required"),
            PiMessagesToolChoice::Function(name) => {
                json!({ "type": "function", "function": { "name": name } })
            }
        }
    }
}

/// `PiMessagesOptions extends StreamOptions`
#[derive(Debug, Clone, Default)]
pub struct PiMessagesOptions {
    pub reasoning: Option<ThinkingLevel>,
    pub tool_choice: Option<PiMessagesToolChoice>,
    /// Ask the backend for debug metadata.
    pub debug: bool,
    pub temperature: Option<f64>,
    pub max_tokens: Option<u64>,
    pub cache_retention: Option<CacheRetention>,
    pub session_id: Option<String>,
    pub headers: Option<ProviderHeaders>,
    pub env: Option<ProviderEnv>,
}

/// `class PiMessagesResponseError`
#[derive(Debug, Clone)]
pub struct PiMessagesResponseError {
    pub message: String,
    pub code: Option<String>,
    pub diagnostic_details: Map<String, Value>,
}

/// The error the adapter reports through the `error` event.
#[derive(Debug, Clone)]
enum PiMessagesError {
    Plain(String),
    Response(PiMessagesResponseError),
}

impl PiMessagesError {
    fn message(&self) -> String {
        match self {
            PiMessagesError::Plain(message) => message.clone(),
            PiMessagesError::Response(error) => error.message.clone(),
        }
    }
}

/// `parsePiMessagesErrorBody(body)`
fn parse_error_body(body: &str) -> Option<Value> {
    let parsed: Value = serde_json::from_str(body).ok()?;
    let error = parsed.get("error")?;
    (error.is_object()).then_some(parsed)
}

fn truncate_diagnostic_string(value: &str) -> String {
    let characters: Vec<char> = value.chars().collect();
    if characters.len() > MAX_DIAGNOSTIC_STRING_LENGTH {
        format!(
            "{}…",
            characters[..MAX_DIAGNOSTIC_STRING_LENGTH]
                .iter()
                .collect::<String>()
        )
    } else {
        value.to_string()
    }
}

/// `formatPiMessagesResponseError(response, body, errorBody)`
fn format_response_error(
    status: u16,
    status_text: &str,
    body: &str,
    error_body: Option<&Value>,
) -> String {
    let message = error_body
        .and_then(|error_body| error_body.get("error"))
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str);
    let code = error_body
        .and_then(|error_body| error_body.get("error"))
        .and_then(|error| error.get("code"))
        .and_then(Value::as_str);
    let suffix = message.unwrap_or(body);
    let code_suffix = code.map(|code| format!(" ({code})")).unwrap_or_default();
    format!("{status} {status_text}: {suffix}{code_suffix}")
}

/// `createPiMessagesResponseError(model, url, response, body)`
fn create_response_error(
    model: &Model,
    url: &str,
    status: u16,
    status_text: &str,
    body: &str,
    timestamp: i64,
) -> PiMessagesResponseError {
    let error_body = parse_error_body(body);
    let code = error_body
        .as_ref()
        .and_then(|error_body| error_body.get("error"))
        .and_then(|error| error.get("code"))
        .and_then(Value::as_str)
        .map(str::to_string);

    let mut details = Map::new();
    details.insert("version".into(), json!(1));
    details.insert("provider".into(), json!(model.provider));
    details.insert("model".into(), json!(model.id));
    details.insert("url".into(), json!(url));
    details.insert("status".into(), json!(status));
    details.insert("statusText".into(), json!(status_text));
    if let Some(error) = error_body
        .as_ref()
        .and_then(|error_body| error_body.get("error"))
    {
        details.insert("error".into(), error.clone());
    }
    if error_body.is_none() {
        details.insert("body".into(), json!(truncate_diagnostic_string(body)));
    }
    details.insert("timestampMs".into(), json!(timestamp));

    PiMessagesResponseError {
        message: format_response_error(status, status_text, body, error_body.as_ref()),
        code,
        diagnostic_details: details,
    }
}

/// `resolveCacheRetention(cacheRetention, env)`
fn resolve_cache_retention(
    cache_retention: Option<CacheRetention>,
    env: Option<&ProviderEnv>,
) -> Option<CacheRetention> {
    if let Some(cache_retention) = cache_retention {
        return Some(cache_retention);
    }
    // Backend defaults apply when unset; only the legacy env opt-in is mapped.
    (get_provider_env_value("PI_CACHE_RETENTION", env).as_deref() == Some("long"))
        .then_some(CacheRetention::Long)
}

/// `new URL(`${model.baseUrl.replace(/\/+$/u, "")}/messages`)` plus `?debug=1`.
pub fn build_request_url(model: &Model, debug: bool) -> String {
    let base = model.base_url.trim_end_matches('/');
    let url = format!("{base}/messages");
    if debug { format!("{url}?debug=1") } else { url }
}

/// The `{ model, context, options }` body of the POST.
pub fn build_request_body(
    model: &Model,
    context: &Context,
    options: &PiMessagesOptions,
) -> Result<Value, serde_json::Error> {
    let mut request_options = Map::new();
    if let Some(temperature) = options.temperature {
        request_options.insert("temperature".into(), json!(temperature));
    }
    if let Some(max_tokens) = options.max_tokens {
        request_options.insert("maxTokens".into(), json!(max_tokens));
    }
    if let Some(reasoning) = options.reasoning {
        request_options.insert("reasoning".into(), serde_json::to_value(reasoning)?);
    }
    if let Some(cache_retention) =
        resolve_cache_retention(options.cache_retention, options.env.as_ref())
    {
        request_options.insert(
            "cacheRetention".into(),
            serde_json::to_value(cache_retention)?,
        );
    }
    if let Some(session_id) = &options.session_id {
        request_options.insert("sessionId".into(), json!(session_id));
    }
    if let Some(tool_choice) = &options.tool_choice {
        request_options.insert("toolChoice".into(), tool_choice.to_value());
    }

    Ok(json!({
        "model": model.id,
        "context": serde_json::to_value(context)?,
        "options": Value::Object(request_options),
    }))
}

/// `headers` of the POST.
pub fn build_request_headers(api_key: &str, options: &PiMessagesOptions) -> Vec<(String, String)> {
    let mut headers: Vec<(String, String)> = vec![
        ("authorization".to_string(), format!("Bearer {api_key}")),
        ("accept".to_string(), "text/event-stream".to_string()),
        ("content-type".to_string(), "application/json".to_string()),
    ];
    // The spread of `providerHeadersToRecord` only adds string values; `null`
    // entries are dropped rather than deleting a default.
    if let Some(overrides) = provider_headers_to_record(options.headers.as_ref()) {
        for (name, value) in overrides {
            let lowered = name.to_lowercase();
            match headers
                .iter()
                .position(|(existing, _)| existing.to_lowercase() == lowered)
            {
                Some(index) => headers[index] = (name, value),
                None => headers.push((name, value)),
            }
        }
    }
    headers
}

// ---------------------------------------------------------------------------
// Event conversion
// ---------------------------------------------------------------------------

/// `createEventConverter(model)` — the backend sends already indexed events, so the
/// converter only rebuilds the `partial` message alongside them.
struct EventConverter {
    partial: AssistantMessage,
    tool_json: BTreeMap<usize, String>,
}

impl EventConverter {
    fn new(model: &Model, timestamp: i64) -> Self {
        EventConverter {
            partial: AssistantMessage {
                content: Vec::new(),
                api: model.api.clone(),
                provider: model.provider.clone(),
                model: model.id.clone(),
                response_model: None,
                response_id: None,
                diagnostics: None,
                usage: Usage::default(),
                stop_reason: StopReason::Pending,
                deferred: None,
                error_message: None,
                raw_stop_reason: None,
                end_turn: None,
                timestamp,
            },
            tool_json: BTreeMap::new(),
        }
    }

    /// Grow `content` so `index` exists, as JS array assignment does.
    fn ensure_content(&mut self, index: usize) {
        while self.partial.content.len() <= index {
            self.partial
                .content
                .push(AssistantContent::Text(TextContent::default()));
        }
    }

    fn convert(&mut self, event: &Value, timestamp: i64) -> Option<AssistantMessageEvent> {
        let event_type = event.get("type").and_then(Value::as_str)?;
        let content_index = event
            .get("contentIndex")
            .and_then(Value::as_u64)
            .map(|index| index as usize);

        match event_type {
            "done" | "error" => {
                let usage: Usage = event
                    .get("usage")
                    .cloned()
                    .and_then(|usage| serde_json::from_value(usage).ok())
                    .unwrap_or_default();
                self.partial.usage = usage;
                self.partial.response_id = event
                    .get("responseId")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let reason = event.get("reason").and_then(Value::as_str).unwrap_or("");
                self.append_rewrite_diagnostic(event.get("rewrite"), timestamp);

                if event_type == "done" {
                    self.partial.stop_reason = match reason {
                        "length" => StopReason::Length,
                        "toolUse" => StopReason::ToolUse,
                        _ => StopReason::Stop,
                    };
                    let done_reason = match reason {
                        "length" => DoneReason::Length,
                        "toolUse" => DoneReason::ToolUse,
                        _ => DoneReason::Stop,
                    };
                    return Some(AssistantMessageEvent::Done {
                        reason: done_reason,
                        message: self.partial.clone(),
                    });
                }

                self.partial.stop_reason = if reason == "aborted" {
                    StopReason::Aborted
                } else {
                    StopReason::Error
                };
                self.partial.error_message = event
                    .get("errorMessage")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                return Some(AssistantMessageEvent::Error {
                    reason: if reason == "aborted" {
                        ErrorReason::Aborted
                    } else {
                        ErrorReason::Error
                    },
                    error: self.partial.clone(),
                });
            }

            "start" => {
                return Some(AssistantMessageEvent::Start {
                    partial: self.partial.clone(),
                });
            }

            "text_start" => {
                let index = content_index?;
                self.ensure_content(index);
                self.partial.content[index] = AssistantContent::Text(TextContent::default());
                return Some(AssistantMessageEvent::TextStart {
                    content_index: index,
                    partial: self.partial.clone(),
                });
            }
            "text_delta" => {
                let index = content_index?;
                let delta = event
                    .get("delta")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                self.ensure_content(index);
                if let AssistantContent::Text(block) = &mut self.partial.content[index] {
                    block.text.push_str(&delta);
                }
                return Some(AssistantMessageEvent::TextDelta {
                    content_index: index,
                    delta,
                    partial: self.partial.clone(),
                });
            }
            "text_end" => {
                let index = content_index?;
                let content = event
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                self.ensure_content(index);
                if let AssistantContent::Text(block) = &mut self.partial.content[index] {
                    block.text = content.clone();
                    block.text_signature = event
                        .get("contentSignature")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                }
                return Some(AssistantMessageEvent::TextEnd {
                    content_index: index,
                    content,
                    partial: self.partial.clone(),
                });
            }

            "thinking_start" => {
                let index = content_index?;
                self.ensure_content(index);
                self.partial.content[index] =
                    AssistantContent::Thinking(ThinkingContent::default());
                return Some(AssistantMessageEvent::ThinkingStart {
                    content_index: index,
                    partial: self.partial.clone(),
                });
            }
            "thinking_delta" => {
                let index = content_index?;
                let delta = event
                    .get("delta")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                self.ensure_content(index);
                if let AssistantContent::Thinking(block) = &mut self.partial.content[index] {
                    block.thinking.push_str(&delta);
                }
                return Some(AssistantMessageEvent::ThinkingDelta {
                    content_index: index,
                    delta,
                    partial: self.partial.clone(),
                });
            }
            "thinking_end" => {
                let index = content_index?;
                let content = event
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                self.ensure_content(index);
                if let AssistantContent::Thinking(block) = &mut self.partial.content[index] {
                    block.thinking = content.clone();
                    block.thinking_signature = event
                        .get("contentSignature")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    block.redacted = event.get("redacted").and_then(Value::as_bool);
                }
                return Some(AssistantMessageEvent::ThinkingEnd {
                    content_index: index,
                    content,
                    partial: self.partial.clone(),
                });
            }

            "toolcall_start" => {
                let index = content_index?;
                self.ensure_content(index);
                self.partial.content[index] = AssistantContent::ToolCall(ToolCall {
                    id: event
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    name: event
                        .get("toolName")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    arguments: Map::new(),
                    thought_signature: None,
                    namespace: None,
                    extra: Map::new(),
                });
                self.tool_json.insert(index, String::new());
                return Some(AssistantMessageEvent::ToolcallStart {
                    content_index: index,
                    partial: self.partial.clone(),
                });
            }
            "toolcall_delta" => {
                let index = content_index?;
                let delta = event
                    .get("delta")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let json = self.tool_json.entry(index).or_default();
                json.push_str(&delta);
                let parsed = parse_streaming_json(Some(json.as_str()));
                self.ensure_content(index);
                if let AssistantContent::ToolCall(block) = &mut self.partial.content[index] {
                    block.arguments = parsed.as_object().cloned().unwrap_or_default();
                }
                return Some(AssistantMessageEvent::ToolcallDelta {
                    content_index: index,
                    delta,
                    partial: self.partial.clone(),
                });
            }
            "toolcall_end" => {
                let index = content_index?;
                self.ensure_content(index);
                if let Some(tool_call) = event.get("toolCall")
                    && let Ok(parsed) = serde_json::from_value::<ToolCall>(tool_call.clone())
                    && let AssistantContent::ToolCall(block) = &mut self.partial.content[index]
                {
                    // `Object.assign(block, event.toolCall)`
                    block.id = parsed.id;
                    block.name = parsed.name;
                    block.arguments = parsed.arguments;
                    if parsed.thought_signature.is_some() {
                        block.thought_signature = parsed.thought_signature;
                    }
                    if parsed.namespace.is_some() {
                        block.namespace = parsed.namespace;
                    }
                }
                self.tool_json.remove(&index);
                let AssistantContent::ToolCall(tool_call) = self.partial.content[index].clone()
                else {
                    return None;
                };
                return Some(AssistantMessageEvent::ToolcallEnd {
                    content_index: index,
                    tool_call,
                    partial: self.partial.clone(),
                });
            }

            _ => {}
        }

        None
    }

    /// `appendRewriteDiagnostic(message, rewrite)`
    fn append_rewrite_diagnostic(&mut self, rewrite: Option<&Value>, timestamp: i64) {
        let Some(rewrite) = rewrite.filter(|rewrite| !rewrite.is_null()) else {
            return;
        };
        append_assistant_message_diagnostic(
            &mut self.partial.diagnostics,
            crate::utils::diagnostics::AssistantMessageDiagnostic {
                r#type: "pi_messages_rewrite".to_string(),
                timestamp,
                error: None,
                details: rewrite.as_object().cloned(),
            },
        );
    }
}

/// `parsePiMessagesEvent(raw)` — only the first `data:` line counts.
fn parse_event(raw: &str) -> Option<Value> {
    let data = raw
        .split('\n')
        .find(|line| line.starts_with("data:"))
        .map(|line| line[5..].trim())?;
    if data.is_empty() || data == "[DONE]" {
        return None;
    }
    serde_json::from_str(data).ok()
}

// ---------------------------------------------------------------------------
// Transport
// ---------------------------------------------------------------------------

/// `stream(model, context, options)`
pub fn stream(
    model: Model,
    context: Context,
    request: ProviderRequestOptions,
    options: PiMessagesOptions,
) -> AssistantMessageEventStream {
    let outer = create_assistant_message_event_stream();
    let stream = outer.clone();

    tokio::spawn(async move {
        let timestamp = crate::auth::resolve::now_ms();
        let mut converter = EventConverter::new(&model, timestamp);
        if let Err(error) = run_request(
            &model,
            &context,
            &request,
            &options,
            &mut converter,
            &stream,
            timestamp,
        )
        .await
        {
            let aborted = request
                .signal
                .as_ref()
                .is_some_and(tokio_util::sync::CancellationToken::is_cancelled);
            stream.push(create_error_event(&model, &error, aborted, timestamp));
        }
    });

    outer
}

/// `createErrorEvent(model, error, aborted)`
fn create_error_event(
    model: &Model,
    error: &PiMessagesError,
    aborted: bool,
    timestamp: i64,
) -> AssistantMessageEvent {
    let mut assistant_message = AssistantMessage {
        content: Vec::new(),
        api: model.api.clone(),
        provider: model.provider.clone(),
        model: model.id.clone(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: if aborted {
            StopReason::Aborted
        } else {
            StopReason::Error
        },
        deferred: None,
        error_message: Some(error.message()),
        raw_stop_reason: None,
        end_turn: None,
        timestamp,
    };

    if !aborted && let PiMessagesError::Response(response_error) = error {
        append_assistant_message_diagnostic(
            &mut assistant_message.diagnostics,
            create_assistant_message_diagnostic(
                "pi_messages_response_failure",
                DiagnosticErrorInfo {
                    name: Some("PiMessagesResponseError".to_string()),
                    message: response_error.message.clone(),
                    stack: None,
                    code: response_error.code.clone().map(Value::String),
                },
                Some(response_error.diagnostic_details.clone()),
                timestamp,
            ),
        );
    }

    AssistantMessageEvent::Error {
        reason: if aborted {
            ErrorReason::Aborted
        } else {
            ErrorReason::Error
        },
        error: assistant_message,
    }
}

async fn run_request(
    model: &Model,
    context: &Context,
    request: &ProviderRequestOptions,
    options: &PiMessagesOptions,
    converter: &mut EventConverter,
    stream: &AssistantMessageEventStream,
    timestamp: i64,
) -> Result<(), PiMessagesError> {
    let Some(api_key) = request.api_key.as_deref().filter(|key| !key.is_empty()) else {
        return Err(PiMessagesError::Plain(format!(
            "No API key provided for provider \"{}\"",
            model.provider
        )));
    };

    let url = build_request_url(model, options.debug);
    let mut payload = build_request_body(model, context, options)
        .map_err(|error| PiMessagesError::Plain(error.to_string()))?;
    if let Some(on_payload) = &request.on_payload
        && let Some(next) = on_payload(payload.clone(), model).await
    {
        payload = next;
    }

    let fetch = request
        .fetch
        .clone()
        .unwrap_or_else(|| std::sync::Arc::new(ReqwestFetch::default()));
    let response = fetch
        .fetch(FetchRequest {
            method: "POST".to_string(),
            url: url.clone(),
            headers: build_request_headers(api_key, options),
            body: Some(
                serde_json::to_vec(&payload)
                    .map_err(|error| PiMessagesError::Plain(error.to_string()))?,
            ),
        })
        .await
        .map_err(|error| PiMessagesError::Plain(error.to_string()))?;

    if let Some(on_response) = &request.on_response {
        on_response(
            crate::types::ProviderResponse {
                status: response.status,
                headers: response.headers.iter().cloned().collect(),
            },
            model,
        )
        .await;
    }

    if !(200..300).contains(&response.status) {
        let status = response.status;
        let status_text = response.status_text.clone();
        let body = read_body(response.body).await;
        return Err(PiMessagesError::Response(create_response_error(
            model,
            &url,
            status,
            &status_text,
            &body,
            timestamp,
        )));
    }

    let mut buffer = String::new();
    let mut utf8 = Utf8StreamDecoder::new();
    let mut body = response.body;
    loop {
        let chunk = match &mut body {
            FetchBody::Bytes(bytes) => {
                let text = String::from_utf8_lossy(bytes).into_owned();
                *bytes = Vec::new();
                if text.is_empty() { None } else { Some(text) }
            }
            FetchBody::Stream(receiver) => match receiver.recv().await {
                Some(Ok(bytes)) => Some(utf8.decode(&bytes)),
                Some(Err(error)) => return Err(PiMessagesError::Plain(error.to_string())),
                None => Some(utf8.finish()).filter(|tail| !tail.is_empty()),
            },
        };
        let Some(chunk) = chunk else { break };
        buffer.push_str(&chunk);
        buffer = buffer.replace("\r\n", "\n");

        while let Some(split) = buffer.find("\n\n") {
            let raw = buffer[..split].to_string();
            buffer = buffer[split + 2..].to_string();
            if let Some(event) = parse_event(&raw)
                && let Some(converted) = converter.convert(&event, timestamp)
            {
                let terminal = matches!(
                    converted,
                    AssistantMessageEvent::Done { .. } | AssistantMessageEvent::Error { .. }
                );
                stream.push(converted);
                if terminal {
                    return Ok(());
                }
            }
        }
    }

    if !buffer.trim().is_empty()
        && let Some(event) = parse_event(&buffer)
        && let Some(converted) = converter.convert(&event, timestamp)
    {
        let terminal = matches!(
            converted,
            AssistantMessageEvent::Done { .. } | AssistantMessageEvent::Error { .. }
        );
        stream.push(converted);
        if terminal {
            return Ok(());
        }
    }

    Err(PiMessagesError::Plain(format!(
        "{} stream ended without a terminal event",
        model.provider
    )))
}

async fn read_body(body: FetchBody) -> String {
    match body {
        FetchBody::Bytes(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        FetchBody::Stream(mut receiver) => {
            let mut bytes = Vec::new();
            while let Some(Ok(chunk)) = receiver.recv().await {
                bytes.extend_from_slice(&chunk);
            }
            String::from_utf8_lossy(&bytes).into_owned()
        }
    }
}

/// `streamSimple(model, context, options)`
pub fn stream_simple(
    model: Model,
    context: Context,
    options: Option<SimpleStreamOptions>,
) -> AssistantMessageEventStream {
    let options = options.unwrap_or_default();
    let pi_options = PiMessagesOptions {
        reasoning: options.reasoning,
        // `toolChoice` and `debug` only exist when the caller passes a
        // such widening, so both stay unset here.
        tool_choice: None,
        debug: false,
        temperature: options.base.temperature,
        max_tokens: options.base.max_tokens,
        cache_retention: options.base.cache_retention,
        session_id: options.base.session_id.clone(),
        headers: options.base.base.headers.clone(),
        env: options.base.base.env.clone(),
    };
    stream(model, context, options.base.base, pi_options)
}
