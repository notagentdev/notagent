//! Amazon Bedrock Converse-Stream adapter.
//!
//! 1:1 port of `packages/ai/src/api/bedrock-converse-stream.ts`. The request is built as
//! the `ConverseStreamCommand` input the TS code hands to the AWS SDK — that object is
//! what `onPayload` sees, so it is the contract the fixtures pin down. The transport maps
//! it onto `aws-sdk-bedrockruntime` (master-plan substitution class 3).

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde_json::{Map, Value, json};

use crate::api::constrained_sampling::{
    ConstrainedSamplingError, get_json_schema_tool_parameters, resolve_json_schema_strict_sampling,
};
use crate::api::transform_messages::transform_messages;
use crate::models::calculate_cost;
use crate::types::{
    AssistantContent, AssistantMessage, AssistantMessageEvent, CacheRetention, Context, Message,
    Model, ProviderEnv, SimpleStreamOptions, StopReason, TextContent, TextOrImageContent,
    ThinkingBudgets, ThinkingContent, ThinkingLevel, Tool, ToolCall, UserContent,
};
use crate::utils::event_stream::AssistantMessageEventStream;
use crate::utils::json_parse::parse_streaming_json;
use crate::utils::provider_env::get_provider_env_value;
use crate::utils::sanitize_unicode::sanitize_surrogates;

const EMPTY_TEXT_PLACEHOLDER: &str = "<empty>";

/// `thinkingDisplay?: "summarized" | "omitted"`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BedrockThinkingDisplay {
    Summarized,
    Omitted,
}

impl BedrockThinkingDisplay {
    fn as_str(self) -> &'static str {
        match self {
            BedrockThinkingDisplay::Summarized => "summarized",
            BedrockThinkingDisplay::Omitted => "omitted",
        }
    }
}

/// `toolChoice?: "auto" | "any" | "none" | { type: "tool", name }`
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BedrockToolChoice {
    Auto,
    Any,
    None,
    Tool { name: String },
}

/// `BedrockOptions extends StreamOptions`
#[derive(Debug, Clone, Default)]
pub struct BedrockOptions {
    pub region: Option<String>,
    pub profile: Option<String>,
    pub tool_choice: Option<BedrockToolChoice>,
    pub reasoning: Option<ThinkingLevel>,
    pub thinking_budgets: Option<ThinkingBudgets>,
    /// Default `true`; only budget-based Claude models use it.
    pub interleaved_thinking: Option<bool>,
    pub thinking_display: Option<BedrockThinkingDisplay>,
    pub request_metadata: Option<BTreeMap<String, String>>,
    pub bearer_token: Option<String>,
    pub api_key: Option<String>,
    pub max_tokens: Option<u64>,
    pub temperature: Option<f64>,
    pub cache_retention: Option<CacheRetention>,
    pub env: Option<ProviderEnv>,
}

/// Error of this adapter.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct BedrockError(pub String);

// ---------------------------------------------------------------------------
// Model predicates
// ---------------------------------------------------------------------------

/// `getModelMatchCandidates(modelId, modelName)`
fn model_match_candidates(model_id: &str, model_name: &str) -> Vec<String> {
    let values: Vec<&str> = if model_name.is_empty() {
        vec![model_id]
    } else {
        vec![model_id, model_name]
    };
    let separators = regex::Regex::new(r"[\s_.:]+").expect("valid pattern");
    values
        .into_iter()
        .flat_map(|value| {
            let lower = value.to_lowercase();
            let dashed = separators.replace_all(&lower, "-").to_string();
            [lower, dashed]
        })
        .collect()
}

/// `supportsAdaptiveThinking(modelId, modelName)` — Opus 4.6+ and Sonnet 4.6+.
pub fn supports_adaptive_thinking(model_id: &str, model_name: &str) -> bool {
    model_match_candidates(model_id, model_name)
        .iter()
        .any(|candidate| {
            [
                "opus-4-6",
                "opus-4-7",
                "opus-4-8",
                "opus-5",
                "sonnet-4-6",
                "sonnet-5",
                "fable-5",
            ]
            .iter()
            .any(|marker| candidate.contains(marker))
        })
}

/// `supportsNativeXhighEffort(model)`
pub fn supports_native_xhigh_effort(model_id: &str, model_name: &str) -> bool {
    model_match_candidates(model_id, model_name)
        .iter()
        .any(|candidate| {
            ["opus-4-7", "opus-4-8", "opus-5", "sonnet-5", "fable-5"]
                .iter()
                .any(|marker| candidate.contains(marker))
        })
}

/// `isAnthropicClaudeModel(model)`
pub fn is_anthropic_claude_model(model_id: &str, model_name: &str) -> bool {
    let id = model_id.to_lowercase();
    let name = model_name.to_lowercase();
    id.contains("anthropic.claude")
        || id.contains("anthropic/claude")
        || name.contains("anthropic.claude")
        || name.contains("anthropic/claude")
        || name.contains("claude")
}

/// `supportsThinkingSignature(model)` — only Claude accepts the signature field.
fn supports_thinking_signature(model: &Model) -> bool {
    is_anthropic_claude_model(&model.id, &model.name)
}

/// `supportsPromptCaching(model, env)`
pub fn supports_prompt_caching(model: &Model, env: Option<&ProviderEnv>) -> bool {
    let candidates = model_match_candidates(&model.id, &model.name);
    let has_claude_ref = candidates
        .iter()
        .any(|candidate| candidate.contains("claude"));
    if !has_claude_ref {
        // Application inference profiles carry no model name in the ARN, so this is the
        // documented escape hatch.
        return get_provider_env_value("AWS_BEDROCK_FORCE_CACHE", env).as_deref() == Some("1");
    }
    let any = |markers: &[&str]| {
        candidates
            .iter()
            .any(|candidate| markers.iter().any(|marker| candidate.contains(marker)))
    };
    // Claude 5, Claude 4.x, Claude 3.7 Sonnet, Claude 3.5 Haiku.
    any(&["fable-5", "opus-5", "sonnet-5"])
        || any(&["-4-"])
        || any(&["claude-3-7-sonnet"])
        || any(&["claude-3-5-haiku"])
}

/// `isGovCloudBedrockTarget(model, options)`
pub fn is_gov_cloud_bedrock_target(model_id: &str, region: Option<&str>) -> bool {
    if region.is_some_and(|region| region.to_lowercase().starts_with("us-gov-")) {
        return true;
    }
    let model_id = model_id.to_lowercase();
    model_id.starts_with("us-gov.") || model_id.starts_with("arn:aws-us-gov:")
}

// ---------------------------------------------------------------------------
// Region, endpoint and credentials
// ---------------------------------------------------------------------------

/// `getConfiguredBedrockRegion(options)`
pub fn configured_region(options: &BedrockOptions) -> Option<String> {
    options
        .region
        .clone()
        .filter(|region| !region.is_empty())
        .or_else(|| {
            get_provider_env_value("AWS_REGION", options.env.as_ref())
                .filter(|value| !value.is_empty())
        })
        .or_else(|| {
            get_provider_env_value("AWS_DEFAULT_REGION", options.env.as_ref())
                .filter(|value| !value.is_empty())
        })
}

/// `getStandardBedrockEndpointRegion(baseUrl)`
pub fn standard_endpoint_region(base_url: &str) -> Option<String> {
    if base_url.is_empty() {
        return None;
    }
    let url = url::Url::parse(base_url).ok()?;
    let host = url.host_str()?.to_lowercase();
    static PATTERN: OnceLock<regex::Regex> = OnceLock::new();
    let pattern = PATTERN.get_or_init(|| {
        regex::Regex::new(r"^bedrock-runtime(?:-fips)?\.([a-z0-9-]+)\.amazonaws\.com(?:\.cn)?$")
            .expect("valid pattern")
    });
    Some(pattern.captures(&host)?.get(1)?.as_str().to_string())
}

/// `shouldUseExplicitBedrockEndpoint(baseUrl, configuredRegion, hasAmbientConfiguredProfile)`
pub fn should_use_explicit_endpoint(
    base_url: &str,
    configured_region: Option<&str>,
    has_ambient_profile: bool,
) -> bool {
    if standard_endpoint_region(base_url).is_none() {
        return true;
    }
    configured_region.is_none() && !has_ambient_profile
}

/// `getConfiguredBedrockCredentials(env)`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BedrockCredentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: Option<String>,
}

pub fn configured_credentials(env: Option<&ProviderEnv>) -> Option<BedrockCredentials> {
    let access_key_id =
        get_provider_env_value("AWS_ACCESS_KEY_ID", env).filter(|value| !value.is_empty())?;
    let secret_access_key =
        get_provider_env_value("AWS_SECRET_ACCESS_KEY", env).filter(|value| !value.is_empty())?;
    Some(BedrockCredentials {
        access_key_id,
        secret_access_key,
        session_token: get_provider_env_value("AWS_SESSION_TOKEN", env)
            .filter(|value| !value.is_empty()),
    })
}

/// The client configuration the adapter derives, mirroring the TS `config` object.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BedrockClientConfig {
    pub profile: Option<String>,
    pub region: Option<String>,
    pub endpoint: Option<String>,
    pub credentials: Option<BedrockCredentials>,
    pub bearer_token: Option<String>,
    pub force_http1: bool,
    pub proxy_url: Option<String>,
}

