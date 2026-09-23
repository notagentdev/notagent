use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{Map, Value};

use crate::api::constrained_sampling::{
    GrammarToolInputJsonBuffer, append_grammar_tool_input_json_delta,
    create_grammar_tool_input_properties,
};
use crate::api::openai_completions_compat::{ResolvedOpenAICompletionsCompat, get_compat};
use crate::api::openai_completions_params::{
    OpenAICompletionsOptions, build_client_headers, build_params, get_client_api_key,
    resolve_cache_retention,
};
use crate::api::simple_options::build_base_options;
use crate::api::sse::SseDecoder;
use crate::models::{calculate_cost, clamp_thinking_level};
use crate::types::AssistantMessageEvent;
use crate::types::{
    AssistantContent, AssistantMessage, CacheRetention, Context, DoneReason, ErrorReason, Model,
    ModelThinkingLevel, ProviderRequestOptions, SimpleStreamOptions, StopReason, TextContent,
    ThinkingContent, ThinkingLevel, ToolCall, Usage,
};
use crate::utils::diagnostics::{AssistantMessageDiagnostic, append_assistant_message_diagnostic};
use crate::utils::error_body::{
    NormalizedProviderError, RawProviderError, format_provider_error, normalize_provider_error,
};
use crate::utils::event_stream::{
    AssistantMessageEventStream, create_assistant_message_event_stream,
};
use crate::utils::fetch::{FetchBody, FetchFunction, FetchRequest, ReqwestFetch};
use crate::utils::json_parse::parse_streaming_json;
use crate::utils::provider_retry::{
    ProviderErrorInfo, ProviderRetryError, ProviderRetryOptions, retry_provider_request,
};

/// The HTTP half of an `APIError`; boxed so the error itself stays small.
#[derive(Debug, Clone)]
pub struct ApiErrorResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    /// `error.error` — the parsed JSON body, as the openai SDK exposes it.
    pub body: Option<Value>,
    /// `error.error.metadata.raw` — OpenRouter puts upstream detail here.
    pub raw_metadata: Option<String>,
}

/// Error of the stream; the caller turns it into an `error` event.
#[derive(Debug, Clone, Default)]
pub struct OpenAICompletionsStreamError {
    pub message: String,
    /// Absent for plain thrown errors (aborts, transport failures).
    pub response: Option<Box<ApiErrorResponse>>,
}

impl OpenAICompletionsStreamError {
    fn message(message: impl Into<String>) -> Self {
        OpenAICompletionsStreamError {
            message: message.into(),
            response: None,
        }
    }

    fn normalized(&self) -> NormalizedProviderError {
        normalize_provider_error(&RawProviderError {
            status: self.response.as_ref().map(|response| response.status),
            // The openai SDK carries no `body`; the parsed body lives in `error.error`.
            body_text: None,
            body_json: self
                .response
                .as_ref()
                .and_then(|response| response.body.clone()),
            message: self.message.clone(),
        })
    }
}

impl ProviderErrorInfo for OpenAICompletionsStreamError {
    fn status(&self) -> Option<u16> {
        self.response.as_ref().map(|response| response.status)
    }

    fn header(&self, name: &str) -> Option<String> {
        self.response.as_ref().and_then(|response| {
            response
                .headers
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.clone())
        })
    }

    fn message(&self) -> String {
        self.message.clone()
    }
}

// ---------------------------------------------------------------------------
// Usage and stop reasons
// ---------------------------------------------------------------------------

/// `parseChunkUsage(rawUsage, model)`
pub fn parse_chunk_usage(raw_usage: &Value, model: &Model) -> Usage {
    let number = |value: Option<&Value>| value.and_then(Value::as_f64).unwrap_or(0.0);
    let prompt_tokens = number(raw_usage.get("prompt_tokens"));
    let details = raw_usage.get("prompt_tokens_details");
    // `?? prompt_cache_hit_tokens`: only a missing or null `cached_tokens` falls through.
    let cache_read_tokens = match details.and_then(|details| details.get("cached_tokens")) {
        Some(Value::Null) | None => number(raw_usage.get("prompt_cache_hit_tokens")),
        Some(value) => value.as_f64().unwrap_or(0.0),
    };
    let cache_write_tokens = number(details.and_then(|details| details.get("cache_write_tokens")));

    // Documented OpenAI/OpenRouter semantics: `cached_tokens` counts cache reads (hits).
    // OpenAI never emits `cache_write_tokens`, but OpenRouter-compatible providers can
    // report writes separately. Writes are not subtracted from the read count, otherwise
    // spec-compliant providers are under-reported.
    let input = (prompt_tokens - cache_read_tokens - cache_write_tokens).max(0.0);
    // OpenAI's `completion_tokens` already includes `reasoning_tokens`.
    let output_tokens = number(raw_usage.get("completion_tokens"));
    let reasoning = number(
        raw_usage
            .get("completion_tokens_details")
            .and_then(|details| details.get("reasoning_tokens")),
    );

    let mut usage = Usage {
        input: input as u64,
        output: output_tokens as u64,
        cache_read: cache_read_tokens as u64,
        cache_write: cache_write_tokens as u64,
        reasoning: Some(reasoning as u64),
        total_tokens: Some((input + output_tokens + cache_read_tokens + cache_write_tokens) as u64),
        ..Usage::default()
    };
    calculate_cost(model, &mut usage);
    usage
}

