//! Shared building blocks of the three OpenAI Responses APIs.
//!
//! 1:1 port of `packages/ai/src/api/openai-responses-shared.ts`: message and tool
//! conversion plus the stream state machine that `openai-responses.ts`,
//! `azure-openai-responses.ts` and `openai-codex-responses.ts` all drive.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};

use crate::api::constrained_sampling::{
    ConstrainedSamplingError, GrammarSyntax, GrammarToolInputJsonBuffer,
    append_grammar_tool_input_json_delta, get_grammar_tool_input, get_json_schema_tool_parameters,
    resolve_grammar_constrained_sampling, resolve_json_schema_strict_sampling,
};
use crate::api::transform_messages::transform_messages;
use crate::models::calculate_cost;
use crate::types::{
    AssistantContent, AssistantMessage, AssistantMessageEvent, Context, Message, Modality, Model,
    StopReason, TextContent, TextOrImageContent, TextSignatureV1, ThinkingContent, Tool, ToolCall,
    Usage, UserContent,
};
use crate::utils::hash::short_hash;
use crate::utils::json_parse::parse_streaming_json;
use crate::utils::sanitize_unicode::sanitize_surrogates;

/// Error of the shared stream; the caller turns it into an `error` event.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ResponsesStreamError(pub String);

// ---------------------------------------------------------------------------
// Utilities
// ---------------------------------------------------------------------------

/// `encodeTextSignatureV1(id, phase?)`
fn encode_text_signature_v1(id: &str, phase: Option<&Value>) -> String {
    let mut payload = Map::new();
    payload.insert("v".to_string(), json!(1));
    payload.insert("id".to_string(), json!(id));
    if let Some(phase) = phase.filter(|phase| !phase.is_null()) {
        payload.insert("phase".to_string(), phase.clone());
    }
    Value::Object(payload).to_string()
}

/// `parseTextSignature(signature)` — the legacy plain-id form still has to parse.
fn parse_text_signature(signature: Option<&str>) -> Option<TextSignatureV1> {
    let signature = signature?;
    if signature.is_empty() {
        return None;
    }
    if signature.starts_with('{')
        && let Ok(parsed) = serde_json::from_str::<Value>(signature)
        && parsed.get("v").and_then(Value::as_u64) == Some(1)
        && let Some(id) = parsed.get("id").and_then(Value::as_str)
    {
        let phase = match parsed.get("phase").and_then(Value::as_str) {
            Some("commentary") => Some(crate::types::TextSignaturePhase::Commentary),
            Some("final_answer") => Some(crate::types::TextSignaturePhase::FinalAnswer),
            _ => None,
        };
        return Some(TextSignatureV1 {
            v: 1,
            id: id.to_string(),
            phase,
        });
    }
    Some(TextSignatureV1 {
        v: 1,
        id: signature.to_string(),
        phase: None,
    })
}

/// `convertToolResultOutput(model, content)` — a string, or text plus image parts.
fn convert_tool_result_output(model: &Model, content: &[TextOrImageContent]) -> Value {
    let text_result = content
        .iter()
        .filter_map(|block| match block {
            TextOrImageContent::Text(text) => Some(text.text.as_str()),
            TextOrImageContent::Image(_) => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    let images: Vec<_> = content
        .iter()
        .filter_map(|block| match block {
            TextOrImageContent::Image(image) => Some(image),
            TextOrImageContent::Text(_) => None,
        })
        .collect();
    let has_text = !text_result.is_empty();

    if images.is_empty() || !model.input.contains(&Modality::Image) {
        let text = if has_text {
            text_result.as_str()
        } else if !images.is_empty() {
            "(see attached image)"
        } else {
            "(no tool output)"
        };
        return json!(sanitize_surrogates(text));
    }

    let mut output = Vec::new();
    if has_text {
        output.push(json!({ "type": "input_text", "text": sanitize_surrogates(&text_result) }));
    }
    for image in images {
        output.push(json!({
            "type": "input_image",
            "detail": "auto",
            "image_url": format!("data:{};base64,{}", image.mime_type, image.data),
        }));
    }
    Value::Array(output)
}

// ---------------------------------------------------------------------------
// Options
// ---------------------------------------------------------------------------

/// `ConvertResponsesToolsOptions`
#[derive(Debug, Clone, Default)]
pub struct ConvertResponsesToolsOptions {
    /// `strict?: boolean | null` — `None` is TS `undefined`, which defaults to `false`.
    pub strict: Option<Option<bool>>,
    pub supports_strict_mode: Option<bool>,
    pub supports_openai_grammar_tools: Option<bool>,
    pub defer_loading: bool,
}

/// `deferredToolsMode?: "additional-tools" | "tool-search"`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponsesDeferredToolsMode {
    AdditionalTools,
    ToolSearch,
}

/// `ConvertResponsesMessagesOptions`
#[derive(Debug, Clone, Default)]
pub struct ConvertResponsesMessagesOptions<'a> {
    /// Default `true`.
    pub include_system_prompt: Option<bool>,
    pub grammar_tool_input_properties: Option<&'a BTreeMap<String, String>>,
    pub deferred_tools: Option<&'a BTreeMap<String, Tool>>,
    pub deferred_tools_mode: Option<ResponsesDeferredToolsMode>,
    pub tool_options: Option<ConvertResponsesToolsOptions>,
}

// ---------------------------------------------------------------------------
// Message conversion
// ---------------------------------------------------------------------------

fn normalize_id_part(part: &str) -> String {
    let sanitized: String = part
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' || character == '-' {
                character
            } else {
                '_'
            }
        })
        .collect();
    let normalized: String = if sanitized.chars().count() > 64 {
        sanitized.chars().take(64).collect()
    } else {
        sanitized
    };
    normalized.trim_end_matches('_').to_string()
}

