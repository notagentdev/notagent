//! History conversion of the OpenAI Responses adapters.
//!
//! Ports `packages/ai/test/openai-responses-empty-tool-result.test.ts` (58),
//! `openai-responses-message-id.test.ts` (48) and
//! `openai-responses-foreign-toolcall-id.test.ts` (66). The TS suites read the models
//! through the excluded `compat.ts`; the port reads the same entries from the catalog.

use std::collections::BTreeSet;

use notagent_ai::api::openai_responses_shared::{
    ConvertResponsesMessagesOptions, convert_responses_messages,
};
use notagent_ai::model_catalog::get_builtin_model;
use notagent_ai::types::*;
use notagent_ai::utils::hash::short_hash;
use serde_json::{Value, json};

const COPILOT_RAW_TOOL_CALL_ID: &str = "call_4VnzVawQXPB9MgYib7CiQFEY|I9b95oN1wD/cHXKTw3PpRkL6KkCtzTJhUxMouMWYwHeTo2j3htzfSk7YPx2vifiIM4g3A8XXyOj8q4Bt6SLUG7gqY1E3ELkrkVQNHglRfUmWj84lqxJY+Puieb3VKyX0FB+83TUzn91cDMF/4gzt990IzqVrc+nIb9RRscRD070Du16q1glydVjWR0SBJsE6TbY/esOjFpqplogQqrajm1eI++f3eLi73R6q7hVusY0QbeFySVxABCjhN0lXB04caBe1rzHjYzul6MAXj7uq+0r17VLq+yrtyYhN12wkmFqHeqTyEei6EFPbMy24Nc+IbJlkP0OCg02W+gOnyBFcbi2ctvJFSOhSjt1CqBdqCnnhwUqXjbWiT0wh3DmLScRgTHmGkaI+oAcQQjfic65nxj+TnEkReA==";

fn allowed_providers() -> BTreeSet<String> {
    ["openai", "openai-codex", "opencode"]
        .into_iter()
        .map(str::to_string)
        .collect()
}

fn convert(model: &Model, context: &Context) -> Vec<Value> {
    convert_responses_messages(
        model,
        context,
        &allowed_providers(),
        &ConvertResponsesMessagesOptions::default(),
        1,
    )
    .expect("conversion")
}

fn assistant(
    api: &str,
    provider: &str,
    model: &str,
    content: Vec<AssistantContent>,
    stop_reason: StopReason,
) -> Message {
    Message::Assistant(AssistantMessage {
        content,
        api: api.to_string(),
        provider: provider.to_string(),
        model: model.to_string(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: 1,
    })
}

fn user(text: &str) -> Message {
    Message::User(UserMessage {
        content: UserContent::Text(text.to_string()),
        timestamp: 1,
    })
}

fn tool_call(id: &str, name: &str, arguments: Value) -> AssistantContent {
    AssistantContent::ToolCall(ToolCall {
        id: id.to_string(),
        name: name.to_string(),
        arguments: arguments.as_object().cloned().unwrap_or_default(),
        thought_signature: None,
        namespace: None,
        extra: Default::default(),
    })
}

fn tool_result(tool_call_id: &str, tool_name: &str, text: &str) -> Message {
    Message::ToolResult(ToolResultMessage {
        tool_call_id: tool_call_id.to_string(),
        tool_name: tool_name.to_string(),
        content: vec![TextOrImageContent::Text(TextContent {
            text: text.to_string(),
            text_signature: None,
            extra: Default::default(),
        })],
        details: None,
        usage: None,
        added_tool_names: None,
        is_error: false,
        timestamp: 1,
    })
}

#[test]
fn an_empty_tool_result_gets_the_no_output_placeholder() {
    let model = get_builtin_model("openai", "gpt-4o-mini").expect("model");
    let context = Context {
        messages: vec![
            user("Run the command"),
            assistant(
                &model.api,
                &model.provider,
                &model.id,
                vec![tool_call("tool-1", "bash", json!({ "command": "true" }))],
                StopReason::ToolUse,
            ),
            tool_result("tool-1", "bash", ""),
        ],
        ..Default::default()
    };

    let output = convert(&model, &context)
        .into_iter()
        .find(|item| item["type"] == json!("function_call_output"))
        .expect("a function_call_output item");
    assert_eq!(output["output"], json!("(no tool output)"));
    assert!(
        !output["output"]
            .as_str()
            .expect("output")
            .contains("see attached image")
    );
}

#[test]
fn multiple_text_blocks_get_unique_fallback_message_ids() {
    let model = get_builtin_model("openai-codex", "gpt-5.5").expect("model");
    let context = Context {
        system_prompt: Some("You are concise.".to_string()),
        messages: vec![
            user("hello"),
            assistant(
                "anthropic-messages",
                "anthropic",
                "claude-opus-4-8",
                vec![
                    AssistantContent::Thinking(ThinkingContent {
                        thinking: "private reasoning".to_string(),
                        thinking_signature: None,
                        redacted: None,
                        extra: Default::default(),
                    }),
                    AssistantContent::Text(TextContent {
                        text: "visible answer".to_string(),
                        text_signature: None,
                        extra: Default::default(),
                    }),
                ],
                StopReason::Stop,
            ),
        ],
        ..Default::default()
    };

    let ids: Vec<String> = convert(&model, &context)
        .into_iter()
        .filter(|item| item["type"] == json!("message"))
        .filter_map(|item| item["id"].as_str().map(str::to_string))
        .collect();
    assert_eq!(ids, vec!["msg_pi_1", "msg_pi_1_1"]);
    assert_eq!(
        ids.iter().collect::<BTreeSet<_>>().len(),
        ids.len(),
        "the ids stay unique"
    );
}

#[test]
fn a_foreign_copilot_tool_call_id_is_hashed_into_a_bounded_shape() {
    let model = get_builtin_model("openai-codex", "gpt-5.5").expect("model");
    let context = Context {
        system_prompt: Some("You are concise.".to_string()),
        messages: vec![
            user("Use the tool."),
            assistant(
                "openai-responses",
                "github-copilot",
                "gpt-5.5",
                vec![tool_call(
                    COPILOT_RAW_TOOL_CALL_ID,
                    "edit",
                    json!({ "path": "src/styles/app.css" }),
                )],
                StopReason::ToolUse,
            ),
            tool_result(COPILOT_RAW_TOOL_CALL_ID, "edit", "ok"),
        ],
        ..Default::default()
    };

    let function_call = convert(&model, &context)
        .into_iter()
        .find(|item| item["type"] == json!("function_call"))
        .expect("a function_call item");
    let expected = format!(
        "fc_{}",
        short_hash(
            COPILOT_RAW_TOOL_CALL_ID
                .split('|')
                .nth(1)
                .expect("the opaque half")
        )
    );
    let id = function_call["id"].as_str().expect("an item id");
    assert_eq!(id, expected);
    assert!(id.len() <= 64);
    assert!(
        id.strip_prefix("fc_")
            .expect("prefix")
            .chars()
            .all(|character| character.is_ascii_alphanumeric())
    );
}
