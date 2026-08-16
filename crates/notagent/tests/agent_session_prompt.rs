//! Ported from `packages/coding-agent/test/suite/agent-session-prompt.test.ts`.
//!
//! Not ported: the six cases that drive extension commands, extension input
//! handlers, or `sendUserMessage` opting into extension dispatch
//! (`plans/facts/extension-boundary.md`). "throws when prompted during manual
//! compaction" needs a compaction that stays in flight, so it lives in
//! `agent_session_compaction.rs` with the slow faux provider.

mod suite;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use notagent::core::agent_session::{PromptOptions, QueueBehavior};
use notagent_agent::types::{
    AgentTool, AgentToolResult, AgentToolUpdateCallback, BoxFuture, ToolExecutionError,
};
use notagent_ai::providers::faux::{
    FauxResponseStep, faux_assistant_message, faux_text, faux_tool_call,
};
use notagent_ai::types::{
    AssistantContent, ImageContent, Message, StopReason, TextContent, TextOrImageContent,
    UserContent,
};
use serde_json::json;
use suite::{HarnessOptions, create_harness, user_content_text};

fn reply(text: &str) -> FauxResponseStep {
    faux_assistant_message(vec![faux_text(text)], StopReason::Stop).into()
}

fn tool_call_reply(name: &str, id: &str, arguments: serde_json::Value) -> FauxResponseStep {
    faux_assistant_message(
        vec![faux_tool_call(name, arguments, Some(id.to_string()))],
        StopReason::ToolUse,
    )
    .into()
}

/// A tool that records what it was called with and answers with a fixed string.
struct RecordingTool {
    name: String,
    calls: Arc<AtomicUsize>,
    parameters: serde_json::Value,
}

impl RecordingTool {
    fn build(name: &str, calls: Arc<AtomicUsize>) -> Arc<dyn AgentTool> {
        Arc::new(RecordingTool {
            name: name.to_string(),
            calls,
            parameters: json!({ "type": "object", "properties": {} }),
        })
    }
}

impl AgentTool for RecordingTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn label(&self) -> &str {
        &self.name
    }
    fn description(&self) -> &str {
        "a recording test tool"
    }
    fn parameters(&self) -> &serde_json::Value {
        &self.parameters
    }
    fn execute<'a>(
        &'a self,
        _tool_call_id: &'a str,
        _params: serde_json::Value,
        _signal: Option<tokio_util::sync::CancellationToken>,
        _on_update: Option<AgentToolUpdateCallback>,
    ) -> BoxFuture<'a, Result<AgentToolResult, ToolExecutionError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            Ok(AgentToolResult {
                content: vec![TextOrImageContent::Text(TextContent::new("tool done"))],
                ..AgentToolResult::default()
            })
        })
    }
}

#[tokio::test]
async fn prompts_while_idle_and_records_a_single_text_response() {
    let harness = create_harness(HarnessOptions::default());
    harness.set_responses(vec![reply("Hello back")]);

    harness
        .session
        .prompt("Hello", PromptOptions::default())
        .await
        .expect("prompt");

    assert_eq!(harness.user_texts(), vec!["Hello".to_string()]);
    assert_eq!(harness.assistant_texts(), vec!["Hello back".to_string()]);
    assert_eq!(harness.pending_response_count(), 0);
}

#[tokio::test]
async fn handles_a_tool_call_turn_and_waits_for_the_follow_up_response() {
    let calls = Arc::new(AtomicUsize::new(0));
    let harness = create_harness(HarnessOptions {
        tools: Some(vec![RecordingTool::build("probe", Arc::clone(&calls))]),
        ..HarnessOptions::default()
    });
    harness.set_responses(vec![
        tool_call_reply("probe", "call-1", json!({})),
        reply("all done"),
    ]);

    harness
        .session
        .prompt("use the tool", PromptOptions::default())
        .await
        .expect("prompt");

    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(harness.pending_response_count(), 0);
    assert!(
        harness
            .assistant_texts()
            .iter()
            .any(|text| text == "all done")
    );
    // The tool result is in the transcript, between the two assistant turns.
    let messages = harness.session.messages();
    let tool_result_at = messages
        .iter()
        .position(|message| matches!(message, notagent_agent::types::AgentMessage::ToolResult(_)))
        .expect("tool result");
    assert!(tool_result_at > 0 && tool_result_at < messages.len() - 1);
}

#[tokio::test]
async fn executes_several_tool_calls_from_one_response_and_continues_once() {
    let calls = Arc::new(AtomicUsize::new(0));
    let harness = create_harness(HarnessOptions {
        tools: Some(vec![RecordingTool::build("probe", Arc::clone(&calls))]),
        ..HarnessOptions::default()
    });
    harness.set_responses(vec![
        faux_assistant_message(
            vec![
                faux_tool_call("probe", json!({}), Some("call-1".to_string())),
                faux_tool_call("probe", json!({}), Some("call-2".to_string())),
            ],
            StopReason::ToolUse,
        )
        .into(),
        reply("finished"),
    ]);

    harness
        .session
        .prompt("use the tool twice", PromptOptions::default())
        .await
        .expect("prompt");

    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(harness.pending_response_count(), 0);
    assert_eq!(
        harness
            .session
            .messages()
            .iter()
            .filter(|message| matches!(message, notagent_agent::types::AgentMessage::ToolResult(_)))
            .count(),
        2
    );
}