/// The `config` block of `stream`, without the Node-only handler wiring.
pub fn build_client_config(model: &Model, options: &BedrockOptions) -> BedrockClientConfig {
    // A profile configured through the auth flow must beat ambient access keys.
    let options_profile = options
        .profile
        .clone()
        .filter(|profile| !profile.is_empty())
        .or_else(|| {
            options
                .env
                .as_ref()
                .and_then(|env| env.get("AWS_PROFILE").cloned())
                .filter(|value| !value.is_empty())
        });
    let mut config = BedrockClientConfig {
        profile: options_profile.clone().or_else(|| {
            get_provider_env_value("AWS_PROFILE", options.env.as_ref())
                .filter(|value| !value.is_empty())
        }),
        ..BedrockClientConfig::default()
    };

    let region = configured_region(options);
    // `getProviderEnvValue("AWS_PROFILE")` without the scoped env: the ambient process.
    let has_ambient_profile =
        get_provider_env_value("AWS_PROFILE", None).is_some_and(|value| !value.is_empty());
    let endpoint_region = standard_endpoint_region(&model.base_url);
    let use_explicit_endpoint =
        should_use_explicit_endpoint(&model.base_url, region.as_deref(), has_ambient_profile);

    // Only pin standard runtime endpoints when neither a region nor an ambient profile is
    // configured, so custom VPC/proxy endpoints survive without overriding AWS_REGION.
    if use_explicit_endpoint && !model.base_url.is_empty() {
        config.endpoint = Some(model.base_url.clone());
    }

    let skip_auth = get_provider_env_value("AWS_BEDROCK_SKIP_AUTH", options.env.as_ref())
        .as_deref()
        == Some("1");
    let bearer_token = options
        .bearer_token
        .clone()
        .filter(|token| !token.is_empty())
        .or_else(|| options.api_key.clone().filter(|token| !token.is_empty()))
        .or_else(|| {
            get_provider_env_value("AWS_BEARER_TOKEN_BEDROCK", options.env.as_ref())
                .filter(|value| !value.is_empty())
        });

    // Region resolution: ARN-embedded > explicit option > env > the SDK default chain.
    static ARN_PATTERN: OnceLock<regex::Regex> = OnceLock::new();
    let arn_pattern = ARN_PATTERN.get_or_init(|| {
        regex::Regex::new(r"^arn:aws(?:-[a-z0-9-]+)?:bedrock:([a-z0-9-]+):").expect("valid pattern")
    });
    if let Some(captures) = arn_pattern.captures(&model.id) {
        config.region = Some(captures[1].to_string());
    } else if let Some(region) = region {
        config.region = Some(region);
    } else if let Some(endpoint_region) = endpoint_region.filter(|_| use_explicit_endpoint) {
        config.region = Some(endpoint_region);
    } else if !has_ambient_profile {
        config.region = Some("us-east-1".to_string());
    }

    if skip_auth {
        config.credentials = Some(BedrockCredentials {
            access_key_id: "dummy-access-key".to_string(),
            secret_access_key: "dummy-secret-key".to_string(),
            session_token: None,
        });
    }
    if !skip_auth
        && options_profile.is_none()
        && let Some(credentials) = configured_credentials(options.env.as_ref())
    {
        config.credentials = Some(credentials);
    }

    // A malformed proxy URL is not fatal in TS either; the SDK then goes direct.
    config.proxy_url = crate::utils::node_http_proxy::resolve_http_proxy_url_for_target(
        &model.base_url,
        options.env.as_ref(),
    )
    .unwrap_or(None);
    config.force_http1 = config.proxy_url.is_none()
        && get_provider_env_value("AWS_BEDROCK_FORCE_HTTP1", options.env.as_ref()).as_deref()
            == Some("1");

    if bearer_token.is_some() && !skip_auth {
        config.bearer_token = bearer_token;
    }
    config
}

/// Header keys the caller may not overwrite: `x-amz-*` and `host` sign the request,
/// `authorization` belongs to SigV4 or the bearer-token path.
pub fn is_reserved_header(key: &str) -> bool {
    let lower = key.to_lowercase();
    lower.starts_with("x-amz-") || lower == "authorization" || lower == "host"
}

// ---------------------------------------------------------------------------
// Request building
// ---------------------------------------------------------------------------