fn build_foreign_responses_item_id(item_id: &str) -> String {
    let normalized = format!("fc_{}", short_hash(item_id));
    if normalized.chars().count() > 64 {
        normalized.chars().take(64).collect()
    } else {
        normalized
    }
}

/// `convertResponsesMessages(model, context, allowedToolCallProviders, options)`
pub fn convert_responses_messages(
    model: &Model,
    context: &Context,
    allowed_tool_call_providers: &BTreeSet<String>,
    options: &ConvertResponsesMessagesOptions<'_>,
    timestamp: i64,
) -> Result<Vec<Value>, ConstrainedSamplingError> {
    let mut messages: Vec<Value> = Vec::new();
    let mut loaded_tool_names: BTreeSet<String> = BTreeSet::new();

    let allows_pipe_ids = allowed_tool_call_providers.contains(&model.provider);
    let normalize_tool_call_id = |id: &str, source: &AssistantMessage| -> String {
        if !allows_pipe_ids {
            return normalize_id_part(id);
        }
        let Some(separator) = id.find('|') else {
            return normalize_id_part(id);
        };
        let call_id = normalize_id_part(&id[..separator]);
        let raw_item_id = &id[separator + 1..];
        let is_foreign_tool_call = source.provider != model.provider || source.api != model.api;
        let mut item_id = if is_foreign_tool_call {
            build_foreign_responses_item_id(raw_item_id)
        } else {
            normalize_id_part(raw_item_id)
        };
        // The Responses API requires item ids to start with "fc".
        if !item_id.starts_with("fc_") {
            item_id = normalize_id_part(&format!("fc_{item_id}"));
        }
        format!("{call_id}|{item_id}")
    };

    let transformed_messages = transform_messages(
        &context.messages,
        model,
        Some(&|id: &str, source: &AssistantMessage| normalize_tool_call_id(id, source)),
        timestamp,
    );

    let include_system_prompt = options.include_system_prompt.unwrap_or(true);
    if include_system_prompt
        && let Some(system_prompt) = context
            .system_prompt
            .as_deref()
            .filter(|prompt| !prompt.is_empty())
    {
        let supports_developer_role = responses_supports_developer_role(model);
        let role = if model.reasoning && supports_developer_role {
            "developer"
        } else {
            "system"
        };
        messages.push(json!({ "role": role, "content": sanitize_surrogates(system_prompt) }));
    }

    let mut message_index = 0usize;
    for message in &transformed_messages {
        match message {
            Message::User(user) => match &user.content {
                UserContent::Text(text) => {
                    messages.push(json!({
                        "role": "user",
                        "content": [{ "type": "input_text", "text": sanitize_surrogates(text) }],
                    }));
                }
                UserContent::Blocks(blocks) => {
                    let content: Vec<Value> = blocks
                        .iter()
                        .map(|block| match block {
                            TextOrImageContent::Text(text) => {
                                json!({ "type": "input_text", "text": sanitize_surrogates(&text.text) })
                            }
                            TextOrImageContent::Image(image) => json!({
                                "type": "input_image",
                                "detail": "auto",
                                "image_url": format!(
                                    "data:{};base64,{}",
                                    image.mime_type, image.data
                                ),
                            }),
                        })
                        .collect();
                    if content.is_empty() {
                        continue;
                    }
                    messages.push(json!({ "role": "user", "content": content }));
                }
            },
            Message::Assistant(assistant) => {
                let mut output: Vec<Value> = Vec::new();
                let is_same_provider_and_api =
                    assistant.provider == model.provider && assistant.api == model.api;
                let is_same_model = is_same_provider_and_api && assistant.model == model.id;
                let is_different_model = is_same_provider_and_api && assistant.model != model.id;
                let mut text_block_index = 0usize;

                for block in &assistant.content {
                    match block {
                        AssistantContent::Thinking(thinking) => {
                            // The signature holds the whole reasoning item, replayed verbatim.
                            if let Some(signature) = thinking
                                .thinking_signature
                                .as_deref()
                                .filter(|signature| !signature.is_empty())
                            {
                                let item: Value = serde_json::from_str(signature)
                                    .map_err(|error| ConstrainedSamplingError(error.to_string()))?;
                                output.push(item);
                            }
                        }
                        AssistantContent::Text(text) => {
                            let parsed_signature =
                                parse_text_signature(text.text_signature.as_deref());
                            let fallback_message_id = if text_block_index == 0 {
                                format!("msg_pi_{message_index}")
                            } else {
                                format!("msg_pi_{message_index}_{text_block_index}")
                            };
                            text_block_index += 1;
                            // OpenAI caps ids at 64 characters.
                            let message_id = match parsed_signature.as_ref() {
                                None => fallback_message_id,
                                Some(signature) if signature.id.is_empty() => fallback_message_id,
                                Some(signature) if signature.id.chars().count() > 64 => {
                                    format!("msg_{}", short_hash(&signature.id))
                                }
                                Some(signature) => signature.id.clone(),
                            };
                            let mut item = Map::new();
                            item.insert("type".to_string(), json!("message"));
                            item.insert("role".to_string(), json!("assistant"));
                            item.insert(
                                "content".to_string(),
                                json!([{
                                    "type": "output_text",
                                    "text": sanitize_surrogates(&text.text),
                                    "annotations": [],
                                }]),
                            );
                            item.insert("status".to_string(), json!("completed"));
                            item.insert("id".to_string(), json!(message_id));
                            // `phase: undefined` is dropped by JSON.stringify.
                            if let Some(phase) = parsed_signature
                                .as_ref()
                                .and_then(|signature| signature.phase)
                            {
                                item.insert(
                                    "phase".to_string(),
                                    serde_json::to_value(phase).unwrap_or(Value::Null),
                                );
                            }
                            output.push(Value::Object(item));
                        }
                        AssistantContent::ToolCall(tool_call) => {
                            let (call_id, raw_item_id) = match tool_call.id.split_once('|') {
                                Some((call_id, item_id)) => (call_id, Some(item_id)),
                                None => (tool_call.id.as_str(), None),
                            };
                            let custom_input_property = options
                                .grammar_tool_input_properties
                                .and_then(|properties| properties.get(&tool_call.name));
                            let mut item_id = raw_item_id;

                            // A different model of the same provider: dropping the id avoids
                            // OpenAI's fc_/rs_ pairing validation. And a custom-tool id
                            // (ctc_*) cannot be replayed on a function_call item.
                            if (is_different_model
                                && item_id.is_some_and(|id| id.starts_with("fc_")))
                                || (custom_input_property.is_none()
                                    && !item_id.is_some_and(|id| id.starts_with("fc_")))
                            {
                                item_id = None;
                            }

                            let can_replay_namespace = is_same_model
                                || options
                                    .deferred_tools
                                    .is_some_and(|tools| tools.contains_key(&tool_call.name));

                            let mut item = Map::new();
                            item.insert(
                                "type".to_string(),
                                json!(if custom_input_property.is_some() {
                                    "custom_tool_call"
                                } else {
                                    "function_call"
                                }),
                            );
                            // `id: undefined` is dropped by JSON.stringify, so the key is
                            // only written when there is an id to write.
                            if let Some(item_id) = item_id {
                                item.insert("id".to_string(), json!(item_id));
                            }
                            item.insert("call_id".to_string(), json!(call_id));
                            item.insert("name".to_string(), json!(tool_call.name));
                            match custom_input_property {
                                Some(property) => {
                                    item.insert(
                                        "input".to_string(),
                                        json!(sanitize_surrogates(&get_grammar_tool_input(
                                            &tool_call.name,
                                            &tool_call.arguments,
                                            property,
                                        )?)),
                                    );
                                }
                                None => {
                                    item.insert(
                                        "arguments".to_string(),
                                        json!(
                                            serde_json::to_string(&tool_call.arguments)
                                                .unwrap_or_else(|_| "{}".to_string())
                                        ),
                                    );
                                }
                            }
                            if can_replay_namespace && let Some(namespace) = &tool_call.namespace {
                                item.insert("namespace".to_string(), json!(namespace));
                            }
                            output.push(Value::Object(item));
                        }
                    }
                }
                if output.is_empty() {
                    continue;
                }
                messages.extend(output);
            }
            Message::ToolResult(tool_result) => {
                let call_id = tool_result
                    .tool_call_id
                    .split_once('|')
                    .map(|(call_id, _)| call_id)
                    .unwrap_or(tool_result.tool_call_id.as_str());
                let output = convert_tool_result_output(model, &tool_result.content);

                let is_custom = options
                    .grammar_tool_input_properties
                    .is_some_and(|properties| properties.contains_key(&tool_result.tool_name));
                messages.push(json!({
                    "type": if is_custom { "custom_tool_call_output" } else { "function_call_output" },
                    "call_id": call_id,
                    "output": output,
                }));

                let mut deferred_tools: Vec<Tool> = Vec::new();
                for name in tool_result.added_tool_names.iter().flatten() {
                    let Some(tool) = options.deferred_tools.and_then(|tools| tools.get(name))
                    else {
                        continue;
                    };
                    if loaded_tool_names.contains(name) {
                        continue;
                    }
                    loaded_tool_names.insert(name.clone());
                    deferred_tools.push(tool.clone());
                }
                if !deferred_tools.is_empty() {
                    let tool_options = options.tool_options.clone().unwrap_or_default();
                    match options.deferred_tools_mode {
                        Some(ResponsesDeferredToolsMode::AdditionalTools) => {
                            messages.push(json!({
                                "type": "additional_tools",
                                "role": "developer",
                                "tools": convert_responses_tools(&deferred_tools, &tool_options)?,
                            }));
                        }
                        Some(ResponsesDeferredToolsMode::ToolSearch) => {
                            let names: Vec<&str> = deferred_tools
                                .iter()
                                .map(|tool| tool.name.as_str())
                                .collect();
                            let search_call_id = format!(
                                "pi_tool_load_{}",
                                short_hash(&format!(
                                    "{}:{}",
                                    tool_result.tool_call_id,
                                    names.join(",")
                                ))
                            );
                            messages.push(json!({
                                "type": "tool_search_call",
                                "call_id": search_call_id,
                                "execution": "client",
                                "status": "completed",
                                "arguments": { "query": names.join(" "), "limit": names.len() },
                            }));
                            let deferred_options = ConvertResponsesToolsOptions {
                                defer_loading: true,
                                ..tool_options
                            };
                            messages.push(json!({
                                "type": "tool_search_output",
                                "call_id": search_call_id,
                                "execution": "client",
                                "status": "completed",
                                "tools": convert_responses_tools(&deferred_tools, &deferred_options)?,
                            }));
                        }
                        None => {}
                    }
                }
            }
        }
        message_index += 1;
    }

    Ok(messages)
}

