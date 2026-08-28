use std::collections::BTreeSet;
use std::sync::OnceLock;

use serde_json::{Map, Value, json};

use crate::api::constrained_sampling::{
    ConstrainedSamplingError, get_json_schema_tool_parameters, resolve_json_schema_strict_sampling,
};
use crate::api::transform_messages::transform_messages;
use crate::types::{
    AssistantContent, AssistantMessage, Context, Message, Modality, Model, StopReason,
    TextOrImageContent, Tool, UserContent,
};
use crate::utils::sanitize_unicode::sanitize_surrogates;

/// `GoogleThinkingLevel` — mirrors Google's enum values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoogleThinkingLevel {
    Unspecified,
    Minimal,
    Low,
    Medium,
    High,
}

impl GoogleThinkingLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            GoogleThinkingLevel::Unspecified => "THINKING_LEVEL_UNSPECIFIED",
            GoogleThinkingLevel::Minimal => "MINIMAL",
            GoogleThinkingLevel::Low => "LOW",
            GoogleThinkingLevel::Medium => "MEDIUM",
            GoogleThinkingLevel::High => "HIGH",
        }
    }
}

/// `isThinkingPart(part)` — `thought: true` is the definitive marker; a
/// `thoughtSignature` can sit on any part and says nothing about its kind.
pub fn is_thinking_part(part: &Value) -> bool {
    part.get("thought") == Some(&Value::Bool(true))
}

/// `retainThoughtSignature(existing, incoming)`
/// Some backends send `thoughtSignature` only on the first delta of a block; this keeps
/// the last non-empty one instead of letting a later delta clear it. It never moves a
/// signature between distinct parts.
pub fn retain_thought_signature(
    existing: Option<String>,
    incoming: Option<&str>,
) -> Option<String> {
    match incoming.filter(|signature| !signature.is_empty()) {
        Some(incoming) => Some(incoming.to_string()),
        None => existing,
    }
}

/// Thought signatures have to be base64 for Google's APIs (TYPE_BYTES).
fn is_valid_thought_signature(signature: Option<&str>) -> bool {
    let Some(signature) = signature.filter(|signature| !signature.is_empty()) else {
        return false;
    };
    if signature.len() % 4 != 0 {
        return false;
    }
    static PATTERN: OnceLock<regex::Regex> = OnceLock::new();
    PATTERN
        .get_or_init(|| regex::Regex::new(r"^[A-Za-z0-9+/]+={0,2}$").expect("valid pattern"))
        .is_match(signature)
}

/// `resolveThoughtSignature(isSameProviderAndModel, signature)`
fn resolve_thought_signature(
    is_same_provider_and_model: bool,
    signature: Option<&str>,
) -> Option<String> {
    (is_same_provider_and_model && is_valid_thought_signature(signature))
        .then(|| signature.expect("checked above").to_string())
}

/// `getGeminiMajorVersion(modelId)`
fn gemini_major_version(model_id: &str) -> Option<u32> {
    static PATTERN: OnceLock<regex::Regex> = OnceLock::new();
    let pattern = PATTERN
        .get_or_init(|| regex::Regex::new(r"^gemini(?:-live)?-(\d+)").expect("valid pattern"));
    let lowercase = model_id.to_lowercase();
    let captures = pattern.captures(&lowercase)?;
    captures.get(1)?.as_str().parse::<u32>().ok()
}

/// `requiresToolCallId(modelId)`
pub fn requires_tool_call_id(model_id: &str) -> bool {
    model_id.starts_with("claude-")
        || model_id.starts_with("gpt-oss-")
        || gemini_major_version(model_id).is_some_and(|version| version >= 3)
}

/// `supportsMultimodalFunctionResponse(modelId)`
fn supports_multimodal_function_response(model_id: &str) -> bool {
    match gemini_major_version(model_id) {
        Some(version) => version >= 3,
        None => true,
    }
}

