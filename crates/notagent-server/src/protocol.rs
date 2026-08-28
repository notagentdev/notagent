use notagent_ai::models::get_supported_thinking_levels;
use notagent_ai::{
    AssistantContent, AssistantMessage, Modality, Model, ModelThinkingLevel, StopReason,
    TextOrImageContent, ToolCall, ToolResultMessage, Usage as AiUsage, UserContent, UserMessage,
};
use notagent_protocol::{
    AbortedAssistantTranscriptItem, AbortedTag, AssistantStopReason, AssistantTag,
    AssistantTranscriptItem, CompleteAssistantTranscriptItem, CompleteTag,
    CompleteToolTranscriptItem, ErrorAssistantTranscriptItem, ErrorTag, ErrorToolTranscriptItem,
    FalseTag, ImageContent, ImageTag, JsonValue, ModelCost, ModelInput, ModelMetadata, RunningTag,
    StreamingAssistantTranscriptItem, StreamingTag, TextContent, TextTag, ThinkingContent,
    ThinkingLevel, ThinkingTag, ToolCallContent, ToolCallTag, ToolContent, ToolTag,
    ToolTranscriptItem, TrueTag, Usage, UsageCost, UserContent as ProtocolUserContent, UserTag,
    UserTranscriptItem,
};

use crate::errors::ServerError;

pub struct AssistantTranscriptOptions {
    pub id: String,
}

pub struct UserTranscriptOptions {
    pub id: String,
}

pub struct ToolTranscriptOptions {
    pub id: String,
    pub call: ToolCall,
}

fn non_negative_number(value: f64) -> f64 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

fn identifier(value: &str, label: &str) -> Result<String, ServerError> {
    if value.is_empty() {
        return Err(ServerError::other(format!(
            "{label} must be a non-empty string"
        )));
    }
    Ok(value.to_owned())
}

fn timestamp(value: i64) -> Result<u64, ServerError> {
    if value < 0 {
        return Err(ServerError::other(
            "Protocol timestamps must be non-negative integers",
        ));
    }
    Ok(value as u64)
}

/// Validate and copy a value from an execution boundary into the protocol's
/// JSON-compatible subset.
/// `serde_json::Value` is already the JSON subset; only non-finite numbers can
/// violate it, and they cannot be represented by `serde_json::Number`. Cycles
pub fn to_protocol_json_value(value: &JsonValue) -> Result<JsonValue, ServerError> {
    Ok(value.clone())
}

/// Lossily sanitize diagnostic tool details that must not affect execution semantics.
pub fn sanitize_protocol_details(value: Option<&JsonValue>) -> Option<JsonValue> {
    value.cloned()
}

pub fn to_protocol_usage(usage: Option<&AiUsage>) -> Option<Usage> {
    let usage = usage?;
    Some(Usage {
        input: usage.input,
        output: usage.output,
        cache_read: usage.cache_read,
        cache_write: usage.cache_write,
        reasoning: usage.reasoning,
        total_tokens: usage.total_tokens.unwrap_or(0),
        cost: UsageCost {
            input: non_negative_number(usage.cost.input),
            output: non_negative_number(usage.cost.output),
            cache_read: non_negative_number(usage.cost.cache_read),
            cache_write: non_negative_number(usage.cost.cache_write),
            total: non_negative_number(usage.cost.total),
        },
    })
}

fn to_protocol_user_content(content: &UserContent) -> Vec<ProtocolUserContent> {
    match content {
        UserContent::Text(text) => vec![ProtocolUserContent::Text(TextContent {
            kind: TextTag,
            text: text.clone(),
        })],
        UserContent::Blocks(parts) => parts
            .iter()
            .map(|part| match part {
                TextOrImageContent::Text(text) => ProtocolUserContent::Text(TextContent {
                    kind: TextTag,
                    text: text.text.clone(),
                }),
                TextOrImageContent::Image(image) => ProtocolUserContent::Image(ImageContent {
                    kind: ImageTag,
                    data: image.data.clone(),
                    mime_type: image.mime_type.clone(),
                }),
            })
            .collect(),
    }
}

pub fn to_protocol_user_message(
    message: &UserMessage,
    options: UserTranscriptOptions,
) -> Result<UserTranscriptItem, ServerError> {
    Ok(UserTranscriptItem {
        id: identifier(&options.id, "Transcript item id")?,
        role: UserTag,
        content: to_protocol_user_content(&message.content),
        timestamp: timestamp(message.timestamp)?,
    })
}