/// `model.compat?.supportsDeveloperRole !== false`
fn responses_supports_developer_role(model: &Model) -> bool {
    match &model.compat {
        Some(crate::types::ModelCompat::OpenAIResponses(compat)) => {
            compat.supports_developer_role != Some(false)
        }
        Some(crate::types::ModelCompat::OpenAICompletions(compat)) => {
            compat.supports_developer_role != Some(false)
        }
        _ => true,
    }
}

// ---------------------------------------------------------------------------
// Tool conversion
// ---------------------------------------------------------------------------

/// `convertResponsesTools(tools, options)`
pub fn convert_responses_tools(
    tools: &[Tool],
    options: &ConvertResponsesToolsOptions,
) -> Result<Vec<Value>, ConstrainedSamplingError> {
    let default_strict = options.strict.unwrap_or(Some(false));
    let supports_strict_mode = options.supports_strict_mode.unwrap_or(true);
    let supports_openai_grammar_tools = options.supports_openai_grammar_tools.unwrap_or(false);

    let mut converted = Vec::with_capacity(tools.len());
    for tool in tools {
        if let Some(grammar) =
            resolve_grammar_constrained_sampling(tool, supports_openai_grammar_tools)?
        {
            let syntax = match grammar.format {
                GrammarSyntax::Lark => "lark",
                GrammarSyntax::Regex => "regex",
            };
            let mut item = Map::new();
            item.insert("type".to_string(), json!("custom"));
            item.insert("name".to_string(), json!(tool.name));
            item.insert("description".to_string(), json!(tool.description));
            item.insert(
                "format".to_string(),
                json!({ "type": "grammar", "syntax": syntax, "definition": grammar.definition }),
            );
            if options.defer_loading {
                item.insert("defer_loading".to_string(), json!(true));
            }
            converted.push(Value::Object(item));
            continue;
        }

        let constrained_strict = resolve_json_schema_strict_sampling(tool, supports_strict_mode)?;
        // `constrainedStrict ?? defaultStrict` — `undefined` falls through, `null` does not
        // reach this branch because `resolveJsonSchemaStrictSampling` never returns it.
        let strict = match constrained_strict {
            Some(value) => Some(value),
            None => default_strict,
        };
        let mut item = Map::new();
        item.insert("type".to_string(), json!("function"));
        item.insert("name".to_string(), json!(tool.name));
        item.insert("description".to_string(), json!(tool.description));
        item.insert(
            "parameters".to_string(),
            get_json_schema_tool_parameters(tool, Some(strict == Some(true))),
        );
        if options.defer_loading {
            item.insert("defer_loading".to_string(), json!(true));
        }
        if supports_strict_mode {
            item.insert(
                "strict".to_string(),
                strict.map(|strict| json!(strict)).unwrap_or(Value::Null),
            );
        }
        converted.push(Value::Object(item));
    }
    Ok(converted)
}

