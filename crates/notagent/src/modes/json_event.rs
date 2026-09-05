use serde_json::{Map, Value, json};

use notagent_agent::types::{AgentEvent, AgentToolResult};

use crate::core::agent_session::{AgentSessionEvent, CompactionReason, SummarizationSource};
use crate::core::compaction::compaction::CompactionResult;

fn object(entries: Vec<(&str, Value)>) -> Value {
    let mut map = Map::new();
    for (key, value) in entries {
        map.insert(key.to_owned(), value);
    }
    Value::Object(map)
}

fn optional(map: &mut Map<String, Value>, key: &str, value: Option<Value>) {
    if let Some(value) = value {
        map.insert(key.to_owned(), value);
    }
}

fn tool_result_to_json(result: &AgentToolResult) -> Value {
    let mut map = Map::new();
    map.insert(
        "content".to_owned(),
        serde_json::to_value(&result.content).unwrap_or(Value::Null),
    );
    optional(&mut map, "details", result.details.clone());
    optional(
        &mut map,
        "usage",
        result
            .usage
            .as_ref()
            .map(|usage| serde_json::to_value(usage).unwrap_or(Value::Null)),
    );
    optional(
        &mut map,
        "addedToolNames",
        result
            .added_tool_names
            .as_ref()
            .map(|names| serde_json::to_value(names).unwrap_or(Value::Null)),
    );
    optional(&mut map, "terminate", result.terminate.map(Value::from));
    Value::Object(map)
}

pub fn compaction_result_to_json(result: &CompactionResult) -> Value {
    let mut map = Map::new();
    map.insert("summary".to_owned(), Value::from(result.summary.clone()));
    map.insert(
        "firstKeptEntryId".to_owned(),
        Value::from(result.first_kept_entry_id.clone()),
    );
    map.insert("tokensBefore".to_owned(), Value::from(result.tokens_before));
    optional(
        &mut map,
        "estimatedTokensAfter",
        result.estimated_tokens_after.map(Value::from),
    );
    if result.dropped_tokens > 0 {
        map.insert(
            "droppedTokens".to_owned(),
            Value::from(result.dropped_tokens),
        );
    }
    optional(
        &mut map,
        "usage",
        result
            .usage
            .as_ref()
            .map(|usage| serde_json::to_value(usage).unwrap_or(Value::Null)),
    );
    optional(&mut map, "details", result.details.clone());
    Value::Object(map)
}

/// `strip_partial` implements the one transformation of `toJsonEvent`.
fn agent_event_to_json(event: &AgentEvent, strip_partial: bool) -> Value {
    let message = |value: &notagent_agent::types::AgentMessage| {
        serde_json::to_value(value).unwrap_or(Value::Null)
    };
    match event {
        AgentEvent::AgentStart => object(vec![("type", Value::from("agent_start"))]),
        AgentEvent::AgentEnd { messages } => object(vec![
            ("type", Value::from("agent_end")),
            (
                "messages",
                serde_json::to_value(messages).unwrap_or(Value::Null),
            ),
        ]),
        AgentEvent::TurnStart => object(vec![("type", Value::from("turn_start"))]),
        AgentEvent::TurnEnd {
            message: turn_message,
            tool_results,
        } => object(vec![
            ("type", Value::from("turn_end")),
            ("message", message(turn_message)),
            (
                "toolResults",
                serde_json::to_value(tool_results).unwrap_or(Value::Null),
            ),
        ]),
        AgentEvent::MessageStart {
            message: start_message,
        } => object(vec![
            ("type", Value::from("message_start")),
            ("message", message(start_message)),
        ]),
        AgentEvent::MessageUpdate {
            message: update_message,
            assistant_message_event,
        } => {
            let mut assistant_event =
                serde_json::to_value(assistant_message_event).unwrap_or(Value::Null);
            if strip_partial && let Value::Object(map) = &mut assistant_event {
                map.remove("partial");
            }
            object(vec![
                ("type", Value::from("message_update")),
                ("message", message(update_message)),
                ("assistantMessageEvent", assistant_event),
            ])
        }
        AgentEvent::MessageEnd {
            message: end_message,
        } => object(vec![
            ("type", Value::from("message_end")),
            ("message", message(end_message)),
        ]),
        AgentEvent::ToolExecutionStart {
            tool_call_id,
            tool_name,
            args,
        } => object(vec![
            ("type", Value::from("tool_execution_start")),
            ("toolCallId", Value::from(tool_call_id.clone())),
            ("toolName", Value::from(tool_name.clone())),
            ("args", args.clone()),
        ]),
        AgentEvent::ToolExecutionUpdate {
            tool_call_id,
            tool_name,
            args,
            partial_result,
        } => object(vec![
            ("type", Value::from("tool_execution_update")),
            ("toolCallId", Value::from(tool_call_id.clone())),
            ("toolName", Value::from(tool_name.clone())),
            ("args", args.clone()),
            ("partialResult", tool_result_to_json(partial_result)),
        ]),
        AgentEvent::ToolExecutionEnd {
            tool_call_id,
            tool_name,
            result,
            is_error,
        } => object(vec![
            ("type", Value::from("tool_execution_end")),
            ("toolCallId", Value::from(tool_call_id.clone())),
            ("toolName", Value::from(tool_name.clone())),
            ("result", tool_result_to_json(result)),
            ("isError", Value::from(*is_error)),
        ]),
    }
}

