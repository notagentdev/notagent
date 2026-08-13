//! Anthropic Messages request building.
//!
//! 1:1 port of the request half of `packages/ai/src/api/anthropic-messages.ts`
//! (`getAnthropicCompat`, `getCacheControl`, `convertMessages`, `convertTools`,
//! `buildParams`, the Claude Code tool-name mapping and the beta-header assembly).
//! The TS code hands the payload to the Anthropic SDK only to sign and send it, so the
//! payload itself is what has to stay byte-identical.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};

use crate::api::constrained_sampling::{
    get_json_schema_tool_parameters, resolve_json_schema_strict_sampling,
};
use crate::api::transform_messages::transform_messages;
use crate::types::{
    AnthropicMessagesCompat, AssistantContent, CacheRetention, Context, Message, Model,
    ProviderEnv, ProviderHeaders, TextOrImageContent, Tool, ToolResultMessage, UserContent,
};
use crate::utils::deferred_tools::split_deferred_tools;
use crate::utils::provider_env::get_provider_env_value;
use crate::utils::sanitize_unicode::sanitize_surrogates;

pub const FINE_GRAINED_TOOL_STREAMING_BETA: &str = "fine-grained-tool-streaming-2025-05-14";
pub const INTERLEAVED_THINKING_BETA: &str = "interleaved-thinking-2025-05-14";

/// Stealth mode: mimic Claude Code's identity exactly.
pub const CLAUDE_CODE_VERSION: &str = "2.1.75";

/// Claude Code 2.x tool names in canonical casing.
pub const CLAUDE_CODE_TOOLS: [&str; 17] = [
    "Read",
    "Write",
    "Edit",
    "Bash",
    "Grep",
    "Glob",
    "AskUserQuestion",
    "EnterPlanMode",
    "ExitPlanMode",
    "KillShell",
    "NotebookEdit",
    "Skill",
    "Task",
    "TaskOutput",
    "TodoWrite",
    "WebFetch",
    "WebSearch",
];

/// `toClaudeCodeName(name)` — canonical casing when the name matches case-insensitively.
pub fn to_claude_code_name(name: &str) -> String {
    let lower = name.to_lowercase();
    CLAUDE_CODE_TOOLS
        .iter()
        .find(|candidate| candidate.to_lowercase() == lower)
        .map(|candidate| (*candidate).to_string())
        .unwrap_or_else(|| name.to_string())
}

/// `isOAuthToken(apiKey)`
pub fn is_oauth_token(api_key: &str) -> bool {
    api_key.contains("sk-ant-oat")
}

/// The resolved `AnthropicMessagesCompat` with every default applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedAnthropicCompat {
    pub supports_eager_tool_input_streaming: bool,
    pub supports_long_cache_retention: bool,
    pub send_session_affinity_headers: bool,
    pub supports_cache_control_on_tools: bool,
    pub supports_temperature: bool,
    pub allow_empty_signature: bool,
    pub supports_strict_tools: bool,
    pub supports_tool_references: bool,
}

fn anthropic_compat(model: &Model) -> Option<&AnthropicMessagesCompat> {
    model
        .compat
        .as_ref()
        .and_then(crate::types::ModelCompat::as_anthropic_messages)
}

/// `defaultSupportsToolReferences(model)` — first-party models from Claude 4.5 upward,
/// excluding Haiku.
pub fn default_supports_tool_references(model: &Model) -> bool {
    if model.provider != "anthropic" || model.id.contains("haiku") {
        return false;
    }
    // `^claude-(?:opus|sonnet|fable)-(\d+)(?:-(\d+))?(?:-|$)`
    let Some(rest) = model.id.strip_prefix("claude-") else {
        return false;
    };
    let Some(rest) = ["opus-", "sonnet-", "fable-"]
        .iter()
        .find_map(|family| rest.strip_prefix(*family))
    else {
        return false;
    };
    let mut parts = rest.split('-');
    let Some(major) = parts.next().and_then(|part| part.parse::<u32>().ok()) else {
        return false;
    };
    let minor = parts
        .next()
        .filter(|part| {
            part.len() < 8 && part.chars().all(|c| c.is_ascii_digit()) && !part.is_empty()
        })
        .and_then(|part| part.parse::<u32>().ok())
        .unwrap_or(0);
    major > 4 || (major == 4 && minor >= 5)
}

