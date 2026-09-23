use serde_json::Value;

use crate::models::calculate_cost;
use crate::types::{
    AssistantContent, AssistantMessage, AssistantMessageEvent, DoneReason, Model, StopReason,
    TextContent, ThinkingContent, ToolCall, Usage,
};
use crate::utils::json_parse::parse_streaming_json;
use crate::utils::provider_retry::{
    ProviderErrorInfo, ProviderRetryError, ProviderRetryOptions, retry_provider_request,
};

pub const ANTHROPIC_MESSAGE_EVENTS: [&str; 6] = [
    "message_start",
    "message_delta",
    "message_stop",
    "content_block_start",
    "content_block_delta",
    "content_block_stop",
];

/// Per-block streaming scratch: Anthropic's block index and the raw tool-argument JSON.
#[derive(Debug, Default, Clone)]
struct BlockScratch {
    index: i64,
    partial_json: String,
}

/// Streaming state of one Anthropic response.
pub struct AnthropicStreamState {
    pub output: AssistantMessage,
    scratch: Vec<BlockScratch>,
    model: Model,
    /// Maps Claude Code tool names back to the caller's names for OAuth requests.
    is_oauth: bool,
    tool_names: Vec<String>,
    saw_message_start: bool,
    saw_message_stop: bool,
}

/// A failed HTTP attempt; the retry policy probes its status and headers.
struct RetryableRequestError {
    message: String,
    status: Option<u16>,
    headers: Vec<(String, String)>,
}

impl ProviderErrorInfo for RetryableRequestError {
    fn status(&self) -> Option<u16> {
        self.status
    }

    fn header(&self, name: &str) -> Option<String> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.clone())
    }

    fn message(&self) -> String {
        self.message.clone()
    }
}

/// Error of the stream state machine; the caller turns it into an `error` event.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct AnthropicStreamError(pub String);

