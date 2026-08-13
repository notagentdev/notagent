//! Request building for OpenAI-compatible chat completions.
//!
//! 1:1 port of the request half of `packages/ai/src/api/openai-completions.ts`
//! (`buildParams`, `convertMessages`, `convertTools`, the Anthropic-style
//! `cache_control` placement, the chat-template resolution and the client headers).
//! The TS code hands the payload to the OpenAI SDK only to sign and send it, so the
//! payload itself is what has to stay byte-identical — the fixtures in
//! `tests/fixtures/openai-completions-payloads.jsonl` come straight out of the TS
//! implementation.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

use crate::api::constrained_sampling::{
    ConstrainedSamplingError, GrammarSyntax, get_grammar_tool_input,
    get_json_schema_tool_parameters, resolve_grammar_constrained_sampling,
    resolve_json_schema_strict_sampling,
};
use crate::api::github_copilot_headers::{build_copilot_dynamic_headers, has_copilot_vision_input};
use crate::api::openai_completions_compat::ResolvedOpenAICompletionsCompat;
use crate::api::openai_prompt_cache::clamp_openai_prompt_cache_key;
use crate::api::simple_options::{MIN_ANSWER_TOKENS, clamp_reasoning};
use crate::api::transform_messages::transform_messages;
use crate::types::{
    AssistantContent, CacheControlFormat, CacheRetention, ChatTemplateKwargValue,
    ChatTemplateVarName, Context, DeferredToolsMode, MaxTokensField, Message, Modality, Model,
    ModelThinkingLevel, ProviderEnv, ProviderHeaders, SessionAffinityFormat, TextOrImageContent,
    ThinkingBudgets, ThinkingFormat, ThinkingLevel, Tool, UserContent,
};
use crate::utils::hash::short_hash;
use crate::utils::provider_env::get_provider_env_value;
use crate::utils::sanitize_unicode::sanitize_surrogates;

/// `OpenAICompletionsOptions extends StreamOptions` — the fields the payload depends on.
///
/// `toolChoice` stays a raw `Value`: the SDK type is an open union of `"none" | "auto" |
/// "required"` and several object forms, and the payload copies it through unchanged.
#[derive(Debug, Clone, Default)]
pub struct OpenAICompletionsOptions {
    pub tool_choice: Option<Value>,
    pub reasoning_effort: Option<ThinkingLevel>,
    /// Only consulted when `compat.supportsThinkingTokenBudget` is set.
    pub thinking_budgets: Option<ThinkingBudgets>,
    pub max_tokens: Option<u64>,
    pub temperature: Option<f64>,
    pub sampling_params: Option<Map<String, Value>>,
    pub cache_retention: Option<CacheRetention>,
    pub session_id: Option<String>,
    pub env: Option<ProviderEnv>,
}

/// `OpenAICompatCacheControl { type: "ephemeral", ttl? }`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAICompatCacheControl {
    pub ttl: Option<String>,
}

impl OpenAICompatCacheControl {
    fn to_value(&self) -> Value {
        let mut object = Map::new();
        object.insert("type".to_string(), json!("ephemeral"));
        if let Some(ttl) = &self.ttl {
            object.insert("ttl".to_string(), json!(ttl));
        }
        Value::Object(object)
    }
}

/// `hasHeader(headers, name)`
pub fn has_header(headers: Option<&ProviderHeaders>, name: &str) -> bool {
    let Some(headers) = headers else {
        return false;
    };
    let expected = name.to_lowercase();
    headers.iter().any(|(key, value)| {
        key.to_lowercase() == expected
            && value.as_ref().is_some_and(|value| !value.trim().is_empty())
    })
}

/// `getClientApiKey(provider, apiKey, headers)` — errors when neither key nor header exists.
pub fn get_client_api_key(
    provider: &str,
    api_key: Option<&str>,
    headers: Option<&ProviderHeaders>,
) -> Result<String, ConstrainedSamplingError> {
    if let Some(api_key) = api_key.filter(|key| !key.is_empty()) {
        return Ok(api_key.to_string());
    }
    if has_header(headers, "authorization") || has_header(headers, "cf-aig-authorization") {
        return Ok("unused".to_string());
    }
    Err(ConstrainedSamplingError(format!(
        "No API key for provider: {provider}"
    )))
}

/// `hasToolHistory(messages)`
fn has_tool_history(messages: &[Message]) -> bool {
    messages.iter().any(|message| match message {
        Message::ToolResult(_) => true,
        Message::Assistant(message) => message
            .content
            .iter()
            .any(|block| matches!(block, AssistantContent::ToolCall(_))),
        Message::User(_) => false,
    })
}

/// `getDeferredToolNames(messages)` — insertion-ordered like the TS `Set`.
fn get_deferred_tool_names(messages: &[Message]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for message in messages {
        if let Message::ToolResult(message) = message {
            for name in message.added_tool_names.iter().flatten() {
                if !names.contains(name) {
                    names.push(name.clone());
                }
            }
        }
    }
    names
}