/// `getAnthropicCompat(model)`
pub fn get_anthropic_compat(model: &Model) -> ResolvedAnthropicCompat {
    let compat = anthropic_compat(model);
    ResolvedAnthropicCompat {
        supports_eager_tool_input_streaming: compat
            .and_then(|compat| compat.supports_eager_tool_input_streaming)
            .unwrap_or(true),
        supports_long_cache_retention: compat
            .and_then(|compat| compat.supports_long_cache_retention)
            .unwrap_or(true),
        send_session_affinity_headers: compat
            .and_then(|compat| compat.send_session_affinity_headers)
            .unwrap_or(false),
        supports_cache_control_on_tools: compat
            .and_then(|compat| compat.supports_cache_control_on_tools)
            .unwrap_or(true),
        supports_temperature: compat
            .and_then(|compat| compat.supports_temperature)
            .unwrap_or(true),
        allow_empty_signature: compat
            .and_then(|compat| compat.allow_empty_signature)
            .unwrap_or(false),
        supports_strict_tools: compat
            .and_then(|compat| compat.supports_strict_tools)
            .unwrap_or(false),
        supports_tool_references: compat
            .and_then(|compat| compat.supports_tool_references)
            .unwrap_or_else(|| default_supports_tool_references(model)),
    }
}

/// `forceAdaptiveThinking` — not part of the resolved defaults in TS either.
pub fn force_adaptive_thinking(model: &Model) -> bool {
    anthropic_compat(model)
        .and_then(|compat| compat.force_adaptive_thinking)
        .unwrap_or(false)
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

/// `getCacheControl(model, cacheRetention, env)`
pub fn get_cache_control(
    model: &Model,
    cache_retention: Option<CacheRetention>,
    env: Option<&ProviderEnv>,
) -> (CacheRetention, Option<Value>) {
    let retention = resolve_cache_retention(cache_retention, env);
    if retention == CacheRetention::None {
        return (retention, None);
    }
    let ttl = retention == CacheRetention::Long
        && get_anthropic_compat(model).supports_long_cache_retention;
    let mut control = Map::new();
    control.insert("type".to_string(), json!("ephemeral"));
    if ttl {
        control.insert("ttl".to_string(), json!("1h"));
    }
    (retention, Some(Value::Object(control)))
}

/// `normalizeToolCallId(id)` — Anthropic requires `^[a-zA-Z0-9_-]{1,64}$`.
pub fn normalize_tool_call_id(id: &str) -> String {
    id.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' || character == '-' {
                character
            } else {
                '_'
            }
        })
        .take(64)
        .collect()
}

/// `convertContentBlocks(content)` — text-only content collapses into one string.
fn convert_content_blocks(content: &[TextOrImageContent]) -> Value {
    let has_images = content
        .iter()
        .any(|block| matches!(block, TextOrImageContent::Image(_)));
    if !has_images {
        let joined = content
            .iter()
            .map(|block| match block {
                TextOrImageContent::Text(text) => text.text.as_str(),
                // TS reads `.text` off the image block too, which yields undefined and
                // joins as an empty segment; unreachable because of the guard above.
                TextOrImageContent::Image(_) => "",
            })
            .collect::<Vec<_>>()
            .join("\n");
        return json!(sanitize_surrogates(&joined));
    }

    let mut blocks: Vec<Value> = content
        .iter()
        .map(|block| match block {
            TextOrImageContent::Text(text) => {
                json!({"type": "text", "text": sanitize_surrogates(&text.text)})
            }
            TextOrImageContent::Image(image) => json!({
                "type": "image",
                "source": {"type": "base64", "media_type": image.mime_type, "data": image.data}
            }),
        })
        .collect();

    // Images without text get a placeholder text block.
    if !blocks
        .iter()
        .any(|block| block.get("type") == Some(&json!("text")))
    {
        blocks.insert(0, json!({"type": "text", "text": "(see attached image)"}));
    }
    Value::Array(blocks)
}

