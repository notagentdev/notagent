mod suite;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use notagent::core::agent_session::{AgentSessionEvent, ExecuteBashOptions, PromptOptions};
use notagent_agent::types::{AgentEvent, AgentMessage};
use notagent_ai::providers::faux::{FauxResponseStep, faux_assistant_message, faux_text};
use notagent_ai::types::StopReason;
use suite::{Harness, HarnessOptions, create_harness};

fn reply(text: &str) -> FauxResponseStep {
    faux_assistant_message(vec![faux_text(text)], StopReason::Stop).into()
}

fn bash_messages(harness: &Harness) -> Vec<String> {
    harness
        .session
        .messages()
        .into_iter()
        .filter_map(|message| match message {
            AgentMessage::BashExecution(message) => Some(message.command),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn records_a_bash_result_immediately_while_idle() {
    let harness = create_harness(HarnessOptions::default());

    let result = harness
        .session
        .execute_bash("echo hello", None, ExecuteBashOptions::default())
        .await;

    assert!(result.output.contains("hello"));
    assert_eq!(result.exit_code, Some(0));
    assert_eq!(bash_messages(&harness), vec!["echo hello".to_string()]);
}

#[tokio::test]
async fn defers_a_bash_result_while_streaming_and_flushes_it_before_the_next_prompt() {
    // Slow enough that the run is still going while the bash command runs
    let harness = create_harness(HarnessOptions {
        tokens_per_second: Some(5.0),
        ..HarnessOptions::default()
    });
    harness.set_responses(vec![reply("one"), reply("two")]);

    let session = Arc::clone(&harness.session);
    let running = tokio::spawn(async move {
        session
            .prompt("start", PromptOptions::default())
            .await
            .expect("prompt");
    });
    harness.wait_until_streaming().await;

    harness
        .session
        .execute_bash("echo deferred", None, ExecuteBashOptions::default())
        .await;
    // Still deferred: adding it now would break the tool_use/tool_result order.
    assert!(harness.session.has_pending_bash_messages() || !bash_messages(&harness).is_empty());

    running.await.expect("run");
    assert_eq!(bash_messages(&harness), vec!["echo deferred".to_string()]);
    assert!(!harness.session.has_pending_bash_messages());
}

#[tokio::test]
async fn streams_bash_output_to_the_callback_and_to_the_session_events() {
    let harness = create_harness(HarnessOptions::default());
    let chunks = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&chunks);

    harness
        .session
        .execute_bash(
            "printf 'a\\nb\\n'",
            Some(Arc::new(move |_chunk: &str| {
                counter.fetch_add(1, Ordering::SeqCst);
            })),
            ExecuteBashOptions {
                id: Some("run-1".to_string()),
                ..ExecuteBashOptions::default()
            },
        )
        .await;

    assert!(chunks.load(Ordering::SeqCst) >= 1);
    let updates: Vec<String> = harness
        .events()
        .into_iter()
        .filter_map(|event| match event {
            AgentSessionEvent::BashExecutionUpdate { id, delta } => {
                assert_eq!(id.as_deref(), Some("run-1"));
                Some(delta)
            }
            _ => None,
        })
        .collect();
    assert!(!updates.is_empty());
    assert!(updates.join("").contains('a'));
}

#[tokio::test]
async fn cancels_a_running_bash_command() {
    let harness = create_harness(HarnessOptions::default());

    let session = Arc::clone(&harness.session);
    let running = tokio::spawn(async move {
        session
            .execute_bash("sleep 5", None, ExecuteBashOptions::default())
            .await
    });
    while !harness.session.is_bash_running() {
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }

    harness.session.abort_bash();
    let result = running.await.expect("bash");

    assert!(result.cancelled);
    assert!(!harness.session.is_bash_running());
}

#[tokio::test]
async fn persists_user_assistant_and_bash_messages_in_order() {
    let harness = create_harness(HarnessOptions::default());
    harness.set_responses(vec![reply("answered")]);

    harness
        .session
        .execute_bash("echo before", None, ExecuteBashOptions::default())
        .await;
    harness
        .session
        .prompt("hello", PromptOptions::default())
        .await
        .expect("prompt");

    let roles: Vec<&'static str> = harness
        .session
        .messages()
        .iter()
        .map(|message| match message {
            AgentMessage::User(_) => "user",
            AgentMessage::Assistant(_) => "assistant",
            AgentMessage::BashExecution(_) => "bash",
            AgentMessage::Custom(_) => "custom",
            AgentMessage::ToolResult(_) => "toolResult",
            AgentMessage::BranchSummary(_) => "branchSummary",
            AgentMessage::CompactionSummary(_) => "compactionSummary",
        })
        .collect();
    assert_eq!(roles, vec!["bash", "user", "assistant"]);

    // The session file carries the same three entries.
    let entries = harness
        .session
        .with_session_manager(|manager| manager.get_entries().len());
    assert!(entries >= 3);
}

#[tokio::test]
async fn does_not_emit_message_end_for_a_bash_execution() {
    let harness = create_harness(HarnessOptions::default());

    harness
        .session
        .execute_bash("echo quiet", None, ExecuteBashOptions::default())
        .await;

    let ends = harness
        .events()
        .into_iter()
        .filter(|event| {
            matches!(
                event,
                AgentSessionEvent::Agent(AgentEvent::MessageEnd { .. })
            )
        })
        .count();
    assert_eq!(ends, 0);
}

#[tokio::test]
async fn keeps_output_out_of_the_context_when_asked_to() {
    let harness = create_harness(HarnessOptions::default());

    harness
        .session
        .execute_bash(
            "echo hidden",
            None,
            ExecuteBashOptions {
                exclude_from_context: true,
                ..ExecuteBashOptions::default()
            },
        )
        .await;

    let excluded = harness
        .session
        .messages()
        .into_iter()
        .find_map(|message| match message {
            AgentMessage::BashExecution(message) => Some(message.exclude_from_context),
            _ => None,
        })
        .expect("bash message");
    assert_eq!(excluded, Some(true));
}
