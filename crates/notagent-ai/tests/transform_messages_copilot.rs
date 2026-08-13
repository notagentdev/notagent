//! Port of `packages/ai/test/transform-messages-copilot-openai-to-anthropic.test.ts` (191 LOC).
//!
//! Session migration from an OpenAI-shaped history onto Copilot's Claude models: thinking
//! blocks degrade to text, thought signatures are dropped and trailing tool calls get
//! synthetic error results.

use notagent_ai::api::transform_messages::transform_messages;
use notagent_ai::types::*;
use serde_json::Map;

const TIMESTAMP: i64 = 1_700_000_000_000;

/// The normalizer `anthropic.ts` passes in.
fn anthropic_normalize_tool_call_id(id: &str, _source: &AssistantMessage) -> String {
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

fn copilot_claude_model() -> Model {
    Model {
        id: "claude-sonnet-4.6".to_string(),
        name: "Claude Sonnet 4.6".to_string(),
        api: "anthropic-messages".to_string(),
        provider: "github-copilot".to_string(),
        base_url: "https://api.individual.githubcopilot.com".to_string(),
        reasoning: true,
        thinking_level_map: None,
        input: vec![Modality::Text, Modality::Image],
        cost: ModelCost::default(),
        context_window: 128_000,
        max_tokens: 16_000,
        sampling_params: None,
        headers: None,
        compat: None,
    }
}

fn user(text: &str) -> Message {
    Message::User(UserMessage {
        content: UserContent::Text(text.to_string()),
        timestamp: TIMESTAMP,
    })
}

fn assistant(api: &str, model: &str, content: Vec<AssistantContent>, stop: StopReason) -> Message {
    Message::Assistant(AssistantMessage {
        content,
        api: api.to_string(),
        provider: "github-copilot".to_string(),
        model: model.to_string(),
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

fn tool_result(tool_call_id: &str, tool_name: &str, text: &str) -> Message {
    Message::ToolResult(ToolResultMessage {
        tool_call_id: tool_call_id.to_string(),
        tool_name: tool_name.to_string(),
        content: vec![TextOrImageContent::Text(TextContent {
            text: text.to_string(),
            ..TextContent::default()
        })],
        details: None,
        usage: None,
        added_tool_names: None,
        is_error: false,
        timestamp: TIMESTAMP,
    })
}

fn arguments(pairs: &[(&str, serde_json::Value)]) -> Map<String, serde_json::Value> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), value.clone()))
        .collect()
}

fn first_assistant(messages: &[Message]) -> &AssistantMessage {
    messages
        .iter()
        .find_map(|message| match message {
            Message::Assistant(message) => Some(message),
            _ => None,
        })
        .expect("assistant message")
}

fn run(messages: &[Message]) -> Vec<Message> {
    transform_messages(
        messages,
        &copilot_claude_model(),
        Some(&anthropic_normalize_tool_call_id),
        TIMESTAMP,
    )
}

#[test]
fn converts_thinking_blocks_to_plain_text_when_source_model_differs() {
    let messages = vec![
        user("hello"),
        assistant(
            "openai-completions",
            "gpt-4o",
            vec![
                AssistantContent::Thinking(ThinkingContent {
                    thinking: "Let me think about this...".to_string(),
                    thinking_signature: Some("reasoning_content".to_string()),
                    ..ThinkingContent::default()
                }),
                AssistantContent::Text(TextContent {
                    text: "Hi there!".to_string(),
                    ..TextContent::default()
                }),
            ],
            StopReason::Stop,
        ),
    ];

    let result = run(&messages);
    let assistant_message = first_assistant(&result);
    let text_blocks = assistant_message
        .content
        .iter()
        .filter(|block| matches!(block, AssistantContent::Text(_)))
        .count();
    let thinking_blocks = assistant_message
        .content
        .iter()
        .filter(|block| matches!(block, AssistantContent::Thinking(_)))
        .count();

    assert_eq!(thinking_blocks, 0);
    assert!(text_blocks >= 2, "{text_blocks}");
}

#[test]
fn removes_thought_signature_from_tool_calls_when_migrating_between_models() {
    let messages = vec![
        user("run a command"),
        assistant(
            "openai-responses",
            "gpt-5",
            vec![AssistantContent::ToolCall(ToolCall {
                id: "call_123".to_string(),
                name: "bash".to_string(),
                arguments: arguments(&[("command", serde_json::json!("ls"))]),
                thought_signature: Some(
                    serde_json::to_string(&serde_json::json!({
                        "type": "reasoning.encrypted",
                        "id": "call_123",
                        "data": "encrypted",
                    }))
                    .expect("json"),
                ),
                ..ToolCall::default()
            })],
            StopReason::ToolUse,
        ),
        tool_result("call_123", "bash", "output"),
    ];

    let result = run(&messages);
    let tool_call = first_assistant(&result)
        .content
        .iter()
        .find_map(|block| match block {
            AssistantContent::ToolCall(block) => Some(block),
            _ => None,
        })
        .expect("tool call");

    assert_eq!(tool_call.thought_signature, None);
}

#[test]
fn adds_synthetic_tool_results_for_trailing_orphaned_tool_calls() {
    let messages = vec![
        user("read the file"),
        assistant(
            "openai-responses",
            "gpt-5",
            vec![AssistantContent::ToolCall(ToolCall {
                id: "call_123|fc_123".to_string(),
                name: "read".to_string(),
                arguments: arguments(&[("path", serde_json::json!("README.md"))]),
                ..ToolCall::default()
            })],
            StopReason::ToolUse,
        ),
    ];

    let result = run(&messages);
    let Some(Message::ToolResult(last)) = result.last() else {
        panic!("expected a trailing tool result, got {:?}", result.last());
    };
    assert_eq!(last.tool_call_id, "call_123_fc_123");
    assert_eq!(last.tool_name, "read");
    assert!(last.is_error);
    assert_eq!(
        last.content,
        vec![TextOrImageContent::Text(TextContent {
            text: "No result provided".to_string(),
            ..TextContent::default()
        })]
    );
}

#[test]
fn adds_synthetic_results_only_for_trailing_tool_calls_that_are_still_missing_results() {
    let messages = vec![
        user("run commands"),
        assistant(
            "openai-responses",
            "gpt-5",
            vec![
                AssistantContent::ToolCall(ToolCall {
                    id: "call_1|fc_1".to_string(),
                    name: "read".to_string(),
                    arguments: arguments(&[("path", serde_json::json!("README.md"))]),
                    ..ToolCall::default()
                }),
                AssistantContent::ToolCall(ToolCall {
                    id: "call_2|fc_2".to_string(),
                    name: "bash".to_string(),
                    arguments: arguments(&[("command", serde_json::json!("pwd"))]),
                    ..ToolCall::default()
                }),
            ],
            StopReason::ToolUse,
        ),
        tool_result("call_1|fc_1", "read", "done"),
    ];

    let result = run(&messages);
    let synthetic: Vec<&ToolResultMessage> = result
        .iter()
        .filter_map(|message| match message {
            Message::ToolResult(message) if message.is_error => Some(message),
            _ => None,
        })
        .collect();

    assert_eq!(synthetic.len(), 1);
    assert_eq!(synthetic[0].tool_call_id, "call_2_fc_2");
    assert_eq!(synthetic[0].tool_name, "bash");
    assert_eq!(
        synthetic[0].content,
        vec![TextOrImageContent::Text(TextContent {
            text: "No result provided".to_string(),
            ..TextContent::default()
        })]
    );
}
