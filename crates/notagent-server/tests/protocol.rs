//! Port of `packages/server/test/protocol.test.ts`.

use notagent_ai::{
    AssistantContent, AssistantMessage, StopReason, TextContent as AiTextContent,
    TextOrImageContent, ToolCall, ToolResultMessage, Usage, UsageCost, UserContent, UserMessage,
};
use notagent_ai::{Modality, Model, ModelCost as AiModelCost};
use notagent_protocol::{
    AssistantStopReason, AssistantTranscriptItem, HelloTag, ModelRef, ProtocolVersionTag,
    ServerHello, ServerMessage, ServerSnapshot, SessionPhase, SessionSnapshot, ThinkingLevel,
    ToolTranscriptItem, TranscriptItem, encode_server_message,
};
use notagent_server::protocol::{
    AssistantTranscriptOptions, ToolTranscriptOptions, UserTranscriptOptions,
    to_protocol_assistant_message, to_protocol_model_metadata, to_protocol_tool_result_message,
    to_protocol_user_message,
};
use serde_json::json;

fn empty_usage() -> Usage {
    Usage {
        input: 0,
        output: 0,
        cache_read: 0,
        cache_write: 0,
        cache_write1h: None,
        reasoning: None,
        total_tokens: Some(0),
        cost: UsageCost {
            input: 0.0,
            output: 0.0,
            cache_read: 0.0,
            cache_write: 0.0,
            total: 0.0,
        },
    }
}