/// `resolveCacheRetention(cacheRetention, env)`
fn resolve_cache_retention(
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

fn cache_point(cache_retention: CacheRetention) -> Value {
    let mut point = Map::new();
    point.insert("type".to_string(), json!("default"));
    if cache_retention == CacheRetention::Long {
        point.insert("ttl".to_string(), json!("1h"));
    }
    json!({ "cachePoint": point })
}

/// `createNonBlankTextBlock(text)`
fn non_blank_text_block(text: &str) -> Option<Value> {
    let sanitized = sanitize_surrogates(text);
    (!sanitized.trim().is_empty()).then(|| json!({ "text": sanitized }))
}

/// `createRequiredTextBlock(text)`
fn required_text_block(text: &str) -> Value {
    non_blank_text_block(text).unwrap_or_else(|| json!({ "text": EMPTY_TEXT_PLACEHOLDER }))
}

/// `createImageBlock(mimeType, data)`
fn image_block(mime_type: &str, data: &str) -> Result<Value, BedrockError> {
    let format = match mime_type {
        "image/jpeg" | "image/jpg" => "jpeg",
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        other => return Err(BedrockError(format!("Unknown image type: {other}"))),
    };
    // The SDK takes raw bytes; the fixture encodes them back to base64 for comparison.
    Ok(json!({ "source": { "bytes": { "__bytes__": data } }, "format": format }))
}

/// `sanitizeBedrockDocument(value)` — empty keys are not valid document members.
fn sanitize_bedrock_document(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(sanitize_bedrock_document).collect()),
        Value::Object(object) => Value::Object(
            object
                .iter()
                .filter(|(key, _)| !key.is_empty())
                .map(|(key, value)| (key.clone(), sanitize_bedrock_document(value)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// `convertToolResultContent(content)`
fn convert_tool_result_content(content: &[TextOrImageContent]) -> Result<Vec<Value>, BedrockError> {
    let mut result = Vec::new();
    for block in content {
        match block {
            TextOrImageContent::Image(image) => {
                result.push(json!({ "image": image_block(&image.mime_type, &image.data)? }));
            }
            TextOrImageContent::Text(text) => {
                if let Some(block) = non_blank_text_block(&text.text) {
                    result.push(block);
                }
            }
        }
    }
    if result.is_empty() {
        result.push(json!({ "text": EMPTY_TEXT_PLACEHOLDER }));
    }
    Ok(result)
}

/// `buildSystemPrompt(systemPrompt, model, cacheRetention, env)`
fn build_system_prompt(
    system_prompt: Option<&str>,
    model: &Model,
    cache_retention: CacheRetention,
    env: Option<&ProviderEnv>,
) -> Option<Vec<Value>> {
    let system_prompt = system_prompt.filter(|prompt| !prompt.is_empty())?;
    let mut blocks = vec![json!({ "text": sanitize_surrogates(system_prompt) })];
    if cache_retention != CacheRetention::None && supports_prompt_caching(model, env) {
        blocks.push(cache_point(cache_retention));
    }
    Some(blocks)
}

/// `normalizeToolCallId(id)`
fn normalize_tool_call_id(id: &str) -> String {
    let sanitized: String = id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' || character == '-' {
                character
            } else {
                '_'
            }
        })
        .collect();
    sanitized.chars().take(64).collect()
}

/// `convertMessages(context, model, cacheRetention, env)`
pub fn convert_messages(
    context: &Context,
    model: &Model,
    cache_retention: CacheRetention,
    env: Option<&ProviderEnv>,
    timestamp: i64,
) -> Result<Vec<Value>, BedrockError> {
    let mut result: Vec<Value> = Vec::new();
    let transformed_messages = transform_messages(
        &context.messages,
        model,
        Some(&|id: &str, _source: &AssistantMessage| normalize_tool_call_id(id)),
        timestamp,
    );

    let mut index = 0usize;
    while index < transformed_messages.len() {
        match &transformed_messages[index] {
            Message::User(user) => {
                let mut content: Vec<Value> = Vec::new();
                match &user.content {
                    UserContent::Text(text) => content.push(required_text_block(text)),
                    UserContent::Blocks(blocks) => {
                        for block in blocks {
                            match block {
                                TextOrImageContent::Text(text) => {
                                    if let Some(block) = non_blank_text_block(&text.text) {
                                        content.push(block);
                                    }
                                }
                                TextOrImageContent::Image(image) => {
                                    content.push(
                                        json!({ "image": image_block(&image.mime_type, &image.data)? }),
                                    );
                                }
                            }
                        }
                        if content.is_empty() {
                            content.push(json!({ "text": EMPTY_TEXT_PLACEHOLDER }));
                        }
                    }
                }
                result.push(json!({ "role": "user", "content": content }));
            }
            Message::Assistant(assistant) => {
                // Bedrock rejects an empty content array, which an aborted turn produces.
                if assistant.content.is_empty() {
                    index += 1;
                    continue;
                }
                let mut content_blocks: Vec<Value> = Vec::new();
                for block in &assistant.content {
                    match block {
                        AssistantContent::Text(text) => {
                            if let Some(block) = non_blank_text_block(&text.text) {
                                content_blocks.push(block);
                            }
                        }
                        AssistantContent::ToolCall(tool_call) => {
                            content_blocks.push(json!({
                                "toolUse": {
                                    "toolUseId": tool_call.id,
                                    "name": tool_call.name,
                                    "input": sanitize_bedrock_document(&Value::Object(
                                        tool_call.arguments.clone()
                                    )),
                                },
                            }));
                        }
                        AssistantContent::Thinking(thinking) => {
                            let thinking_text = sanitize_surrogates(&thinking.thinking);
                            if thinking_text.trim().is_empty() {
                                continue;
                            }
                            if supports_thinking_signature(model) {
                                // Signatures arrive after the thinking deltas; a replay
                                // without one is rejected, so it falls back to plain text
                                // exactly as the Anthropic adapter does.
                                match thinking
                                    .thinking_signature
                                    .as_deref()
                                    .filter(|signature| !signature.trim().is_empty())
                                {
                                    None => content_blocks.push(json!({ "text": thinking_text })),
                                    Some(signature) => content_blocks.push(json!({
                                        "reasoningContent": {
                                            "reasoningText": {
                                                "text": thinking_text,
                                                "signature": signature,
                                            },
                                        },
                                    })),
                                }
                            } else {
                                content_blocks.push(json!({
                                    "reasoningContent": { "reasoningText": { "text": thinking_text } },
                                }));
                            }
                        }
                    }
                }
                if content_blocks.is_empty() {
                    index += 1;
                    continue;
                }
                result.push(json!({ "role": "assistant", "content": content_blocks }));
            }
            Message::ToolResult(_) => {
                // Bedrock wants every tool result of a turn in one user message.
                let mut tool_results: Vec<Value> = Vec::new();
                let mut scan = index;
                while scan < transformed_messages.len() {
                    let Message::ToolResult(tool_result) = &transformed_messages[scan] else {
                        break;
                    };
                    tool_results.push(json!({
                        "toolResult": {
                            "toolUseId": tool_result.tool_call_id,
                            "content": convert_tool_result_content(&tool_result.content)?,
                            "status": if tool_result.is_error { "error" } else { "success" },
                        },
                    }));
                    scan += 1;
                }
                index = scan - 1;
                result.push(json!({ "role": "user", "content": tool_results }));
            }
        }
        index += 1;
    }

    // The cache point goes on the last user message.
    if cache_retention != CacheRetention::None
        && supports_prompt_caching(model, env)
        && let Some(last) = result.last_mut()
        && last.get("role") == Some(&json!("user"))
        && let Some(content) = last.get_mut("content").and_then(Value::as_array_mut)
    {
        content.push(cache_point(cache_retention));
    }

    Ok(result)
}

/// `convertToolConfig(tools, toolChoice, supportsStrictMode)`
pub fn convert_tool_config(
    tools: Option<&[Tool]>,
    tool_choice: Option<&BedrockToolChoice>,
    supports_strict_mode: bool,
) -> Result<Option<Value>, ConstrainedSamplingError> {
    let Some(tools) = tools.filter(|tools| !tools.is_empty()) else {
        return Ok(None);
    };
    if tool_choice == Some(&BedrockToolChoice::None) {
        return Ok(None);
    }

    let mut bedrock_tools = Vec::with_capacity(tools.len());
    for tool in tools {
        let strict = resolve_json_schema_strict_sampling(tool, supports_strict_mode)?;
        let mut spec = Map::new();
        spec.insert("name".to_string(), json!(tool.name));
        spec.insert("description".to_string(), json!(tool.description));
        spec.insert(
            "inputSchema".to_string(),
            json!({ "json": get_json_schema_tool_parameters(tool, strict) }),
        );
        if strict == Some(true) {
            spec.insert("strict".to_string(), json!(true));
        }
        bedrock_tools.push(json!({ "toolSpec": spec }));
    }

    let mut config = Map::new();
    config.insert("tools".to_string(), Value::Array(bedrock_tools));
    let choice = match tool_choice {
        Some(BedrockToolChoice::Auto) => Some(json!({ "auto": {} })),
        Some(BedrockToolChoice::Any) => Some(json!({ "any": {} })),
        Some(BedrockToolChoice::Tool { name }) => Some(json!({ "tool": { "name": name } })),
        _ => None,
    };
    if let Some(choice) = choice {
        config.insert("toolChoice".to_string(), choice);
    }
    Ok(Some(Value::Object(config)))
}

/// `mapThinkingLevelToEffort(model, level)`
fn map_thinking_level_to_effort(model: &Model, level: ThinkingLevel) -> String {
    if level == ThinkingLevel::Xhigh && supports_native_xhigh_effort(&model.id, &model.name) {
        return "xhigh".to_string();
    }
    if let Some(Some(mapped)) = model
        .thinking_level_map
        .as_ref()
        .and_then(|map| map.get(&level.into()))
    {
        return mapped.clone();
    }
    match level {
        ThinkingLevel::Minimal | ThinkingLevel::Low => "low",
        ThinkingLevel::Medium => "medium",
        ThinkingLevel::High => "high",
        _ => "high",
    }
    .to_string()
}

/// `buildAdditionalModelRequestFields(model, options)`
pub fn build_additional_model_request_fields(
    model: &Model,
    options: &BedrockOptions,
) -> Option<Value> {
    let reasoning = options.reasoning?;
    if !model.reasoning {
        return None;
    }
    if !is_anthropic_claude_model(&model.id, &model.name) {
        return None;
    }

    // GovCloud rejects the Claude thinking.display field, so it is omitted there.
    let display = if is_gov_cloud_bedrock_target(&model.id, configured_region(options).as_deref()) {
        None
    } else {
        Some(
            options
                .thinking_display
                .unwrap_or(BedrockThinkingDisplay::Summarized)
                .as_str(),
        )
    };

    let adaptive = supports_adaptive_thinking(&model.id, &model.name);
    let mut result = Map::new();
    if adaptive {
        let mut thinking = Map::new();
        thinking.insert("type".to_string(), json!("adaptive"));
        if let Some(display) = display {
            thinking.insert("display".to_string(), json!(display));
        }
        result.insert("thinking".to_string(), Value::Object(thinking));
        result.insert(
            "output_config".to_string(),
            json!({ "effort": map_thinking_level_to_effort(model, reasoning) }),
        );
    } else {
        // Budget-based Claude clamps the extended levels to high.
        let default_budget = match reasoning {
            ThinkingLevel::Minimal => 1024,
            ThinkingLevel::Low => 2048,
            ThinkingLevel::Medium => 8192,
            _ => 16384,
        };
        let level = match reasoning {
            ThinkingLevel::Xhigh | ThinkingLevel::Max => ThinkingLevel::High,
            other => other,
        };
        let budget = options
            .thinking_budgets
            .and_then(|budgets| match level {
                ThinkingLevel::Minimal => budgets.minimal,
                ThinkingLevel::Low => budgets.low,
                ThinkingLevel::Medium => budgets.medium,
                _ => budgets.high,
            })
            .unwrap_or(default_budget);
        let mut thinking = Map::new();
        thinking.insert("type".to_string(), json!("enabled"));
        thinking.insert("budget_tokens".to_string(), json!(budget));
        if let Some(display) = display {
            thinking.insert("display".to_string(), json!(display));
        }
        result.insert("thinking".to_string(), Value::Object(thinking));
    }

    if !adaptive && options.interleaved_thinking.unwrap_or(true) {
        result.insert(
            "anthropic_beta".to_string(),
            json!(["interleaved-thinking-2025-05-14"]),
        );
    }
    Some(Value::Object(result))
}

/// The `ConverseStreamCommand` input, which is what `onPayload` receives in TS.
pub fn build_command_input(
    model: &Model,
    context: &Context,
    options: &BedrockOptions,
    timestamp: i64,
) -> Result<Value, BedrockError> {
    let supports_strict_mode = model
        .compat
        .as_ref()
        .and_then(|compat| compat.as_bedrock())
        .and_then(|compat| compat.supports_strict_mode)
        .unwrap_or(false);
    let cache_retention = resolve_cache_retention(options.cache_retention, options.env.as_ref());
    // Claude models default to the model cap; everything else leaves it unset.
    let inference_max_tokens = options
        .max_tokens
        .or_else(|| is_anthropic_claude_model(&model.id, &model.name).then_some(model.max_tokens));

    let mut input = Map::new();
    input.insert("modelId".to_string(), json!(model.id));
    input.insert(
        "messages".to_string(),
        Value::Array(convert_messages(
            context,
            model,
            cache_retention,
            options.env.as_ref(),
            timestamp,
        )?),
    );
    if let Some(system) = build_system_prompt(
        context.system_prompt.as_deref(),
        model,
        cache_retention,
        options.env.as_ref(),
    ) {
        input.insert("system".to_string(), Value::Array(system));
    }
    let mut inference_config = Map::new();
    if let Some(max_tokens) = inference_max_tokens {
        inference_config.insert("maxTokens".to_string(), json!(max_tokens));
    }
    if let Some(temperature) = options.temperature {
        inference_config.insert("temperature".to_string(), json!(temperature));
    }
    input.insert(
        "inferenceConfig".to_string(),
        Value::Object(inference_config),
    );
    if let Some(tool_config) = convert_tool_config(
        context.tools.as_deref(),
        options.tool_choice.as_ref(),
        supports_strict_mode,
    )
    .map_err(|error| BedrockError(error.to_string()))?
    {
        input.insert("toolConfig".to_string(), tool_config);
    }
    if let Some(additional) = build_additional_model_request_fields(model, options) {
        input.insert("additionalModelRequestFields".to_string(), additional);
    }
    if let Some(request_metadata) = &options.request_metadata {
        input.insert(
            "requestMetadata".to_string(),
            Value::Object(
                request_metadata
                    .iter()
                    .map(|(key, value)| (key.clone(), json!(value)))
                    .collect(),
            ),
        );
    }
    Ok(Value::Object(input))
}

// ---------------------------------------------------------------------------
// Stop reasons and errors
// ---------------------------------------------------------------------------

/// `mapStopReason(reason)`
pub fn map_stop_reason(reason: Option<&str>) -> (StopReason, Option<String>) {
    match reason {
        Some("end_turn") | Some("stop_sequence") => (StopReason::Stop, None),
        Some("max_tokens") | Some("model_context_window_exceeded") => (StopReason::Length, None),
        Some("tool_use") => (StopReason::ToolUse, None),
        Some(reason) if !reason.is_empty() => (
            StopReason::Error,
            Some(format!("Provider stopped with: {reason}")),
        ),
        _ => (StopReason::Error, None),
    }
}

/// `BEDROCK_ERROR_PREFIXES`
pub fn bedrock_error_prefix(name: &str) -> &str {
    match name {
        "InternalServerException" => "Internal server error",
        "ModelStreamErrorException" => "Model stream error",
        "ValidationException" => "Validation error",
        "ThrottlingException" => "Throttling error",
        "ServiceUnavailableException" => "Service unavailable",
        other => other,
    }
}

pub const BEDROCK_DATA_RETENTION_DOCS_URL: &str =
    "https://docs.aws.amazon.com/bedrock/latest/userguide/data-retention.html";

/// `formatBedrockError(error)`
pub fn format_bedrock_error(
    exception_name: Option<&str>,
    status: Option<u16>,
    body: Option<&str>,
    message: &str,
) -> String {
    let normalized = crate::utils::error_body::normalize_provider_error(
        &crate::utils::error_body::RawProviderError {
            status,
            body_text: body.map(str::to_string),
            body_json: None,
            message: message.to_string(),
        },
    );
    // The raw body with its status is surfaced when the SDK did not fold it into the
    // message; that is what keeps a gateway 403 from collapsing to `Unknown: UnknownError`.
    let core = match (
        normalized.message_carries_body,
        normalized.status,
        &normalized.body,
    ) {
        (false, Some(status), Some(body)) => format!("{status}: {body}"),
        _ => normalized.message.clone(),
    };
    let hint = if regex::Regex::new(r"(?i)data retention mode")
        .expect("valid pattern")
        .is_match(&core)
    {
        format!(" See {BEDROCK_DATA_RETENTION_DOCS_URL} for supported data retention modes.")
    } else {
        String::new()
    };
    match exception_name {
        Some(name) => format!("{}: {core}{hint}", bedrock_error_prefix(name)),
        None => format!("{core}{hint}"),
    }
}

/// Over-long values are dropped rather than truncated: a truncated request id is not one.
const MAX_BEDROCK_DIAGNOSTIC_VALUE_CHARS: usize = 200;

/// `normalizeDiagnosticValue(value)`
pub fn normalize_diagnostic_value(value: Option<&str>) -> Option<String> {
    let trimmed = value?.trim();
    if trimmed.is_empty() || trimmed.chars().count() > MAX_BEDROCK_DIAGNOSTIC_VALUE_CHARS {
        return None;
    }
    Some(trimmed.to_string())
}

/// `extractBedrockErrorCode(error)` — modeled Bedrock errors all end in `Exception`.
pub fn extract_bedrock_error_code(name: Option<&str>) -> Option<String> {
    let name = name?;
    if !name.ends_with("Exception") {
        return None;
    }
    normalize_diagnostic_value(Some(name))
}

/// `appendBedrockFailureDiagnostic(output, error, fallbackRequestId)`
pub fn bedrock_failure_diagnostic_details(
    status: Option<u16>,
    exception_name: Option<&str>,
    request_id: Option<&str>,
    fallback_request_id: Option<&str>,
) -> Option<Map<String, Value>> {
    let mut details = Map::new();
    if let Some(status) = status {
        details.insert("status".to_string(), json!(status));
    }
    if let Some(error_code) = extract_bedrock_error_code(exception_name) {
        details.insert("errorCode".to_string(), json!(error_code));
    }
    if let Some(request_id) =
        normalize_diagnostic_value(request_id).or_else(|| fallback_request_id.map(str::to_string))
    {
        details.insert("requestId".to_string(), json!(request_id));
    }
    (!details.is_empty()).then_some(details)
}

// ---------------------------------------------------------------------------
// Streaming state
// ---------------------------------------------------------------------------

/// Scratch state of one content block; TS keeps `index` and `partialJson` on the block.
#[derive(Debug, Clone)]
struct BlockScratch {
    /// `contentBlockIndex` of the provider, which is not the content index.
    index: Option<i64>,
    partial_json: Option<String>,
}

/// The `for await (const item of response.stream)` body as a state machine.
pub struct BedrockStreamState {
    pub output: AssistantMessage,
    model: Model,
    scratch: Vec<BlockScratch>,
}

impl BedrockStreamState {
    pub fn new(model: &Model, timestamp: i64) -> Self {
        BedrockStreamState {
            output: AssistantMessage {
                content: Vec::new(),
                // TS pins the api to the literal, independent of `model.api`.
                api: "bedrock-converse-stream".to_string(),
                provider: model.provider.clone(),
                model: model.id.clone(),
                response_model: None,
                response_id: None,
                diagnostics: None,
                usage: crate::types::Usage {
                    total_tokens: Some(0),
                    ..crate::types::Usage::default()
                },
                stop_reason: StopReason::Pending,
                deferred: None,
                error_message: None,
                raw_stop_reason: None,
                end_turn: None,
                timestamp,
            },
            model: model.clone(),
            scratch: Vec::new(),
        }
    }

    fn position_of(&self, content_block_index: i64) -> Option<usize> {
        self.scratch
            .iter()
            .position(|scratch| scratch.index == Some(content_block_index))
    }

    /// One decoded stream item.
    pub fn process_item(
        &mut self,
        item: &Value,
    ) -> Result<Vec<AssistantMessageEvent>, BedrockError> {
        let mut emitted = Vec::new();

        if let Some(message_start) = item.get("messageStart") {
            if message_start.get("role").and_then(Value::as_str) != Some("assistant") {
                return Err(BedrockError(
                    "Unexpected assistant message start but got user message start instead"
                        .to_string(),
                ));
            }
            emitted.push(AssistantMessageEvent::Start {
                partial: self.output.clone(),
            });
        } else if let Some(event) = item.get("contentBlockStart") {
            self.handle_content_block_start(event, &mut emitted);
        } else if let Some(event) = item.get("contentBlockDelta") {
            self.handle_content_block_delta(event, &mut emitted);
        } else if let Some(event) = item.get("contentBlockStop") {
            self.handle_content_block_stop(event, &mut emitted);
        } else if let Some(event) = item.get("messageStop") {
            let reason = event.get("stopReason").and_then(Value::as_str);
            self.output.raw_stop_reason = reason.map(str::to_string);
            let (stop_reason, error_message) = map_stop_reason(reason);
            self.output.stop_reason = stop_reason;
            if let Some(error_message) = error_message {
                self.output.error_message = Some(error_message);
            }
        } else if let Some(event) = item.get("metadata") {
            self.handle_metadata(event);
        } else {
            // The five modeled mid-stream exceptions are rethrown by TS.
            for key in [
                "internalServerException",
                "modelStreamErrorException",
                "validationException",
                "throttlingException",
                "serviceUnavailableException",
            ] {
                if let Some(exception) = item.get(key) {
                    let message = exception
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or(key)
                        .to_string();
                    return Err(BedrockError(message));
                }
            }
        }
        Ok(emitted)
    }

    /// `handleContentBlockStart(event, …)` — only tool calls get a start event.
    fn handle_content_block_start(
        &mut self,
        event: &Value,
        emitted: &mut Vec<AssistantMessageEvent>,
    ) {
        let index = event.get("contentBlockIndex").and_then(Value::as_i64);
        let Some(tool_use) = event.get("start").and_then(|start| start.get("toolUse")) else {
            return;
        };
        self.output
            .content
            .push(AssistantContent::ToolCall(ToolCall {
                id: tool_use
                    .get("toolUseId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                name: tool_use
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                ..ToolCall::default()
            }));
        self.scratch.push(BlockScratch {
            index,
            partial_json: Some(String::new()),
        });
        emitted.push(AssistantMessageEvent::ToolcallStart {
            content_index: self.output.content.len() - 1,
            partial: self.output.clone(),
        });
    }

    /// `handleContentBlockDelta(event, …)`
    fn handle_content_block_delta(
        &mut self,
        event: &Value,
        emitted: &mut Vec<AssistantMessageEvent>,
    ) {
        let content_block_index = event.get("contentBlockIndex").and_then(Value::as_i64);
        let Some(delta) = event.get("delta") else {
            return;
        };
        let position = content_block_index.and_then(|index| self.position_of(index));

        if let Some(text) = delta.get("text").and_then(Value::as_str) {
            // Text blocks get no `contentBlockStart`, so the first delta opens them.
            let position = match position {
                Some(position) => position,
                None => {
                    self.output
                        .content
                        .push(AssistantContent::Text(TextContent::default()));
                    self.scratch.push(BlockScratch {
                        index: content_block_index,
                        partial_json: None,
                    });
                    let position = self.output.content.len() - 1;
                    emitted.push(AssistantMessageEvent::TextStart {
                        content_index: position,
                        partial: self.output.clone(),
                    });
                    position
                }
            };
            if let AssistantContent::Text(block) = &mut self.output.content[position] {
                block.text.push_str(text);
                emitted.push(AssistantMessageEvent::TextDelta {
                    content_index: position,
                    delta: text.to_string(),
                    partial: self.output.clone(),
                });
            }
            return;
        }

        if let Some(tool_use) = delta.get("toolUse")
            && let Some(position) = position
            && matches!(self.output.content[position], AssistantContent::ToolCall(_))
        {
            let input = tool_use
                .get("input")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let partial_json = {
                let scratch = &mut self.scratch[position];
                let buffer = scratch.partial_json.get_or_insert_with(String::new);
                buffer.push_str(input);
                buffer.clone()
            };
            if let AssistantContent::ToolCall(block) = &mut self.output.content[position] {
                block.arguments = parse_streaming_json(Some(&partial_json))
                    .as_object()
                    .cloned()
                    .unwrap_or_default();
            }
            emitted.push(AssistantMessageEvent::ToolcallDelta {
                content_index: position,
                delta: input.to_string(),
                partial: self.output.clone(),
            });
            return;
        }

        if let Some(reasoning) = delta.get("reasoningContent") {
            let position = match position {
                Some(position) => position,
                None => {
                    self.output
                        .content
                        .push(AssistantContent::Thinking(ThinkingContent {
                            thinking_signature: Some(String::new()),
                            ..ThinkingContent::default()
                        }));
                    self.scratch.push(BlockScratch {
                        index: content_block_index,
                        partial_json: None,
                    });
                    let position = self.output.content.len() - 1;
                    emitted.push(AssistantMessageEvent::ThinkingStart {
                        content_index: position,
                        partial: self.output.clone(),
                    });
                    position
                }
            };
            if let AssistantContent::Thinking(block) = &mut self.output.content[position] {
                if let Some(text) = reasoning
                    .get("text")
                    .and_then(Value::as_str)
                    .filter(|text| !text.is_empty())
                {
                    block.thinking.push_str(text);
                    let delta = text.to_string();
                    emitted.push(AssistantMessageEvent::ThinkingDelta {
                        content_index: position,
                        delta,
                        partial: self.output.clone(),
                    });
                }
                if let Some(signature) = reasoning
                    .get("signature")
                    .and_then(Value::as_str)
                    .filter(|signature| !signature.is_empty())
                    && let AssistantContent::Thinking(block) = &mut self.output.content[position]
                {
                    block
                        .thinking_signature
                        .get_or_insert_with(String::new)
                        .push_str(signature);
                }
            }
        }
    }

    /// `handleContentBlockStop(event, …)`
    fn handle_content_block_stop(
        &mut self,
        event: &Value,
        emitted: &mut Vec<AssistantMessageEvent>,
    ) {
        let Some(content_block_index) = event.get("contentBlockIndex").and_then(Value::as_i64)
        else {
            return;
        };
        let Some(position) = self.position_of(content_block_index) else {
            return;
        };
        // `delete block.index` — the scratch index is cleared, not the block.
        self.scratch[position].index = None;

        match &self.output.content[position] {
            AssistantContent::Text(block) => {
                let content = block.text.clone();
                emitted.push(AssistantMessageEvent::TextEnd {
                    content_index: position,
                    content,
                    partial: self.output.clone(),
                });
            }
            AssistantContent::Thinking(block) => {
                let content = block.thinking.clone();
                emitted.push(AssistantMessageEvent::ThinkingEnd {
                    content_index: position,
                    content,
                    partial: self.output.clone(),
                });
            }
            AssistantContent::ToolCall(_) => {
                let partial_json = self.scratch[position].partial_json.take();
                let parsed = parse_streaming_json(partial_json.as_deref());
                if let AssistantContent::ToolCall(block) = &mut self.output.content[position] {
                    block.arguments = parsed.as_object().cloned().unwrap_or_default();
                }
                let AssistantContent::ToolCall(tool_call) = &self.output.content[position] else {
                    unreachable!("checked above");
                };
                let tool_call = tool_call.clone();
                emitted.push(AssistantMessageEvent::ToolcallEnd {
                    content_index: position,
                    tool_call,
                    partial: self.output.clone(),
                });
            }
        }
    }

    /// `handleMetadata(event, model, output)`
    fn handle_metadata(&mut self, event: &Value) {
        let Some(usage) = event.get("usage") else {
            return;
        };
        let number = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
        self.output.usage.input = number("inputTokens");
        self.output.usage.output = number("outputTokens");
        self.output.usage.cache_read = number("cacheReadInputTokens");
        self.output.usage.cache_write = number("cacheWriteInputTokens");
        let total = usage
            .get("totalTokens")
            .and_then(Value::as_u64)
            .filter(|total| *total != 0)
            .unwrap_or(self.output.usage.input + self.output.usage.output);
        self.output.usage.total_tokens = Some(total);
        calculate_cost(&self.model, &mut self.output.usage);
    }

    /// The checks after the item loop.
    pub fn finish(&self) -> Result<crate::types::DoneReason, BedrockError> {
        if self.output.stop_reason == StopReason::Pending {
            return Err(BedrockError(
                "Bedrock stream ended without a stop reason".to_string(),
            ));
        }
        if self.output.stop_reason == StopReason::Error
            || self.output.stop_reason == StopReason::Aborted
        {
            return Err(BedrockError(
                self.output
                    .error_message
                    .clone()
                    .filter(|message| !message.is_empty())
                    .unwrap_or_else(|| "An unknown error occurred".to_string()),
            ));
        }
        Ok(match self.output.stop_reason {
            StopReason::Length => crate::types::DoneReason::Length,
            StopReason::ToolUse => crate::types::DoneReason::ToolUse,
            StopReason::Deferred => crate::types::DoneReason::Deferred,
            _ => crate::types::DoneReason::Stop,
        })
    }
}

// ---------------------------------------------------------------------------
// Transport
// ---------------------------------------------------------------------------

use aws_sdk_bedrockruntime as bedrock;
use aws_smithy_types::Document;

/// Maps a JSON value onto the Smithy `Document` the Converse API takes.
fn to_document(value: &Value) -> Document {
    match value {
        Value::Null => Document::Null,
        Value::Bool(value) => Document::Bool(*value),
        Value::Number(number) => match (number.as_u64(), number.as_i64(), number.as_f64()) {
            (Some(value), _, _) => Document::Number(aws_smithy_types::Number::PosInt(value)),
            (_, Some(value), _) => Document::Number(aws_smithy_types::Number::NegInt(value)),
            (_, _, Some(value)) => Document::Number(aws_smithy_types::Number::Float(value)),
            _ => Document::Null,
        },
        Value::String(value) => Document::String(value.clone()),
        Value::Array(items) => Document::Array(items.iter().map(to_document).collect()),
        Value::Object(object) => Document::Object(
            object
                .iter()
                .map(|(key, value)| (key.clone(), to_document(value)))
                .collect(),
        ),
    }
}

/// Base64-decodes the `{"__bytes__": "…"}` marker the image blocks carry.
fn image_bytes(source: &Value) -> Result<Vec<u8>, BedrockError> {
    use base64::Engine;
    let encoded = source
        .get("bytes")
        .and_then(|bytes| bytes.get("__bytes__"))
        .and_then(Value::as_str)
        .ok_or_else(|| BedrockError("Image block without bytes".to_string()))?;
    base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|error| BedrockError(format!("Invalid image data: {error}")))
}

fn image_format(format: &str) -> Result<bedrock::types::ImageFormat, BedrockError> {
    Ok(match format {
        "jpeg" => bedrock::types::ImageFormat::Jpeg,
        "png" => bedrock::types::ImageFormat::Png,
        "gif" => bedrock::types::ImageFormat::Gif,
        "webp" => bedrock::types::ImageFormat::Webp,
        other => return Err(BedrockError(format!("Unknown image type: {other}"))),
    })
}

fn to_image(image: &Value) -> Result<bedrock::types::ImageBlock, BedrockError> {
    let format = image
        .get("format")
        .and_then(Value::as_str)
        .ok_or_else(|| BedrockError("Image block without format".to_string()))?;
    let source = image
        .get("source")
        .ok_or_else(|| BedrockError("Image block without source".to_string()))?;
    bedrock::types::ImageBlock::builder()
        .format(image_format(format)?)
        .source(bedrock::types::ImageSource::Bytes(
            aws_smithy_types::Blob::new(image_bytes(source)?),
        ))
        .build()
        .map_err(|error| BedrockError(error.to_string()))
}

fn to_cache_point(point: &Value) -> bedrock::types::CachePointBlock {
    let mut builder =
        bedrock::types::CachePointBlock::builder().r#type(bedrock::types::CachePointType::Default);
    if let Some(ttl) = point.get("ttl").and_then(Value::as_str) {
        builder = builder.ttl(bedrock::types::CacheTtl::from(ttl));
    }
    builder.build().expect("type is always set")
}

fn to_content_block(block: &Value) -> Result<bedrock::types::ContentBlock, BedrockError> {
    if let Some(text) = block.get("text").and_then(Value::as_str) {
        return Ok(bedrock::types::ContentBlock::Text(text.to_string()));
    }
    if let Some(image) = block.get("image") {
        return Ok(bedrock::types::ContentBlock::Image(to_image(image)?));
    }
    if let Some(point) = block.get("cachePoint") {
        return Ok(bedrock::types::ContentBlock::CachePoint(to_cache_point(
            point,
        )));
    }
    if let Some(tool_use) = block.get("toolUse") {
        let tool_use = bedrock::types::ToolUseBlock::builder()
            .tool_use_id(
                tool_use
                    .get("toolUseId")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )
            .name(
                tool_use
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )
            .input(to_document(tool_use.get("input").unwrap_or(&Value::Null)))
            .build()
            .map_err(|error| BedrockError(error.to_string()))?;
        return Ok(bedrock::types::ContentBlock::ToolUse(tool_use));
    }
    if let Some(tool_result) = block.get("toolResult") {
        let mut builder = bedrock::types::ToolResultBlock::builder()
            .tool_use_id(
                tool_result
                    .get("toolUseId")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )
            .status(match tool_result.get("status").and_then(Value::as_str) {
                Some("error") => bedrock::types::ToolResultStatus::Error,
                _ => bedrock::types::ToolResultStatus::Success,
            });
        for content in tool_result
            .get("content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            builder = builder.content(to_tool_result_content(content)?);
        }
        return Ok(bedrock::types::ContentBlock::ToolResult(
            builder
                .build()
                .map_err(|error| BedrockError(error.to_string()))?,
        ));
    }
    if let Some(reasoning) = block.get("reasoningContent") {
        let reasoning_text = reasoning
            .get("reasoningText")
            .ok_or_else(|| BedrockError("Reasoning block without text".to_string()))?;
        let mut builder = bedrock::types::ReasoningTextBlock::builder().text(
            reasoning_text
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        );
        if let Some(signature) = reasoning_text.get("signature").and_then(Value::as_str) {
            builder = builder.signature(signature);
        }
        return Ok(bedrock::types::ContentBlock::ReasoningContent(
            bedrock::types::ReasoningContentBlock::ReasoningText(
                builder
                    .build()
                    .map_err(|error| BedrockError(error.to_string()))?,
            ),
        ));
    }
    Err(BedrockError(format!("Unsupported content block: {block}")))
}

fn to_tool_result_content(
    block: &Value,
) -> Result<bedrock::types::ToolResultContentBlock, BedrockError> {
    if let Some(text) = block.get("text").and_then(Value::as_str) {
        return Ok(bedrock::types::ToolResultContentBlock::Text(
            text.to_string(),
        ));
    }
    if let Some(image) = block.get("image") {
        return Ok(bedrock::types::ToolResultContentBlock::Image(to_image(
            image,
        )?));
    }
    Err(BedrockError(format!(
        "Unsupported tool result content: {block}"
    )))
}

/// Turns the command input built by [`build_command_input`] into the SDK request.
pub fn to_converse_stream_request(
    client: &bedrock::Client,
    input: &Value,
) -> Result<bedrock::operation::converse_stream::builders::ConverseStreamFluentBuilder, BedrockError>
{
    let mut request = client.converse_stream().model_id(
        input
            .get("modelId")
            .and_then(Value::as_str)
            .ok_or_else(|| BedrockError("Command input without modelId".to_string()))?,
    );

    for message in input
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let role = match message.get("role").and_then(Value::as_str) {
            Some("assistant") => bedrock::types::ConversationRole::Assistant,
            _ => bedrock::types::ConversationRole::User,
        };
        let mut builder = bedrock::types::Message::builder().role(role);
        for block in message
            .get("content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            builder = builder.content(to_content_block(block)?);
        }
        request = request.messages(
            builder
                .build()
                .map_err(|error| BedrockError(error.to_string()))?,
        );
    }

    for block in input
        .get("system")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let system = if let Some(text) = block.get("text").and_then(Value::as_str) {
            bedrock::types::SystemContentBlock::Text(text.to_string())
        } else if let Some(point) = block.get("cachePoint") {
            bedrock::types::SystemContentBlock::CachePoint(to_cache_point(point))
        } else {
            return Err(BedrockError(format!("Unsupported system block: {block}")));
        };
        request = request.system(system);
    }

    if let Some(inference) = input.get("inferenceConfig").and_then(Value::as_object) {
        let mut builder = bedrock::types::InferenceConfiguration::builder();
        if let Some(max_tokens) = inference.get("maxTokens").and_then(Value::as_i64) {
            builder = builder.max_tokens(max_tokens as i32);
        }
        if let Some(temperature) = inference.get("temperature").and_then(Value::as_f64) {
            builder = builder.temperature(temperature as f32);
        }
        request = request.inference_config(builder.build());
    }

    if let Some(tool_config) = input.get("toolConfig") {
        let mut builder = bedrock::types::ToolConfiguration::builder();
        for tool in tool_config
            .get("tools")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let spec = tool
                .get("toolSpec")
                .ok_or_else(|| BedrockError("Tool without toolSpec".to_string()))?;
            let tool_spec = bedrock::types::ToolSpecification::builder()
                .name(spec.get("name").and_then(Value::as_str).unwrap_or_default())
                .set_description(
                    spec.get("description")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                )
                .input_schema(bedrock::types::ToolInputSchema::Json(to_document(
                    spec.get("inputSchema")
                        .and_then(|schema| schema.get("json"))
                        .unwrap_or(&Value::Null),
                )))
                .build()
                .map_err(|error| BedrockError(error.to_string()))?;
            builder = builder.tools(bedrock::types::Tool::ToolSpec(tool_spec));
        }
        if let Some(choice) = tool_config.get("toolChoice") {
            let choice = if choice.get("auto").is_some() {
                bedrock::types::ToolChoice::Auto(bedrock::types::AutoToolChoice::builder().build())
            } else if choice.get("any").is_some() {
                bedrock::types::ToolChoice::Any(bedrock::types::AnyToolChoice::builder().build())
            } else if let Some(tool) = choice.get("tool") {
                bedrock::types::ToolChoice::Tool(
                    bedrock::types::SpecificToolChoice::builder()
                        .name(tool.get("name").and_then(Value::as_str).unwrap_or_default())
                        .build()
                        .map_err(|error| BedrockError(error.to_string()))?,
                )
            } else {
                return Err(BedrockError(format!("Unsupported tool choice: {choice}")));
            };
            builder = builder.tool_choice(choice);
        }
        request = request.tool_config(
            builder
                .build()
                .map_err(|error| BedrockError(error.to_string()))?,
        );
    }

    if let Some(additional) = input.get("additionalModelRequestFields") {
        request = request.additional_model_request_fields(to_document(additional));
    }
    if let Some(metadata) = input.get("requestMetadata").and_then(Value::as_object) {
        for (key, value) in metadata {
            if let Some(value) = value.as_str() {
                request = request.request_metadata(key, value);
            }
        }
    }
    Ok(request)
}