fn summarization_source_fields(source: SummarizationSource) -> Vec<(&'static str, Value)> {
    match source {
        SummarizationSource::BranchSummary => vec![("source", Value::from("branchSummary"))],
        SummarizationSource::Compaction(reason) => vec![
            ("source", Value::from("compaction")),
            ("reason", Value::from(reason.as_str())),
        ],
    }
}

/// The session event as it goes on the wire, with the streaming snapshot removed.
pub fn to_json_event(event: &AgentSessionEvent) -> Value {
    session_event_to_json(event, true)
}

/// The session event with every field kept, `partial` included.
pub fn session_event_to_json(event: &AgentSessionEvent, strip_partial: bool) -> Value {
    match event {
        AgentSessionEvent::Agent(agent_event) => agent_event_to_json(agent_event, strip_partial),
        AgentSessionEvent::AgentEnd {
            messages,
            will_retry,
        } => object(vec![
            ("type", Value::from("agent_end")),
            (
                "messages",
                serde_json::to_value(messages).unwrap_or(Value::Null),
            ),
            ("willRetry", Value::from(*will_retry)),
        ]),
        AgentSessionEvent::AgentSettled => object(vec![("type", Value::from("agent_settled"))]),
        AgentSessionEvent::PersistenceError { error_message } => json!({
            "type": "persistence_error", "errorMessage": error_message,
        }),
        AgentSessionEvent::QueueUpdate {
            steering,
            follow_up,
        } => object(vec![
            ("type", Value::from("queue_update")),
            ("steering", json!(steering)),
            ("followUp", json!(follow_up)),
        ]),
        AgentSessionEvent::CompactionStart { reason } => object(vec![
            ("type", Value::from("compaction_start")),
            ("reason", Value::from(reason.as_str())),
        ]),
        AgentSessionEvent::EntryAppended { entry } => object(vec![
            ("type", Value::from("entry_appended")),
            (
                "entry",
                serde_json::to_value(entry.as_ref()).unwrap_or(Value::Null),
            ),
        ]),
        AgentSessionEvent::SessionInfoChanged { name } => {
            // `name: string | undefined` is a declared field of the event and
            // stays present, unlike the optional fields that vanish.
            object(vec![
                ("type", Value::from("session_info_changed")),
                ("name", name.clone().map(Value::from).unwrap_or(Value::Null)),
            ])
        }
        AgentSessionEvent::ThinkingLevelChanged { level } => object(vec![
            ("type", Value::from("thinking_level_changed")),
            ("level", serde_json::to_value(level).unwrap_or(Value::Null)),
        ]),
        AgentSessionEvent::CompactionEnd {
            reason,
            result,
            aborted,
            will_retry,
            error_message,
        } => {
            let mut map = Map::new();
            map.insert("type".to_owned(), Value::from("compaction_end"));
            map.insert("reason".to_owned(), Value::from(reason.as_str()));
            map.insert(
                "result".to_owned(),
                result
                    .as_ref()
                    .map(|result| compaction_result_to_json(result))
                    .unwrap_or(Value::Null),
            );
            map.insert("aborted".to_owned(), Value::from(*aborted));
            map.insert("willRetry".to_owned(), Value::from(*will_retry));
            optional(
                &mut map,
                "errorMessage",
                error_message.clone().map(Value::from),
            );
            Value::Object(map)
        }
        AgentSessionEvent::AutoRetryStart {
            attempt,
            max_attempts,
            delay_ms,
            error_message,
        } => object(vec![
            ("type", Value::from("auto_retry_start")),
            ("attempt", Value::from(*attempt)),
            ("maxAttempts", Value::from(*max_attempts)),
            ("delayMs", Value::from(*delay_ms)),
            ("errorMessage", Value::from(error_message.clone())),
        ]),
        AgentSessionEvent::AutoRetryEnd {
            success,
            attempt,
            final_error,
        } => {
            let mut map = Map::new();
            map.insert("type".to_owned(), Value::from("auto_retry_end"));
            map.insert("success".to_owned(), Value::from(*success));
            map.insert("attempt".to_owned(), Value::from(*attempt));
            optional(&mut map, "finalError", final_error.clone().map(Value::from));
            Value::Object(map)
        }
        AgentSessionEvent::SummarizationRetryScheduled {
            attempt,
            max_attempts,
            delay_ms,
            error_message,
        } => object(vec![
            ("type", Value::from("summarization_retry_scheduled")),
            ("attempt", Value::from(*attempt)),
            ("maxAttempts", Value::from(*max_attempts)),
            ("delayMs", Value::from(*delay_ms)),
            ("errorMessage", Value::from(error_message.clone())),
        ]),
        AgentSessionEvent::SummarizationRetryAttemptStart { source } => {
            let mut entries = vec![("type", Value::from("summarization_retry_attempt_start"))];
            entries.extend(summarization_source_fields(*source));
            object(entries)
        }
        AgentSessionEvent::SummarizationRetryFinished => {
            object(vec![("type", Value::from("summarization_retry_finished"))])
        }
        AgentSessionEvent::BashExecutionUpdate { id, delta } => {
            let mut map = Map::new();
            map.insert("type".to_owned(), Value::from("bash_execution_update"));
            optional(&mut map, "id", id.clone().map(Value::from));
            map.insert("delta".to_owned(), Value::from(delta.clone()));
            Value::Object(map)
        }
    }
}

/// Reason strings of [`CompactionReason`], for callers that build events.
pub fn compaction_reason_str(reason: CompactionReason) -> &'static str {
    reason.as_str()
}