/// `getToolsByName(tools, names)` — keeps the order of `names`, drops unknown ones.
fn get_tools_by_name<'a>(tools: Option<&'a [Tool]>, names: &[String]) -> Vec<&'a Tool> {
    let Some(tools) = tools else {
        return Vec::new();
    };
    names
        .iter()
        .filter_map(|name| tools.iter().find(|tool| &tool.name == name))
        .collect()
}

/// `resolveCacheRetention(cacheRetention, env)`
pub fn resolve_cache_retention(
    cache_retention: Option<CacheRetention>,
    env: Option<&ProviderEnv>,
) -> CacheRetention {
    if let Some(cache_retention) = cache_retention {
        return cache_retention;
    }
    if get_provider_env_value("NOTAGENT_CACHE_RETENTION", env).as_deref() == Some("long") {
        return CacheRetention::Long;
    }
    CacheRetention::Short
}

/// `getCompatCacheControl(compat, cacheRetention)`
fn get_compat_cache_control(
    compat: &ResolvedOpenAICompletionsCompat,
    cache_retention: CacheRetention,
) -> Option<OpenAICompatCacheControl> {
    if compat.cache_control_format != Some(CacheControlFormat::Anthropic)
        || cache_retention == CacheRetention::None
    {
        return None;
    }
    let ttl = if cache_retention == CacheRetention::Long && compat.supports_long_cache_retention {
        Some("1h".to_string())
    } else {
        None
    };
    Some(OpenAICompatCacheControl { ttl })
}

// ---------------------------------------------------------------------------
// Thinking-level lookups
// ---------------------------------------------------------------------------

/// `model.thinkingLevelMap?.[level]` with `undefined` and `null` kept apart.
///
/// The distinction is load-bearing: the zai and baseten branches test
/// `mapped === undefined` explicitly, while qwen and the OpenAI default use `??`,
/// which also swallows `null`.
fn mapped_level(model: &Model, level: ModelThinkingLevel) -> Option<Option<String>> {
    model
        .thinking_level_map
        .as_ref()
        .and_then(|map| map.get(&level))
        .cloned()
}

/// `model.thinkingLevelMap?.[level] ?? fallback`
fn mapped_level_or(model: &Model, level: ModelThinkingLevel, fallback: &str) -> String {
    match mapped_level(model, level) {
        Some(Some(value)) => value,
        _ => fallback.to_string(),
    }
}

/// `model.thinkingLevelMap?.off !== null` — true unless the entry is explicitly `null`.
fn off_is_not_null(model: &Model) -> bool {
    !matches!(mapped_level(model, ModelThinkingLevel::Off), Some(None))
}

fn effort_str(effort: ThinkingLevel) -> &'static str {
    match effort {
        ThinkingLevel::Minimal => "minimal",
        ThinkingLevel::Low => "low",
        ThinkingLevel::Medium => "medium",
        ThinkingLevel::High => "high",
        ThinkingLevel::Xhigh => "xhigh",
        ThinkingLevel::Max => "max",
    }
}

// ---------------------------------------------------------------------------
// chat_template_kwargs / chat_template_args
// ---------------------------------------------------------------------------

/// `resolveChatTemplateKwargValue(model, options, value)`
fn resolve_chat_template_kwarg_value(
    model: &Model,
    options: &OpenAICompletionsOptions,
    value: &Value,
) -> Option<Value> {
    // Primitives (including `null`) pass through: TS bails out on
    // `typeof value !== "object" || value === null`.
    let Ok(ChatTemplateKwargValue::Var(var)) =
        serde_json::from_value::<ChatTemplateKwargValue>(value.clone())
    else {
        return Some(value.clone());
    };

    let reasoning_effort = options.reasoning_effort;
    if reasoning_effort.is_none() && var.omit_when_off == Some(true) {
        return None;
    }
    if var.var == ChatTemplateVarName::ThinkingEnabled {
        return Some(json!(reasoning_effort.is_some()));
    }

    let mapped = match reasoning_effort {
        Some(effort) => mapped_level(model, effort.into()),
        None => mapped_level(model, ModelThinkingLevel::Off),
    };
    match mapped {
        // The entry is missing: fall back to the requested level, which may itself be absent.
        None => reasoning_effort.map(|effort| json!(effort_str(effort))),
        Some(Some(value)) => Some(json!(value)),
        Some(None) => None,
    }
}

/// `buildChatTemplateValues(model, options, values)`
fn build_chat_template_values(
    model: &Model,
    options: &OpenAICompletionsOptions,
    values: &Map<String, Value>,
) -> Option<Map<String, Value>> {
    let mut resolved = Map::new();
    for (key, value) in values {
        if let Some(value) = resolve_chat_template_kwarg_value(model, options, value) {
            resolved.insert(key.clone(), value);
        }
    }
    if resolved.is_empty() {
        None
    } else {
        Some(resolved)
    }
}

// ---------------------------------------------------------------------------
// cache_control placement
// ---------------------------------------------------------------------------

