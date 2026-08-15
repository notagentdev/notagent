//! Ported from `packages/coding-agent/test/suite/agent-session-queue.test.ts`.
//!
//! Not ported: the four cases about extension commands — dispatching one while
//! idle and refusing to queue one (`plans/facts/extension-boundary.md`).

mod suite;

use std::sync::Arc;

use notagent::core::agent_session::{
    AgentSessionEvent, DeliverAs, PromptOptions, QueueBehavior, SendCustomMessageOptions,
};
use notagent::core::messages::CustomMessage;
use notagent_agent::types::{AgentMessage, QueueMode};
use notagent_agent::types::{
    AgentTool, AgentToolResult, AgentToolUpdateCallback, BoxFuture, ToolExecutionError,
};
use notagent_ai::providers::faux::{
    FauxResponseStep, faux_assistant_message, faux_text, faux_tool_call,
};
use notagent_ai::types::{StopReason, TextContent, TextOrImageContent, UserContent};
use tokio::sync::Notify;

use suite::{Harness, HarnessOptions, create_harness};

fn reply(text: &str) -> FauxResponseStep {
    faux_assistant_message(vec![faux_text(text)], StopReason::Stop).into()
}

fn custom(text: &str) -> CustomMessage {
    CustomMessage {
        custom_type: "note".to_string(),
        content: UserContent::Blocks(vec![TextOrImageContent::Text(TextContent::new(text))]),
        display: false,
        details: None,
        timestamp: 0,
    }
}

/// A tool that holds the turn open until it is released, as the TypeScript
/// suite's `wait` tool does. Polling for `isStreaming` instead would be a race:
/// a run with nothing to wait for can finish before the poll ever sees it.
struct GateTool {
    started: Arc<Notify>,
    release: Arc<Notify>,
    parameters: serde_json::Value,
}

impl GateTool {
    fn build(started: Arc<Notify>, release: Arc<Notify>) -> Arc<dyn AgentTool> {
        Arc::new(GateTool {
            started,
            release,
            parameters: serde_json::json!({ "type": "object", "properties": {} }),
        })
    }
}

impl AgentTool for GateTool {
    fn name(&self) -> &str {
        "wait"
    }
    fn label(&self) -> &str {
        "Wait"
    }
    fn description(&self) -> &str {
        "Wait for release"
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
        let started = Arc::clone(&self.started);
        let release = Arc::clone(&self.release);
        Box::pin(async move {
            let waiting = release.notified();
            tokio::pin!(waiting);
            waiting.as_mut().enable();
            started.notify_waiters();
            waiting.await;
            Ok(AgentToolResult {
                content: vec![TextOrImageContent::Text(TextContent::new("released"))],
                ..AgentToolResult::default()
            })
        })
    }
}

/// A harness whose first turn stops inside the gate tool.
struct WaitingRun {
    harness: Harness,
    started: Arc<Notify>,
    release: Arc<Notify>,
}

impl WaitingRun {
    fn create() -> Self {
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let harness = create_harness(HarnessOptions {
            tools: Some(vec![GateTool::build(
                Arc::clone(&started),
                Arc::clone(&release),
            )]),
            ..HarnessOptions::default()
        });
        WaitingRun {
            harness,
            started,
            release,
        }
    }

    /// Starts the run and returns once the gate tool is executing.
    async fn start(&self) -> tokio::task::JoinHandle<()> {
        let session = Arc::clone(&self.harness.session);
        let waiting = self.started.notified();
        tokio::pin!(waiting);
        waiting.as_mut().enable();
        let handle = tokio::spawn(async move {
            session
                .prompt("start", PromptOptions::default())
                .await
                .expect("prompt");
        });
        waiting.await;
        handle
    }

    fn release(&self) {
        self.release.notify_waiters();
    }
}