/// `convertToolResult(...)`
fn convert_tool_result(
    message: &ToolResultMessage,
    is_oauth_token: bool,
    deferred_tool_names: &BTreeSet<String>,
    loaded_tool_names: &mut BTreeSet<String>,
    normalize_tool_name: &dyn Fn(&str) -> String,
) -> (Value, Vec<Value>) {
    let mut references = Vec::new();
    for name in message.added_tool_names.iter().flatten() {
        let normalized_name = normalize_tool_name(name);
        if !deferred_tool_names.contains(&normalized_name)
            || loaded_tool_names.contains(&normalized_name)
        {
            continue;
        }
        loaded_tool_names.insert(normalized_name);
        references.push(json!({
            "type": "tool_reference",
            "tool_name": if is_oauth_token { to_claude_code_name(name) } else { name.clone() }
        }));
    }
    let converted_content = convert_content_blocks(&message.content);

    // Anthropic rejects tool references mixed with ordinary tool-result content.
    let tool_result = json!({
        "type": "tool_result",
        "tool_use_id": message.tool_call_id,
        "content": if references.is_empty() { converted_content.clone() } else { Value::Array(references.clone()) },
        "is_error": message.is_error
    });
    let sibling_content = if references.is_empty() {
        Vec::new()
    } else {
        match converted_content {
            Value::String(text) => vec![json!({"type": "text", "text": text})],
            Value::Array(blocks) => blocks,
            other => vec![other],
        }
    };
    (tool_result, sibling_content)
}