/// `addCacheControlToTextContent(message, cacheControl)`
fn add_cache_control_to_text_content(
    message: &mut Value,
    cache_control: &OpenAICompatCacheControl,
) -> bool {
    let Some(object) = message.as_object_mut() else {
        return false;
    };
    match object.get("content") {
        Some(Value::String(content)) => {
            if content.is_empty() {
                return false;
            }
            let text = content.clone();
            object.insert(
                "content".to_string(),
                json!([{ "type": "text", "text": text, "cache_control": cache_control.to_value() }]),
            );
            true
        }
        Some(Value::Array(_)) => {
            let Some(Value::Array(parts)) = object.get_mut("content") else {
                return false;
            };
            for part in parts.iter_mut().rev() {
                if part.get("type").and_then(Value::as_str) == Some("text")
                    && let Some(part) = part.as_object_mut()
                {
                    part.insert("cache_control".to_string(), cache_control.to_value());
                    return true;
                }
            }
            false
        }
        _ => false,
    }
}

/// `applyAnthropicCacheControl(messages, tools, cacheControl)`
fn apply_anthropic_cache_control(
    messages: &mut [Value],
    tools: Option<&mut Vec<Value>>,
    cache_control: &OpenAICompatCacheControl,
) {
    // System prompt.
    for message in messages.iter_mut() {
        let role = message.get("role").and_then(Value::as_str);
        if role == Some("system") || role == Some("developer") {
            add_cache_control_to_text_content(message, cache_control);
            break;
        }
    }

    // Last tool.
    if let Some(tools) = tools
        && let Some(last) = tools.last_mut()
        && let Some(object) = last.as_object_mut()
    {
        object.insert("cache_control".to_string(), cache_control.to_value());
    }

    // Last conversation message.
    for message in messages.iter_mut().rev() {
        let role = message.get("role").and_then(Value::as_str);
        if matches!(role, Some("user") | Some("assistant") | Some("tool"))
            && add_cache_control_to_text_content(message, cache_control)
        {
            break;
        }
    }
}

// ---------------------------------------------------------------------------
// convertTools
// ---------------------------------------------------------------------------

/// `convertTools(tools, compat)`
pub fn convert_tools(
    tools: &[&Tool],
    compat: &ResolvedOpenAICompletionsCompat,
) -> Result<Vec<Value>, ConstrainedSamplingError> {
    let mut converted = Vec::with_capacity(tools.len());
    for tool in tools {
        if let Some(grammar) =
            resolve_grammar_constrained_sampling(tool, compat.supports_openai_grammar_tools)?
        {
            let syntax = match grammar.format {
                GrammarSyntax::Lark => "lark",
                GrammarSyntax::Regex => "regex",
            };
            converted.push(json!({
                "type": "custom",
                "custom": {
                    "name": tool.name,
                    "description": tool.description,
                    "format": {
                        "type": "grammar",
                        "grammar": { "syntax": syntax, "definition": grammar.definition },
                    },
                },
            }));
            continue;
        }

        let strict = resolve_json_schema_strict_sampling(tool, compat.supports_strict_mode)?;
        let mut function = Map::new();
        function.insert("name".to_string(), json!(tool.name));
        function.insert("description".to_string(), json!(tool.description));
        function.insert(
            "parameters".to_string(),
            get_json_schema_tool_parameters(tool, strict),
        );
        if compat.supports_strict_mode {
            function.insert("strict".to_string(), json!(strict.unwrap_or(false)));
        }
        let mut object = Map::new();
        object.insert("type".to_string(), json!("function"));
        object.insert("function".to_string(), Value::Object(function));
        converted.push(Value::Object(object));
    }
    Ok(converted)
}

// ---------------------------------------------------------------------------
// convertMessages
// ---------------------------------------------------------------------------

/// `normalizeToolCallId(id)` — pipe-separated Responses ids collapse into one id.
fn normalize_tool_call_id(model: &Model, id: &str) -> String {
    if let Some(separator_index) = id.find('|') {
        let sanitize = |value: &str| -> String {
            value
                .chars()
                .map(|character| {
                    if character.is_ascii_alphanumeric() || character == '_' || character == '-' {
                        character
                    } else {
                        '_'
                    }
                })
                .collect()
        };
        let call_id = sanitize(&id[..separator_index]);
        let item_id = sanitize(&id[separator_index + 1..]);
        let combined_id = if item_id.is_empty() {
            call_id.clone()
        } else {
            format!("{call_id}_{item_id}")
        };
        // TS measures with `String.length` (UTF-16 code units); sanitizing already
        // reduced everything to ASCII, so bytes and code units agree here.
        if combined_id.len() <= 40 {
            return combined_id;
        }
        let hash: String = short_hash(id).chars().take(8).collect();
        let prefix_length = 40usize.saturating_sub(hash.len() + 1).max(1);
        let prefix: String = call_id.chars().take(prefix_length).collect();
        return format!("{prefix}_{hash}");
    }

    if model.provider == "openai" && id.chars().count() > 40 {
        return id.chars().take(40).collect();
    }
    id.to_string()
}