/// `supportsGoogleStrictToolSampling(modelId)` — Gemini 3+ enforces required parameters
/// in the validated tool-calling modes.
pub fn supports_google_strict_tool_sampling(model_id: &str) -> bool {
    gemini_major_version(model_id).is_some_and(|version| version >= 3)
}

/// `convertMessages(model, context)`
pub fn convert_messages(model: &Model, context: &Context, timestamp: i64) -> Vec<Value> {
    let mut contents: Vec<Value> = Vec::new();
    let requires_id = requires_tool_call_id(&model.id);
    let normalize_tool_call_id = |id: &str, _source: &AssistantMessage| -> String {
        if !requires_id {
            return id.to_string();
        }
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
    };

    let transformed_messages = transform_messages(
        &context.messages,
        model,
        Some(&normalize_tool_call_id),
        timestamp,
    );

    for message in &transformed_messages {
        match message {
            Message::User(user) => match &user.content {
                UserContent::Text(text) => {
                    contents.push(json!({
                        "role": "user",
                        "parts": [{ "text": sanitize_surrogates(text) }],
                    }));
                }
                UserContent::Blocks(blocks) => {
                    let parts: Vec<Value> = blocks
                        .iter()
                        .map(|block| match block {
                            TextOrImageContent::Text(text) => {
                                json!({ "text": sanitize_surrogates(&text.text) })
                            }
                            TextOrImageContent::Image(image) => json!({
                                "inlineData": {
                                    "mimeType": image.mime_type,
                                    "data": image.data,
                                },
                            }),
                        })
                        .collect();
                    if parts.is_empty() {
                        continue;
                    }
                    contents.push(json!({ "role": "user", "parts": parts }));
                }
            },
            Message::Assistant(assistant) => {
                let mut parts: Vec<Value> = Vec::new();
                // Thinking blocks are only kept when provider and model match.
                let is_same_provider_and_model =
                    assistant.provider == model.provider && assistant.model == model.id;

                for block in &assistant.content {
                    match block {
                        AssistantContent::Text(text) => {
                            let thought_signature = resolve_thought_signature(
                                is_same_provider_and_model,
                                text.text_signature.as_deref(),
                            );
                            // An empty text block survives only when it carries a signature:
                            // Gemini attaches signatures to parts with no visible text and
                            // needs them echoed back, otherwise the reasoning chain breaks and
                            // the model ends turns with a thought-only STOP.
                            if text.text.trim().is_empty() && thought_signature.is_none() {
                                continue;
                            }
                            let mut part = Map::new();
                            part.insert("text".to_string(), json!(sanitize_surrogates(&text.text)));
                            if let Some(thought_signature) = thought_signature {
                                part.insert(
                                    "thoughtSignature".to_string(),
                                    json!(thought_signature),
                                );
                            }
                            parts.push(Value::Object(part));
                        }
                        AssistantContent::Thinking(thinking) => {
                            if is_same_provider_and_model {
                                let thought_signature = resolve_thought_signature(
                                    is_same_provider_and_model,
                                    thinking.thinking_signature.as_deref(),
                                );
                                // Same rule as for text blocks.
                                if thinking.thinking.trim().is_empty()
                                    && thought_signature.is_none()
                                {
                                    continue;
                                }
                                let mut part = Map::new();
                                part.insert("thought".to_string(), json!(true));
                                part.insert(
                                    "text".to_string(),
                                    json!(sanitize_surrogates(&thinking.thinking)),
                                );
                                if let Some(thought_signature) = thought_signature {
                                    part.insert(
                                        "thoughtSignature".to_string(),
                                        json!(thought_signature),
                                    );
                                }
                                parts.push(Value::Object(part));
                            } else {
                                // Cross-model the signature is unusable, so empty blocks stay
                                // dropped and the rest becomes plain text (no tags, which the
                                // model would mimic).
                                if thinking.thinking.trim().is_empty() {
                                    continue;
                                }
                                parts.push(
                                    json!({ "text": sanitize_surrogates(&thinking.thinking) }),
                                );
                            }
                        }
                        AssistantContent::ToolCall(tool_call) => {
                            let thought_signature = resolve_thought_signature(
                                is_same_provider_and_model,
                                tool_call.thought_signature.as_deref(),
                            );
                            let mut function_call = Map::new();
                            function_call.insert("name".to_string(), json!(tool_call.name));
                            function_call.insert(
                                "args".to_string(),
                                Value::Object(tool_call.arguments.clone()),
                            );
                            if requires_id {
                                function_call.insert("id".to_string(), json!(tool_call.id));
                            }
                            let mut part = Map::new();
                            part.insert("functionCall".to_string(), Value::Object(function_call));
                            if let Some(thought_signature) = thought_signature {
                                part.insert(
                                    "thoughtSignature".to_string(),
                                    json!(thought_signature),
                                );
                            }
                            parts.push(Value::Object(part));
                        }
                    }
                }

                if parts.is_empty() {
                    continue;
                }
                contents.push(json!({ "role": "model", "parts": parts }));
            }
            Message::ToolResult(tool_result) => {
                let text_result = tool_result
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        TextOrImageContent::Text(text) => Some(text.text.as_str()),
                        TextOrImageContent::Image(_) => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let image_content: Vec<_> = if model.input.contains(&Modality::Image) {
                    tool_result
                        .content
                        .iter()
                        .filter_map(|block| match block {
                            TextOrImageContent::Image(image) => Some(image),
                            TextOrImageContent::Text(_) => None,
                        })
                        .collect()
                } else {
                    Vec::new()
                };

                let has_text = !text_result.is_empty();
                let has_images = !image_content.is_empty();

                // Gemini 3+ takes images nested inside functionResponse.parts; Claude behind
                // Cloud Code Assist and Gemini < 3 still need a separate user turn.
                let multimodal = supports_multimodal_function_response(&model.id);

                let response_value = if has_text {
                    sanitize_surrogates(&text_result)
                } else if has_images {
                    "(see attached image)".to_string()
                } else {
                    String::new()
                };
                let image_parts: Vec<Value> = image_content
                    .iter()
                    .map(|image| {
                        json!({
                            "inlineData": { "mimeType": image.mime_type, "data": image.data },
                        })
                    })
                    .collect();

                let mut function_response = Map::new();
                function_response.insert("name".to_string(), json!(tool_result.tool_name));
                // The SDK documents "output" for success and "error" for failures.
                function_response.insert(
                    "response".to_string(),
                    if tool_result.is_error {
                        json!({ "error": response_value })
                    } else {
                        json!({ "output": response_value })
                    },
                );
                if has_images && multimodal {
                    function_response
                        .insert("parts".to_string(), Value::Array(image_parts.clone()));
                }
                if requires_id {
                    function_response.insert("id".to_string(), json!(tool_result.tool_call_id));
                }
                let function_response_part = json!({ "functionResponse": function_response });

                // Cloud Code Assist wants all function responses in one user turn.
                let merged = contents
                    .last_mut()
                    .filter(|last| last.get("role") == Some(&json!("user")))
                    .and_then(|last| {
                        let parts = last.get_mut("parts")?.as_array_mut()?;
                        parts
                            .iter()
                            .any(|part| part.get("functionResponse").is_some())
                            .then(|| parts.push(function_response_part.clone()))
                    })
                    .is_some();
                if !merged {
                    contents.push(json!({ "role": "user", "parts": [function_response_part] }));
                }

                // Gemini < 3 gets the images as their own user message.
                if has_images && !multimodal {
                    let mut parts = vec![json!({ "text": "Tool result image:" })];
                    parts.extend(image_parts);
                    contents.push(json!({ "role": "user", "parts": parts }));
                }
            }
        }
    }

    contents
}