/// `mapStopReason(reason)`
pub fn map_stop_reason(reason: Option<&str>) -> (StopReason, Option<String>) {
    match reason {
        None => (StopReason::Stop, None),
        Some("stop") | Some("end") => (StopReason::Stop, None),
        Some("length") => (StopReason::Length, None),
        Some("function_call") | Some("tool_calls") => (StopReason::ToolUse, None),
        Some("content_filter") => (
            StopReason::Error,
            Some("Provider finish_reason: content_filter".to_string()),
        ),
        Some("network_error") => (
            StopReason::Error,
            Some("Provider finish_reason: network_error".to_string()),
        ),
        Some(reason) => (
            StopReason::Error,
            Some(format!("Provider finish_reason: {reason}")),
        ),
    }
}

// ---------------------------------------------------------------------------
// Streaming state
// ---------------------------------------------------------------------------

/// Diagnostic type of a server-reported fallback from multi-token prediction to
/// plain autoregressive decoding.
pub const MTP_DISABLED_DIAGNOSTIC: &str = "mtplx_mtp_disabled";

/// and deletes them in `finishBlock`.
#[derive(Debug, Clone, Default)]
struct ToolCallScratch {
    partial_args: Option<String>,
    custom_input: Option<CustomInput>,
    stream_index: Option<i64>,
}

#[derive(Debug, Clone)]
struct CustomInput {
    property: String,
    json_buffer: GrammarToolInputJsonBuffer,
}

/// One `delta.tool_calls[]` entry.
#[derive(Debug, Clone, Default)]
struct StreamingToolCallDelta {
    index: Option<i64>,
    id: Option<String>,
    function_name: Option<String>,
    function_arguments: Option<String>,
    custom_name: Option<String>,
    custom_input: Option<String>,
    has_function: bool,
    has_custom: bool,
}

/// The `for await (const chunk of openaiStream)` body as a reusable state machine.
pub struct OpenAICompletionsStreamState {
    pub output: AssistantMessage,
    model: Model,
    compat: ResolvedOpenAICompletionsCompat,
    grammar_tool_input_properties: BTreeMap<String, String>,
    scratch: Vec<Option<ToolCallScratch>>,
    text_index: Option<usize>,
    thinking_index: Option<usize>,
    has_finish_reason: bool,
    tool_call_index_by_stream_index: BTreeMap<i64, usize>,
    tool_call_index_by_id: BTreeMap<String, usize>,
    pending_reasoning_details_by_tool_call_id: BTreeMap<String, String>,
}

impl OpenAICompletionsStreamState {
    pub fn new(
        model: &Model,
        compat: ResolvedOpenAICompletionsCompat,
        grammar_tool_input_properties: BTreeMap<String, String>,
        timestamp: i64,
    ) -> Self {
        OpenAICompletionsStreamState {
            output: AssistantMessage {
                content: vec![],
                api: model.api.clone(),
                provider: model.provider.clone(),
                model: model.id.clone(),
                response_model: None,
                response_id: None,
                diagnostics: None,
                usage: Usage {
                    total_tokens: Some(0),
                    ..Usage::default()
                },
                stop_reason: StopReason::Pending,
                deferred: None,
                error_message: None,
                raw_stop_reason: None,
                end_turn: None,
                timestamp,
            },
            model: model.clone(),
            compat,
            grammar_tool_input_properties,
            scratch: Vec::new(),
            text_index: None,
            thinking_index: None,
            has_finish_reason: false,
            tool_call_index_by_stream_index: BTreeMap::new(),
            tool_call_index_by_id: BTreeMap::new(),
            pending_reasoning_details_by_tool_call_id: BTreeMap::new(),
        }
    }

    fn push_block(&mut self, block: AssistantContent, scratch: Option<ToolCallScratch>) -> usize {
        self.output.content.push(block);
        self.scratch.push(scratch);
        self.output.content.len() - 1
    }

    /// `ensureTextBlock()` — one text block per stream, created on first delta.
    fn ensure_text_block(&mut self, emitted: &mut Vec<AssistantMessageEvent>) -> usize {
        if let Some(index) = self.text_index {
            return index;
        }
        let index = self.push_block(
            AssistantContent::Text(TextContent {
                text: String::new(),
                ..TextContent::default()
            }),
            None,
        );
        self.text_index = Some(index);
        emitted.push(AssistantMessageEvent::TextStart {
            content_index: index,
            partial: self.output.clone(),
        });
        index
    }

    /// `ensureThinkingBlock(signature)`
    fn ensure_thinking_block(
        &mut self,
        thinking_signature: &str,
        emitted: &mut Vec<AssistantMessageEvent>,
    ) -> usize {
        if let Some(index) = self.thinking_index {
            return index;
        }
        let index = self.push_block(
            AssistantContent::Thinking(ThinkingContent {
                thinking: String::new(),
                thinking_signature: Some(thinking_signature.to_string()),
                ..ThinkingContent::default()
            }),
            None,
        );
        self.thinking_index = Some(index);
        emitted.push(AssistantMessageEvent::ThinkingStart {
            content_index: index,
            partial: self.output.clone(),
        });
        index
    }

    fn tool_call_mut(&mut self, index: usize) -> &mut ToolCall {
        match &mut self.output.content[index] {
            AssistantContent::ToolCall(tool_call) => tool_call,
            _ => unreachable!("scratch index always points at a tool call"),
        }
    }