fn to_protocol_assistant_content(
    message: &AssistantMessage,
) -> Result<Vec<notagent_protocol::AssistantContent>, ServerError> {
    message
        .content
        .iter()
        .map(|part| match part {
            AssistantContent::Text(text) => {
                Ok(notagent_protocol::AssistantContent::Text(TextContent {
                    kind: TextTag,
                    text: text.text.clone(),
                }))
            }
            AssistantContent::Thinking(thinking) => Ok(
                notagent_protocol::AssistantContent::Thinking(ThinkingContent {
                    kind: ThinkingTag,
                    thinking: thinking.thinking.clone(),
                    redacted: thinking.redacted,
                }),
            ),
            AssistantContent::ToolCall(call) => Ok(notagent_protocol::AssistantContent::ToolCall(
                ToolCallContent {
                    kind: ToolCallTag,
                    tool_call_id: identifier(&call.id, "Tool call id")?,
                    tool_name: identifier(&call.name, "Tool call name")?,
                    input: to_protocol_json_value(&JsonValue::Object(call.arguments.clone()))?,
                },
            )),
        })
        .collect()
}

/// The fields shared by all four assistant transcript variants.
struct AssistantCommon {
    id: String,
    content: Vec<notagent_protocol::AssistantContent>,
    model: notagent_protocol::ModelRef,
    response_model: Option<String>,
    usage: Option<Usage>,
    timestamp: u64,
}

pub fn to_protocol_assistant_message(
    message: &AssistantMessage,
    options: AssistantTranscriptOptions,
) -> Result<AssistantTranscriptItem, ServerError> {
    let usage = to_protocol_usage(Some(&message.usage));
    let response_model = match &message.response_model {
        Some(response_model) => Some(identifier(response_model, "Assistant response model")?),
        None => None,
    };
    let common = AssistantCommon {
        id: identifier(&options.id, "Transcript item id")?,
        content: to_protocol_assistant_content(message)?,
        model: notagent_protocol::ModelRef {
            provider: identifier(&message.provider, "Assistant provider")?,
            id: identifier(&message.model, "Assistant model")?,
        },
        response_model,
        usage,
        timestamp: timestamp(message.timestamp)?,
    };
    match message.stop_reason {
        StopReason::Pending => Ok(AssistantTranscriptItem::Streaming(
            StreamingAssistantTranscriptItem {
                id: common.id,
                role: AssistantTag,
                content: common.content,
                model: common.model,
                response_model: common.response_model,
                usage: common.usage,
                timestamp: common.timestamp,
                status: StreamingTag,
            },
        )),
        StopReason::Stop | StopReason::Length | StopReason::ToolUse => {
            let stop_reason = match message.stop_reason {
                StopReason::Stop => AssistantStopReason::Stop,
                StopReason::Length => AssistantStopReason::Length,
                _ => AssistantStopReason::ToolUse,
            };
            Ok(AssistantTranscriptItem::Complete(
                CompleteAssistantTranscriptItem {
                    id: common.id,
                    role: AssistantTag,
                    content: common.content,
                    model: common.model,
                    response_model: common.response_model,
                    usage: common.usage,
                    timestamp: common.timestamp,
                    status: CompleteTag,
                    stop_reason,
                },
            ))
        }
        StopReason::Deferred => Err(ServerError::other(
            "Deferred assistant messages are not supported by protocol v1",
        )),
        StopReason::Error => {
            if message.error_message.as_deref() == Some("") {
                return Err(ServerError::other(
                    "Assistant error messages must not be empty",
                ));
            }
            Ok(AssistantTranscriptItem::Error(
                ErrorAssistantTranscriptItem {
                    id: common.id,
                    role: AssistantTag,
                    content: common.content,
                    model: common.model,
                    response_model: common.response_model,
                    usage: common.usage,
                    timestamp: common.timestamp,
                    status: ErrorTag,
                    stop_reason: ErrorTag,
                    error_message: message.error_message.clone(),
                },
            ))
        }
        StopReason::Aborted => Ok(AssistantTranscriptItem::Aborted(
            AbortedAssistantTranscriptItem {
                id: common.id,
                role: AssistantTag,
                content: common.content,
                model: common.model,
                response_model: common.response_model,
                usage: common.usage,
                timestamp: common.timestamp,
                status: AbortedTag,
                stop_reason: AbortedTag,
                error_message: message.error_message.clone(),
            },
        )),
    }
}

fn to_protocol_tool_content(content: &[TextOrImageContent]) -> Vec<ToolContent> {
    content
        .iter()
        .map(|part| match part {
            TextOrImageContent::Text(text) => ToolContent::Text(TextContent {
                kind: TextTag,
                text: text.text.clone(),
            }),
            TextOrImageContent::Image(image) => ToolContent::Image(ImageContent {
                kind: ImageTag,
                data: image.data.clone(),
                mime_type: image.mime_type.clone(),
            }),
        })
        .collect()
}