/// Maps one SDK stream event back onto the JSON shape [`BedrockStreamState`] consumes.
pub fn stream_item_to_json(event: &bedrock::types::ConverseStreamOutput) -> Value {
    use bedrock::types::ConverseStreamOutput;
    match event {
        ConverseStreamOutput::MessageStart(event) => json!({
            "messageStart": { "role": event.role().as_str() },
        }),
        ConverseStreamOutput::ContentBlockStart(event) => {
            let mut start = Map::new();
            if let Some(bedrock::types::ContentBlockStart::ToolUse(tool_use)) = event.start() {
                start.insert(
                    "toolUse".to_string(),
                    json!({ "toolUseId": tool_use.tool_use_id(), "name": tool_use.name() }),
                );
            }
            json!({
                "contentBlockStart": {
                    "contentBlockIndex": event.content_block_index(),
                    "start": Value::Object(start),
                },
            })
        }
        ConverseStreamOutput::ContentBlockDelta(event) => {
            let mut delta = Map::new();
            match event.delta() {
                Some(bedrock::types::ContentBlockDelta::Text(text)) => {
                    delta.insert("text".to_string(), json!(text));
                }
                Some(bedrock::types::ContentBlockDelta::ToolUse(tool_use)) => {
                    delta.insert("toolUse".to_string(), json!({ "input": tool_use.input() }));
                }
                Some(bedrock::types::ContentBlockDelta::ReasoningContent(reasoning)) => {
                    let mut content = Map::new();
                    match reasoning {
                        bedrock::types::ReasoningContentBlockDelta::Text(text) => {
                            content.insert("text".to_string(), json!(text));
                        }
                        bedrock::types::ReasoningContentBlockDelta::Signature(signature) => {
                            content.insert("signature".to_string(), json!(signature));
                        }
                        _ => {}
                    }
                    delta.insert("reasoningContent".to_string(), Value::Object(content));
                }
                _ => {}
            }
            json!({
                "contentBlockDelta": {
                    "contentBlockIndex": event.content_block_index(),
                    "delta": Value::Object(delta),
                },
            })
        }
        ConverseStreamOutput::ContentBlockStop(event) => json!({
            "contentBlockStop": { "contentBlockIndex": event.content_block_index() },
        }),
        ConverseStreamOutput::MessageStop(event) => json!({
            "messageStop": { "stopReason": event.stop_reason().as_str() },
        }),
        ConverseStreamOutput::Metadata(event) => {
            let mut usage = Map::new();
            if let Some(tokens) = event.usage() {
                usage.insert("inputTokens".to_string(), json!(tokens.input_tokens()));
                usage.insert("outputTokens".to_string(), json!(tokens.output_tokens()));
                usage.insert("totalTokens".to_string(), json!(tokens.total_tokens()));
                if let Some(cache_read) = tokens.cache_read_input_tokens() {
                    usage.insert("cacheReadInputTokens".to_string(), json!(cache_read));
                }
                if let Some(cache_write) = tokens.cache_write_input_tokens() {
                    usage.insert("cacheWriteInputTokens".to_string(), json!(cache_write));
                }
            }
            json!({ "metadata": { "usage": Value::Object(usage) } })
        }
        _ => Value::Object(Map::new()),
    }
}