/// `JSON_SCHEMA_META_DECLARATIONS`
fn json_schema_meta_declarations() -> &'static BTreeSet<&'static str> {
    static DECLARATIONS: OnceLock<BTreeSet<&'static str>> = OnceLock::new();
    DECLARATIONS.get_or_init(|| {
        [
            "$schema",
            "$id",
            "$anchor",
            "$dynamicAnchor",
            "$vocabulary",
            "$comment",
            "$defs",
            // pre-draft-2019-09 equivalent of $defs
            "definitions",
        ]
        .into_iter()
        .collect()
    })
}

/// `sanitizeForOpenApi(schema)` — drops the meta declarations, recursively.
fn sanitize_for_openapi(schema: &Value) -> Value {
    let Some(object) = schema.as_object() else {
        return schema.clone();
    };
    let mut result = Map::new();
    for (key, value) in object {
        if json_schema_meta_declarations().contains(key.as_str()) {
            continue;
        }
        result.insert(key.clone(), sanitize_for_openapi(value));
    }
    Value::Object(result)
}

/// `convertTools(tools, useParameters, supportsStrictMode)`
/// `parametersJsonSchema` carries full JSON Schema; `useParameters` switches to the
/// legacy OpenAPI 3.03 field, which Cloud Code Assist needs for Claude models because it
/// translates `parameters` into Anthropic's `input_schema`.
pub fn convert_tools(
    tools: &[Tool],
    use_parameters: bool,
    supports_strict_mode: bool,
) -> Result<Option<Value>, ConstrainedSamplingError> {
    if tools.is_empty() {
        return Ok(None);
    }
    let mut declarations = Vec::with_capacity(tools.len());
    for tool in tools {
        let strict = resolve_json_schema_strict_sampling(tool, supports_strict_mode)?;
        let parameters = get_json_schema_tool_parameters(tool, strict);
        let mut declaration = Map::new();
        declaration.insert("name".to_string(), json!(tool.name));
        declaration.insert("description".to_string(), json!(tool.description));
        if use_parameters {
            declaration.insert("parameters".to_string(), sanitize_for_openapi(&parameters));
        } else {
            declaration.insert("parametersJsonSchema".to_string(), parameters);
        }
        declarations.push(Value::Object(declaration));
    }
    Ok(Some(json!([{ "functionDeclarations": declarations }])))
}

