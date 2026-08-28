use notagent_ai::api::bedrock_converse_stream::{
    BedrockOptions, BedrockStreamState, build_command_input,
};
use notagent_ai::types::*;
use serde_json::{Map, Value, json};

const TIMESTAMP: i64 = 1_700_000_000_000;

fn base_model() -> Model {
    serde_json::from_value(json!({
        "id": "us.anthropic.claude-sonnet-4-5-20250929-v1:0",
        "name": "Claude Sonnet 4.5 (US)",
        "api": "bedrock-converse-stream",
        "provider": "amazon-bedrock",
        "baseUrl": "https://bedrock-runtime.us-east-1.amazonaws.com",
        "reasoning": true,
        "input": ["text", "image"],
        "cost": { "input": 3, "output": 15, "cacheRead": 0.3, "cacheWrite": 3.75 },
        "contextWindow": 200000,
        "maxTokens": 64000,
        "compat": { "supportsStrictMode": true },
    }))
    .expect("model")
}

fn nova_model() -> Model {
    Model {
        id: "amazon.nova-lite-v1:0".to_string(),
        name: "Nova Lite".to_string(),
        reasoning: false,
        compat: None,
        ..base_model()
    }
}

/// The generator pins the AWS environment so no ambient configuration leaks in.
fn options() -> BedrockOptions {
    BedrockOptions {
        cache_retention: Some(CacheRetention::None),
        env: Some(
            [
                ("AWS_REGION", "us-east-1"),
                ("AWS_ACCESS_KEY_ID", "x"),
                ("AWS_SECRET_ACCESS_KEY", "y"),
            ]
            .into_iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect::<ProviderEnv>(),
        ),
        ..BedrockOptions::default()
    }
}

fn capture_payload(context: &Context) -> Value {
    capture_payload_for(&base_model(), context)
}

fn capture_payload_for(model: &Model, context: &Context) -> Value {
    build_command_input(model, context, &options(), TIMESTAMP).expect("command input")
}

fn user_text(text: &str) -> Message {
    Message::User(UserMessage {
        content: UserContent::Text(text.to_string()),
        timestamp: TIMESTAMP,
    })
}

fn user_blocks(content: Vec<TextOrImageContent>) -> Message {
    Message::User(UserMessage {
        content: UserContent::Blocks(content),
        timestamp: TIMESTAMP,
    })
}

fn assistant(content: Vec<AssistantContent>, stop: StopReason) -> Message {
    Message::Assistant(AssistantMessage {
        content,
        api: "bedrock-converse-stream".to_string(),
        provider: "amazon-bedrock".to_string(),
        model: base_model().id,
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: stop,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: TIMESTAMP,
    })
}

fn text(text: &str) -> TextContent {
    TextContent {
        text: text.to_string(),
        ..TextContent::default()
    }
}

fn arguments(value: Value) -> Map<String, Value> {
    value.as_object().cloned().expect("object")
}

fn messages_of(payload: &Value) -> &Vec<Value> {
    payload["messages"].as_array().expect("messages")
}

fn context_with(messages: Vec<Message>) -> Context {
    Context {
        messages,
        ..Context::default()
    }
}

// ---------------------------------------------------------------------------
// Bedrock constrained sampling
// ---------------------------------------------------------------------------

#[test]
fn gates_native_strict_tool_use_by_model_capability() {
    let tool = |strict: &str| Tool {
        name: "lookup".to_string(),
        description: "Look up a value".to_string(),
        parameters: json!({
            "type": "object",
            "properties": { "value": { "type": "string" } },
            "required": ["value"],
        }),
        constrained_sampling: Some(
            serde_json::from_value(json!({ "type": "json_schema", "strict": strict }))
                .expect("constrained sampling"),
        ),
    };

    let mut context = context_with(vec![user_text("Use the tool")]);
    context.tools = Some(vec![tool("require")]);
    let payload = capture_payload(&context);
    assert_eq!(
        payload["toolConfig"]["tools"][0]["toolSpec"]["strict"],
        true
    );

    context.tools = Some(vec![tool("prefer")]);
    let nova_payload = capture_payload_for(&nova_model(), &context);
    assert_eq!(
        nova_payload["toolConfig"]["tools"][0]["toolSpec"].get("strict"),
        None
    );
}

// ---------------------------------------------------------------------------
// Bedrock tool arguments
// ---------------------------------------------------------------------------

#[test]
fn preserves_empty_property_names_in_streamed_tool_arguments() {
    let model = base_model();
    let mut state = BedrockStreamState::new(&model, TIMESTAMP);
    for item in [
        json!({ "messageStart": { "role": "assistant" } }),
        json!({ "contentBlockStart": { "contentBlockIndex": 0, "start": { "toolUse": { "toolUseId": "tool-1", "name": "edit" } } } }),
        json!({ "contentBlockDelta": { "contentBlockIndex": 0, "delta": { "toolUse": { "input": "{\"path\":\"/workspace/foobar/file.js\",\"edits\":[{\"oldText\":\"first\",\"newText\":\"updated first\"},{\"oldText\":\"second\",\"newText\":\"updated second\",\"\":\"\"}]}" } } } }),
        json!({ "contentBlockStop": { "contentBlockIndex": 0 } }),
        json!({ "messageStop": { "stopReason": "tool_use" } }),
    ] {
        state.process_item(&item).expect("item");
    }

    let AssistantContent::ToolCall(tool_call) = &state.output.content[0] else {
        panic!("expected a tool call, got {:?}", state.output.content[0]);
    };
    assert_eq!(tool_call.id, "tool-1");
    assert_eq!(tool_call.name, "edit");
    assert_eq!(
        Value::Object(tool_call.arguments.clone()),
        json!({
            "path": "/workspace/foobar/file.js",
            "edits": [
                { "oldText": "first", "newText": "updated first" },
                { "oldText": "second", "newText": "updated second", "": "" },
            ],
        })
    );
}