// ---------------------------------------------------------------------------
// Transport and stream driver
// ---------------------------------------------------------------------------

/// TS swaps the SDK's default HTTP/2 handler for a `NodeHttpHandler` in exactly two
/// cases: a resolved proxy (with http/https proxy agents) and
/// `AWS_BEDROCK_FORCE_HTTP1=1`. Both branches speak HTTP/1.1, so this port installs a
/// reqwest-backed client — the crate's HTTP client — for them (substitution class 3).
#[derive(Debug, Clone)]
struct ReqwestHttpClient {
    client: reqwest::Client,
}

impl ReqwestHttpClient {
    fn new(proxy_url: Option<&str>) -> Result<Self, BedrockError> {
        // `no_proxy()` first: reqwest would otherwise add the ambient proxy env on top
        // of the resolved one, which the Node agents never do.
        let mut builder = reqwest::Client::builder().http1_only().no_proxy();
        if let Some(proxy_url) = proxy_url {
            builder = builder.proxy(
                reqwest::Proxy::all(proxy_url).map_err(|error| BedrockError(error.to_string()))?,
            );
        }
        Ok(ReqwestHttpClient {
            client: builder
                .build()
                .map_err(|error| BedrockError(error.to_string()))?,
        })
    }
}

impl aws_smithy_runtime_api::client::http::HttpConnector for ReqwestHttpClient {
    fn call(
        &self,
        request: aws_smithy_runtime_api::client::orchestrator::HttpRequest,
    ) -> aws_smithy_runtime_api::client::http::HttpConnectorFuture {
        use aws_smithy_runtime_api::client::result::ConnectorError;

        let client = self.client.clone();
        aws_smithy_runtime_api::client::http::HttpConnectorFuture::new(async move {
            let request = request
                .try_into_http1x()
                .map_err(|error| ConnectorError::user(Box::new(error)))?;
            let (parts, body) = request.into_parts();
            // The Converse request body is a single serialized JSON document; only the
            // response is an event stream.
            let body = body.bytes().unwrap_or_default().to_vec();
            let mut outgoing = client.request(parts.method, parts.uri.to_string());
            for (name, value) in parts.headers.iter() {
                outgoing = outgoing.header(name, value);
            }
            let response = outgoing
                .body(body)
                .send()
                .await
                .map_err(|error| ConnectorError::io(Box::new(error)))?;
            let (parts, body) = http::Response::from(response).into_parts();
            aws_smithy_runtime_api::client::orchestrator::HttpResponse::try_from(
                http::Response::from_parts(
                    parts,
                    aws_smithy_types::body::SdkBody::from_body_1_x(body),
                ),
            )
            .map_err(|error| ConnectorError::other(Box::new(error), None))
        })
    }
}