// ---------------------------------------------------------------------------
// Stream processing
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct CustomInput {
    property: String,
    json_buffer: GrammarToolInputJsonBuffer,
}

#[derive(Debug, Clone)]
enum SlotKind {
    Thinking,
    Text,
    ToolCall {
        partial_json: Option<String>,
        custom_input: Option<CustomInput>,
    },
}

#[derive(Debug, Clone)]
struct OutputSlot {
    kind: SlotKind,
    content_index: usize,
}

/// `OpenAIResponsesStreamOptions`
#[derive(Default)]
pub struct ResponsesStreamOptions<'a> {
    pub service_tier: Option<String>,
    pub grammar_tool_input_properties: Option<&'a BTreeMap<String, String>>,
    /// `resolveServiceTier(responseTier, requestTier)`
    #[allow(clippy::type_complexity)]
    pub resolve_service_tier:
        Option<Box<dyn Fn(Option<&str>, Option<&str>) -> Option<String> + Send + Sync + 'a>>,
    /// `applyServiceTierPricing(usage, serviceTier)`
    #[allow(clippy::type_complexity)]
    pub apply_service_tier_pricing:
        Option<Box<dyn Fn(&mut Usage, Option<&str>) + Send + Sync + 'a>>,
}

/// `processResponsesStream(...)` as a state machine driven event by event.
pub struct ResponsesStreamState {
    pub output: AssistantMessage,
    model: Model,
    slots: BTreeMap<i64, OutputSlot>,
    /// Reasoning blocks by item id, for the Azure signature backfill.
    reasoning_blocks_by_id: BTreeMap<String, usize>,
    saw_terminal_response_event: bool,
}

impl ResponsesStreamState {
    pub fn new(output: AssistantMessage, model: &Model) -> Self {
        ResponsesStreamState {
            output,
            model: model.clone(),
            slots: BTreeMap::new(),
            reasoning_blocks_by_id: BTreeMap::new(),
            saw_terminal_response_event: false,
        }
    }

    pub fn saw_terminal_response_event(&self) -> bool {
        self.saw_terminal_response_event
    }

    fn slot(&self, output_index: i64, kind: &str) -> Option<&OutputSlot> {
        let slot = self.slots.get(&output_index)?;
        let matches = matches!(
            (&slot.kind, kind),
            (SlotKind::Thinking, "thinking")
                | (SlotKind::Text, "text")
                | (SlotKind::ToolCall { .. }, "toolCall")
        );
        matches.then_some(slot)
    }

    fn text_block_mut(&mut self, index: usize) -> &mut TextContent {
        match &mut self.output.content[index] {
            AssistantContent::Text(block) => block,
            _ => unreachable!("slot points at a text block"),
        }
    }

    fn thinking_block_mut(&mut self, index: usize) -> &mut ThinkingContent {
        match &mut self.output.content[index] {
            AssistantContent::Thinking(block) => block,
            _ => unreachable!("slot points at a thinking block"),
        }
    }

    fn tool_call_mut(&mut self, index: usize) -> &mut ToolCall {
        match &mut self.output.content[index] {
            AssistantContent::ToolCall(block) => block,
            _ => unreachable!("slot points at a tool call"),
        }
    }

    /// `applyMessagePhaseStopReason(item)`
    fn apply_message_phase_stop_reason(&mut self, item: &Value) {
        if item.get("type").and_then(Value::as_str) == Some("message")
            && item.get("phase").and_then(Value::as_str) == Some("final_answer")
        {
            self.output.stop_reason = StopReason::Stop;
        }
    }

    /// `getCustomToolCallInput(block)`
    fn custom_tool_call_input(&self, content_index: usize, property: &str) -> String {
        match &self.output.content[content_index] {
            AssistantContent::ToolCall(block) => block
                .arguments
                .get(property)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            _ => String::new(),
        }
    }