fn assistant_message(content: Vec<AssistantContent>, stop_reason: StopReason) -> AssistantMessage {
    AssistantMessage {
        content,
        api: "test-api".to_owned(),
        provider: "test-provider".to_owned(),
        model: "model-1".to_owned(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: empty_usage(),
        stop_reason,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: 123,
    }
}

fn test_model() -> Model {
    Model {
        id: "model-1".to_owned(),
        name: "Model One".to_owned(),
        api: "test-api".to_owned(),
        provider: "test-provider".to_owned(),
        base_url: "https://example.test".to_owned(),
        reasoning: true,
        thinking_level_map: None,
        input: vec![Modality::Text, Modality::Image],
        cost: AiModelCost {
            input: 1.0,
            output: 2.0,
            cache_read: 0.1,
            cache_write: 0.2,
            tiers: None,
        },
        context_window: 100_000,
        max_tokens: 10_000,
        sampling_params: None,
        headers: None,
        compat: None,
    }
}

/// Port of `assertValidServerPayload`.
fn assert_valid_server_payload(item: TranscriptItem) {
    let hello = ServerMessage::Hello(ServerHello {
        kind: HelloTag,
        version: ProtocolVersionTag,
        connection_id: "connection-1".to_owned(),
        snapshot: ServerSnapshot {
            server_id: "server-1".to_owned(),
            protocol_version: ProtocolVersionTag,
            revision: 0,
            sessions: vec![notagent_protocol::SessionMetadata {
                id: "session-1".to_owned(),
                created_at: 1,
                updated_at: Some(1),
                parent_session_id: None,
                session_name: Some("Session one".to_owned()),
                cwd: Some("/workspace".to_owned()),
            }],
            models: vec![to_protocol_model_metadata(&test_model(), true).expect("maps")],
        },
    });
    encode_server_message(&hello, None).expect("hello encodes");

    let event = ServerMessage::Event(notagent_protocol::EventEnvelope {
        kind: notagent_protocol::EventTag,
        event: notagent_protocol::ServerEvent::SessionSnapshot(
            notagent_protocol::SessionSnapshotEvent {
                kind: notagent_protocol::SessionSnapshotTag,
                snapshot: SessionSnapshot {
                    id: "session-1".to_owned(),
                    name: None,
                    cwd: "/workspace".to_owned(),
                    created_at: 1,
                    updated_at: 1,
                    phase: SessionPhase::Idle,
                    model: ModelRef::new("test-provider", "model-1"),
                    thinking_level: ThinkingLevel::Off,
                    attached: true,
                    locked: true,
                    revision: 1,
                    transcript: vec![item],
                    queued_steer: vec![],
                    queued_steer_count: 0,
                },
            },
        ),
    });
    encode_server_message(&event, None).expect("event encodes");
}

#[test]
fn maps_model_metadata_and_produces_protocol_valid_output() {
    let result = to_protocol_model_metadata(&test_model(), true).expect("maps");
    assert_eq!(result.provider, "test-provider");
    assert_eq!(result.id, "model-1");
    assert_eq!(result.api, "test-api");
    assert_eq!(
        result.input,
        vec![
            notagent_protocol::ModelInput::Text,
            notagent_protocol::ModelInput::Image
        ]
    );
    assert!(result.authenticated);
    assert!(
        result
            .supported_thinking_levels
            .contains(&ThinkingLevel::Off)
    );
}

#[test]
fn exhaustively_maps_assistant_content_and_stop_reasons() {
    let message = AssistantMessage {
        usage: Usage {
            input: 1,
            output: 2,
            cache_read: 3,
            cache_write: 4,
            cache_write1h: None,
            reasoning: None,
            total_tokens: Some(10),
            cost: UsageCost {
                input: 0.1,
                output: 0.2,
                cache_read: 0.3,
                cache_write: 0.4,
                total: 1.0,
            },
        },
        ..assistant_message(
            vec![
                AssistantContent::Text(AiTextContent::new("hello")),
                AssistantContent::Thinking(notagent_ai::ThinkingContent {
                    thinking: "hmm".to_owned(),
                    thinking_signature: None,
                    redacted: Some(false),
                    extra: serde_json::Map::new(),
                }),
                AssistantContent::ToolCall(ToolCall {
                    id: "call-1".to_owned(),
                    name: "read".to_owned(),
                    arguments: json!({ "path": "README.md" }).as_object().unwrap().clone(),
                    thought_signature: None,
                    namespace: None,
                    extra: serde_json::Map::new(),
                }),
            ],
            StopReason::ToolUse,
        )
    };

    let result = to_protocol_assistant_message(
        &message,
        AssistantTranscriptOptions {
            id: "message-1".to_owned(),
        },
    )
    .expect("maps");
    let AssistantTranscriptItem::Complete(complete) = &result else {
        panic!("expected a complete item")
    };
    assert_eq!(complete.id, "message-1");
    assert_eq!(complete.stop_reason, AssistantStopReason::ToolUse);
    assert_eq!(complete.model, ModelRef::new("test-provider", "model-1"));
    assert_eq!(
        serde_json::to_value(&complete.content).expect("serializes"),
        json!([
            { "type": "text", "text": "hello" },
            { "type": "thinking", "thinking": "hmm", "redacted": false },
            { "type": "toolCall", "toolCallId": "call-1", "toolName": "read", "input": { "path": "README.md" } },
        ])
    );
    assert_valid_server_payload(TranscriptItem::Assistant(result));
}

#[test]
fn maps_user_and_tool_messages() {
    // The TS case additionally feeds a circular `details` object; cycles are not
    // representable with `serde_json::Value` (deviation class 1).
    let user = UserMessage {
        content: UserContent::Text("hello".to_owned()),
        timestamp: 1,
    };
    let call = ToolCall {
        id: "call-1".to_owned(),
        name: "read".to_owned(),
        arguments: json!({ "path": "README.md" }).as_object().unwrap().clone(),
        thought_signature: None,
        namespace: None,
        extra: serde_json::Map::new(),
    };
    let tool = ToolResultMessage {
        tool_call_id: "call-1".to_owned(),
        tool_name: "read".to_owned(),
        content: vec![TextOrImageContent::Text(AiTextContent::new("result"))],
        details: Some(json!({ "lines": 3 })),
        usage: None,
        added_tool_names: None,
        is_error: false,
        timestamp: 2,
    };

    let user_result = to_protocol_user_message(
        &user,
        UserTranscriptOptions {
            id: "user-1".to_owned(),
        },
    )
    .expect("maps");
    assert_eq!(user_result.id, "user-1");
    assert_eq!(
        serde_json::to_value(&user_result.content).expect("serializes"),
        json!([{ "type": "text", "text": "hello" }])
    );
    assert_valid_server_payload(TranscriptItem::User(user_result));

    let tool_result = to_protocol_tool_result_message(
        &tool,
        ToolTranscriptOptions {
            id: "tool-1".to_owned(),
            call: call.clone(),
        },
    )
    .expect("maps");
    let ToolTranscriptItem::Complete(complete) = &tool_result else {
        panic!("expected a complete tool item")
    };
    assert_eq!(complete.id, "tool-1");
    assert_eq!(complete.tool_name, "read");
    assert_eq!(complete.input, json!({ "path": "README.md" }));
    assert_eq!(complete.details, Some(json!({ "lines": 3 })));
    assert_valid_server_payload(TranscriptItem::Tool(tool_result));
}

#[test]
fn rejects_tool_results_associated_with_a_different_call() {
    let call = ToolCall {
        id: "call-1".to_owned(),
        name: "read".to_owned(),
        arguments: json!({ "path": "README.md" }).as_object().unwrap().clone(),
        thought_signature: None,
        namespace: None,
        extra: serde_json::Map::new(),
    };
    let result = ToolResultMessage {
        tool_call_id: "call-2".to_owned(),
        tool_name: "read".to_owned(),
        content: vec![TextOrImageContent::Text(AiTextContent::new("result"))],
        details: None,
        usage: None,
        added_tool_names: None,
        is_error: false,
        timestamp: 2,
    };

    let error = to_protocol_tool_result_message(
        &result,
        ToolTranscriptOptions {
            id: "tool-1".to_owned(),
            call: call.clone(),
        },
    )
    .expect_err("rejects");
    assert!(
        error.to_string().to_lowercase().contains("tool call"),
        "{error}"
    );

    let mismatched_name = ToolResultMessage {
        tool_call_id: "call-1".to_owned(),
        tool_name: "write".to_owned(),
        ..result
    };
    let error = to_protocol_tool_result_message(
        &mismatched_name,
        ToolTranscriptOptions {
            id: "tool-1".to_owned(),
            call,
        },
    )
    .expect_err("rejects");
    assert!(
        error.to_string().to_lowercase().contains("tool call"),
        "{error}"
    );
}

#[test]
fn derives_streaming_status_from_a_pending_stop_reason() {
    let message = assistant_message(
        vec![AssistantContent::Text(AiTextContent::new("partial"))],
        StopReason::Pending,
    );
    let result = to_protocol_assistant_message(
        &message,
        AssistantTranscriptOptions {
            id: "message-pending".to_owned(),
        },
    )
    .expect("maps");
    assert!(matches!(result, AssistantTranscriptItem::Streaming(_)));
    let encoded = serde_json::to_value(&result).expect("serializes");
    assert!(encoded.get("stopReason").is_none(), "{encoded}");
    assert_valid_server_payload(TranscriptItem::Assistant(result));
}

#[test]
fn preserves_optional_non_empty_assistant_error_messages() {
    let message = assistant_message(vec![], StopReason::Error);
    let result = to_protocol_assistant_message(
        &message,
        AssistantTranscriptOptions {
            id: "message-error".to_owned(),
        },
    )
    .expect("maps");
    let AssistantTranscriptItem::Error(error_item) = &result else {
        panic!("expected an error item")
    };
    assert!(error_item.error_message.is_none());
    assert_valid_server_payload(TranscriptItem::Assistant(result));

    let empty = AssistantMessage {
        error_message: Some(String::new()),
        ..message.clone()
    };
    assert!(
        to_protocol_assistant_message(
            &empty,
            AssistantTranscriptOptions {
                id: "message-error".to_owned()
            }
        )
        .is_err()
    );

    let failed = AssistantMessage {
        error_message: Some("failed".to_owned()),
        ..message
    };
    let result = to_protocol_assistant_message(
        &failed,
        AssistantTranscriptOptions {
            id: "message-error".to_owned(),
        },
    )
    .expect("maps");
    let AssistantTranscriptItem::Error(error_item) = &result else {
        panic!("expected an error item")
    };
    assert_eq!(error_item.error_message.as_deref(), Some("failed"));
    assert_valid_server_payload(TranscriptItem::Assistant(result));
}

#[test]
fn rejects_invalid_source_identifiers_and_timestamps() {
    let message = assistant_message(
        vec![AssistantContent::ToolCall(ToolCall {
            id: String::new(),
            name: "read".to_owned(),
            arguments: serde_json::Map::new(),
            thought_signature: None,
            namespace: None,
            extra: serde_json::Map::new(),
        })],
        StopReason::ToolUse,
    );
    let error = to_protocol_assistant_message(
        &message,
        AssistantTranscriptOptions {
            id: "assistant-1".to_owned(),
        },
    )
    .expect_err("rejects");
    assert!(
        error.to_string().to_lowercase().contains("tool call id"),
        "{error}"
    );

    // TS uses `Number.NaN`; the Rust timestamp is an i64, so the equivalent
    // violation is a negative value (`!Number.isSafeInteger(value) || value < 0`).
    let user = UserMessage {
        content: UserContent::Text("hello".to_owned()),
        timestamp: -1,
    };
    let error = to_protocol_user_message(
        &user,
        UserTranscriptOptions {
            id: "user-1".to_owned(),
        },
    )
    .expect_err("rejects");
    assert!(
        error.to_string().to_lowercase().contains("timestamp"),
        "{error}"
    );
}

// `rejects lossy tool input conversions` and `rejects sparse execution data`
// have no Rust equivalent: Infinity, bigint, undefined, cycles and array holes
// cannot be represented by `serde_json::Value` (deviation class 1).