/// `addCustomHeadersMiddleware(client, headers)` — the TS middleware runs in the `build`
/// step, after serialization and before signing; `modify_before_signing` is that point.
#[derive(Debug)]
struct CustomHeadersInterceptor {
    headers: BTreeMap<String, String>,
}

impl aws_smithy_runtime_api::client::interceptors::Intercept for CustomHeadersInterceptor {
    fn name(&self) -> &'static str {
        "notagent-ai-custom-headers"
    }

    fn modify_before_signing(
        &self,
        context: &mut aws_smithy_runtime_api::client::interceptors::context::BeforeTransmitInterceptorContextMut<'_>,
        _runtime_components: &aws_smithy_runtime_api::client::runtime_components::RuntimeComponents,
        _cfg: &mut aws_smithy_types::config_bag::ConfigBag,
    ) -> Result<(), aws_smithy_runtime_api::box_error::BoxError> {
        let headers = context.request_mut().headers_mut();
        for (key, value) in &self.headers {
            if !is_reserved_header(key) {
                headers.try_insert(key.clone(), value.clone())?;
            }
        }
        Ok(())
    }
}

/// Records the HTTP status of the response so `onResponse` sees what
/// `response.$metadata.httpStatusCode` carries in TS.
#[derive(Debug, Default, Clone)]
struct ResponseStatusInterceptor {
    status: std::sync::Arc<std::sync::Mutex<Option<u16>>>,
}