/// `convertMessages(...)`
pub fn convert_messages(
    transformed_messages: &[Message],
    is_oauth_token: bool,
    cache_control: Option<&Value>,
    allow_empty_signature: bool,
    deferred_tool_names: &BTreeSet<String>,
    normalize_tool_name: &dyn Fn(&str) -> String,
) -> Vec<Value> {
    let mut params: Vec<Value> = Vec::new();
    let mut loaded_tool_names: BTreeSet<String> = BTreeSet::new();

    let mut index = 0;
    while index < transformed_messages.len() {
        match &transformed_messages[index] {
            Message::User(message) => match &message.content {
                UserContent::Text(text) => {
                    if !text.trim().is_empty() {
                        params.push(json!({"role": "user", "content": sanitize_surrogates(text)}));
                    }
                }
                UserContent::Blocks(blocks) => {
                    let converted: Vec<Value> = blocks
                        .iter()
                        .filter_map(|block| match block {
                            TextOrImageContent::Text(text) => {
                                if text.text.trim().is_empty() {
                                    None
                                } else {
                                    Some(json!({"type": "text", "text": sanitize_surrogates(&text.text)}))
                                }
                            }
                            TextOrImageContent::Image(image) => Some(json!({
                                "type": "image",
                                "source": {"type": "base64", "media_type": image.mime_type, "data": image.data}
                            })),
                        })
                        .collect();
                    if !converted.is_empty() {
                        params.push(json!({"role": "user", "content": converted}));
                    }
                }
            },
            Message::Assistant(message) => {
                let mut blocks: Vec<Value> = Vec::new();
                for block in &message.content {
                    match block {
                        AssistantContent::Text(text) => {
                            if text.text.trim().is_empty() {
                                continue;
                            }
                            blocks.push(
                                json!({"type": "text", "text": sanitize_surrogates(&text.text)}),
                            );
                        }
                        AssistantContent::Thinking(thinking) => {
                            if thinking.redacted.unwrap_or(false) {
                                blocks.push(json!({
                                    "type": "redacted_thinking",
                                    "data": thinking.thinking_signature.clone().unwrap_or_default()
                                }));
                                continue;
                            }
                            let signature =
                                thinking.thinking_signature.as_deref().unwrap_or_default();
                            let has_signature = !signature.trim().is_empty();
                            if thinking.thinking.trim().is_empty() && !has_signature {
                                continue;
                            }
                            if !has_signature {
                                // Without a signature Anthropic rejects the block, so it
                                // becomes text unless the model tolerates empty ones.
                                blocks.push(if allow_empty_signature {
                                    json!({
                                        "type": "thinking",
                                        "thinking": sanitize_surrogates(&thinking.thinking),
                                        "signature": ""
                                    })
                                } else {
                                    json!({"type": "text", "text": sanitize_surrogates(&thinking.thinking)})
                                });
                            } else {
                                blocks.push(json!({
                                    "type": "thinking",
                                    "thinking": sanitize_surrogates(&thinking.thinking),
                                    "signature": signature
                                }));
                            }
                        }
                        AssistantContent::ToolCall(tool_call) => {
                            blocks.push(json!({
                                "type": "tool_use",
                                "id": tool_call.id,
                                "name": if is_oauth_token { to_claude_code_name(&tool_call.name) } else { tool_call.name.clone() },
                                "input": Value::Object(tool_call.arguments.clone())
                            }));
                        }
                    }
                }
                if !blocks.is_empty() {
                    params.push(json!({"role": "assistant", "content": blocks}));
                }
            }
            Message::ToolResult(_) => {
                // Consecutive tool results are collected, as the z.ai Anthropic endpoint needs.
                let mut tool_results = Vec::new();
                let mut sibling_content = Vec::new();
                let mut cursor = index;
                while let Some(Message::ToolResult(message)) = transformed_messages.get(cursor) {
                    let (tool_result, siblings) = convert_tool_result(
                        message,
                        is_oauth_token,
                        deferred_tool_names,
                        &mut loaded_tool_names,
                        normalize_tool_name,
                    );
                    tool_results.push(tool_result);
                    sibling_content.extend(siblings);
                    cursor += 1;
                }
                index = cursor - 1;
                // Displaced reference-bearing results must follow every tool_result block.
                tool_results.extend(sibling_content);
                params.push(json!({"role": "user", "content": tool_results}));
            }
        }
        index += 1;
    }

    // Cache the conversation history by marking the last user message.
    if let Some(cache_control) = cache_control
        && let Some(last_message) = params.last_mut()
        && last_message.get("role") == Some(&json!("user"))
    {
        match last_message.get_mut("content") {
            Some(Value::Array(blocks)) => {
                if let Some(last_block) = blocks.last_mut()
                    && matches!(
                        last_block.get("type").and_then(Value::as_str),
                        Some("text" | "image" | "tool_result")
                    )
                    && let Some(object) = last_block.as_object_mut()
                {
                    object.insert("cache_control".to_string(), cache_control.clone());
                }
            }
            Some(Value::String(text)) => {
                let text = text.clone();
                last_message["content"] =
                    json!([{"type": "text", "text": text, "cache_control": cache_control.clone()}]);
            }
            _ => {}
        }
    }

    params
}

/// `convertTools(...)`
pub fn convert_tools(
    tools: &[Tool],
    is_oauth_token: bool,
    supports_eager_tool_input_streaming: bool,
    supports_strict_tools: bool,
    cache_control: Option<&Value>,
    defer_loading: bool,
) -> Vec<Value> {
    tools
        .iter()
        .enumerate()
        .map(|(index, tool)| {
            let strict =
                resolve_json_schema_strict_sampling(tool, supports_strict_tools).unwrap_or(None);
            let parameters = get_json_schema_tool_parameters(tool, strict);
            let legacy_input_schema = json!({
                "type": "object",
                "properties": parameters.get("properties").cloned().unwrap_or_else(|| json!({})),
                "required": parameters.get("required").cloned().unwrap_or_else(|| json!([]))
            });
            let input_schema = if strict == Some(true) {
                let mut merged = parameters.as_object().cloned().unwrap_or_default();
                for (key, value) in legacy_input_schema.as_object().expect("object") {
                    merged.insert(key.clone(), value.clone());
                }
                Value::Object(merged)
            } else {
                legacy_input_schema
            };

            let mut entry = Map::new();
            entry.insert(
                "name".to_string(),
                json!(if is_oauth_token {
                    to_claude_code_name(&tool.name)
                } else {
                    tool.name.clone()
                }),
            );
            entry.insert("description".to_string(), json!(tool.description));
            if supports_eager_tool_input_streaming {
                entry.insert("eager_input_streaming".to_string(), json!(true));
            }
            if strict == Some(true) {
                entry.insert("strict".to_string(), json!(true));
            }
            entry.insert("input_schema".to_string(), input_schema);
            if defer_loading {
                entry.insert("defer_loading".to_string(), json!(true));
            }
            if let Some(cache_control) = cache_control
                && index == tools.len() - 1
            {
                entry.insert("cache_control".to_string(), cache_control.clone());
            }
            Value::Object(entry)
        })
        .collect()
}