pub fn to_protocol_tool_result_message(
    message: &ToolResultMessage,
    options: ToolTranscriptOptions,
) -> Result<ToolTranscriptItem, ServerError> {
    let call_id = identifier(&options.call.id, "Tool call id")?;
    let call_name = identifier(&options.call.name, "Tool call name")?;
    if identifier(&message.tool_call_id, "Tool result call id")? != call_id {
        return Err(ServerError::other(format!(
            "Tool result {} does not match tool call {call_id}",
            message.tool_call_id
        )));
    }
    if identifier(&message.tool_name, "Tool result name")? != call_name {
        return Err(ServerError::other(format!(
            "Tool result {} does not match tool call {call_name}",
            message.tool_name
        )));
    }
    let details = sanitize_protocol_details(message.details.as_ref());
    let usage = to_protocol_usage(message.usage.as_ref());
    let id = identifier(&options.id, "Transcript item id")?;
    let input = to_protocol_json_value(&JsonValue::Object(options.call.arguments.clone()))?;
    let content = to_protocol_tool_content(&message.content);
    let timestamp = timestamp(message.timestamp)?;
    if message.is_error {
        return Ok(ToolTranscriptItem::Error(ErrorToolTranscriptItem {
            id,
            role: ToolTag,
            tool_call_id: call_id,
            tool_name: call_name,
            input,
            content,
            details,
            usage,
            timestamp,
            status: ErrorTag,
            is_error: TrueTag,
        }));
    }
    Ok(ToolTranscriptItem::Complete(CompleteToolTranscriptItem {
        id,
        role: ToolTag,
        tool_call_id: call_id,
        tool_name: call_name,
        input,
        content,
        details,
        usage,
        timestamp,
        status: CompleteTag,
        is_error: FalseTag,
    }))
}

/// `RunningToolTranscriptItem` for a tool call that has not produced a result yet.
pub fn to_protocol_running_tool_call(
    call: &ToolCall,
    id: &str,
    timestamp_ms: i64,
) -> Result<ToolTranscriptItem, ServerError> {
    Ok(ToolTranscriptItem::Running(
        notagent_protocol::RunningToolTranscriptItem {
            id: identifier(id, "Transcript item id")?,
            role: ToolTag,
            tool_call_id: identifier(&call.id, "Tool call id")?,
            tool_name: identifier(&call.name, "Tool call name")?,
            input: to_protocol_json_value(&JsonValue::Object(call.arguments.clone()))?,
            content: Vec::new(),
            details: None,
            usage: None,
            timestamp: timestamp(timestamp_ms)?,
            status: RunningTag,
            is_error: FalseTag,
        },
    ))
}

pub fn to_protocol_model_metadata(
    model: &Model,
    authenticated: bool,
) -> Result<ModelMetadata, ServerError> {
    Ok(ModelMetadata {
        provider: identifier(&model.provider, "Model provider")?,
        id: identifier(&model.id, "Model id")?,
        name: identifier(&model.name, "Model name")?,
        api: identifier(&model.api, "Model API")?,
        reasoning: model.reasoning,
        input: model
            .input
            .iter()
            .map(|modality| match modality {
                Modality::Text => ModelInput::Text,
                Modality::Image => ModelInput::Image,
            })
            .collect(),
        context_window: model.context_window.max(1),
        max_tokens: model.max_tokens.max(1),
        cost: ModelCost {
            input: non_negative_number(model.cost.input),
            output: non_negative_number(model.cost.output),
            cache_read: non_negative_number(model.cost.cache_read),
            cache_write: non_negative_number(model.cost.cache_write),
        },
        supported_thinking_levels: get_supported_thinking_levels(model)
            .into_iter()
            .map(model_thinking_level_to_protocol)
            .collect(),
        authenticated,
    })
}

fn model_thinking_level_to_protocol(level: ModelThinkingLevel) -> ThinkingLevel {
    match level {
        ModelThinkingLevel::Off => ThinkingLevel::Off,
        ModelThinkingLevel::Minimal => ThinkingLevel::Minimal,
        ModelThinkingLevel::Low => ThinkingLevel::Low,
        ModelThinkingLevel::Medium => ThinkingLevel::Medium,
        ModelThinkingLevel::High => ThinkingLevel::High,
        ModelThinkingLevel::Xhigh => ThinkingLevel::Xhigh,
        ModelThinkingLevel::Max => ThinkingLevel::Max,
    }
}