    /// `getCustomToolCallInput(block)`
    fn custom_tool_call_input(&self, index: usize) -> String {
        let Some(Some(scratch)) = self.scratch.get(index) else {
            return String::new();
        };
        let Some(custom_input) = &scratch.custom_input else {
            return String::new();
        };
        match &self.output.content[index] {
            AssistantContent::ToolCall(tool_call) => tool_call
                .arguments
                .get(&custom_input.property)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            _ => String::new(),
        }
    }

    /// `appendCustomToolCallInput(block, nextInput, close)`
    fn append_custom_tool_call_input(
        &mut self,
        index: usize,
        next_input: &str,
        close: bool,
    ) -> Result<Option<String>, OpenAICompletionsStreamError> {
        let Some(Some(scratch)) = self.scratch.get_mut(index) else {
            return Ok(None);
        };
        let Some(custom_input) = &mut scratch.custom_input else {
            return Ok(None);
        };
        let property = custom_input.property.clone();
        let delta = append_grammar_tool_input_json_delta(
            &mut custom_input.json_buffer,
            &property,
            next_input,
            close,
        )
        .map_err(|error| OpenAICompletionsStreamError::message(error.to_string()))?;
        let mut arguments = Map::new();
        arguments.insert(property, Value::String(next_input.to_string()));
        self.tool_call_mut(index).arguments = arguments;
        Ok(delta)
    }

    /// `applyPendingReasoningDetail(block)`
    fn apply_pending_reasoning_detail(&mut self, index: usize) {
        let id = self.tool_call_mut(index).id.clone();
        if id.is_empty() {
            return;
        }
        if let Some(detail) = self.pending_reasoning_details_by_tool_call_id.remove(&id) {
            self.tool_call_mut(index).thought_signature = Some(detail);
        }
    }

    /// `ensureToolCallBlock(toolCall)`
    fn ensure_tool_call_block(
        &mut self,
        delta: &StreamingToolCallDelta,
        emitted: &mut Vec<AssistantMessageEvent>,
    ) -> usize {
        let stream_index = delta.index;
        let name = delta
            .function_name
            .clone()
            .or_else(|| delta.custom_name.clone())
            .unwrap_or_default();

        let mut index = stream_index
            .and_then(|stream_index| self.tool_call_index_by_stream_index.get(&stream_index))
            .copied();
        if index.is_none()
            && let Some(id) = delta.id.as_ref().filter(|id| !id.is_empty())
        {
            index = self.tool_call_index_by_id.get(id).copied();
        }

        let index = match index {
            Some(index) => index,
            None => {
                // The "input" fallback should never be taken; it only gives an invented
                // tool a place to stash its argument.
                let custom_input_property = if delta.has_custom && !delta.has_function {
                    Some(
                        self.grammar_tool_input_properties
                            .get(&name)
                            .cloned()
                            .unwrap_or_else(|| "input".to_string()),
                    )
                } else {
                    None
                };
                let mut arguments = Map::new();
                if let Some(property) = &custom_input_property {
                    arguments.insert(property.clone(), Value::String(String::new()));
                }
                let scratch = ToolCallScratch {
                    partial_args: if custom_input_property.is_none() {
                        Some(String::new())
                    } else {
                        None
                    },
                    custom_input: custom_input_property.map(|property| CustomInput {
                        property,
                        json_buffer: GrammarToolInputJsonBuffer::default(),
                    }),
                    stream_index,
                };
                let index = self.push_block(
                    AssistantContent::ToolCall(ToolCall {
                        id: delta.id.clone().unwrap_or_default(),
                        name: name.clone(),
                        arguments,
                        ..ToolCall::default()
                    }),
                    Some(scratch),
                );
                if let Some(stream_index) = stream_index {
                    self.tool_call_index_by_stream_index
                        .insert(stream_index, index);
                }
                if let Some(id) = delta.id.as_ref().filter(|id| !id.is_empty()) {
                    self.tool_call_index_by_id.insert(id.clone(), index);
                }
                emitted.push(AssistantMessageEvent::ToolcallStart {
                    content_index: index,
                    partial: self.output.clone(),
                });
                index
            }
        };

        if let Some(stream_index) = stream_index
            && let Some(Some(scratch)) = self.scratch.get_mut(index)
            && scratch.stream_index.is_none()
        {
            scratch.stream_index = Some(stream_index);
            self.tool_call_index_by_stream_index
                .insert(stream_index, index);
        }
        if let Some(id) = delta.id.as_ref().filter(|id| !id.is_empty()) {
            self.tool_call_index_by_id.insert(id.clone(), index);
        }
        if self.tool_call_mut(index).name.is_empty() && !name.is_empty() {
            self.tool_call_mut(index).name = name;
        }
        if delta.has_custom
            && !delta.has_function
            && self
                .scratch
                .get(index)
                .and_then(Option::as_ref)
                .is_some_and(|scratch| scratch.custom_input.is_none())
        {
            let block_name = self.tool_call_mut(index).name.clone();
            let property = self
                .grammar_tool_input_properties
                .get(&block_name)
                .cloned()
                .unwrap_or_else(|| "input".to_string());
            let mut arguments = Map::new();
            arguments.insert(property.clone(), Value::String(String::new()));
            self.tool_call_mut(index).arguments = arguments;
            if let Some(Some(scratch)) = self.scratch.get_mut(index) {
                scratch.custom_input = Some(CustomInput {
                    property,
                    json_buffer: GrammarToolInputJsonBuffer::default(),
                });
                scratch.partial_args = None;
            }
        }
        self.apply_pending_reasoning_detail(index);
        index
    }