// ---------------------------------------------------------------------------
// bedrock convertMessages skips unknown content types
// ---------------------------------------------------------------------------

// the type system. `AssistantContent` and `TextOrImageContent` are closed enums in Rust
// and reject such a block at deserialization, so those states are unrepresentable and the
// empty by conversion being replaced or skipped — is covered here and by the blank and
// surrogate cases below, which reach the same code paths.

#[test]
fn replaces_user_messages_left_empty_by_conversion_with_a_placeholder() {
    let payload = capture_payload(&context_with(vec![user_blocks(Vec::new())]));

    let messages = messages_of(&payload);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["content"], json!([{ "text": "<empty>" }]));
}

#[test]
fn replaces_blank_user_string_content_with_a_placeholder() {
    let payload = capture_payload(&context_with(vec![user_text("   ")]));

    let messages = messages_of(&payload);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["content"], json!([{ "text": "<empty>" }]));
}

#[test]
fn filters_blank_user_text_blocks_when_other_content_remains() {
    let payload = capture_payload(&context_with(vec![user_blocks(vec![
        TextOrImageContent::Text(text("")),
        TextOrImageContent::Text(text("hello")),
    ])]));

    let messages = messages_of(&payload);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["content"], json!([{ "text": "hello" }]));
}

// (`String.fromCharCode(0xd83d)`), which a Rust `String` cannot hold — `sanitize_surrogates`
// is the identity here for exactly that reason (class 1, see the `sanitize_unicode.rs`
// ledger row). The branch they reach — a text block that converts to nothing — is covered
// with an empty text block instead.

#[test]
fn skips_assistant_text_blocks_that_convert_to_nothing() {
    let payload = capture_payload(&context_with(vec![assistant(
        vec![AssistantContent::Text(text(""))],
        StopReason::Stop,
    )]));

    assert_eq!(messages_of(&payload).len(), 0);
}

#[test]
fn replaces_blank_tool_result_content_with_a_placeholder() {
    let payload = capture_payload(&context_with(vec![Message::ToolResult(
        ToolResultMessage {
            tool_call_id: "tool-1".to_string(),
            tool_name: "tool".to_string(),
            content: vec![TextOrImageContent::Text(text(""))],
            details: None,
            usage: None,
            added_tool_names: None,
            is_error: false,
            timestamp: TIMESTAMP,
        },
    )]));

    let messages = messages_of(&payload);
    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0]["content"][0]["toolResult"]["content"],
        json!([{ "text": "<empty>" }])
    );
}

#[test]
fn skips_assistant_messages_left_empty_by_conversion() {
    let payload = capture_payload(&context_with(vec![assistant(Vec::new(), StopReason::Stop)]));

    assert_eq!(messages_of(&payload).len(), 0);
}

#[test]
fn removes_empty_property_names_only_from_replayed_bedrock_input() {
    let tool_arguments = json!({
        "path": "/workspace/foobar/file.js",
        "edits": [
            { "oldText": "first", "newText": "updated first" },
            { "oldText": "second", "newText": "updated second", "": "" },
        ],
    });
    let context = context_with(vec![
        assistant(
            vec![AssistantContent::ToolCall(ToolCall {
                id: "tool-1".to_string(),
                name: "edit".to_string(),
                arguments: arguments(tool_arguments.clone()),
                ..ToolCall::default()
            })],
            StopReason::ToolUse,
        ),
        Message::ToolResult(ToolResultMessage {
            tool_call_id: "tool-1".to_string(),
            tool_name: "edit".to_string(),
            content: vec![TextOrImageContent::Text(text("done"))],
            details: None,
            usage: None,
            added_tool_names: None,
            is_error: false,
            timestamp: TIMESTAMP,
        }),
        user_text("Continue"),
    ]);

    let payload = capture_payload(&context);
    assert_eq!(
        messages_of(&payload)[0]["content"][0]["toolUse"]["input"],
        json!({
            "path": "/workspace/foobar/file.js",
            "edits": [
                { "oldText": "first", "newText": "updated first" },
                { "oldText": "second", "newText": "updated second" },
            ],
        })
    );
    // The source arguments are untouched.
    let Message::Assistant(source) = &context.messages[0] else {
        panic!("expected the assistant message");
    };
    let AssistantContent::ToolCall(source) = &source.content[0] else {
        panic!("expected the tool call");
    };
    assert_eq!(Value::Object(source.arguments.clone()), tool_arguments);
}