/// `ConvertCompletionsMessagesOptions`
#[derive(Debug, Clone, Default)]
pub struct ConvertCompletionsMessagesOptions<'a> {
    pub grammar_tool_input_properties: Option<&'a BTreeMap<String, String>>,
}

/// `convertMessages(model, context, compat, options)`
pub fn convert_messages(
    model: &Model,
    context: &Context,
    compat: &ResolvedOpenAICompletionsCompat,
    options: &ConvertCompletionsMessagesOptions<'_>,
    timestamp: i64,
) -> Result<Vec<Value>, ConstrainedSamplingError> {
    let mut params: Vec<Value> = Vec::new();

    let transformed_messages = transform_messages(
        &context.messages,
        model,
        Some(&|id: &str, _source: &crate::types::AssistantMessage| {
            normalize_tool_call_id(model, id)
        }),
        timestamp,
    );

    if let Some(system_prompt) = &context.system_prompt
        && !system_prompt.is_empty()
    {
        let use_developer_role = model.reasoning && compat.supports_developer_role;
        let role = if use_developer_role {
            "developer"
        } else {
            "system"
        };
        params.push(json!({ "role": role, "content": sanitize_surrogates(system_prompt) }));
    }

    let mut last_role: Option<&'static str> = None;

    let mut index = 0usize;
    while index < transformed_messages.len() {
        let message = &transformed_messages[index];
        let role = match message {
            Message::User(_) => "user",
            Message::Assistant(_) => "assistant",
            Message::ToolResult(_) => "toolResult",
        };

        // Some providers reject a user message right after a tool result.
        if compat.requires_assistant_after_tool_result
            && last_role == Some("toolResult")
            && role == "user"
        {
            params.push(json!({
                "role": "assistant",
                "content": "I have processed the tool results.",
            }));
        }

        match message {
            Message::User(user) => match &user.content {
                UserContent::Text(text) => {
                    params.push(json!({ "role": "user", "content": sanitize_surrogates(text) }));
                }
                UserContent::Blocks(blocks) => {
                    let content: Vec<Value> = blocks
                        .iter()
                        .map(|block| match block {
                            TextOrImageContent::Text(text) => {
                                json!({ "type": "text", "text": sanitize_surrogates(&text.text) })
                            }
                            TextOrImageContent::Image(image) => json!({
                                "type": "image_url",
                                "image_url": {
                                    "url": format!("data:{};base64,{}", image.mime_type, image.data),
                                },
                            }),
                        })
                        .collect();
                    if content.is_empty() {
                        index += 1;
                        continue;
                    }
                    params.push(json!({ "role": "user", "content": content }));
                }
            },
            Message::Assistant(assistant) => {
                let mut assistant_message = Map::new();
                assistant_message.insert("role".to_string(), json!("assistant"));
                // Some providers reject `null` content, so an empty string stands in.
                assistant_message.insert(
                    "content".to_string(),
                    if compat.requires_assistant_after_tool_result {
                        json!("")
                    } else {
                        Value::Null
                    },
                );

                let assistant_text_parts: Vec<String> = assistant
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        AssistantContent::Text(text) if !text.text.trim().is_empty() => {
                            Some(sanitize_surrogates(&text.text))
                        }
                        _ => None,
                    })
                    .collect();
                let assistant_text = assistant_text_parts.concat();

                let non_empty_thinking_blocks: Vec<_> = assistant
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        AssistantContent::Thinking(thinking)
                            if !thinking.thinking.trim().is_empty() =>
                        {
                            Some(thinking)
                        }
                        _ => None,
                    })
                    .collect();

                if !non_empty_thinking_blocks.is_empty() {
                    if compat.requires_thinking_as_text {
                        // Plain text, no tags — tags invite the model to mimic them.
                        let thinking_text = non_empty_thinking_blocks
                            .iter()
                            .map(|block| sanitize_surrogates(&block.thinking))
                            .collect::<Vec<_>>()
                            .join("\n\n");
                        let mut content = vec![json!({ "type": "text", "text": thinking_text })];
                        content.extend(
                            assistant_text_parts
                                .iter()
                                .map(|text| json!({ "type": "text", "text": text })),
                        );
                        assistant_message.insert("content".to_string(), Value::Array(content));
                    } else {
                        // Assistant content always goes out as a plain string: the array
                        // form makes some models (DeepSeek V3.2 via NVIDIA NIM) mirror the
                        // block structure literally.
                        if !assistant_text.is_empty() {
                            assistant_message
                                .insert("content".to_string(), json!(assistant_text.clone()));
                        }

                        let mut signature = non_empty_thinking_blocks[0].thinking_signature.clone();
                        if model.provider == "opencode-go"
                            && signature.as_deref() == Some("reasoning")
                        {
                            signature = Some("reasoning_content".to_string());
                        }
                        if let Some(signature) = signature.filter(|value| !value.is_empty()) {
                            let joined = non_empty_thinking_blocks
                                .iter()
                                .map(|block| block.thinking.clone())
                                .collect::<Vec<_>>()
                                .join("\n");
                            assistant_message.insert(signature, json!(joined));
                        }
                    }
                } else if !assistant_text.is_empty() {
                    assistant_message.insert("content".to_string(), json!(assistant_text.clone()));
                }

                let tool_calls: Vec<_> = assistant
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        AssistantContent::ToolCall(tool_call) => Some(tool_call),
                        _ => None,
                    })
                    .collect();
                if !tool_calls.is_empty() {
                    let mut converted = Vec::with_capacity(tool_calls.len());
                    for tool_call in &tool_calls {
                        let custom_input_property = options
                            .grammar_tool_input_properties
                            .and_then(|properties| properties.get(&tool_call.name));
                        if let Some(property) = custom_input_property {
                            converted.push(json!({
                                "id": tool_call.id,
                                "type": "custom",
                                "custom": {
                                    "name": tool_call.name,
                                    "input": sanitize_surrogates(&get_grammar_tool_input(
                                        &tool_call.name,
                                        &tool_call.arguments,
                                        property,
                                    )?),
                                },
                            }));
                        } else {
                            converted.push(json!({
                                "id": tool_call.id,
                                "type": "function",
                                "function": {
                                    "name": tool_call.name,
                                    "arguments": serde_json::to_string(&tool_call.arguments)
                                        .unwrap_or_else(|_| "{}".to_string()),
                                },
                            }));
                        }
                    }
                    assistant_message.insert("tool_calls".to_string(), Value::Array(converted));

                    let reasoning_details: Vec<Value> = tool_calls
                        .iter()
                        .filter_map(|tool_call| tool_call.thought_signature.as_ref())
                        .filter_map(|signature| serde_json::from_str::<Value>(signature).ok())
                        // `.filter(Boolean)` drops `null` and `0` and `""` alike.
                        .filter(|detail| !is_falsy(detail))
                        .collect();
                    if !reasoning_details.is_empty() {
                        assistant_message.insert(
                            "reasoning_details".to_string(),
                            Value::Array(reasoning_details),
                        );
                    }
                }

                if compat.requires_reasoning_content_on_assistant_messages
                    && model.reasoning
                    && !assistant_message.contains_key("reasoning_content")
                {
                    assistant_message.insert("reasoning_content".to_string(), json!(""));
                }

                // Providers demand "either content or tool_calls, but not none"; an aborted
                // assistant turn can have neither.
                let has_content = match assistant_message.get("content") {
                    Some(Value::String(content)) => !content.is_empty(),
                    Some(Value::Array(content)) => !content.is_empty(),
                    _ => false,
                };
                if !has_content && !assistant_message.contains_key("tool_calls") {
                    index += 1;
                    continue;
                }
                params.push(Value::Object(assistant_message));
            }
            Message::ToolResult(_) => {
                let mut image_blocks: Vec<Value> = Vec::new();
                let mut deferred_tool_names: Vec<String> = Vec::new();
                let mut scan = index;

                while scan < transformed_messages.len() {
                    let Message::ToolResult(tool_message) = &transformed_messages[scan] else {
                        break;
                    };

                    let text_result = tool_message
                        .content
                        .iter()
                        .filter_map(|block| match block {
                            TextOrImageContent::Text(text) => Some(text.text.as_str()),
                            TextOrImageContent::Image(_) => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    let has_images = tool_message
                        .content
                        .iter()
                        .any(|block| matches!(block, TextOrImageContent::Image(_)));

                    let tool_result_text = if !text_result.is_empty() {
                        text_result.as_str()
                    } else if has_images {
                        "(see attached image)"
                    } else {
                        "(no tool output)"
                    };
                    let mut tool_result_message = Map::new();
                    tool_result_message.insert("role".to_string(), json!("tool"));
                    tool_result_message.insert(
                        "content".to_string(),
                        json!(sanitize_surrogates(tool_result_text)),
                    );
                    tool_result_message
                        .insert("tool_call_id".to_string(), json!(tool_message.tool_call_id));
                    if compat.requires_tool_result_name && !tool_message.tool_name.is_empty() {
                        tool_result_message
                            .insert("name".to_string(), json!(tool_message.tool_name));
                    }
                    params.push(Value::Object(tool_result_message));

                    if compat.deferred_tools_mode == Some(DeferredToolsMode::Kimi) {
                        for name in tool_message.added_tool_names.iter().flatten() {
                            if !deferred_tool_names.contains(name) {
                                deferred_tool_names.push(name.clone());
                            }
                        }
                    }

                    if has_images && model.input.contains(&Modality::Image) {
                        for block in &tool_message.content {
                            if let TextOrImageContent::Image(image) = block {
                                image_blocks.push(json!({
                                    "type": "image_url",
                                    "image_url": {
                                        "url": format!(
                                            "data:{};base64,{}",
                                            image.mime_type, image.data
                                        ),
                                    },
                                }));
                            }
                        }
                    }
                    scan += 1;
                }

                index = scan - 1;

                if !image_blocks.is_empty() {
                    if compat.requires_assistant_after_tool_result {
                        params.push(json!({
                            "role": "assistant",
                            "content": "I have processed the tool results.",
                        }));
                    }
                    let mut content = vec![
                        json!({ "type": "text", "text": "Attached image(s) from tool result:" }),
                    ];
                    content.extend(image_blocks);
                    params.push(json!({ "role": "user", "content": content }));
                    last_role = Some("user");
                } else {
                    last_role = Some("toolResult");
                }

                if !deferred_tool_names.is_empty() {
                    let deferred_tools =
                        get_tools_by_name(context.tools.as_deref(), &deferred_tool_names);
                    if !deferred_tools.is_empty() {
                        // Kimi accepts a system message that carries tools and no content.
                        params.push(json!({
                            "role": "system",
                            "tools": convert_tools(&deferred_tools, compat)?,
                        }));
                    }
                }
                index += 1;
                continue;
            }
        }

        last_role = Some(role);
        index += 1;
    }

    Ok(params)
}