    /// `finishBlock(block)` for every block, in content order.
    pub fn finish_blocks(
        &mut self,
    ) -> Result<Vec<AssistantMessageEvent>, OpenAICompletionsStreamError> {
        let mut emitted = Vec::new();
        for index in 0..self.output.content.len() {
            match &self.output.content[index] {
                AssistantContent::Text(block) => {
                    let content = block.text.clone();
                    emitted.push(AssistantMessageEvent::TextEnd {
                        content_index: index,
                        content,
                        partial: self.output.clone(),
                    });
                }
                AssistantContent::Thinking(block) => {
                    let content = block.thinking.clone();
                    emitted.push(AssistantMessageEvent::ThinkingEnd {
                        content_index: index,
                        content,
                        partial: self.output.clone(),
                    });
                }
                AssistantContent::ToolCall(_) => {
                    let has_custom_input = self
                        .scratch
                        .get(index)
                        .and_then(Option::as_ref)
                        .is_some_and(|scratch| scratch.custom_input.is_some());
                    if has_custom_input {
                        let input = self.custom_tool_call_input(index);
                        if let Some(delta) =
                            self.append_custom_tool_call_input(index, &input, true)?
                        {
                            emitted.push(AssistantMessageEvent::ToolcallDelta {
                                content_index: index,
                                delta,
                                partial: self.output.clone(),
                            });
                        }
                    } else {
                        let partial_args = self
                            .scratch
                            .get(index)
                            .and_then(Option::as_ref)
                            .and_then(|scratch| scratch.partial_args.clone());
                        let parsed = parse_streaming_json(partial_args.as_deref());
                        self.tool_call_mut(index).arguments =
                            parsed.as_object().cloned().unwrap_or_default();
                    }
                    // Finalize in place and drop the scratch buffers so a replay only
                    // carries parsed arguments.
                    self.scratch[index] = None;
                    let tool_call = self.tool_call_mut(index).clone();
                    emitted.push(AssistantMessageEvent::ToolcallEnd {
                        content_index: index,
                        tool_call,
                        partial: self.output.clone(),
                    });
                }
            }
        }
        Ok(emitted)
    }