/// `shouldUseFineGrainedToolStreamingBeta(model, context)`
pub fn should_use_fine_grained_tool_streaming_beta(model: &Model, context: &Context) -> bool {
    context
        .tools
        .as_ref()
        .is_some_and(|tools| !tools.is_empty())
        && !get_anthropic_compat(model).supports_eager_tool_input_streaming
}

/// Anthropic-specific request options (`AnthropicOptions`).
#[derive(Debug, Clone, Default)]
pub struct AnthropicOptions {
    pub thinking_enabled: Option<bool>,
    pub thinking_budget_tokens: Option<u64>,
    pub effort: Option<AnthropicEffort>,
    pub thinking_display: Option<AnthropicThinkingDisplay>,
    /// Default: true.
    pub interleaved_thinking: Option<bool>,
    pub tool_choice: Option<AnthropicToolChoice>,
    pub max_tokens: Option<u64>,
    pub temperature: Option<f64>,
    pub cache_retention: Option<CacheRetention>,
    /// `StreamOptions.sessionId` — providers with `sendSessionAffinityHeaders` pin the
    /// prompt cache to it.
    pub session_id: Option<String>,
    pub metadata: Option<Map<String, Value>>,
    pub env: Option<ProviderEnv>,
}

/// `AnthropicEffort`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnthropicEffort {
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl AnthropicEffort {
    pub fn as_str(self) -> &'static str {
        match self {
            AnthropicEffort::Low => "low",
            AnthropicEffort::Medium => "medium",
            AnthropicEffort::High => "high",
            AnthropicEffort::Xhigh => "xhigh",
            AnthropicEffort::Max => "max",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "low" => Some(AnthropicEffort::Low),
            "medium" => Some(AnthropicEffort::Medium),
            "high" => Some(AnthropicEffort::High),
            "xhigh" => Some(AnthropicEffort::Xhigh),
            "max" => Some(AnthropicEffort::Max),
            _ => None,
        }
    }
}

/// `AnthropicThinkingDisplay`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnthropicThinkingDisplay {
    Summarized,
    Omitted,
}

impl AnthropicThinkingDisplay {
    fn as_str(self) -> &'static str {
        match self {
            AnthropicThinkingDisplay::Summarized => "summarized",
            AnthropicThinkingDisplay::Omitted => "omitted",
        }
    }
}

/// `toolChoice`
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnthropicToolChoice {
    Auto,
    Any,
    None,
    Tool { name: String },
}

/// `mapThinkingLevelToEffort(model, level)`
pub fn map_thinking_level_to_effort(
    model: &Model,
    level: Option<crate::types::ThinkingLevel>,
) -> AnthropicEffort {
    if let Some(level) = level
        && let Some(Some(mapped)) = model
            .thinking_level_map
            .as_ref()
            .and_then(|map| map.get(&crate::types::ModelThinkingLevel::from(level)))
        && let Some(effort) = AnthropicEffort::parse(mapped)
    {
        return effort;
    }
    match level {
        Some(crate::types::ThinkingLevel::Minimal | crate::types::ThinkingLevel::Low) => {
            AnthropicEffort::Low
        }
        Some(crate::types::ThinkingLevel::Medium) => AnthropicEffort::Medium,
        _ => AnthropicEffort::High,
    }
}