#[tokio::test]
async fn preserves_image_attachments_in_the_provider_context() {
    let harness = create_harness(HarnessOptions::default());
    harness.set_responses(vec![reply("saw it")]);

    harness
        .session
        .prompt(
            "look at this",
            PromptOptions {
                images: vec![ImageContent {
                    data: "aGk=".to_string(),
                    mime_type: "image/png".to_string(),
                }],
                ..PromptOptions::default()
            },
        )
        .await
        .expect("prompt");

    let messages = harness.session.messages();
    let user = messages
        .iter()
        .find_map(|message| match message {
            notagent_agent::types::AgentMessage::User(user) => Some(user),
            _ => None,
        })
        .expect("user message");
    let UserContent::Blocks(blocks) = &user.content else {
        panic!("blocks");
    };
    assert!(
        blocks
            .iter()
            .any(|block| matches!(block, TextOrImageContent::Image(_)))
    );

    // The image survives the conversion the provider sees.
    let converted: Vec<Message> = notagent::core::messages::convert_to_llm(&messages);
    assert!(converted.iter().any(|message| {
        match message {
            Message::User(user) => match &user.content {
                UserContent::Blocks(blocks) => blocks
                    .iter()
                    .any(|block| matches!(block, TextOrImageContent::Image(_))),
                _ => false,
            },
            _ => false,
        }
    }));
}

#[tokio::test]
async fn refuses_a_prompt_during_streaming_without_a_queue_behaviour() {
    // Slow enough that the first run is still going when the second prompt
    // arrives (interface request B-16).
    let harness = create_harness(HarnessOptions {
        tokens_per_second: Some(5.0),
        ..HarnessOptions::default()
    });
    harness.set_responses(vec![reply("one"), reply("two")]);

    let session = Arc::clone(&harness.session);
    let running = tokio::spawn(async move {
        session
            .prompt("first", PromptOptions::default())
            .await
            .expect("prompt");
    });

    // Wait until the run is actually in flight.
    harness.wait_until_streaming().await;

    let error = harness
        .session
        .prompt("second", PromptOptions::default())
        .await
        .expect_err("should refuse");
    assert!(error.contains("streamingBehavior"));

    running.await.expect("first prompt");
}

#[tokio::test]
async fn queues_a_prompt_during_streaming_when_told_how() {
    let harness = create_harness(HarnessOptions {
        tokens_per_second: Some(5.0),
        ..HarnessOptions::default()
    });
    harness.set_responses(vec![reply("one"), reply("two")]);

    let session = Arc::clone(&harness.session);
    let running = tokio::spawn(async move {
        session
            .prompt("first", PromptOptions::default())
            .await
            .expect("prompt");
    });
    harness.wait_until_streaming().await;

    harness
        .session
        .prompt(
            "second",
            PromptOptions {
                streaming_behavior: Some(QueueBehavior::FollowUp),
                ..PromptOptions::default()
            },
        )
        .await
        .expect("queued");

    running.await.expect("first prompt");
    assert!(harness.user_texts().contains(&"second".to_string()));
}

#[tokio::test]
async fn refuses_a_prompt_without_configured_credentials() {
    let harness = create_harness(HarnessOptions {
        with_configured_auth: Some(false),
        ..HarnessOptions::default()
    });
    harness.set_responses(vec![reply("never")]);

    let error = harness
        .session
        .prompt("hello", PromptOptions::default())
        .await
        .expect_err("should refuse");
    assert!(error.contains("No API key found"));
}

#[tokio::test]
async fn send_user_message_while_idle_starts_a_turn() {
    let harness = create_harness(HarnessOptions::default());
    harness.set_responses(vec![reply("answered")]);

    harness
        .session
        .send_user_message(
            &[TextOrImageContent::Text(TextContent::new("from the sdk"))],
            None,
            false,
        )
        .await
        .expect("send");

    assert_eq!(harness.user_texts(), vec!["from the sdk".to_string()]);
    assert_eq!(harness.assistant_texts(), vec!["answered".to_string()]);
}

#[tokio::test]
async fn the_last_assistant_text_is_what_copy_would_take() {
    let harness = create_harness(HarnessOptions::default());
    harness.set_responses(vec![reply("first"), reply("second")]);
    harness
        .session
        .prompt("one", PromptOptions::default())
        .await
        .expect("prompt");
    harness
        .session
        .prompt("two", PromptOptions::default())
        .await
        .expect("prompt");

    assert_eq!(
        harness.session.get_last_assistant_text().as_deref(),
        Some("second")
    );
}

#[tokio::test]
async fn reports_the_texts_of_both_sides_for_the_session_statistics() {
    let harness = create_harness(HarnessOptions::default());
    harness.set_responses(vec![reply("hello")]);
    harness
        .session
        .prompt("hi", PromptOptions::default())
        .await
        .expect("prompt");

    let stats = harness.session.get_session_stats();
    assert_eq!(stats.user_messages, 1);
    assert_eq!(stats.assistant_messages, 1);
    assert_eq!(stats.total_messages, 2);
    assert_eq!(stats.session_id, harness.session.session_id());
}

/// The harness helper the other cases lean on, checked once directly.
#[test]
fn reads_the_text_of_both_content_shapes() {
    assert_eq!(
        user_content_text(&UserContent::Text("plain".into())),
        "plain"
    );
    assert_eq!(
        user_content_text(&UserContent::Blocks(vec![
            TextOrImageContent::Text(TextContent::new("a")),
            TextOrImageContent::Text(TextContent::new("b")),
        ])),
        "a\nb"
    );
}

/// `AssistantContent` is re-exported by the harness; keeping the import used
/// here documents that the suite reads assistant content directly.
#[test]
fn assistant_content_is_reachable() {
    let content = AssistantContent::Text(TextContent::new("x"));
    assert!(matches!(content, AssistantContent::Text(_)));
}