    /// Records a server-reported MTP fallback as an informational diagnostic.
    /// `error` stays `None`: the turn succeeded, only slower. The details carry
    /// the server's own field names untranslated. One diagnostic per stream —
    /// the reason arrives on the final chunk, but a server that repeated it
    /// must not produce duplicates.
    fn record_mtp_disabled_reason(&mut self, chunk: &Map<String, Value>) {
        let Some(stats) = chunk.get("mtplx_stats").and_then(Value::as_object) else {
            return;
        };
        let Some(reason) = stats
            .get("mtp_disabled_reason")
            .and_then(Value::as_str)
            .filter(|reason| !reason.is_empty())
        else {
            return;
        };
        let already_recorded = self
            .output
            .diagnostics
            .iter()
            .flatten()
            .any(|diagnostic| diagnostic.r#type == MTP_DISABLED_DIAGNOSTIC);
        if already_recorded {
            return;
        }
        let mut details = Map::new();
        details.insert(
            "mtp_disabled_reason".to_string(),
            Value::String(reason.to_string()),
        );
        if let Some(mode) = stats.get("generation_mode").and_then(Value::as_str) {
            details.insert(
                "generation_mode".to_string(),
                Value::String(mode.to_string()),
            );
        }
        append_assistant_message_diagnostic(
            &mut self.output.diagnostics,
            AssistantMessageDiagnostic {
                r#type: MTP_DISABLED_DIAGNOSTIC.to_string(),
                timestamp: crate::auth::resolve::now_ms(),
                error: None,
                details: Some(details),
            },
        );
    }

    /// One decoded `ChatCompletionChunk`.
    pub fn process_chunk(
        &mut self,
        chunk: &Value,
    ) -> Result<Vec<AssistantMessageEvent>, OpenAICompletionsStreamError> {
        let mut emitted = Vec::new();
        let Some(chunk) = chunk.as_object() else {
            return Ok(emitted);
        };

        // OpenAI documents `ChatCompletionChunk.id` as the completion identifier, and
        // every chunk of one stream carries the same value.
        if self.output.response_id.as_deref().is_none_or(str::is_empty)
            && let Some(id) = chunk.get("id").and_then(Value::as_str)
        {
            self.output.response_id = Some(id.to_string());
        }
        if let Some(model) = chunk.get("model").and_then(Value::as_str)
            && !model.is_empty()
            && model != self.model.id
            && self
                .output
                .response_model
                .as_deref()
                .is_none_or(str::is_empty)
        {
            self.output.response_model = Some(model.to_string());
        }
        if let Some(usage) = chunk.get("usage").filter(|usage| is_truthy(usage)) {
            self.output.usage = parse_chunk_usage(usage, &self.model);
        }

        // An MTPLX server appends an `mtplx_stats` block to its final chunk. The
        // one field acted on is `mtp_disabled_reason`: when set, the server has
        // fallen back from multi-token prediction to plain autoregressive
        // decoding, which is invisible in the output and shows only as a slower
        // turn. Keyed on the field rather than the provider id, because a
        // models.json-configured remote server carries a different provider id
        // and still sends the block. Nothing else in the block is recorded.
        self.record_mtp_disabled_reason(chunk);

        let Some(choice) = chunk
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
        else {
            return Ok(emitted);
        };

        // Fallback: some providers (Moonshot) put usage on the choice instead.
        if !chunk.get("usage").is_some_and(is_truthy)
            && let Some(usage) = choice.get("usage").filter(|usage| is_truthy(usage))
        {
            self.output.usage = parse_chunk_usage(usage, &self.model);
        }

        if let Some(finish_reason) = choice
            .get("finish_reason")
            .and_then(Value::as_str)
            .filter(|reason| !reason.is_empty())
        {
            self.output.raw_stop_reason = Some(finish_reason.to_string());
            let (stop_reason, error_message) = map_stop_reason(Some(finish_reason));
            self.output.stop_reason = stop_reason;
            if let Some(error_message) = error_message {
                self.output.error_message = Some(error_message);
            }
            self.has_finish_reason = true;
        }

        let Some(delta) = choice.get("delta").and_then(Value::as_object) else {
            return Ok(emitted);
        };

        if let Some(content) = delta.get("content").and_then(Value::as_str)
            && !content.is_empty()
        {
            let index = self.ensure_text_block(&mut emitted);
            match &mut self.output.content[index] {
                AssistantContent::Text(block) => block.text.push_str(content),
                _ => unreachable!("text_index always points at a text block"),
            }
            emitted.push(AssistantMessageEvent::TextDelta {
                content_index: index,
                delta: content.to_string(),
                partial: self.output.clone(),
            });
        }

        // Reasoning arrives as `reasoning_content` (llama.cpp) or `reasoning` elsewhere.
        // Only the first non-empty field is used: chutes.ai returns both with the same
        // content, which would otherwise be emitted twice.
        let reasoning_field = ["reasoning_content", "reasoning", "reasoning_text"]
            .into_iter()
            .find(|field| {
                delta
                    .get(*field)
                    .and_then(Value::as_str)
                    .is_some_and(|value| !value.is_empty())
            });
        if let Some(field) = reasoning_field
            && let Some(content) = delta.get(field).and_then(Value::as_str)
            && !content.is_empty()
        {
            let thinking_signature = if self.model.provider == "opencode-go" && field == "reasoning"
            {
                "reasoning_content"
            } else {
                field
            };
            let index = self.ensure_thinking_block(thinking_signature, &mut emitted);
            match &mut self.output.content[index] {
                AssistantContent::Thinking(block) => block.thinking.push_str(content),
                _ => unreachable!("thinking_index always points at a thinking block"),
            }
            emitted.push(AssistantMessageEvent::ThinkingDelta {
                content_index: index,
                delta: content.to_string(),
                partial: self.output.clone(),
            });
        }

        if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
            for raw in tool_calls {
                let tool_call = parse_tool_call_delta(raw);
                let index = self.ensure_tool_call_block(&tool_call, &mut emitted);
                if self.tool_call_mut(index).id.is_empty()
                    && let Some(id) = tool_call.id.as_ref().filter(|id| !id.is_empty())
                {
                    self.tool_call_mut(index).id = id.clone();
                    self.tool_call_index_by_id.insert(id.clone(), index);
                }
                let name = tool_call
                    .function_name
                    .clone()
                    .or_else(|| tool_call.custom_name.clone());
                if self.tool_call_mut(index).name.is_empty()
                    && let Some(name) = name.filter(|name| !name.is_empty())
                {
                    self.tool_call_mut(index).name = name;
                }

                let mut delta_text = String::new();
                if let Some(arguments) = tool_call
                    .function_arguments
                    .as_ref()
                    .filter(|arguments| !arguments.is_empty())
                {
                    delta_text = arguments.clone();
                    let partial_args = match self.scratch.get_mut(index).and_then(Option::as_mut) {
                        Some(scratch) => {
                            let partial = scratch.partial_args.get_or_insert_with(String::new);
                            partial.push_str(arguments);
                            partial.clone()
                        }
                        None => arguments.clone(),
                    };
                    let parsed = parse_streaming_json(Some(&partial_args));
                    self.tool_call_mut(index).arguments =
                        parsed.as_object().cloned().unwrap_or_default();
                } else if let Some(input) = tool_call
                    .custom_input
                    .as_ref()
                    .filter(|input| !input.is_empty())
                {
                    let next_input = format!("{}{input}", self.custom_tool_call_input(index));
                    delta_text = self
                        .append_custom_tool_call_input(index, &next_input, false)?
                        .unwrap_or_default();
                }
                emitted.push(AssistantMessageEvent::ToolcallDelta {
                    content_index: index,
                    delta: delta_text,
                    partial: self.output.clone(),
                });
            }
        }

        if let Some(details) = delta.get("reasoning_details").and_then(Value::as_array) {
            for detail in details {
                let Some((id, serialized)) = encrypted_reasoning_detail(detail) else {
                    continue;
                };
                match self.tool_call_index_by_id.get(&id).copied() {
                    Some(index) => {
                        self.tool_call_mut(index).thought_signature = Some(serialized);
                    }
                    None => {
                        self.pending_reasoning_details_by_tool_call_id
                            .insert(id, serialized);
                    }
                }
            }
        }

        Ok(emitted)
    }