/// `FunctionCallingConfigMode`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionCallingConfigMode {
    Auto,
    None,
    Any,
    Validated,
}

impl FunctionCallingConfigMode {
    pub fn as_str(self) -> &'static str {
        match self {
            FunctionCallingConfigMode::Auto => "AUTO",
            FunctionCallingConfigMode::None => "NONE",
            FunctionCallingConfigMode::Any => "ANY",
            FunctionCallingConfigMode::Validated => "VALIDATED",
        }
    }
}

/// `mapToolChoice(choice)`
pub fn map_tool_choice(choice: &str) -> FunctionCallingConfigMode {
    match choice {
        "auto" => FunctionCallingConfigMode::Auto,
        "none" => FunctionCallingConfigMode::None,
        "any" => FunctionCallingConfigMode::Any,
        _ => FunctionCallingConfigMode::Auto,
    }
}

/// `resolveGoogleFunctionCallingMode(tools, toolChoice, supportsStrictMode)`
pub fn resolve_google_function_calling_mode(
    tools: &[Tool],
    tool_choice: Option<&str>,
    supports_strict_mode: bool,
) -> Result<Option<FunctionCallingConfigMode>, ConstrainedSamplingError> {
    let mut use_strict_mode = false;
    for tool in tools {
        if resolve_json_schema_strict_sampling(tool, supports_strict_mode)? == Some(true) {
            use_strict_mode = true;
        }
    }
    if matches!(tool_choice, Some("none") | Some("any")) {
        return Ok(Some(map_tool_choice(tool_choice.expect("matched"))));
    }
    if use_strict_mode {
        return Ok(Some(FunctionCallingConfigMode::Validated));
    }
    Ok(tool_choice.map(map_tool_choice))
}

/// `mapStopReasonString(reason)` — everything but STOP and MAX_TOKENS is an error.
pub fn map_stop_reason(reason: &str) -> StopReason {
    match reason {
        "STOP" => StopReason::Stop,
        "MAX_TOKENS" => StopReason::Length,
        _ => StopReason::Error,
    }
}