    /// `appendCustomToolCallInput(block, nextInput, close)`
    fn append_custom_tool_call_input(
        &mut self,
        output_index: i64,
        next_input: &str,
        close: bool,
    ) -> Result<Option<String>, ResponsesStreamError> {
        let Some(slot) = self.slots.get_mut(&output_index) else {
            return Ok(None);
        };
        let content_index = slot.content_index;
        let SlotKind::ToolCall {
            custom_input: Some(custom_input),
            ..
        } = &mut slot.kind
        else {
            return Ok(None);
        };
        let property = custom_input.property.clone();
        let delta = append_grammar_tool_input_json_delta(
            &mut custom_input.json_buffer,
            &property,
            next_input,
            close,
        )
        .map_err(|error| ResponsesStreamError(error.to_string()))?;
        let mut arguments = Map::new();
        arguments.insert(property, json!(next_input));
        self.tool_call_mut(content_index).arguments = arguments;
        Ok(delta)
    }

    /// `createSlot(outputIndex, item)`
    fn create_slot(
        &mut self,
        output_index: i64,
        item: &Value,
        grammar_tool_input_properties: Option<&BTreeMap<String, String>>,
        emitted: &mut Vec<AssistantMessageEvent>,
    ) -> Option<usize> {
        let item_type = item.get("type").and_then(Value::as_str)?;
        match item_type {
            "reasoning" => {
                self.output
                    .content
                    .push(AssistantContent::Thinking(ThinkingContent::default()));
                let content_index = self.output.content.len() - 1;
                self.slots.insert(
                    output_index,
                    OutputSlot {
                        kind: SlotKind::Thinking,
                        content_index,
                    },
                );
                emitted.push(AssistantMessageEvent::ThinkingStart {
                    content_index,
                    partial: self.output.clone(),
                });
                Some(content_index)
            }
            "message" => {
                self.apply_message_phase_stop_reason(item);
                self.output
                    .content
                    .push(AssistantContent::Text(TextContent::default()));
                let content_index = self.output.content.len() - 1;
                self.slots.insert(
                    output_index,
                    OutputSlot {
                        kind: SlotKind::Text,
                        content_index,
                    },
                );
                emitted.push(AssistantMessageEvent::TextStart {
                    content_index,
                    partial: self.output.clone(),
                });
                Some(content_index)
            }
            "function_call" => {
                let mut block = ToolCall {
                    id: format!(
                        "{}|{}",
                        item.get("call_id").and_then(Value::as_str).unwrap_or(""),
                        item.get("id").and_then(Value::as_str).unwrap_or("")
                    ),
                    name: item
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    ..ToolCall::default()
                };
                if let Some(namespace) = item.get("namespace").filter(|value| !value.is_null()) {
                    block.namespace = namespace.as_str().map(str::to_string);
                }
                self.output.content.push(AssistantContent::ToolCall(block));
                let content_index = self.output.content.len() - 1;
                self.slots.insert(
                    output_index,
                    OutputSlot {
                        kind: SlotKind::ToolCall {
                            partial_json: Some(
                                item.get("arguments")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_string(),
                            ),
                            custom_input: None,
                        },
                        content_index,
                    },
                );
                emitted.push(AssistantMessageEvent::ToolcallStart {
                    content_index,
                    partial: self.output.clone(),
                });
                Some(content_index)
            }
            "custom_tool_call" => {
                let name = item
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let input_property = grammar_tool_input_properties
                    .and_then(|properties| properties.get(&name).cloned())
                    .unwrap_or_else(|| "input".to_string());
                let input = item
                    .get("input")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let mut arguments = Map::new();
                arguments.insert(input_property.clone(), json!(input));
                let mut block = ToolCall {
                    id: format!(
                        "{}|{}",
                        item.get("call_id").and_then(Value::as_str).unwrap_or(""),
                        item.get("id").and_then(Value::as_str).unwrap_or("")
                    ),
                    name,
                    arguments,
                    ..ToolCall::default()
                };
                if let Some(namespace) = item.get("namespace").filter(|value| !value.is_null()) {
                    block.namespace = namespace.as_str().map(str::to_string);
                }
                self.output.content.push(AssistantContent::ToolCall(block));
                let content_index = self.output.content.len() - 1;
                self.slots.insert(
                    output_index,
                    OutputSlot {
                        kind: SlotKind::ToolCall {
                            partial_json: None,
                            custom_input: Some(CustomInput {
                                property: input_property,
                                json_buffer: GrammarToolInputJsonBuffer::default(),
                            }),
                        },
                        content_index,
                    },
                );
                emitted.push(AssistantMessageEvent::ToolcallStart {
                    content_index,
                    partial: self.output.clone(),
                });
                Some(content_index)
            }
            _ => None,
        }
    }

    /// Azure can leave `encrypted_content` out of `response.output_item.done` and only
    /// send it in `response.completed`. Backfilling keeps `store: false` replay stateless
    /// (notagentdev/notagent#6409).
    fn backfill_reasoning_signatures(&mut self, response_output: &[Value]) {
        for item in response_output {
            if item.get("type").and_then(Value::as_str) != Some("reasoning") {
                continue;
            }
            let Some(encrypted) = item
                .get("encrypted_content")
                .filter(|value| !value.is_null() && value.as_str() != Some(""))
            else {
                continue;
            };
            let Some(id) = item.get("id").and_then(Value::as_str) else {
                continue;
            };
            let Some(content_index) = self.reasoning_blocks_by_id.get(id).copied() else {
                continue;
            };
            let Some(signature) = self
                .thinking_block_mut(content_index)
                .thinking_signature
                .clone()
                .filter(|signature| !signature.is_empty())
            else {
                continue;
            };
            let Ok(mut stored) = serde_json::from_str::<Value>(&signature) else {
                continue;
            };
            if stored
                .get("encrypted_content")
                .is_some_and(|value| !value.is_null() && value.as_str() != Some(""))
            {
                continue;
            }
            if let Some(object) = stored.as_object_mut() {
                object.insert("encrypted_content".to_string(), encrypted.clone());
            }
            self.thinking_block_mut(content_index).thinking_signature = Some(stored.to_string());
        }
    }