    /// The block after the chunk loop: finish reasons, abort and error checks.
    pub fn finish(&mut self, aborted: bool) -> Result<DoneReason, OpenAICompletionsStreamError> {
        if aborted {
            return Err(OpenAICompletionsStreamError::message("Request was aborted"));
        }
        if self.output.stop_reason == StopReason::Aborted {
            return Err(OpenAICompletionsStreamError::message("Request was aborted"));
        }
        if !self.has_finish_reason && !self.compat.supports_finish_reason {
            self.output.stop_reason = if self
                .output
                .content
                .iter()
                .any(|block| matches!(block, AssistantContent::ToolCall(_)))
            {
                StopReason::ToolUse
            } else {
                StopReason::Stop
            };
        }
        if self.output.stop_reason == StopReason::Error {
            return Err(OpenAICompletionsStreamError::message(
                self.output
                    .error_message
                    .clone()
                    .filter(|message| !message.is_empty())
                    .unwrap_or_else(|| "Provider returned an error stop reason".to_string()),
            ));
        }
        if (self.compat.supports_finish_reason && !self.has_finish_reason)
            || self.output.stop_reason == StopReason::Pending
        {
            return Err(OpenAICompletionsStreamError::message(
                "Stream ended without finish_reason",
            ));
        }
        Ok(match self.output.stop_reason {
            StopReason::Stop => DoneReason::Stop,
            StopReason::Length => DoneReason::Length,
            StopReason::ToolUse => DoneReason::ToolUse,
            StopReason::Deferred => DoneReason::Deferred,
            // `stop`, `length`, `toolUse` and `deferred` are the only reachable values
            // here; the checks above rejected the rest.
            _ => DoneReason::Stop,
        })
    }
}

/// `!!value` for a decoded JSON value.
fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::String(value) => !value.is_empty(),
        Value::Number(number) => number.as_f64().is_some_and(|value| value != 0.0),
        _ => true,
    }
}

fn parse_tool_call_delta(raw: &Value) -> StreamingToolCallDelta {
    let function = raw.get("function");
    let custom = raw.get("custom");
    StreamingToolCallDelta {
        index: raw.get("index").and_then(Value::as_i64),
        id: raw.get("id").and_then(Value::as_str).map(str::to_string),
        function_name: function
            .and_then(|function| function.get("name"))
            .and_then(Value::as_str)
            .map(str::to_string),
        function_arguments: function
            .and_then(|function| function.get("arguments"))
            .and_then(Value::as_str)
            .map(str::to_string),
        custom_name: custom
            .and_then(|custom| custom.get("name"))
            .and_then(Value::as_str)
            .map(str::to_string),
        custom_input: custom
            .and_then(|custom| custom.get("input"))
            .and_then(Value::as_str)
            .map(str::to_string),
        // `toolCall.custom && !toolCall.function` — presence, not content.
        has_function: function.is_some_and(is_truthy),
        has_custom: custom.is_some_and(is_truthy),
    }
}

/// `isEncryptedReasoningDetail(detail)` plus the `JSON.stringify` the caller applies.
fn encrypted_reasoning_detail(detail: &Value) -> Option<(String, String)> {
    let object = detail.as_object()?;
    if object.get("type").and_then(Value::as_str) != Some("reasoning.encrypted") {
        return None;
    }
    let id = object.get("id").and_then(Value::as_str)?;
    let data = object.get("data").and_then(Value::as_str)?;
    if id.is_empty() || data.is_empty() {
        return None;
    }
    Some((id.to_string(), serde_json::to_string(detail).ok()?))
}

// ---------------------------------------------------------------------------
// Transport
// ---------------------------------------------------------------------------

/// `stream(model, context, options)` — the full request path.
/// Nothing is thrown after the call: every failure ends the returned stream with an
/// `error` event, as the stream contract requires.
pub fn stream(
    model: Model,
    context: Context,
    request: ProviderRequestOptions,
    options: OpenAICompletionsOptions,
) -> AssistantMessageEventStream {
    let outer = create_assistant_message_event_stream();
    let stream = outer.clone();

    tokio::spawn(async move {
        let timestamp = crate::auth::resolve::now_ms();
        let compat = get_compat(&model);
        let grammar_tool_input_properties = create_grammar_tool_input_properties(
            context.tools.as_deref(),
            compat.supports_openai_grammar_tools,
        )
        .unwrap_or_default();
        let mut state = OpenAICompletionsStreamState::new(
            &model,
            compat.clone(),
            grammar_tool_input_properties.clone(),
            timestamp,
        );

        let outcome = run_request(
            &model,
            &context,
            &request,
            &options,
            &compat,
            &grammar_tool_input_properties,
            &mut state,
            &stream,
            timestamp,
        )
        .await;

        match outcome {
            Ok(reason) => {
                stream.push(AssistantMessageEvent::Done {
                    reason,
                    message: state.output.clone(),
                });
                stream.end(Some(state.output));
            }
            Err(error) => {
                // out of the block types, so only the stop reason has to be set here.
                let aborted = request
                    .signal
                    .as_ref()
                    .is_some_and(|signal| signal.is_cancelled());
                state.output.stop_reason = if aborted {
                    StopReason::Aborted
                } else {
                    StopReason::Error
                };
                let mut message = format_provider_error(&error.normalized(), None);
                // Some providers put extra detail here when routed through OpenRouter.
                // `normalizeProviderError` already folds the parsed body into the message,
                // so the raw metadata is only appended when it is not there yet.
                let raw_metadata = error
                    .response
                    .as_ref()
                    .and_then(|response| response.raw_metadata.clone());
                if let Some(raw) = raw_metadata.as_deref().filter(|raw| !raw.is_empty())
                    && !message.contains(raw)
                {
                    message.push('\n');
                    message.push_str(raw);
                }
                state.output.error_message = Some(message);
                stream.push(AssistantMessageEvent::Error {
                    reason: if aborted {
                        ErrorReason::Aborted
                    } else {
                        ErrorReason::Error
                    },
                    error: state.output.clone(),
                });
                stream.end(Some(state.output));
            }
        }
    });

    outer
}