impl aws_smithy_runtime_api::client::interceptors::Intercept for ResponseStatusInterceptor {
    fn name(&self) -> &'static str {
        "notagent-ai-response-status"
    }

    fn read_before_deserialization(
        &self,
        context: &aws_smithy_runtime_api::client::interceptors::context::BeforeDeserializationInterceptorContextRef<'_>,
        _runtime_components: &aws_smithy_runtime_api::client::runtime_components::RuntimeComponents,
        _cfg: &mut aws_smithy_types::config_bag::ConfigBag,
    ) -> Result<(), aws_smithy_runtime_api::box_error::BoxError> {
        *self
            .status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some(context.response().status().as_u16());
        Ok(())
    }
}

impl aws_smithy_runtime_api::client::http::HttpClient for ReqwestHttpClient {
    fn http_connector(
        &self,
        _settings: &aws_smithy_runtime_api::client::http::HttpConnectorSettings,
        _components: &aws_smithy_runtime_api::client::runtime_components::RuntimeComponents,
    ) -> aws_smithy_runtime_api::client::http::SharedHttpConnector {
        aws_smithy_runtime_api::client::http::SharedHttpConnector::new(self.clone())
    }
}

/// `new BedrockRuntimeClient(config)` — the resolved [`BedrockClientConfig`] mapped onto
/// the Rust SDK. The default credential chain applies wherever TS leaves the field unset.
async fn build_bedrock_client(
    config: &BedrockClientConfig,
    custom_headers: Option<BTreeMap<String, String>>,
    status: &ResponseStatusInterceptor,
) -> Result<bedrock::Client, BedrockError> {
    let mut loader = aws_config::defaults(aws_config::BehaviorVersion::latest());
    if let Some(profile) = config
        .profile
        .as_ref()
        .filter(|profile| !profile.is_empty())
    {
        loader = loader.profile_name(profile);
    }
    if let Some(region) = config.region.as_ref().filter(|region| !region.is_empty()) {
        loader = loader.region(bedrock::config::Region::new(region.clone()));
    }
    let shared = loader.load().await;

    let mut builder = bedrock::config::Builder::from(&shared);
    if let Some(endpoint) = config
        .endpoint
        .as_ref()
        .filter(|endpoint| !endpoint.is_empty())
    {
        builder = builder.endpoint_url(endpoint);
    }
    if let Some(credentials) = &config.credentials {
        builder = builder.credentials_provider(bedrock::config::Credentials::new(
            credentials.access_key_id.clone(),
            credentials.secret_access_key.clone(),
            credentials.session_token.clone(),
            None,
            "notagent-ai",
        ));
    }
    if let Some(token) = &config.bearer_token {
        builder = builder
            .bearer_token(bedrock::config::Token::new(token.clone(), None))
            .auth_scheme_preference([aws_smithy_runtime_api::client::auth::AuthSchemeId::from(
                "httpBearerAuth",
            )]);
    }
    if config.proxy_url.is_some() || config.force_http1 {
        builder = builder.http_client(ReqwestHttpClient::new(config.proxy_url.as_deref())?);
    }
    if let Some(headers) = custom_headers {
        builder = builder.interceptor(CustomHeadersInterceptor { headers });
    }
    builder = builder.interceptor(status.clone());
    Ok(bedrock::Client::from_conf(builder.build()))
}

/// The five modeled mid-stream exceptions plus the operation errors carry their shape
/// name in TS's `error.name`; [`format_bedrock_error`] turns it into the legacy prefix.
fn converse_stream_error_name(
    error: &bedrock::operation::converse_stream::ConverseStreamError,
) -> String {
    use bedrock::operation::converse_stream::ConverseStreamError;
    match error {
        ConverseStreamError::AccessDeniedException(_) => "AccessDeniedException".to_string(),
        ConverseStreamError::InternalServerException(_) => "InternalServerException".to_string(),
        ConverseStreamError::ModelErrorException(_) => "ModelErrorException".to_string(),
        ConverseStreamError::ModelNotReadyException(_) => "ModelNotReadyException".to_string(),
        ConverseStreamError::ModelTimeoutException(_) => "ModelTimeoutException".to_string(),
        ConverseStreamError::ResourceNotFoundException(_) => {
            "ResourceNotFoundException".to_string()
        }
        ConverseStreamError::ServiceUnavailableException(_) => {
            "ServiceUnavailableException".to_string()
        }
        ConverseStreamError::ThrottlingException(_) => "ThrottlingException".to_string(),
        ConverseStreamError::ValidationException(_) => "ValidationException".to_string(),
        ConverseStreamError::ModelStreamErrorException(_) => {
            "ModelStreamErrorException".to_string()
        }
        other => aws_sdk_bedrockruntime::error::ProvideErrorMetadata::code(other)
            .unwrap_or("Error")
            .to_string(),
    }
}

fn converse_stream_output_error_name(
    error: &bedrock::types::error::ConverseStreamOutputError,
) -> String {
    use bedrock::types::error::ConverseStreamOutputError;
    match error {
        ConverseStreamOutputError::InternalServerException(_) => {
            "InternalServerException".to_string()
        }
        ConverseStreamOutputError::ModelStreamErrorException(_) => {
            "ModelStreamErrorException".to_string()
        }
        ConverseStreamOutputError::ServiceUnavailableException(_) => {
            "ServiceUnavailableException".to_string()
        }
        ConverseStreamOutputError::ThrottlingException(_) => "ThrottlingException".to_string(),
        ConverseStreamOutputError::ValidationException(_) => "ValidationException".to_string(),
        other => aws_sdk_bedrockruntime::error::ProvideErrorMetadata::code(other)
            .unwrap_or("Error")
            .to_string(),
    }
}

/// What `formatBedrockError`/`appendBedrockFailureDiagnostic` need from one failure.
struct BedrockFailure {
    /// `error.name` for modeled service exceptions, `None` for transport errors.
    exception_name: Option<String>,
    status: Option<u16>,
    body: Option<String>,
    request_id: Option<String>,
    message: String,
}

impl BedrockFailure {
    /// A failure that never reached the service (request build, client setup).
    fn from_local(error: BedrockError) -> Self {
        BedrockFailure {
            exception_name: None,
            status: None,
            body: None,
            request_id: None,
            message: error.0,
        }
    }