/// `.filter(Boolean)` on a parsed JSON value.
fn is_falsy(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Bool(value) => !value,
        Value::String(value) => value.is_empty(),
        Value::Number(number) => number.as_f64().is_some_and(|value| value == 0.0),
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// buildParams
// ---------------------------------------------------------------------------

/// `buildParams(model, context, options, compat, cacheRetention, grammarToolInputProperties)`
pub fn build_params(
    model: &Model,
    context: &Context,
    options: &OpenAICompletionsOptions,
    compat: &ResolvedOpenAICompletionsCompat,
    cache_retention: CacheRetention,
    grammar_tool_input_properties: &BTreeMap<String, String>,
    timestamp: i64,
) -> Result<Value, ConstrainedSamplingError> {
    let mut messages = convert_messages(
        model,
        context,
        compat,
        &ConvertCompletionsMessagesOptions {
            grammar_tool_input_properties: Some(grammar_tool_input_properties),
        },
        timestamp,
    )?;
    let cache_control = get_compat_cache_control(compat, cache_retention);

    let deferred_tool_names = if compat.deferred_tools_mode == Some(DeferredToolsMode::Kimi) {
        get_deferred_tool_names(&context.messages)
    } else {
        Vec::new()
    };
    let active_tools: Option<Vec<&Tool>> = context.tools.as_ref().map(|tools| {
        tools
            .iter()
            .filter(|tool| !deferred_tool_names.contains(&tool.name))
            .collect()
    });
    let mut zai_tool_stream = false;
    let mut tools: Option<Vec<Value>> = match &active_tools {
        Some(active_tools) if !active_tools.is_empty() => {
            zai_tool_stream = compat.zai_tool_stream;
            Some(convert_tools(active_tools, compat)?)
        }
        // Anthropic via LiteLLM/proxy requires the tools param once the conversation
        // carries tool calls or tool results.
        _ if has_tool_history(&context.messages) => Some(Vec::new()),
        _ => None,
    };

    if let Some(cache_control) = &cache_control {
        apply_anthropic_cache_control(&mut messages, tools.as_mut(), cache_control);
    }

    let prompt_cache_key = if (model.base_url.contains("api.openai.com")
        && cache_retention != CacheRetention::None)
        || (cache_retention == CacheRetention::Long && compat.supports_long_cache_retention)
    {
        clamp_openai_prompt_cache_key(options.session_id.as_deref())
    } else {
        None
    };

    let mut params = Map::new();
    params.insert("model".to_string(), json!(model.id));
    params.insert("messages".to_string(), Value::Array(messages));
    params.insert("stream".to_string(), json!(true));
    if let Some(prompt_cache_key) = prompt_cache_key {
        params.insert("prompt_cache_key".to_string(), json!(prompt_cache_key));
    }
    if cache_retention == CacheRetention::Long && compat.supports_long_cache_retention {
        params.insert("prompt_cache_retention".to_string(), json!("24h"));
    }

    if compat.supports_usage_in_streaming {
        params.insert(
            "stream_options".to_string(),
            json!({ "include_usage": true }),
        );
    }
    if compat.supports_store {
        params.insert("store".to_string(), json!(false));
    }

    if let Some(max_tokens) = options.max_tokens.filter(|value| *value != 0) {
        match compat.max_tokens_field {
            MaxTokensField::MaxTokens => {
                params.insert("max_tokens".to_string(), json!(max_tokens));
            }
            MaxTokensField::MaxCompletionTokens => {
                params.insert("max_completion_tokens".to_string(), json!(max_tokens));
            }
        }
    }
    if let Some(temperature) = options.temperature {
        params.insert("temperature".to_string(), json!(temperature));
    }

    if let Some(tools) = tools {
        params.insert("tools".to_string(), Value::Array(tools));
        if zai_tool_stream {
            params.insert("tool_stream".to_string(), json!(true));
        }
    }

    if let Some(tool_choice) = &options.tool_choice {
        params.insert("tool_choice".to_string(), tool_choice.clone());
    }

    apply_thinking_format(model, options, compat, &mut params);

    // vLLM caps reasoning with a top-level thinking_token_budget, independent of the
    // thinking format: the same server can serve zai, qwen and chat-template models.
    // Reasoning and the answer share max_tokens there, so an uncapped reasoning phase
    // can eat the whole response and leave neither answer nor tool call.
    if compat.supports_thinking_token_budget
        && let Some(reasoning_effort) = options.reasoning_effort
        && model.reasoning
    {
        let level = clamp_reasoning(Some(reasoning_effort)).expect("Some in, Some out");
        let budgets = options.thinking_budgets.unwrap_or_default();
        let budget_for_level = match level {
            ThinkingLevel::Minimal => budgets.minimal.unwrap_or(1024),
            ThinkingLevel::Low => budgets.low.unwrap_or(2048),
            ThinkingLevel::Medium => budgets.medium.unwrap_or(8192),
            // `clampReasoning` already folded xhigh and max into high.
            _ => budgets.high.unwrap_or(16384),
        };
        let ceiling = params
            .get("max_tokens")
            .or_else(|| params.get("max_completion_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(model.max_tokens);
        // Always leave room for the answer, otherwise the budget recreates the bug it prevents.
        let budget = budget_for_level.min(ceiling.saturating_sub(MIN_ANSWER_TOKENS));
        if budget > 0 {
            params.insert("thinking_token_budget".to_string(), json!(budget));
        }
    }

    // OpenRouter provider routing preferences.
    if let Some(routing) = model_open_router_routing(model) {
        params.insert(
            "provider".to_string(),
            serde_json::to_value(routing).unwrap_or(Value::Null),
        );
    }

    // Vercel AI Gateway provider routing preferences.
    if let Some(routing) = model_vercel_gateway_routing(model)
        && (routing.only.is_some() || routing.order.is_some())
    {
        let mut gateway_options = Map::new();
        if let Some(only) = &routing.only {
            gateway_options.insert("only".to_string(), json!(only));
        }
        if let Some(order) = &routing.order {
            gateway_options.insert("order".to_string(), json!(order));
        }
        params.insert(
            "providerOptions".to_string(),
            json!({ "gateway": gateway_options }),
        );
    }

    // Last, so custom keys override the named request fields.
    if let Some(sampling_params) = &options.sampling_params {
        for (key, value) in sampling_params {
            params.insert(key.clone(), value.clone());
        }
    }

    Ok(Value::Object(params))
}

fn model_open_router_routing(model: &Model) -> Option<&crate::types::OpenRouterRouting> {
    match &model.compat {
        Some(crate::types::ModelCompat::OpenAICompletions(compat)) => {
            compat.open_router_routing.as_ref()
        }
        _ => None,
    }
}

fn model_vercel_gateway_routing(model: &Model) -> Option<&crate::types::VercelGatewayRouting> {
    match &model.compat {
        Some(crate::types::ModelCompat::OpenAICompletions(compat)) => {
            compat.vercel_gateway_routing.as_ref()
        }
        _ => None,
    }
}

/// The eleven `thinkingFormat` branches of `buildParams`.
fn apply_thinking_format(
    model: &Model,
    options: &OpenAICompletionsOptions,
    compat: &ResolvedOpenAICompletionsCompat,
    params: &mut Map<String, Value>,
) {
    let effort = options.reasoning_effort;
    let effort_level: Option<ModelThinkingLevel> = effort.map(Into::into);

    match compat.thinking_format {
        ThinkingFormat::Zai if model.reasoning => {
            params.insert(
                "thinking".to_string(),
                if effort.is_some() {
                    json!({ "type": "enabled", "clear_thinking": false })
                } else {
                    json!({ "type": "disabled" })
                },
            );
            if let Some(level) = effort_level
                && compat.supports_reasoning_effort
            {
                // An explicit `null` disables the field here; `undefined` falls back.
                let mapped = mapped_level(model, level);
                let resolved = match mapped {
                    None => Some(effort_str(effort.expect("level implies effort")).to_string()),
                    Some(value) => value,
                };
                if let Some(resolved) = resolved {
                    params.insert("reasoning_effort".to_string(), json!(resolved));
                }
            }
        }
        ThinkingFormat::Qwen if model.reasoning => {
            params.insert("enable_thinking".to_string(), json!(effort.is_some()));
            if let Some(level) = effort_level
                && compat.supports_reasoning_effort
            {
                let resolved = mapped_level_or(model, level, effort_str(effort.expect("level")));
                params.insert("reasoning_effort".to_string(), json!(resolved));
            }
        }
        ThinkingFormat::QwenChatTemplate if model.reasoning => {
            params.insert(
                "chat_template_kwargs".to_string(),
                json!({ "enable_thinking": effort.is_some(), "preserve_thinking": true }),
            );
        }
        ThinkingFormat::ChatTemplate if model.reasoning => {
            if let Some(values) =
                build_chat_template_values(model, options, &compat.chat_template_kwargs)
            {
                params.insert("chat_template_kwargs".to_string(), Value::Object(values));
            }
        }
        ThinkingFormat::Baseten if model.reasoning => {
            if let Some(values) =
                build_chat_template_values(model, options, &compat.chat_template_args)
            {
                params.insert("chat_template_args".to_string(), Value::Object(values));
            }
            if compat.supports_reasoning_effort {
                let mapped = match effort_level {
                    Some(level) => mapped_level(model, level),
                    None => mapped_level(model, ModelThinkingLevel::Off),
                };
                let resolved = match mapped {
                    None => effort.map(|effort| effort_str(effort).to_string()),
                    Some(value) => value,
                };
                if let Some(resolved) = resolved {
                    params.insert("reasoning_effort".to_string(), json!(resolved));
                }
            }
        }
        ThinkingFormat::Deepseek if model.reasoning => {
            if effort.is_some() {
                params.insert("thinking".to_string(), json!({ "type": "enabled" }));
            } else if off_is_not_null(model) {
                params.insert("thinking".to_string(), json!({ "type": "disabled" }));
            }
            if let Some(level) = effort_level
                && compat.supports_reasoning_effort
            {
                let resolved = mapped_level_or(model, level, effort_str(effort.expect("level")));
                params.insert("reasoning_effort".to_string(), json!(resolved));
            }
        }
        ThinkingFormat::Openrouter if model.reasoning => {
            // OpenRouter normalizes reasoning across providers via a nested object.
            if let Some(level) = effort_level {
                let resolved = mapped_level_or(model, level, effort_str(effort.expect("level")));
                params.insert("reasoning".to_string(), json!({ "effort": resolved }));
            } else if off_is_not_null(model) {
                let resolved = mapped_level_or(model, ModelThinkingLevel::Off, "none");
                params.insert("reasoning".to_string(), json!({ "effort": resolved }));
            }
        }
        ThinkingFormat::AntLing if model.reasoning && effort.is_some() => {
            // No fallback here: a missing entry drops the field entirely.
            if let Some(Some(resolved)) = mapped_level(model, effort_level.expect("effort")) {
                params.insert("reasoning".to_string(), json!({ "effort": resolved }));
            }
        }
        ThinkingFormat::Together if model.reasoning => {
            params.insert(
                "reasoning".to_string(),
                json!({ "enabled": effort.is_some() }),
            );
            if let Some(level) = effort_level
                && compat.supports_reasoning_effort
            {
                let resolved = mapped_level_or(model, level, effort_str(effort.expect("level")));
                params.insert("reasoning_effort".to_string(), json!(resolved));
            }
        }
        ThinkingFormat::StringThinking if model.reasoning => {
            if let Some(level) = effort_level {
                let resolved = mapped_level_or(model, level, effort_str(effort.expect("level")));
                params.insert("thinking".to_string(), json!(resolved));
            } else if off_is_not_null(model) {
                let resolved = mapped_level_or(model, ModelThinkingLevel::Off, "none");
                params.insert("thinking".to_string(), json!(resolved));
            }
        }
        // The `else if` chain falls through to the OpenAI style whenever the format
        // matched but the model does not reason.
        _ => {
            if let Some(level) = effort_level {
                if model.reasoning && compat.supports_reasoning_effort {
                    let resolved =
                        mapped_level_or(model, level, effort_str(effort.expect("level")));
                    params.insert("reasoning_effort".to_string(), json!(resolved));
                }
            } else if model.reasoning
                && compat.supports_reasoning_effort
                && let Some(Some(off_value)) = mapped_level(model, ModelThinkingLevel::Off)
            {
                params.insert("reasoning_effort".to_string(), json!(off_value));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Client headers
// ---------------------------------------------------------------------------

/// The `defaultHeaders` half of `createClient(model, context, …)`.
pub fn build_client_headers(
    model: &Model,
    context: &Context,
    options_headers: Option<&ProviderHeaders>,
    session_id: Option<&str>,
    compat: &ResolvedOpenAICompletionsCompat,
) -> BTreeMap<String, Option<String>> {
    let mut headers: BTreeMap<String, Option<String>> = model
        .headers
        .clone()
        .unwrap_or_default()
        .into_iter()
        .map(|(key, value)| (key, Some(value)))
        .collect();

    if model.provider == "github-copilot" {
        let has_images = has_copilot_vision_input(&context.messages);
        for (key, value) in build_copilot_dynamic_headers(&context.messages, has_images) {
            headers.insert(key, Some(value));
        }
    }

    if let Some(session_id) = session_id.filter(|value| !value.is_empty())
        && compat.send_session_affinity_headers
    {
        if compat.session_affinity_format == Some(SessionAffinityFormat::Openrouter) {
            headers.insert("x-session-id".to_string(), Some(session_id.to_string()));
        } else {
            if compat.session_affinity_format == Some(SessionAffinityFormat::Openai) {
                headers.insert("session_id".to_string(), Some(session_id.to_string()));
            }
            headers.insert(
                "x-client-request-id".to_string(),
                Some(session_id.to_string()),
            );
            headers.insert(
                "x-session-affinity".to_string(),
                Some(session_id.to_string()),
            );
        }
    }

    // Options headers merge last so they can override the defaults.
    if let Some(options_headers) = options_headers {
        for (key, value) in options_headers {
            headers.insert(key.clone(), value.clone());
        }
    }

    headers
}