/// `buildParams(model, context, isOAuthToken, options)`
pub fn build_params(
    model: &Model,
    context: &Context,
    is_oauth_token: bool,
    options: &AnthropicOptions,
    timestamp: i64,
) -> Value {
    let (_, cache_control) =
        get_cache_control(model, options.cache_retention, options.env.as_ref());
    let compat = get_anthropic_compat(model);
    let transformed_messages = transform_messages(
        &context.messages,
        model,
        Some(&|id: &str, _source: &crate::types::AssistantMessage| normalize_tool_call_id(id)),
        timestamp,
    );

    let normalize_tool_name: Box<dyn Fn(&str) -> String> = if is_oauth_token {
        Box::new(to_claude_code_name)
    } else {
        Box::new(|name: &str| name.to_string())
    };

    let tool_context = Context {
        system_prompt: context.system_prompt.clone(),
        messages: transformed_messages.clone(),
        tools: context.tools.clone(),
    };
    let placement = split_deferred_tools(&tool_context, compat.supports_tool_references, |name| {
        normalize_tool_name(name)
    });
    let mut immediate_tools = placement.immediate;
    let mut deferred_tools: Vec<Tool> = placement.deferred.into_values().collect();
    if immediate_tools.is_empty() && !deferred_tools.is_empty() {
        immediate_tools = deferred_tools;
        deferred_tools = Vec::new();
    }
    let deferred_tool_names: BTreeSet<String> = deferred_tools
        .iter()
        .map(|tool| normalize_tool_name(&tool.name))
        .collect();

    let mut params = Map::new();
    params.insert("model".to_string(), json!(model.id));
    params.insert(
        "messages".to_string(),
        Value::Array(convert_messages(
            &transformed_messages,
            is_oauth_token,
            cache_control.as_ref(),
            compat.allow_empty_signature,
            &deferred_tool_names,
            &normalize_tool_name,
        )),
    );
    params.insert(
        "max_tokens".to_string(),
        json!(options.max_tokens.unwrap_or(model.max_tokens)),
    );
    params.insert("stream".to_string(), json!(true));

    // OAuth requests must carry the Claude Code identity as the first system block.
    let system_block = |text: &str| {
        let mut block = Map::new();
        block.insert("type".to_string(), json!("text"));
        block.insert("text".to_string(), json!(text));
        if let Some(cache_control) = &cache_control {
            block.insert("cache_control".to_string(), cache_control.clone());
        }
        Value::Object(block)
    };
    if is_oauth_token {
        let mut system = vec![system_block(
            "You are Claude Code, Anthropic's official CLI for Claude.",
        )];
        if let Some(system_prompt) = &context.system_prompt {
            system.push(system_block(&sanitize_surrogates(system_prompt)));
        }
        params.insert("system".to_string(), Value::Array(system));
    } else if let Some(system_prompt) = &context.system_prompt {
        params.insert(
            "system".to_string(),
            json!([system_block(&sanitize_surrogates(system_prompt))]),
        );
    }

    // Temperature is incompatible with extended thinking and rejected by Opus 4.7+.
    if let Some(temperature) = options.temperature
        && !options.thinking_enabled.unwrap_or(false)
        && compat.supports_temperature
    {
        params.insert("temperature".to_string(), json!(temperature));
    }

    if !immediate_tools.is_empty() || !deferred_tools.is_empty() {
        let mut tools = convert_tools(
            &immediate_tools,
            is_oauth_token,
            compat.supports_eager_tool_input_streaming,
            compat.supports_strict_tools,
            if compat.supports_cache_control_on_tools {
                cache_control.as_ref()
            } else {
                None
            },
            false,
        );
        tools.extend(convert_tools(
            &deferred_tools,
            is_oauth_token,
            compat.supports_eager_tool_input_streaming,
            compat.supports_strict_tools,
            None,
            true,
        ));
        params.insert("tools".to_string(), Value::Array(tools));
    }

    if model.reasoning {
        if options.thinking_enabled.unwrap_or(false) {
            let display = options
                .thinking_display
                .unwrap_or(AnthropicThinkingDisplay::Summarized);
            if force_adaptive_thinking(model) {
                params.insert(
                    "thinking".to_string(),
                    json!({"type": "adaptive", "display": display.as_str()}),
                );
                if let Some(effort) = options.effort {
                    params.insert(
                        "output_config".to_string(),
                        json!({"effort": effort.as_str()}),
                    );
                }
            } else {
                params.insert(
                    "thinking".to_string(),
                    json!({
                        "type": "enabled",
                        "budget_tokens": options.thinking_budget_tokens.filter(|budget| *budget != 0).unwrap_or(1024),
                        "display": display.as_str()
                    }),
                );
            }
        } else if options.thinking_enabled == Some(false)
            && model
                .thinking_level_map
                .as_ref()
                .and_then(|map| map.get(&crate::types::ModelThinkingLevel::Off))
                != Some(&None)
        {
            params.insert("thinking".to_string(), json!({"type": "disabled"}));
        }
    }

    if let Some(metadata) = &options.metadata
        && let Some(user_id) = metadata.get("user_id").and_then(Value::as_str)
    {
        params.insert("metadata".to_string(), json!({"user_id": user_id}));
    }

    if let Some(tool_choice) = &options.tool_choice {
        params.insert(
            "tool_choice".to_string(),
            match tool_choice {
                AnthropicToolChoice::Auto => json!({"type": "auto"}),
                AnthropicToolChoice::Any => json!({"type": "any"}),
                AnthropicToolChoice::None => json!({"type": "none"}),
                AnthropicToolChoice::Tool { name } => json!({"type": "tool", "name": name}),
            },
        );
    }

    Value::Object(params)
}