#[allow(clippy::too_many_arguments)]
async fn run_request(
    model: &Model,
    context: &Context,
    request: &ProviderRequestOptions,
    options: &OpenAICompletionsOptions,
    compat: &ResolvedOpenAICompletionsCompat,
    grammar_tool_input_properties: &BTreeMap<String, String>,
    state: &mut OpenAICompletionsStreamState,
    stream: &AssistantMessageEventStream,
    timestamp: i64,
) -> Result<DoneReason, OpenAICompletionsStreamError> {
    let api_key = get_client_api_key(
        &model.provider,
        request.api_key.as_deref(),
        request.headers.as_ref(),
    )
    .map_err(|error| OpenAICompletionsStreamError::message(error.to_string()))?;

    let cache_retention = resolve_cache_retention(options.cache_retention, options.env.as_ref());
    let cache_session_id = if cache_retention == CacheRetention::None {
        None
    } else {
        options.session_id.as_deref()
    };

    let mut params = build_params(
        model,
        context,
        options,
        compat,
        cache_retention,
        grammar_tool_input_properties,
        timestamp,
    )
    .map_err(|error| OpenAICompletionsStreamError::message(error.to_string()))?;
    if let Some(on_payload) = &request.on_payload
        && let Some(replacement) = on_payload(params.clone(), model).await
    {
        params = replacement;
    }

    let headers = build_client_headers(
        model,
        context,
        request.headers.as_ref(),
        cache_session_id,
        compat,
    );
    let mut request_headers: Vec<(String, String)> = headers
        .into_iter()
        .filter_map(|(key, value)| value.map(|value| (key, value)))
        .collect();
    request_headers.push(("content-type".to_string(), "application/json".to_string()));
    // The SDK sends `accept: application/json` even for streaming requests.
    request_headers.push(("accept".to_string(), "application/json".to_string()));
    if !request_headers
        .iter()
        .any(|(key, _)| key.eq_ignore_ascii_case("authorization"))
    {
        request_headers.push(("authorization".to_string(), format!("Bearer {api_key}")));
    }

    let fetch: FetchFunction = request
        .fetch
        .clone()
        .unwrap_or_else(|| Arc::new(ReqwestFetch::default()));
    let url = format!("{}/chat/completions", model.base_url.trim_end_matches('/'));
    let body = serde_json::to_vec(&params)
        .map_err(|error| OpenAICompletionsStreamError::message(error.to_string()))?;

    // can honour the abort signal.
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
                    .map_err(|error| OpenAICompletionsStreamError::message(error.to_string()))?;
                if response.status < 200 || response.status >= 300 {
                    let headers = response.headers.clone();
                    let status = response.status;
                    let body = read_body(response.body).await;
                    return Err(api_error(status, headers, &body));
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
        ProviderRetryError::Request(error) => error,
        ProviderRetryError::RetryDelayTooLong(message) => {
            OpenAICompletionsStreamError::message(message)
        }
        ProviderRetryError::Aborted => OpenAICompletionsStreamError::message("Request aborted"),
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
    let pump = |events: Vec<crate::api::sse::ServerSentEvent>,
                state: &mut OpenAICompletionsStreamState|
     -> Result<bool, OpenAICompletionsStreamError> {
        for event in events {
            match process_sse_event(&event, state) {
                Ok(emitted) => {
                    for emitted in emitted {
                        stream.push(emitted);
                    }
                }
                Err(StreamDone::Finished) => return Ok(true),
                Err(StreamDone::Failed(error)) => return Err(error),
            }
        }
        Ok(false)
    };

    match response.body {
        FetchBody::Bytes(bytes) => {
            let text = String::from_utf8_lossy(&bytes).to_string();
            let events: Vec<_> = decoder
                .feed(&text)
                .into_iter()
                .chain(decoder.finish())
                .collect();
            pump(events, state)?;
        }
        FetchBody::Stream(mut body) => {
            let mut done = false;
            while !done {
                let Some(chunk) = body.recv().await else {
                    break;
                };
                if request
                    .signal
                    .as_ref()
                    .is_some_and(|signal| signal.is_cancelled())
                {
                    return Err(OpenAICompletionsStreamError::message("Request was aborted"));
                }
                let chunk = chunk
                    .map_err(|error| OpenAICompletionsStreamError::message(error.to_string()))?;
                done = pump(decoder.feed_bytes(&chunk), state)?;
            }
            if !done {
                pump(decoder.finish(), state)?;
            }
        }
    }

    for emitted in state.finish_blocks()? {
        stream.push(emitted);
    }
    let aborted = request
        .signal
        .as_ref()
        .is_some_and(|signal| signal.is_cancelled());
    state.finish(aborted)
}

/// The SDK's SSE loop: `[DONE]` ends the stream, an unparsable payload throws, and a
/// chunk carrying an `error` field becomes an `APIError` without a status.
fn process_sse_event(
    event: &crate::api::sse::ServerSentEvent,
    state: &mut OpenAICompletionsStreamState,
) -> Result<Vec<AssistantMessageEvent>, StreamDone> {
    if event.data.starts_with("[DONE]") {
        return Err(StreamDone::Finished);
    }
    let chunk = serde_json::from_str::<Value>(&event.data).map_err(|error| {
        StreamDone::Failed(OpenAICompletionsStreamError::message(error.to_string()))
    })?;
    if let Some(error) = chunk.get("error").filter(|error| is_truthy(error)) {
        return Err(StreamDone::Failed(api_error_without_status(error)));
    }
    if event.event.as_deref().is_some_and(|name| name == "error") {
        return Err(StreamDone::Failed(api_error_without_status(&Value::Null)));
    }
    state.process_chunk(&chunk).map_err(StreamDone::Failed)
}

/// `[DONE]` versus a real failure — the SSE loop needs both.
enum StreamDone {
    Finished,
    Failed(OpenAICompletionsStreamError),
}

/// `new APIError(undefined, data.error, undefined, headers)`
fn api_error_without_status(error: &Value) -> OpenAICompletionsStreamError {
    let message = match error.get("message").filter(|message| is_truthy(message)) {
        Some(Value::String(message)) => message.clone(),
        Some(message) => message.to_string(),
        None if is_truthy(error) => error.to_string(),
        None => "(no status code or body)".to_string(),
    };
    OpenAICompletionsStreamError {
        message,
        response: None,
    }
}

/// The openai SDK's `APIError.generate(status, errJSON, errText, headers)`.
/// `errJSON` is the parsed body when it is JSON, otherwise the raw text becomes the
/// message. `makeMessage` then prefixes the status, which is what the error text of a
fn api_error(
    status: u16,
    headers: Vec<(String, String)>,
    body: &str,
) -> OpenAICompletionsStreamError {
    // `APIError.generate(status, errorResponse, …)` keeps `errorResponse.error`, not the
    // whole body.
    let err_json = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|body| body.get("error").cloned());
    let message = match err_json.as_ref().filter(|json| is_truthy(json)) {
        Some(json) => match json.get("message").filter(|message| is_truthy(message)) {
            Some(Value::String(message)) => message.clone(),
            Some(message) => message.to_string(),
            None => json.to_string(),
        },
        None => body.to_string(),
    };
    let message = if message.is_empty() {
        format!("{status} status code (no body)")
    } else {
        format!("{status} {message}")
    };
    OpenAICompletionsStreamError {
        message,
        response: Some(Box::new(ApiErrorResponse {
            status,
            headers,
            raw_metadata: err_json.as_ref().and_then(raw_error_metadata),
            body: err_json,
        })),
    }
}

