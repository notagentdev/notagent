mod suite;

use notagent::core::agent_session::PromptOptions;
use notagent::core::messages::convert_to_llm;
use notagent_agent::types::AgentMessage;
use notagent_ai::providers::faux::{FauxResponseStep, faux_assistant_message, faux_text};
use notagent_ai::types::{Message, StopReason, UserContent};
use suite::{HarnessOptions, create_harness, user_content_text};

const MODE_BLOCK_TYPE: &str = "mode_block";

fn reply(text: &str) -> FauxResponseStep {
    faux_assistant_message(vec![faux_text(text)], StopReason::Stop).into()
}

fn llm_text(content: &UserContent) -> String {
    user_content_text(content)
}

#[test]
fn restores_the_last_safe_mode_and_persists_later_switches() {
    let harness = create_harness(HarnessOptions {
        settings: Some(serde_json::json!({ "lastMode": "auto" })),
        ..HarnessOptions::default()
    });

    assert_eq!(
        harness.session.active_mode().map(|mode| mode.id),
        Some("auto".to_string())
    );
    harness.session.set_mode("plan").expect("plan mode");
    assert_eq!(
        harness.settings_manager.get_last_mode().as_deref(),
        Some("plan")
    );
}

#[test]
fn persisted_yolo_is_not_restored_without_the_cli_capability() {
    let harness = create_harness(HarnessOptions {
        settings: Some(serde_json::json!({ "lastMode": "yolo" })),
        ..HarnessOptions::default()
    });

    assert_eq!(
        harness.session.active_mode().map(|mode| mode.id),
        Some("manual".to_string())
    );
}

#[test]
fn yolo_only_participates_in_the_cycle_when_explicitly_enabled() {
    let harness = create_harness(HarnessOptions::default());
    assert_eq!(
        harness.session.cycle_mode().map(|mode| mode.id),
        Some("auto".into())
    );
    assert_eq!(
        harness.session.cycle_mode().map(|mode| mode.id),
        Some("plan".into())
    );

    harness.session.set_mode("manual").expect("manual mode");
    harness.session.set_yolo_cycle_enabled(true);
    assert_eq!(
        harness.session.cycle_mode().map(|mode| mode.id),
        Some("auto".into())
    );
    assert_eq!(
        harness.session.cycle_mode().map(|mode| mode.id),
        Some("yolo".into())
    );
}

#[tokio::test]
async fn keeps_the_block_out_of_the_message_the_user_typed() {
    let harness = create_harness(HarnessOptions::default());
    harness.session.set_mode("plan").expect("plan mode");
    harness.set_responses(vec![reply("noted")]);
    harness
        .session
        .prompt("do the thing", PromptOptions::default())
        .await
        .expect("prompt");

    let messages = harness.session.messages();
    let user = messages
        .iter()
        .find_map(|message| match message {
            AgentMessage::User(user) => Some(user),
            _ => None,
        })
        .expect("user message");
    let text = llm_text(&user.content);
    assert_eq!(text, "do the thing");
    assert!(!text.contains("<mode "));
}

#[tokio::test]
async fn carries_it_in_a_message_the_transcript_does_not_render() {
    let harness = create_harness(HarnessOptions::default());
    harness.session.set_mode("plan").expect("plan mode");
    harness.set_responses(vec![reply("noted")]);
    harness
        .session
        .prompt("do the thing", PromptOptions::default())
        .await
        .expect("prompt");

    let messages = harness.session.messages();
    let block = messages
        .iter()
        .find_map(|message| match message {
            AgentMessage::Custom(custom) if custom.custom_type == MODE_BLOCK_TYPE => Some(custom),
            _ => None,
        })
        .expect("mode block");
    assert!(!block.display);
    assert!(llm_text(&block.content).contains("<mode name=\"plan\""));
}

#[tokio::test]
async fn still_reaches_the_provider_as_ordinary_user_content() {
    let harness = create_harness(HarnessOptions::default());
    harness.session.set_mode("plan").expect("plan mode");
    harness.set_responses(vec![reply("noted")]);
    harness
        .session
        .prompt("do the thing", PromptOptions::default())
        .await
        .expect("prompt");

    let converted = convert_to_llm(&harness.session.messages());
    let carrying = converted
        .iter()
        .filter(|message| match message {
            Message::User(user) => llm_text(&user.content).contains("<mode name=\"plan\""),
            _ => false,
        })
        .count();
    assert_eq!(carrying, 1);
}

#[tokio::test]
async fn arrives_immediately_before_the_prompt_it_applies_to() {
    let harness = create_harness(HarnessOptions::default());
    harness.session.set_mode("plan").expect("plan mode");
    harness.set_responses(vec![reply("noted")]);
    harness
        .session
        .prompt("do the thing", PromptOptions::default())
        .await
        .expect("prompt");

    let messages = harness.session.messages();
    let block_at = messages
        .iter()
        .position(|message| {
            matches!(message, AgentMessage::Custom(custom) if custom.custom_type == MODE_BLOCK_TYPE)
        })
        .expect("mode block");
    let prompt_at = messages
        .iter()
        .position(|message| matches!(message, AgentMessage::User(_)))
        .expect("user message");
    assert_eq!(prompt_at, block_at + 1);
}

#[tokio::test]
async fn is_delivered_once_not_on_every_following_prompt() {
    let harness = create_harness(HarnessOptions::default());
    harness.session.set_mode("plan").expect("plan mode");
    harness.set_responses(vec![reply("one"), reply("two")]);
    harness
        .session
        .prompt("first", PromptOptions::default())
        .await
        .expect("prompt");
    harness
        .session
        .prompt("second", PromptOptions::default())
        .await
        .expect("prompt");

    let blocks = harness
        .session
        .messages()
        .iter()
        .filter(|message| {
            matches!(message, AgentMessage::Custom(custom) if custom.custom_type == MODE_BLOCK_TYPE)
        })
        .count();
    assert_eq!(blocks, 1);
}

/// which is the half of `setMode` the delivery cases never touch.
#[tokio::test]
async fn a_mode_switch_also_changes_which_tools_exist() {
    let harness = create_harness(HarnessOptions::default());
    let plan = harness.session.set_mode("plan").expect("plan mode");
    let active: Vec<String> = harness.session.get_active_tool_names();
    for tool in &plan.tools {
        assert!(
            active.contains(&tool.as_str().to_string()),
            "{} missing from the active set",
            tool.as_str()
        );
    }
}