/// The default headers the TS code hands to the SDK client, per auth mode.
pub fn build_default_headers(
    model: &Model,
    api_key: Option<&str>,
    interleaved_thinking: bool,
    use_fine_grained_tool_streaming_beta: bool,
    options_headers: Option<&ProviderHeaders>,
    dynamic_headers: Option<&BTreeMap<String, String>>,
    session_id: Option<&str>,
) -> (BTreeMap<String, String>, bool) {
    // Adaptive-thinking models have interleaved thinking built in.
    let needs_interleaved_beta = interleaved_thinking && !force_adaptive_thinking(model);
    let mut beta_features: Vec<&str> = Vec::new();
    if use_fine_grained_tool_streaming_beta {
        beta_features.push(FINE_GRAINED_TOOL_STREAMING_BETA);
    }
    if needs_interleaved_beta {
        beta_features.push(INTERLEAVED_THINKING_BETA);
    }

    let mut headers: BTreeMap<String, String> = BTreeMap::new();
    headers.insert("accept".to_string(), "application/json".to_string());
    headers.insert(
        "anthropic-dangerous-direct-browser-access".to_string(),
        "true".to_string(),
    );

    let is_oauth = api_key.is_some_and(is_oauth_token) && model.provider != "github-copilot";
    if model.provider == "github-copilot" {
        if !beta_features.is_empty() {
            headers.insert("anthropic-beta".to_string(), beta_features.join(","));
        }
    } else if is_oauth {
        let mut betas = vec!["claude-code-20250219", "oauth-2025-04-20"];
        betas.extend(beta_features.iter().copied());
        headers.insert("anthropic-beta".to_string(), betas.join(","));
        headers.insert(
            "user-agent".to_string(),
            format!("claude-cli/{CLAUDE_CODE_VERSION}"),
        );
        headers.insert("x-app".to_string(), "cli".to_string());
    } else {
        if !beta_features.is_empty() {
            headers.insert("anthropic-beta".to_string(), beta_features.join(","));
        }
        if let Some(session_id) = session_id
            && get_anthropic_compat(model).send_session_affinity_headers
        {
            headers.insert("x-session-affinity".to_string(), session_id.to_string());
        }
    }

    for (name, value) in model.headers.iter().flatten() {
        headers.insert(name.clone(), value.clone());
    }
    for (name, value) in dynamic_headers.into_iter().flatten() {
        headers.insert(name.clone(), value.clone());
    }
    for (name, value) in options_headers.iter().copied().flatten() {
        match value {
            Some(value) => {
                headers.insert(name.clone(), value.clone());
            }
            // A null value suppresses a default header of the same name.
            None => {
                headers.remove(name);
            }
        }
    }

    (headers, is_oauth)
}