impl AnthropicStreamState {
    pub fn new(model: &Model, is_oauth: bool, tool_names: Vec<String>, timestamp: i64) -> Self {
        AnthropicStreamState {
            output: AssistantMessage {
                content: vec![],
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
            scratch: Vec::new(),
            model: model.clone(),
            is_oauth,
            tool_names,
            saw_message_start: false,
            saw_message_stop: false,
        }
    }

    fn snapshot(&self) -> AssistantMessage {
        self.output.clone()
    }

    fn block_position(&self, index: i64) -> Option<usize> {
        self.scratch
            .iter()
            .position(|scratch| scratch.index == index)
    }

    /// `fromClaudeCodeName(name, tools)` — case-insensitive match against the caller's
    /// tools. Named for what it does rather than `from_*`, which Rust reserves for
    /// constructors.
    fn caller_tool_name(&self, name: &str) -> String {
        if !self.is_oauth {
            return name.to_string();
        }
        let lower_name = name.to_lowercase();
        self.tool_names
            .iter()
            .find(|tool_name| tool_name.to_lowercase() == lower_name)
            .cloned()
            .unwrap_or_else(|| name.to_string())
    }

    /// Whether the stream ended without `message_stop` after a `message_start`.
    pub fn ended_without_message_stop(&self) -> bool {
        self.saw_message_start && !self.saw_message_stop
    }

    /// Processes one decoded Anthropic event and returns the resulting stream events.
    pub fn process_event(
        &mut self,
        event: &Value,
    ) -> Result<Vec<AssistantMessageEvent>, AnthropicStreamError> {
        let event_type = event
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let mut emitted = Vec::new();

        match event_type {
            "message_start" => {
                self.saw_message_start = true;
                let message = event.get("message");
                self.output.response_id = message
                    .and_then(|message| message.get("id"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let usage = message.and_then(|message| message.get("usage"));
                // Captured here so input counts survive an early abort.
                self.output.usage.input = number(usage, "input_tokens");
                self.output.usage.output = number(usage, "output_tokens");
                self.output.usage.cache_read = number(usage, "cache_read_input_tokens");
                self.output.usage.cache_write = number(usage, "cache_creation_input_tokens");
                self.output.usage.cache_write1h = Some(
                    usage
                        .and_then(|usage| usage.get("cache_creation"))
                        .map(|creation| number(Some(creation), "ephemeral_1h_input_tokens"))
                        .unwrap_or(0),
                );
                self.recompute_usage();
            }
            "content_block_start" => {
                let index = event
                    .get("index")
                    .and_then(Value::as_i64)
                    .unwrap_or_default();
                let block = event.get("content_block");
                let block_type = block
                    .and_then(|block| block.get("type"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                match block_type {
                    "text" => {
                        self.push_block(
                            AssistantContent::Text(TextContent::new(
                                block
                                    .and_then(|block| block.get("text"))
                                    .and_then(Value::as_str)
                                    .unwrap_or_default(),
                            )),
                            index,
                        );
                        emitted.push(AssistantMessageEvent::TextStart {
                            content_index: self.output.content.len() - 1,
                            partial: self.snapshot(),
                        });
                    }
                    "thinking" => {
                        self.push_block(
                            AssistantContent::Thinking(ThinkingContent {
                                thinking: block
                                    .and_then(|block| block.get("thinking"))
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_string(),
                                thinking_signature: Some(
                                    block
                                        .and_then(|block| block.get("signature"))
                                        .and_then(Value::as_str)
                                        .unwrap_or_default()
                                        .to_string(),
                                ),
                                redacted: None,
                                extra: Default::default(),
                            }),
                            index,
                        );
                        emitted.push(AssistantMessageEvent::ThinkingStart {
                            content_index: self.output.content.len() - 1,
                            partial: self.snapshot(),
                        });
                    }
                    "redacted_thinking" => {
                        self.push_block(
                            AssistantContent::Thinking(ThinkingContent {
                                thinking: "[Reasoning redacted]".to_string(),
                                thinking_signature: block
                                    .and_then(|block| block.get("data"))
                                    .and_then(Value::as_str)
                                    .map(str::to_string),
                                redacted: Some(true),
                                extra: Default::default(),
                            }),
                            index,
                        );
                        emitted.push(AssistantMessageEvent::ThinkingStart {
                            content_index: self.output.content.len() - 1,
                            partial: self.snapshot(),
                        });
                    }
                    "tool_use" => {
                        let name = block
                            .and_then(|block| block.get("name"))
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        let arguments = block
                            .and_then(|block| block.get("input"))
                            .and_then(Value::as_object)
                            .cloned()
                            .unwrap_or_default();
                        self.push_block(
                            AssistantContent::ToolCall(ToolCall {
                                id: block
                                    .and_then(|block| block.get("id"))
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_string(),
                                name: self.caller_tool_name(name),
                                arguments,
                                thought_signature: None,
                                namespace: None,
                                extra: Default::default(),
                            }),
                            index,
                        );
                        emitted.push(AssistantMessageEvent::ToolcallStart {
                            content_index: self.output.content.len() - 1,
                            partial: self.snapshot(),
                        });
                    }
                    _ => {}
                }
            }
            "content_block_delta" => {
                let index = event
                    .get("index")
                    .and_then(Value::as_i64)
                    .unwrap_or_default();
                let delta = event.get("delta");
                let delta_type = delta
                    .and_then(|delta| delta.get("type"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let Some(position) = self.block_position(index) else {
                    return Ok(emitted);
                };

                match delta_type {
                    "text_delta" => {
                        let text = delta
                            .and_then(|delta| delta.get("text"))
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        if let Some(AssistantContent::Text(block)) =
                            self.output.content.get_mut(position)
                        {
                            block.text.push_str(text);
                            emitted.push(AssistantMessageEvent::TextDelta {
                                content_index: position,
                                delta: text.to_string(),
                                partial: self.snapshot(),
                            });
                        }
                    }
                    "thinking_delta" => {
                        let text = delta
                            .and_then(|delta| delta.get("thinking"))
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        if let Some(AssistantContent::Thinking(block)) =
                            self.output.content.get_mut(position)
                        {
                            block.thinking.push_str(text);
                            emitted.push(AssistantMessageEvent::ThinkingDelta {
                                content_index: position,
                                delta: text.to_string(),
                                partial: self.snapshot(),
                            });
                        }
                    }
                    "input_json_delta" => {
                        let partial_json = delta
                            .and_then(|delta| delta.get("partial_json"))
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        let is_tool_call = matches!(
                            self.output.content.get(position),
                            Some(AssistantContent::ToolCall(_))
                        );
                        if is_tool_call {
                            self.scratch[position].partial_json.push_str(partial_json);
                            let parsed =
                                parse_streaming_json(Some(&self.scratch[position].partial_json));
                            if let Some(AssistantContent::ToolCall(block)) =
                                self.output.content.get_mut(position)
                            {
                                block.arguments = parsed.as_object().cloned().unwrap_or_default();
                            }
                            emitted.push(AssistantMessageEvent::ToolcallDelta {
                                content_index: position,
                                delta: partial_json.to_string(),
                                partial: self.snapshot(),
                            });
                        }
                    }
                    "signature_delta" => {
                        let signature = delta
                            .and_then(|delta| delta.get("signature"))
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        if let Some(AssistantContent::Thinking(block)) =
                            self.output.content.get_mut(position)
                        {
                            block
                                .thinking_signature
                                .get_or_insert_default()
                                .push_str(signature);
                        }
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                let index = event
                    .get("index")
                    .and_then(Value::as_i64)
                    .unwrap_or_default();
                let Some(position) = self.block_position(index) else {
                    return Ok(emitted);
                };
                let partial_json = std::mem::take(&mut self.scratch[position].partial_json);
                self.scratch[position].index = i64::MIN;

                match self.output.content.get_mut(position) {
                    Some(AssistantContent::Text(block)) => {
                        let content = block.text.clone();
                        emitted.push(AssistantMessageEvent::TextEnd {
                            content_index: position,
                            content,
                            partial: self.snapshot(),
                        });
                    }
                    Some(AssistantContent::Thinking(block)) => {
                        let content = block.thinking.clone();
                        emitted.push(AssistantMessageEvent::ThinkingEnd {
                            content_index: position,
                            content,
                            partial: self.snapshot(),
                        });
                    }
                    Some(AssistantContent::ToolCall(block)) => {
                        block.arguments = parse_streaming_json(Some(&partial_json))
                            .as_object()
                            .cloned()
                            .unwrap_or_default();
                        let tool_call = block.clone();
                        emitted.push(AssistantMessageEvent::ToolcallEnd {
                            content_index: position,
                            tool_call,
                            partial: self.snapshot(),
                        });
                    }
                    None => {}
                }
            }
            "message_delta" => {
                let delta = event.get("delta");
                if let Some(stop_reason) = delta
                    .and_then(|delta| delta.get("stop_reason"))
                    .and_then(Value::as_str)
                {
                    self.output.raw_stop_reason = Some(stop_reason.to_string());
                    let stop_details = delta.and_then(|delta| delta.get("stop_details"));
                    let mapped = map_stop_reason(stop_reason, stop_details)?;
                    self.output.stop_reason = mapped.0;
                    if let Some(error_message) = mapped.1 {
                        self.output.error_message = Some(error_message);
                    }
                }
                // Only present fields are updated, so input counts from message_start
                // survive proxies that omit them here.
                if let Some(usage) = event.get("usage") {
                    if let Some(value) = usage.get("input_tokens").and_then(Value::as_u64) {
                        self.output.usage.input = value;
                    }
                    if let Some(value) = usage.get("output_tokens").and_then(Value::as_u64) {
                        self.output.usage.output = value;
                    }
                    if let Some(value) =
                        usage.get("cache_read_input_tokens").and_then(Value::as_u64)
                    {
                        self.output.usage.cache_read = value;
                    }
                    if let Some(value) = usage
                        .get("cache_creation_input_tokens")
                        .and_then(Value::as_u64)
                    {
                        self.output.usage.cache_write = value;
                    }
                    // Anthropic reports thinking tokens as a subset of output_tokens.
                    if let Some(value) = usage
                        .get("output_tokens_details")
                        .and_then(|details| details.get("thinking_tokens"))
                        .and_then(Value::as_u64)
                    {
                        self.output.usage.reasoning = Some(value);
                    }
                }
                self.recompute_usage();
            }
            "message_stop" => {
                self.saw_message_stop = true;
            }
            _ => {}
        }

        Ok(emitted)
    }

    fn push_block(&mut self, block: AssistantContent, index: i64) {
        self.output.content.push(block);
        self.scratch.push(BlockScratch {
            index,
            partial_json: String::new(),
        });
    }

    fn recompute_usage(&mut self) {
        self.output.usage.total_tokens = Some(
            self.output.usage.input
                + self.output.usage.output
                + self.output.usage.cache_read
                + self.output.usage.cache_write,
        );
        let mut usage = self.output.usage;
        calculate_cost(&self.model, &mut usage);
        self.output.usage = usage;
    }

    pub fn finish(&self) -> Result<DoneReason, AnthropicStreamError> {
        match self.output.stop_reason {
            StopReason::Pending => Err(AnthropicStreamError(
                "Anthropic stream ended without a stop reason".to_string(),
            )),
            StopReason::Aborted | StopReason::Error => Err(AnthropicStreamError(
                self.output
                    .error_message
                    .clone()
                    .unwrap_or_else(|| "An unknown error occurred".to_string()),
            )),
            StopReason::Stop => Ok(DoneReason::Stop),
            StopReason::Length => Ok(DoneReason::Length),
            StopReason::ToolUse => Ok(DoneReason::ToolUse),
            StopReason::Deferred => Ok(DoneReason::Deferred),
        }
    }
}

fn number(value: Option<&Value>, field: &str) -> u64 {
    value
        .and_then(|value| value.get(field))
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

pub fn map_stop_reason(
    reason: &str,
    stop_details: Option<&Value>,
) -> Result<(StopReason, Option<String>), AnthropicStreamError> {
    Ok(match reason {
        "end_turn" => (StopReason::Stop, None),
        "max_tokens" => (StopReason::Length, None),
        "tool_use" => (StopReason::ToolUse, None),
        "refusal" => (
            StopReason::Error,
            Some(
                stop_details
                    .and_then(|details| details.get("explanation"))
                    .and_then(Value::as_str)
                    .filter(|explanation| !explanation.is_empty())
                    .unwrap_or("The model refused to complete the request")
                    .to_string(),
            ),
        ),
        // Resubmitting is good enough.
        "pause_turn" => (StopReason::Stop, None),
        // We never send stop sequences, so this should not occur.
        "stop_sequence" => (StopReason::Stop, None),
        "sensitive" => (
            StopReason::Error,
            Some("Provider stopped with: sensitive".to_string()),
        ),
        other => {
            return Err(AnthropicStreamError(format!(
                "Unhandled stop reason: {other}"
            )));
        }
    })
}

// ---------------------------------------------------------------------------
// Transport
// ---------------------------------------------------------------------------

use std::sync::Arc;

use crate::api::anthropic_params::{
    AnthropicOptions, build_default_headers, build_params, is_oauth_token,
    should_use_fine_grained_tool_streaming_beta,
};
use crate::api::sse::SseDecoder;
use crate::types::{Context, ErrorReason, ProviderHeaders, ProviderRequestOptions};
use crate::utils::event_stream::{
    AssistantMessageEventStream, create_assistant_message_event_stream,
};
use crate::utils::fetch::{FetchBody, FetchFunction, FetchRequest, ReqwestFetch};

/// `assertRequestAuth(provider, apiKey, headers)`
fn assert_request_auth(
    provider: &str,
    api_key: Option<&str>,
    headers: Option<&ProviderHeaders>,
) -> Result<(), AnthropicStreamError> {
    if api_key.is_some_and(|key| !key.is_empty()) {
        return Ok(());
    }
    let has_header = |name: &str| {
        headers.is_some_and(|headers| {
            headers.iter().any(|(key, value)| {
                key.to_lowercase() == name
                    && value.as_ref().is_some_and(|value| !value.trim().is_empty())
            })
        })
    };
    if has_header("authorization") || has_header("x-api-key") || has_header("cf-aig-authorization")
    {
        return Ok(());
    }
    Err(AnthropicStreamError(format!(
        "No API key for provider: {provider}"
    )))
}

/// `stream(model, context, options)` — the full request path.
/// Nothing is thrown after the call: every failure ends the returned stream with an
/// `error` event, as the stream contract requires.
pub fn stream(
    model: Model,
    context: Context,
    request: ProviderRequestOptions,
    options: AnthropicOptions,
) -> AssistantMessageEventStream {
    let outer = create_assistant_message_event_stream();
    let stream = outer.clone();

    tokio::spawn(async move {
        let timestamp = crate::auth::resolve::now_ms();
        let mut state = AnthropicStreamState::new(
            &model,
            request.api_key.as_deref().is_some_and(is_oauth_token)
                && model.provider != "github-copilot",
            context
                .tools
                .iter()
                .flatten()
                .map(|tool| tool.name.clone())
                .collect(),
            timestamp,
        );

        let outcome = run_request(&model, &context, &request, &options, &mut state, &stream).await;
        match outcome {
            Ok(reason) => {
                stream.push(AssistantMessageEvent::Done {
                    reason,
                    message: state.output.clone(),
                });
                stream.end(Some(state.output));
            }
            Err(error) => {
                let aborted = request
                    .signal
                    .as_ref()
                    .is_some_and(|signal| signal.is_cancelled());
                state.output.stop_reason = if aborted {
                    StopReason::Aborted
                } else {
                    StopReason::Error
                };
                state.output.error_message = Some(error.to_string());
                let reason = if aborted {
                    ErrorReason::Aborted
                } else {
                    ErrorReason::Error
                };
                stream.push(AssistantMessageEvent::Error {
                    reason,
                    error: state.output.clone(),
                });
                stream.end(Some(state.output));
            }
        }
    });

    outer
}

async fn run_request(
    model: &Model,
    context: &Context,
    request: &ProviderRequestOptions,
    options: &AnthropicOptions,
    state: &mut AnthropicStreamState,
    stream: &AssistantMessageEventStream,
) -> Result<DoneReason, AnthropicStreamError> {
    assert_request_auth(
        &model.provider,
        request.api_key.as_deref(),
        request.headers.as_ref(),
    )?;

    // Copilot wants request-shape headers (`X-Initiator`, `Openai-Intent`, vision).
    let copilot_dynamic_headers = (model.provider == "github-copilot").then(|| {
        crate::api::github_copilot_headers::build_copilot_dynamic_headers(
            &context.messages,
            crate::api::github_copilot_headers::has_copilot_vision_input(&context.messages),
        )
    });
    let (headers, is_oauth) = build_default_headers(
        model,
        request.api_key.as_deref(),
        options.interleaved_thinking.unwrap_or(true),
        should_use_fine_grained_tool_streaming_beta(model, context),
        request.headers.as_ref(),
        copilot_dynamic_headers.as_ref(),
        cache_session_id(options),
    );

    let mut params = build_params(
        model,
        context,
        is_oauth,
        options,
        crate::auth::resolve::now_ms(),
    );
    if let Some(on_payload) = &request.on_payload
        && let Some(replacement) = on_payload(params.clone(), model).await
    {
        params = replacement;
    }

    let mut request_headers: Vec<(String, String)> = headers.into_iter().collect();
    request_headers.push(("content-type".to_string(), "application/json".to_string()));
    request_headers.push(("anthropic-version".to_string(), "2023-06-01".to_string()));
    if let Some(api_key) = request.api_key.as_deref() {
        if is_oauth || model.provider == "github-copilot" {
            request_headers.push(("authorization".to_string(), format!("Bearer {api_key}")));
        } else {
            request_headers.push(("x-api-key".to_string(), api_key.to_string()));
        }
    }

    let fetch: FetchFunction = request
        .fetch
        .clone()
        .unwrap_or_else(|| Arc::new(ReqwestFetch::default()));
    let url = format!("{}/v1/messages", model.base_url.trim_end_matches('/'));
    let body =
        serde_json::to_vec(&params).map_err(|error| AnthropicStreamError(error.to_string()))?;

    // honour the abort signal.
    let response = retry_provider_request(
        || {
            let fetch = fetch.clone();
            let url = url.clone();
            let request_headers = request_headers.clone();
            let body = body.clone();
            async move {
                let response = fetch
                    .fetch(FetchRequest {
                        method: "POST".to_string(),
                        url,
                        headers: request_headers,
                        body: Some(body),
                    })
                    .await
                    .map_err(|error| RetryableRequestError {
                        message: error.to_string(),
                        status: None,
                        headers: Vec::new(),
                    })?;
                if response.status < 200 || response.status >= 300 {
                    let status = response.status;
                    let headers = response.headers.clone();
                    let body = read_body(response.body).await;
                    return Err(RetryableRequestError {
                        message: format!("{status}: {body}"),
                        status: Some(status),
                        headers,
                    });
                }
                Ok(response)
            }
        },
        ProviderRetryOptions {
            max_retries: request.max_retries,
            max_retry_delay_ms: request.max_retry_delay_ms,
            signal: request.signal.clone(),
        },
    )
    .await
    .map_err(|error| match error {
        ProviderRetryError::Request(error) => AnthropicStreamError(error.message),
        ProviderRetryError::RetryDelayTooLong(message) => AnthropicStreamError(message),
        ProviderRetryError::Aborted => AnthropicStreamError("Request aborted".to_string()),
    })?;

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

    stream.push(AssistantMessageEvent::Start {
        partial: state.output.clone(),
    });

    let mut decoder = SseDecoder::new();
    let mut body = match response.body {
        FetchBody::Stream(receiver) => receiver,
        FetchBody::Bytes(bytes) => {
            // A buffered body is decoded in one go.
            let text = String::from_utf8_lossy(&bytes).to_string();
            for event in decoder.feed(&text).into_iter().chain(decoder.finish()) {
                process_sse_event(&event, state, stream)?;
            }
            return finish(state, request);
        }
    };

    while let Some(chunk) = body.recv().await {
        if request
            .signal
            .as_ref()
            .is_some_and(|signal| signal.is_cancelled())
        {
            return Err(AnthropicStreamError("Request was aborted".to_string()));
        }
        let chunk = chunk.map_err(|error| AnthropicStreamError(error.to_string()))?;
        for event in decoder.feed_bytes(&chunk) {
            process_sse_event(&event, state, stream)?;
        }
    }
    for event in decoder.finish() {
        process_sse_event(&event, state, stream)?;
    }

    finish(state, request)
}

fn finish(
    state: &AnthropicStreamState,
    request: &ProviderRequestOptions,
) -> Result<DoneReason, AnthropicStreamError> {
    if request
        .signal
        .as_ref()
        .is_some_and(|signal| signal.is_cancelled())
    {
        return Err(AnthropicStreamError("Request was aborted".to_string()));
    }
    if state.ended_without_message_stop() {
        return Err(AnthropicStreamError(
            "Anthropic stream ended before message_stop".to_string(),
        ));
    }
    state.finish()
}

fn process_sse_event(
    event: &crate::api::sse::ServerSentEvent,
    state: &mut AnthropicStreamState,
    stream: &AssistantMessageEventStream,
) -> Result<(), AnthropicStreamError> {
    let Some(name) = event.event.as_deref() else {
        return Ok(());
    };
    if name == "error" {
        return Err(AnthropicStreamError(event.data.clone()));
    }
    if !ANTHROPIC_MESSAGE_EVENTS.contains(&name) {
        return Ok(());
    }
    let parsed =
        crate::utils::json_parse::parse_json_with_repair(&event.data).map_err(|error| {
            AnthropicStreamError(format!(
                "Could not parse Anthropic SSE event {name}: {error}; data={}; raw={}",
                event.data,
                event.raw.join("\\n")
            ))
        })?;
    for emitted in state.process_event(&parsed)? {
        stream.push(emitted);
    }
    Ok(())
}

async fn read_body(body: FetchBody) -> String {
    match body {
        FetchBody::Bytes(bytes) => String::from_utf8_lossy(&bytes).to_string(),
        FetchBody::Stream(mut receiver) => {
            let mut collected = Vec::new();
            while let Some(Ok(chunk)) = receiver.recv().await {
                collected.extend(chunk);
            }
            String::from_utf8_lossy(&collected).to_string()
        }
    }
}

/// `const cacheSessionId = cacheRetention === "none" ? undefined : options?.sessionId`
fn cache_session_id(options: &AnthropicOptions) -> Option<&str> {
    let retention = crate::api::anthropic_params::resolve_cache_retention(
        options.cache_retention,
        options.env.as_ref(),
    );
    if retention == crate::types::CacheRetention::None {
        return None;
    }
    options.session_id.as_deref().filter(|id| !id.is_empty())
}

/// `streamSimple(model, context, options)` — maps a reasoning level onto the Anthropic
/// thinking options and delegates to [`stream`].
pub fn stream_simple(
    model: Model,
    context: Context,
    options: Option<crate::types::SimpleStreamOptions>,
) -> AssistantMessageEventStream {
    let options = options.unwrap_or_default();
    if let Err(error) = assert_request_auth(
        &model.provider,
        options.base.base.api_key.as_deref(),
        options.base.base.headers.as_ref(),
    ) {
        let stream = create_assistant_message_event_stream();
        let message = crate::api::lazy::create_setup_error_message(
            &model,
            &error,
            crate::auth::resolve::now_ms(),
        );
        stream.push(AssistantMessageEvent::Error {
            reason: ErrorReason::Error,
            error: message.clone(),
        });
        stream.end(Some(message));
        return stream;
    }

    let base = crate::api::simple_options::build_base_options(
        &model,
        &context,
        Some(&options),
        options.base.base.api_key.clone(),
    );
    let request = base.base.clone();
    let mut anthropic = AnthropicOptions {
        max_tokens: base.max_tokens,
        temperature: base.temperature,
        cache_retention: base.cache_retention,
        session_id: base.session_id.clone(),
        metadata: base.metadata.clone(),
        env: base.base.env.clone(),
        ..Default::default()
    };

    let Some(reasoning) = options.reasoning else {
        anthropic.thinking_enabled = Some(false);
        return stream(model, context, request, anthropic);
    };

    if crate::api::anthropic_params::force_adaptive_thinking(&model) {
        // Adaptive thinking: Claude decides when and how much to think.
        anthropic.thinking_enabled = Some(true);
        anthropic.effort = Some(crate::api::anthropic_params::map_thinking_level_to_effort(
            &model,
            Some(reasoning),
        ));
        return stream(model, context, request, anthropic);
    }

    // Budget-based thinking for older models.
    let adjusted = crate::api::simple_options::adjust_max_tokens_for_thinking(
        options.base.max_tokens,
        model.max_tokens,
        reasoning,
        options.thinking_budgets.as_ref(),
    );
    let max_tokens = crate::api::simple_options::clamp_max_tokens_to_context(
        &model,
        &context,
        adjusted.max_tokens,
    );
    anthropic.max_tokens = Some(max_tokens);
    anthropic.thinking_enabled = Some(true);
    anthropic.thinking_budget_tokens = Some(
        adjusted
            .thinking_budget
            .min(max_tokens.saturating_sub(crate::api::simple_options::MIN_ANSWER_TOKENS)),
    );
    stream(model, context, request, anthropic)
}