    /// A modeled service exception: TS reads `error.name`, `error.message` and the raw
    /// `$response` (status, body) off the same object.
    fn from_service_error(
        name: String,
        message: Option<String>,
        raw: &aws_smithy_runtime_api::client::orchestrator::HttpResponse,
    ) -> Self {
        BedrockFailure {
            message: message.unwrap_or_else(|| name.clone()),
            exception_name: Some(name),
            status: Some(raw.status().as_u16()),
            body: raw
                .body()
                .bytes()
                .map(|bytes| String::from_utf8_lossy(bytes).to_string())
                .filter(|body| !body.is_empty()),
            request_id: raw.headers().get("x-amzn-requestid").map(str::to_string),
        }
    }

    /// `formatBedrockError(error)` for this failure.
    fn format(&self) -> String {
        format_bedrock_error(
            self.exception_name.as_deref(),
            self.status,
            self.body.as_deref(),
            &self.message,
        )
    }
}

/// `stream(model, context, options)`
pub fn stream(
    model: Model,
    context: Context,
    request: crate::types::ProviderRequestOptions,
    options: BedrockOptions,
) -> AssistantMessageEventStream {
    let outer = crate::utils::event_stream::create_assistant_message_event_stream();
    let stream = outer.clone();

    tokio::spawn(async move {
        let timestamp = crate::auth::resolve::now_ms();
        let mut state = BedrockStreamState::new(&model, timestamp);
        // Kept outside the request so a mid-stream failure can still be correlated:
        // exceptions delivered as stream events carry no HTTP metadata of their own.
        let mut response_request_id: Option<String> = None;
        let result = run_bedrock_request(
            &model,
            &context,
            &request,
            &options,
            &mut state,
            &stream,
            &mut response_request_id,
        )
        .await;

        match result {
            Ok(reason) => {
                stream.push(AssistantMessageEvent::Done {
                    reason,
                    message: state.output.clone(),
                });
                stream.end(Some(state.output.clone()));
            }
            Err(failure) => {
                let aborted = request
                    .signal
                    .as_ref()
                    .is_some_and(tokio_util::sync::CancellationToken::is_cancelled);
                state.output.stop_reason = if aborted {
                    StopReason::Aborted
                } else {
                    StopReason::Error
                };
                state.output.error_message = Some(failure.format());
                if state.output.stop_reason == StopReason::Error
                    && let Some(details) = bedrock_failure_diagnostic_details(
                        failure.status,
                        failure.exception_name.as_deref(),
                        failure.request_id.as_deref(),
                        response_request_id.as_deref(),
                    )
                {
                    crate::utils::diagnostics::append_assistant_message_diagnostic(
                        &mut state.output.diagnostics,
                        crate::utils::diagnostics::AssistantMessageDiagnostic {
                            r#type: "bedrock_response_failure".to_string(),
                            timestamp: crate::auth::resolve::now_ms(),
                            error: None,
                            details: Some(details),
                        },
                    );
                }
                stream.push(AssistantMessageEvent::Error {
                    reason: if aborted {
                        crate::types::ErrorReason::Aborted
                    } else {
                        crate::types::ErrorReason::Error
                    },
                    error: state.output.clone(),
                });
                stream.end(Some(state.output.clone()));
            }
        }
    });

    outer
}

/// The body of the TS `try` block: build the client, send the command and drain the
/// event stream into `state`.
async fn run_bedrock_request(
    model: &Model,
    context: &Context,
    request: &crate::types::ProviderRequestOptions,
    options: &BedrockOptions,
    state: &mut BedrockStreamState,
    stream: &AssistantMessageEventStream,
    response_request_id: &mut Option<String>,
) -> Result<crate::types::DoneReason, BedrockFailure> {
    use aws_sdk_bedrockruntime::error::ProvideErrorMetadata;
    use aws_smithy_runtime_api::client::result::SdkError;

    let config = build_client_config(model, options);
    let status = ResponseStatusInterceptor::default();
    let client = build_bedrock_client(
        &config,
        crate::utils::headers::provider_headers_to_record(request.headers.as_ref()),
        &status,
    )
    .await
    .map_err(BedrockFailure::from_local)?;

    let mut command_input = build_command_input(model, context, options, state.output.timestamp)
        .map_err(BedrockFailure::from_local)?;
    if let Some(on_payload) = &request.on_payload
        && let Some(next) = on_payload(command_input.clone(), model).await
    {
        command_input = next;
    }

    let operation =
        to_converse_stream_request(&client, &command_input).map_err(BedrockFailure::from_local)?;
    let signal = request.signal.clone().unwrap_or_default();
    let mut response = tokio::select! {
        response = operation.send() => response.map_err(|error| match error {
            SdkError::ServiceError(context) => BedrockFailure::from_service_error(
                converse_stream_error_name(context.err()),
                context.err().message().map(str::to_string),
                context.raw(),
            ),
            other => BedrockFailure::from_local(BedrockError(other.to_string())),
        })?,
        _ = signal.cancelled() => return Err(BedrockFailure::from_local(BedrockError("Request was aborted".to_string()))),
    };

    *response_request_id = normalize_diagnostic_value(
        aws_sdk_bedrockruntime::operation::RequestId::request_id(&response),
    );
    let response_status = *status
        .status
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(on_response) = &request.on_response
        && let Some(status) = response_status
    {
        let mut headers = BTreeMap::new();
        if let Some(request_id) = response_request_id.clone() {
            headers.insert("x-amzn-requestid".to_string(), request_id);
        }
        on_response(crate::types::ProviderResponse { status, headers }, model).await;
    }

    loop {
        let item = tokio::select! {
            item = response.stream.recv() => item,
            _ = signal.cancelled() => return Err(BedrockFailure::from_local(BedrockError("Request was aborted".to_string()))),
        };
        let item = match item {
            Ok(Some(item)) => item,
            Ok(None) => break,
            Err(error) => {
                return Err(match error {
                    // A modeled mid-stream exception carries no HTTP metadata of its
                    // own, exactly as in TS, so only its name and message survive.
                    SdkError::ServiceError(context) => {
                        let name = converse_stream_output_error_name(context.err());
                        BedrockFailure {
                            message: context
                                .err()
                                .message()
                                .map(str::to_string)
                                .unwrap_or_else(|| name.clone()),
                            exception_name: Some(name),
                            status: None,
                            body: None,
                            request_id: None,
                        }
                    }
                    other => BedrockFailure::from_local(BedrockError(other.to_string())),
                });
            }
        };
        for event in state
            .process_item(&stream_item_to_json(&item))
            .map_err(BedrockFailure::from_local)?
        {
            stream.push(event);
        }
    }

    if signal.is_cancelled() {
        return Err(BedrockFailure::from_local(BedrockError(
            "Request was aborted".to_string(),
        )));
    }
    state.finish().map_err(|error| {
        // The scratch fields never leave the state machine, so only the message differs
        // from the TS cleanup, which deletes `index`/`partialJson` off the blocks here.
        BedrockFailure::from_local(error)
    })
}

/// `streamSimple(model, context, options)`
pub fn stream_simple(
    model: Model,
    context: Context,
    options: Option<SimpleStreamOptions>,
) -> AssistantMessageEventStream {
    let base =
        crate::api::simple_options::build_base_options(&model, &context, options.as_ref(), None);
    let reasoning = options.as_ref().and_then(|options| options.reasoning);
    let thinking_budgets = options
        .as_ref()
        .and_then(|options| options.thinking_budgets);
    let bedrock_options =
        |max_tokens: Option<u64>,
         reasoning: Option<ThinkingLevel>,
         thinking_budgets: Option<ThinkingBudgets>| BedrockOptions {
            region: None,
            profile: None,
            // `toolChoice`, `region`, `profile`, `requestMetadata`, `interleavedThinking`
            // and `thinkingDisplay` only exist on the typed `BedrockOptions`; a caller of
            // the simple signature cannot set them, exactly as in TS.
            tool_choice: None,
            reasoning,
            thinking_budgets,
            interleaved_thinking: None,
            thinking_display: None,
            request_metadata: None,
            bearer_token: None,
            api_key: base.base.api_key.clone(),
            max_tokens,
            temperature: base.temperature,
            cache_retention: base.cache_retention,
            env: base.base.env.clone(),
        };

    let Some(reasoning) = reasoning else {
        return stream(
            model,
            context,
            base.base.clone(),
            bedrock_options(base.max_tokens, None, None),
        );
    };

    if is_anthropic_claude_model(&model.id, &model.name) {
        if supports_adaptive_thinking(&model.id, &model.name) {
            return stream(
                model,
                context,
                base.base.clone(),
                bedrock_options(base.max_tokens, Some(reasoning), thinking_budgets),
            );
        }

        let adjusted = crate::api::simple_options::adjust_max_tokens_for_thinking(
            base.max_tokens,
            model.max_tokens,
            reasoning,
            thinking_budgets.as_ref(),
        );
        let max_tokens = crate::api::simple_options::clamp_max_tokens_to_context(
            &model,
            &context,
            adjusted.max_tokens,
        );
        let mut budgets = thinking_budgets.unwrap_or_default();
        let budget = adjusted
            .thinking_budget
            .min(max_tokens.saturating_sub(1024));
        match crate::api::simple_options::clamp_reasoning(Some(reasoning)) {
            Some(ThinkingLevel::Minimal) => budgets.minimal = Some(budget),
            Some(ThinkingLevel::Low) => budgets.low = Some(budget),
            Some(ThinkingLevel::Medium) => budgets.medium = Some(budget),
            _ => budgets.high = Some(budget),
        }
        return stream(
            model,
            context,
            base.base.clone(),
            bedrock_options(Some(max_tokens), Some(reasoning), Some(budgets)),
        );
    }

    stream(
        model,
        context,
        base.base.clone(),
        bedrock_options(base.max_tokens, Some(reasoning), thinking_budgets),
    )
}