    /// `finalizeResponse(response)`
    fn finalize_response(&mut self, response: &Value, options: &ResponsesStreamOptions<'_>) {
        self.saw_terminal_response_event = true;
        if let Some(output) = response.get("output").and_then(Value::as_array) {
            self.backfill_reasoning_signatures(&output.clone());
        }
        if let Some(id) = response
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
        {
            self.output.response_id = Some(id.to_string());
        }
        if let Some(usage) = response.get("usage").filter(|usage| !usage.is_null()) {
            let number = |value: Option<&Value>| value.and_then(Value::as_f64).unwrap_or(0.0);
            let input_details = usage.get("input_tokens_details");
            let cached = number(input_details.and_then(|details| details.get("cached_tokens")));
            let cache_write =
                number(input_details.and_then(|details| details.get("cache_write_tokens")));
            self.output.usage = Usage {
                // OpenAI counts cached and cache-write tokens inside input_tokens.
                input: (number(usage.get("input_tokens")) - cached - cache_write).max(0.0) as u64,
                output: number(usage.get("output_tokens")) as u64,
                cache_read: cached as u64,
                cache_write: cache_write as u64,
                reasoning: Some(number(
                    usage
                        .get("output_tokens_details")
                        .and_then(|details| details.get("reasoning_tokens")),
                ) as u64),
                total_tokens: Some(number(usage.get("total_tokens")) as u64),
                ..Usage::default()
            };
        }
        calculate_cost(&self.model, &mut self.output.usage);
        if let Some(apply) = &options.apply_service_tier_pricing {
            let response_tier = response.get("service_tier").and_then(Value::as_str);
            let service_tier = match &options.resolve_service_tier {
                Some(resolve) => resolve(response_tier, options.service_tier.as_deref()),
                None => response_tier
                    .map(str::to_string)
                    .or_else(|| options.service_tier.clone()),
            };
            apply(&mut self.output.usage, service_tier.as_deref());
        }

        // For an incomplete response the provider's own reason is kept, so max-output
        // truncation and content filtering stay distinguishable.
        let status = response.get("status").and_then(Value::as_str);
        let incomplete_reason = response
            .get("incomplete_details")
            .filter(|details| !details.is_null())
            .and_then(|details| details.get("reason"))
            .and_then(Value::as_str);
        self.output.raw_stop_reason = match (status, incomplete_reason) {
            (Some(status), Some(reason)) => Some(format!("{status}.{reason}")),
            (Some(status), None) => Some(status.to_string()),
            (None, Some(reason)) => Some(format!("undefined.{reason}")),
            (None, None) => None,
        };
        let (stop_reason, error_message) = map_stop_reason(status, incomplete_reason);
        self.output.stop_reason = stop_reason;
        self.output.error_message = error_message;
        if self
            .output
            .content
            .iter()
            .any(|block| matches!(block, AssistantContent::ToolCall(_)))
            && self.output.stop_reason == StopReason::Stop
        {
            self.output.stop_reason = StopReason::ToolUse;
        }
    }