/// The gate tool call the first scripted response makes.
fn gate_call() -> FauxResponseStep {
    faux_assistant_message(
        vec![faux_tool_call("wait", serde_json::json!({}), None)],
        StopReason::ToolUse,
    )
    .into()
}

#[tokio::test]
async fn delivers_a_steering_message_before_the_next_provider_call() {
    let waiting = WaitingRun::create();
    let harness = &waiting.harness;
    harness.set_responses(vec![gate_call(), reply("first"), reply("second")]);
    let running = waiting.start().await;

    harness.session.steer("steered", &[]);
    waiting.release();
    running.await.expect("run");

    let texts = harness.user_texts();
    assert!(texts.contains(&"start".to_string()));
    assert!(texts.contains(&"steered".to_string()));
}

#[tokio::test]
async fn delivers_a_follow_up_only_after_the_current_run_finishes() {
    let waiting = WaitingRun::create();
    let harness = &waiting.harness;
    harness.set_responses(vec![gate_call(), reply("first"), reply("second")]);
    let running = waiting.start().await;

    harness.session.follow_up("later", &[]);
    waiting.release();
    running.await.expect("run");

    let texts = harness.user_texts();
    assert_eq!(texts, vec!["start".to_string(), "later".to_string()]);
}

#[tokio::test]
async fn delivers_several_steering_messages_in_order_one_at_a_time() {
    let waiting = WaitingRun::create();
    let harness = &waiting.harness;
    harness.session.set_steering_mode(QueueMode::OneAtATime);
    harness.set_responses(vec![gate_call(), reply("a"), reply("b"), reply("c")]);
    let running = waiting.start().await;

    harness.session.steer("one", &[]);
    harness.session.steer("two", &[]);
    waiting.release();
    running.await.expect("run");

    let texts = harness.user_texts();
    let one = texts.iter().position(|text| text == "one").expect("one");
    let two = texts.iter().position(|text| text == "two").expect("two");
    assert!(one < two);
}

#[tokio::test]
async fn delivers_all_steering_messages_at_once_in_all_mode() {
    let waiting = WaitingRun::create();
    let harness = &waiting.harness;
    // The TypeScript case switches the mode through the session, which is also
    // the path that persists it.
    harness.session.set_steering_mode(QueueMode::All);
    assert_eq!(harness.session.steering_mode(), QueueMode::All);
    harness.set_responses(vec![gate_call(), reply("a"), reply("b")]);
    let running = waiting.start().await;

    harness.session.steer("one", &[]);
    harness.session.steer("two", &[]);
    waiting.release();
    running.await.expect("run");

    let texts = harness.user_texts();
    assert!(texts.contains(&"one".to_string()));
    assert!(texts.contains(&"two".to_string()));
}

#[tokio::test]
async fn queues_a_custom_message_as_steering_while_streaming() {
    let waiting = WaitingRun::create();
    let harness = &waiting.harness;
    harness.set_responses(vec![gate_call(), reply("a"), reply("b")]);
    let running = waiting.start().await;

    harness
        .session
        .send_custom_message(
            custom("an aside"),
            SendCustomMessageOptions {
                deliver_as: Some(DeliverAs::Steer),
                ..SendCustomMessageOptions::default()
            },
        )
        .await;
    waiting.release();
    running.await.expect("run");

    assert_eq!(harness.custom_messages("note").len(), 1);
}

#[tokio::test]
async fn injects_next_turn_custom_messages_into_the_following_prompt() {
    let harness = create_harness(HarnessOptions::default());
    harness.set_responses(vec![reply("answered")]);

    harness
        .session
        .send_custom_message(
            custom("context for later"),
            SendCustomMessageOptions {
                deliver_as: Some(DeliverAs::NextTurn),
                ..SendCustomMessageOptions::default()
            },
        )
        .await;
    // Nothing yet: the message waits for a prompt.
    assert!(harness.custom_messages("note").is_empty());

    harness
        .session
        .prompt("go", PromptOptions::default())
        .await
        .expect("prompt");

    let messages = harness.session.messages();
    let note_at = messages
        .iter()
        .position(|message| matches!(message, AgentMessage::Custom(custom) if custom.custom_type == "note"))
        .expect("note");
    let prompt_at = messages
        .iter()
        .position(|message| matches!(message, AgentMessage::User(_)))
        .expect("prompt");
    // It rides along with the prompt, directly after it.
    assert_eq!(note_at, prompt_at + 1);
}

