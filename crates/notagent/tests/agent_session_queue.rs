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
use notagent_ai::providers::faux::{FauxResponseStep, faux_assistant_message, faux_text};
use notagent_ai::types::{StopReason, TextContent, TextOrImageContent, UserContent};

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

/// Starts a run and returns once it is actually in flight.
async fn start_run(harness: &Harness, text: &'static str) -> tokio::task::JoinHandle<()> {
    let session = Arc::clone(&harness.session);
    let handle = tokio::spawn(async move {
        session
            .prompt(text, PromptOptions::default())
            .await
            .expect("prompt");
    });
    while !harness.session.is_streaming() {
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }
    handle
}

#[tokio::test]
async fn delivers_a_steering_message_before_the_next_provider_call() {
    let harness = create_harness(HarnessOptions::default());
    harness.set_responses(vec![reply("first"), reply("second")]);
    let running = start_run(&harness, "start").await;

    harness.session.steer("steered", &[]);
    running.await.expect("run");

    let texts = harness.user_texts();
    assert!(texts.contains(&"start".to_string()));
    assert!(texts.contains(&"steered".to_string()));
}

#[tokio::test]
async fn delivers_a_follow_up_only_after_the_current_run_finishes() {
    let harness = create_harness(HarnessOptions::default());
    harness.set_responses(vec![reply("first"), reply("second")]);
    let running = start_run(&harness, "start").await;

    harness.session.follow_up("later", &[]);
    running.await.expect("run");

    let texts = harness.user_texts();
    assert_eq!(texts, vec!["start".to_string(), "later".to_string()]);
}

#[tokio::test]
async fn delivers_several_steering_messages_in_order_one_at_a_time() {
    let harness = create_harness(HarnessOptions::default());
    harness.session.set_steering_mode(QueueMode::OneAtATime);
    harness.set_responses(vec![reply("a"), reply("b"), reply("c")]);
    let running = start_run(&harness, "start").await;

    harness.session.steer("one", &[]);
    harness.session.steer("two", &[]);
    running.await.expect("run");

    let texts = harness.user_texts();
    let one = texts.iter().position(|text| text == "one").expect("one");
    let two = texts.iter().position(|text| text == "two").expect("two");
    assert!(one < two);
}

#[tokio::test]
async fn delivers_all_steering_messages_at_once_in_all_mode() {
    let harness = create_harness(HarnessOptions::default());
    // The TypeScript case switches the mode through the session, which is also
    // the path that persists it.
    harness.session.set_steering_mode(QueueMode::All);
    assert_eq!(harness.session.steering_mode(), QueueMode::All);
    harness.set_responses(vec![reply("a"), reply("b")]);
    let running = start_run(&harness, "start").await;

    harness.session.steer("one", &[]);
    harness.session.steer("two", &[]);
    running.await.expect("run");

    let texts = harness.user_texts();
    assert!(texts.contains(&"one".to_string()));
    assert!(texts.contains(&"two".to_string()));
}

#[tokio::test]
async fn queues_a_custom_message_as_steering_while_streaming() {
    let harness = create_harness(HarnessOptions::default());
    harness.set_responses(vec![reply("a"), reply("b")]);
    let running = start_run(&harness, "start").await;

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
    let harness = create_harness(HarnessOptions::default());
    harness.set_responses(vec![reply("a"), reply("b")]);
    let running = start_run(&harness, "start").await;

    harness.session.follow_up("one", &[]);
    harness.session.follow_up("two", &[]);
    assert_eq!(harness.session.pending_message_count(), 2);
    assert_eq!(harness.session.get_follow_up_messages().len(), 2);

    let (steering, follow_up) = harness.session.clear_queue();
    assert!(steering.is_empty());
    assert_eq!(follow_up, vec!["one".to_string(), "two".to_string()]);
    assert_eq!(harness.session.pending_message_count(), 0);

    running.await.expect("run");
}

#[tokio::test]
async fn removes_a_queued_message_from_the_queue_before_it_is_announced() {
    let harness = create_harness(HarnessOptions::default());
    harness.set_responses(vec![reply("a"), reply("b")]);
    let running = start_run(&harness, "start").await;

    harness.session.follow_up("later", &[]);
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
    let harness = create_harness(HarnessOptions::default());
    harness.set_responses(vec![reply("a"), reply("b")]);
    let running = start_run(&harness, "start").await;

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

    running.await.expect("run");
}