    /// One decoded `ResponseStreamEvent`.
    pub fn process_event(
        &mut self,
        event: &Value,
        options: &ResponsesStreamOptions<'_>,
    ) -> Result<Vec<AssistantMessageEvent>, ResponsesStreamError> {
        let mut emitted = Vec::new();
        let Some(event_type) = event.get("type").and_then(Value::as_str) else {
            return Ok(emitted);
        };
        let output_index = event.get("output_index").and_then(Value::as_i64);
        let delta = event.get("delta").and_then(Value::as_str);

        match event_type {
            "response.created" => {
                if let Some(id) = event
                    .get("response")
                    .and_then(|response| response.get("id"))
                    .and_then(Value::as_str)
                {
                    self.output.response_id = Some(id.to_string());
                }
            }
            "response.output_item.added" => {
                if let (Some(output_index), Some(item)) = (output_index, event.get("item")) {
                    self.create_slot(
                        output_index,
                        item,
                        options.grammar_tool_input_properties,
                        &mut emitted,
                    );
                }
            }
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                let (Some(output_index), Some(delta)) = (output_index, delta) else {
                    return Ok(emitted);
                };
                let Some(content_index) = self
                    .slot(output_index, "thinking")
                    .map(|slot| slot.content_index)
                else {
                    return Ok(emitted);
                };
                self.thinking_block_mut(content_index)
                    .thinking
                    .push_str(delta);
                emitted.push(AssistantMessageEvent::ThinkingDelta {
                    content_index,
                    delta: delta.to_string(),
                    partial: self.output.clone(),
                });
            }
            "response.reasoning_summary_part.done" => {
                let Some(output_index) = output_index else {
                    return Ok(emitted);
                };
                let Some(content_index) = self
                    .slot(output_index, "thinking")
                    .map(|slot| slot.content_index)
                else {
                    return Ok(emitted);
                };
                self.thinking_block_mut(content_index)
                    .thinking
                    .push_str("\n\n");
                emitted.push(AssistantMessageEvent::ThinkingDelta {
                    content_index,
                    delta: "\n\n".to_string(),
                    partial: self.output.clone(),
                });
            }
            "response.output_text.delta" | "response.refusal.delta" => {
                let (Some(output_index), Some(delta)) = (output_index, delta) else {
                    return Ok(emitted);
                };
                let Some(content_index) = self
                    .slot(output_index, "text")
                    .map(|slot| slot.content_index)
                else {
                    return Ok(emitted);
                };
                self.text_block_mut(content_index).text.push_str(delta);
                emitted.push(AssistantMessageEvent::TextDelta {
                    content_index,
                    delta: delta.to_string(),
                    partial: self.output.clone(),
                });
            }
            "response.function_call_arguments.delta" => {
                let (Some(output_index), Some(delta)) = (output_index, delta) else {
                    return Ok(emitted);
                };
                let Some(slot) = self.slots.get_mut(&output_index) else {
                    return Ok(emitted);
                };
                let content_index = slot.content_index;
                let SlotKind::ToolCall {
                    partial_json: Some(partial_json),
                    ..
                } = &mut slot.kind
                else {
                    return Ok(emitted);
                };
                partial_json.push_str(delta);
                let parsed = parse_streaming_json(Some(&partial_json.clone()));
                self.tool_call_mut(content_index).arguments =
                    parsed.as_object().cloned().unwrap_or_default();
                emitted.push(AssistantMessageEvent::ToolcallDelta {
                    content_index,
                    delta: delta.to_string(),
                    partial: self.output.clone(),
                });
            }
            "response.function_call_arguments.done" => {
                let Some(output_index) = output_index else {
                    return Ok(emitted);
                };
                let arguments = event
                    .get("arguments")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let Some(slot) = self.slots.get_mut(&output_index) else {
                    return Ok(emitted);
                };
                let content_index = slot.content_index;
                let SlotKind::ToolCall {
                    partial_json: Some(partial_json),
                    ..
                } = &mut slot.kind
                else {
                    return Ok(emitted);
                };
                let previous = partial_json.clone();
                *partial_json = arguments.clone();
                let parsed = parse_streaming_json(Some(&arguments));
                self.tool_call_mut(content_index).arguments =
                    parsed.as_object().cloned().unwrap_or_default();

                if let Some(delta) = arguments.strip_prefix(previous.as_str())
                    && !delta.is_empty()
                {
                    emitted.push(AssistantMessageEvent::ToolcallDelta {
                        content_index,
                        delta: delta.to_string(),
                        partial: self.output.clone(),
                    });
                }
            }
            "response.custom_tool_call_input.delta" => {
                let (Some(output_index), Some(delta)) = (output_index, delta) else {
                    return Ok(emitted);
                };
                let Some((content_index, property)) = self.custom_slot(output_index) else {
                    return Ok(emitted);
                };
                let next_input = format!(
                    "{}{delta}",
                    self.custom_tool_call_input(content_index, &property)
                );
                if let Some(delta) =
                    self.append_custom_tool_call_input(output_index, &next_input, false)?
                {
                    emitted.push(AssistantMessageEvent::ToolcallDelta {
                        content_index,
                        delta,
                        partial: self.output.clone(),
                    });
                }
            }
            "response.custom_tool_call_input.done" => {
                let Some(output_index) = output_index else {
                    return Ok(emitted);
                };
                let input = event
                    .get("input")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let Some((content_index, _)) = self.custom_slot(output_index) else {
                    return Ok(emitted);
                };
                if let Some(delta) =
                    self.append_custom_tool_call_input(output_index, &input, true)?
                {
                    emitted.push(AssistantMessageEvent::ToolcallDelta {
                        content_index,
                        delta,
                        partial: self.output.clone(),
                    });
                }
            }
            "response.output_item.done" => {
                let (Some(output_index), Some(item)) = (output_index, event.get("item")) else {
                    return Ok(emitted);
                };
                self.apply_message_phase_stop_reason(item);
                if !self.slots.contains_key(&output_index) {
                    self.create_slot(
                        output_index,
                        item,
                        options.grammar_tool_input_properties,
                        &mut emitted,
                    );
                }
                self.finish_item(output_index, item, &mut emitted)?;
            }
            "response.completed" | "response.incomplete" => {
                if let Some(response) = event.get("response") {
                    self.finalize_response(response, options);
                }
            }
            "error" => {
                let code = event
                    .get("code")
                    .and_then(Value::as_str)
                    .unwrap_or("undefined");
                let message = event
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("undefined");
                return Err(ResponsesStreamError(format!(
                    "Error Code {code}: {message}"
                )));
            }
            "response.failed" => {
                self.saw_terminal_response_event = true;
                let response = event.get("response");
                self.output.raw_stop_reason = response
                    .and_then(|response| response.get("status"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let error = response
                    .and_then(|response| response.get("error"))
                    .filter(|error| !error.is_null());
                let reason = response
                    .and_then(|response| response.get("incomplete_details"))
                    .filter(|details| !details.is_null())
                    .and_then(|details| details.get("reason"))
                    .and_then(Value::as_str);
                let message = match (error, reason) {
                    (Some(error), _) => format!(
                        "{}: {}",
                        error
                            .get("code")
                            .and_then(Value::as_str)
                            .filter(|code| !code.is_empty())
                            .unwrap_or("unknown"),
                        error
                            .get("message")
                            .and_then(Value::as_str)
                            .filter(|message| !message.is_empty())
                            .unwrap_or("no message")
                    ),
                    (None, Some(reason)) => format!("incomplete: {reason}"),
                    (None, None) => "Unknown error (no error details in response)".to_string(),
                };
                return Err(ResponsesStreamError(message));
            }
            _ => {}
        }
        Ok(emitted)
    }

    fn custom_slot(&self, output_index: i64) -> Option<(usize, String)> {
        let slot = self.slots.get(&output_index)?;
        match &slot.kind {
            SlotKind::ToolCall {
                custom_input: Some(custom_input),
                ..
            } => Some((slot.content_index, custom_input.property.clone())),
            _ => None,
        }
    }

    /// The `response.output_item.done` branches.
    fn finish_item(
        &mut self,
        output_index: i64,
        item: &Value,
        emitted: &mut Vec<AssistantMessageEvent>,
    ) -> Result<(), ResponsesStreamError> {
        let Some(slot) = self.slots.get(&output_index).cloned() else {
            return Ok(());
        };
        let content_index = slot.content_index;
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or_default();

        match (&slot.kind, item_type) {
            (SlotKind::Thinking, "reasoning") => {
                let summary_text = join_text_parts(item.get("summary"));
                let content_text = join_text_parts(item.get("content"));
                let thinking = if !summary_text.is_empty() {
                    summary_text
                } else if !content_text.is_empty() {
                    content_text
                } else {
                    self.thinking_block_mut(content_index).thinking.clone()
                };
                let block = self.thinking_block_mut(content_index);
                block.thinking = thinking.clone();
                block.thinking_signature = Some(item.to_string());
                if let Some(id) = item.get("id").and_then(Value::as_str) {
                    self.reasoning_blocks_by_id
                        .insert(id.to_string(), content_index);
                }
                emitted.push(AssistantMessageEvent::ThinkingEnd {
                    content_index,
                    content: thinking,
                    partial: self.output.clone(),
                });
                self.slots.remove(&output_index);
            }
            (SlotKind::Text, "message") => {
                let text = item
                    .get("content")
                    .and_then(Value::as_array)
                    .map(|parts| {
                        parts
                            .iter()
                            .map(|part| {
                                if part.get("type").and_then(Value::as_str) == Some("output_text") {
                                    part.get("text").and_then(Value::as_str).unwrap_or_default()
                                } else {
                                    part.get("refusal")
                                        .and_then(Value::as_str)
                                        .unwrap_or_default()
                                }
                            })
                            .collect::<Vec<_>>()
                            .concat()
                    })
                    .unwrap_or_default();
                let signature = encode_text_signature_v1(
                    item.get("id").and_then(Value::as_str).unwrap_or_default(),
                    item.get("phase"),
                );
                let block = self.text_block_mut(content_index);
                block.text = text.clone();
                block.text_signature = Some(signature);
                emitted.push(AssistantMessageEvent::TextEnd {
                    content_index,
                    content: text,
                    partial: self.output.clone(),
                });
                self.slots.remove(&output_index);
            }
            (
                SlotKind::ToolCall {
                    partial_json: Some(partial_json),
                    ..
                },
                "function_call",
            ) => {
                let arguments = item
                    .get("arguments")
                    .and_then(Value::as_str)
                    .filter(|arguments| !arguments.is_empty())
                    .map(str::to_string)
                    .or_else(|| Some(partial_json.clone()).filter(|value| !value.is_empty()))
                    .unwrap_or_else(|| "{}".to_string());
                let parsed = parse_streaming_json(Some(&arguments));
                self.tool_call_mut(content_index).arguments =
                    parsed.as_object().cloned().unwrap_or_default();
                if let Some(namespace) = item.get("namespace").filter(|value| !value.is_null()) {
                    self.tool_call_mut(content_index).namespace =
                        namespace.as_str().map(str::to_string);
                }
                let tool_call = self.tool_call_mut(content_index).clone();
                emitted.push(AssistantMessageEvent::ToolcallEnd {
                    content_index,
                    tool_call,
                    partial: self.output.clone(),
                });
                self.slots.remove(&output_index);
            }
            (
                SlotKind::ToolCall {
                    custom_input: Some(custom_input),
                    ..
                },
                "custom_tool_call",
            ) => {
                let property = custom_input.property.clone();
                let input = item
                    .get("input")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| self.custom_tool_call_input(content_index, &property));
                if let Some(delta) =
                    self.append_custom_tool_call_input(output_index, &input, true)?
                {
                    emitted.push(AssistantMessageEvent::ToolcallDelta {
                        content_index,
                        delta,
                        partial: self.output.clone(),
                    });
                }
                if let Some(namespace) = item.get("namespace").filter(|value| !value.is_null()) {
                    self.tool_call_mut(content_index).namespace =
                        namespace.as_str().map(str::to_string);
                }
                let tool_call = self.tool_call_mut(content_index).clone();
                emitted.push(AssistantMessageEvent::ToolcallEnd {
                    content_index,
                    tool_call,
                    partial: self.output.clone(),
                });
                self.slots.remove(&output_index);
            }
            _ => {}
        }
        Ok(())
    }
}