/// `error.error.metadata.raw`
fn raw_error_metadata(parsed: &Value) -> Option<String> {
    let raw = parsed.get("metadata")?.get("raw")?;
    if !is_truthy(raw) {
        return None;
    }
    Some(match raw {
        Value::String(value) => value.clone(),
        other => other.to_string(),
    })
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

/// `streamSimple(model, context, options)`
pub fn stream_simple(
    model: Model,
    context: Context,
    options: Option<SimpleStreamOptions>,
) -> AssistantMessageEventStream {
    let options = options.unwrap_or_default();
    if let Err(error) = get_client_api_key(
        &model.provider,
        options.base.base.api_key.as_deref(),
        options.base.base.headers.as_ref(),
    ) {
        let stream = create_assistant_message_event_stream();
        let mut state = OpenAICompletionsStreamState::new(
            &model,
            get_compat(&model),
            BTreeMap::new(),
            crate::auth::resolve::now_ms(),
        );
        state.output.stop_reason = StopReason::Error;
        state.output.error_message = Some(error.to_string());
        stream.push(AssistantMessageEvent::Error {
            reason: ErrorReason::Error,
            error: state.output.clone(),
        });
        stream.end(Some(state.output));
        return stream;
    }

    let base = build_base_options(
        &model,
        &context,
        Some(&options),
        options.base.base.api_key.clone(),
    );
    let clamped_reasoning = options
        .reasoning
        .map(|reasoning| clamp_thinking_level(&model, reasoning.into()));
    let reasoning_effort = match clamped_reasoning {
        Some(ModelThinkingLevel::Minimal) => Some(ThinkingLevel::Minimal),
        Some(ModelThinkingLevel::Low) => Some(ThinkingLevel::Low),
        Some(ModelThinkingLevel::Medium) => Some(ThinkingLevel::Medium),
        Some(ModelThinkingLevel::High) => Some(ThinkingLevel::High),
        Some(ModelThinkingLevel::Xhigh) => Some(ThinkingLevel::Xhigh),
        Some(ModelThinkingLevel::Max) => Some(ThinkingLevel::Max),
        Some(ModelThinkingLevel::Off) | None => None,
    };

    stream(
        model,
        context,
        base.base.clone(),
        OpenAICompletionsOptions {
            tool_choice: None,
            reasoning_effort,
            thinking_budgets: options.thinking_budgets,
            max_tokens: base.max_tokens,
            temperature: base.temperature,
            sampling_params: base.sampling_params.clone(),
            cache_retention: base.cache_retention,
            session_id: base.session_id.clone(),
            env: base.base.env.clone(),
        },
    )
}