#[tokio::test]
async fn appends_a_custom_message_without_starting_a_turn() {
    let harness = create_harness(HarnessOptions::default());

    harness
        .session
        .send_custom_message(custom("just a note"), SendCustomMessageOptions::default())
        .await;

    assert_eq!(harness.custom_messages("note").len(), 1);
    // A message_start and a message_end were emitted for it, and no run began.
    let starts = harness
        .events()
        .into_iter()
        .filter(|event| {
            matches!(
                event,
                AgentSessionEvent::Agent(notagent_agent::types::AgentEvent::MessageStart { .. })
            )
        })
        .count();
    assert_eq!(starts, 1);
    assert!(harness.session.is_idle());
}

#[tokio::test]
async fn a_custom_message_can_start_a_turn_of_its_own() {
    let harness = create_harness(HarnessOptions::default());
    harness.set_responses(vec![reply("answered")]);

    harness
        .session
        .send_custom_message(
            custom("wake up"),
            SendCustomMessageOptions {
                trigger_turn: true,
                ..SendCustomMessageOptions::default()
            },
        )
        .await;

    assert_eq!(harness.assistant_texts(), vec!["answered".to_string()]);
}

#[tokio::test]
async fn updates_the_pending_count_and_clears_the_queue_on_demand() {
    let waiting = WaitingRun::create();
    let harness = &waiting.harness;
    harness.set_responses(vec![gate_call(), reply("a"), reply("b")]);
    let running = waiting.start().await;

    harness.session.follow_up("one", &[]);
    harness.session.follow_up("two", &[]);
    assert_eq!(harness.session.pending_message_count(), 2);
    assert_eq!(harness.session.get_follow_up_messages().len(), 2);

    let (steering, follow_up) = harness.session.clear_queue();
    assert!(steering.is_empty());
    assert_eq!(follow_up, vec!["one".to_string(), "two".to_string()]);
    assert_eq!(harness.session.pending_message_count(), 0);

    waiting.release();
    running.await.expect("run");
}

#[tokio::test]
async fn removes_a_queued_message_from_the_queue_before_it_is_announced() {
    let waiting = WaitingRun::create();
    let harness = &waiting.harness;
    harness.set_responses(vec![gate_call(), reply("a"), reply("b")]);
    let running = waiting.start().await;

    harness.session.follow_up("later", &[]);
    waiting.release();
    running.await.expect("run");

    // Delivered, so the queue is empty and a final update said so.
    assert_eq!(harness.session.pending_message_count(), 0);
    let last_update = harness
        .events()
        .into_iter()
        .rev()
        .find_map(|event| match event {
            AgentSessionEvent::QueueUpdate { follow_up, .. } => Some(follow_up),
            _ => None,
        })
        .expect("queue update");
    assert!(last_update.is_empty());
}

#[tokio::test]
async fn a_queue_behaviour_reaches_the_right_queue() {
    let waiting = WaitingRun::create();
    let harness = &waiting.harness;
    harness.set_responses(vec![gate_call(), reply("a"), reply("b")]);
    let running = waiting.start().await;

    harness
        .session
        .prompt(
            "queued",
            PromptOptions {
                streaming_behavior: Some(QueueBehavior::Steer),
                ..PromptOptions::default()
            },
        )
        .await
        .expect("queued");
    assert_eq!(
        harness.session.get_steering_messages(),
        vec!["queued".to_string()]
    );

    waiting.release();
    running.await.expect("run");
}