fn join_text_parts(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_array)
        .map(|parts| {
            parts
                .iter()
                .map(|part| part.get("text").and_then(Value::as_str).unwrap_or_default())
                .collect::<Vec<_>>()
                .join("\n\n")
        })
        .unwrap_or_default()
}

/// `mapStopReason(status, incompleteReason)`
pub fn map_stop_reason(
    status: Option<&str>,
    incomplete_reason: Option<&str>,
) -> (StopReason, Option<String>) {
    match status {
        None | Some("") => (StopReason::Stop, None),
        Some("completed") => (StopReason::Stop, None),
        Some("incomplete") => {
            if incomplete_reason == Some("max_output_tokens") {
                return (StopReason::Length, None);
            }
            (
                StopReason::Error,
                Some(match incomplete_reason {
                    Some(reason) => format!("Response incomplete: {reason}"),
                    None => "Response incomplete without a provider reason".to_string(),
                }),
            )
        }
        Some("failed") | Some("cancelled") => (StopReason::Error, None),
        // These two are wonky ...
        Some("in_progress") | Some("queued") => (StopReason::Stop, None),
        // TS throws on an unknown status; the never-check cannot fire on well-formed
        // provider output, so the port keeps the same error.
        Some(status) => (
            StopReason::Error,
            Some(format!("Unhandled stop reason: {status}")),
        ),
    }
}
