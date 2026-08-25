//! `/init` as the session drives it (port addition, v0.1.35).
//!
//! `core::session_init` proves the brief and the reminder text. What runs here
//! is the part neither can: that a child actually writes the file, that the
//! session reads it back afterwards, and that the main conversation is told
//! what it now says.

mod suite;

use std::path::Path;

use notagent::core::agent_session::PromptOptions;
use notagent_agent::types::AgentMessage;
use notagent_ai::providers::faux::{
    FauxResponseStep, faux_assistant_message, faux_text, faux_tool_call,
};
use notagent_ai::types::{StopReason, UserContent};
use serde_json::json;

use suite::{Harness, HarnessOptions, create_harness};

/// Long enough to be handed back: a shorter answer is sent back for expansion,
/// which would consume another scripted response.
const CHILD_ANSWER: &str = "I read the manifest, the module tree and the test layout, then wrote AGENTS.md at the project root. It covers what the project is, the build and test commands, how the modules are divided, and the commit conventions I found in the history. Nothing else was changed.";

const WRITTEN_GUIDE: &str = "# Repository Guidelines\n\nBuild with `cargo build`.";

fn reply(text: &str) -> FauxResponseStep {
    faux_assistant_message(vec![faux_text(text)], StopReason::Stop).into()
}

/// The child writes the file, then reports back.
fn writing_child(path: &Path, content: &str) -> Vec<FauxResponseStep> {
    vec![
        faux_assistant_message(
            vec![faux_tool_call(
                "write",
                json!({ "path": path.to_string_lossy(), "content": content }),
                None,
            )],
            StopReason::ToolUse,
        )
        .into(),
        reply(CHILD_ANSWER),
    ]
}

/// The reminders `/init` appended, as their text.
fn init_reminders(harness: &Harness) -> Vec<String> {
    harness
        .session
        .messages()
        .into_iter()
        .filter_map(|message| {
            let AgentMessage::Custom(custom) = message else {
                return None;
            };
            if custom.custom_type != "init" {
                return None;
            }
            match custom.content {
                UserContent::Text(text) => Some(text),
                UserContent::Blocks(blocks) => Some(
                    blocks
                        .iter()
                        .filter_map(|block| match block {
                            notagent_ai::types::TextOrImageContent::Text(text) => {
                                Some(text.text.clone())
                            }
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n"),
                ),
            }
        })
        .collect()
}

#[tokio::test]
async fn writes_the_guide_and_reads_it_back_into_the_conversation() {
    let harness = create_harness(HarnessOptions::default());
    let guide = harness.temp.path().join("AGENTS.md");
    harness.set_responses(writing_child(&guide, WRITTEN_GUIDE));

    harness.session.run_init().await.expect("init");

    assert_eq!(
        std::fs::read_to_string(&guide).expect("the child wrote the guide"),
        WRITTEN_GUIDE
    );
    let reminders = init_reminders(&harness);
    assert_eq!(reminders.len(), 1, "exactly one reminder is appended");
    assert!(reminders[0].contains("The user ran /init"));
    assert!(
        reminders[0].contains("Build with `cargo build`."),
        "the reminder carries what was written: {}",
        reminders[0]
    );
}

#[tokio::test]
async fn the_reminder_stays_out_of_the_transcript() {
    let harness = create_harness(HarnessOptions::default());
    let guide = harness.temp.path().join("AGENTS.md");
    harness.set_responses(writing_child(&guide, WRITTEN_GUIDE));

    harness.session.run_init().await.expect("init");

    // The model has to read it; the user already watched the child work.
    let hidden = harness
        .session
        .messages()
        .into_iter()
        .any(|message| matches!(message, AgentMessage::Custom(custom)
            if custom.custom_type == "init" && !custom.display));
    assert!(hidden, "the reminder is not displayed");
}

#[tokio::test]
async fn says_so_when_the_child_wrote_nothing() {
    let harness = create_harness(HarnessOptions::default());
    // No write call: the child claims to be done without touching the file.
    harness.set_responses(vec![reply(CHILD_ANSWER)]);

    harness.session.run_init().await.expect("init");

    assert!(
        !harness.temp.path().join("AGENTS.md").exists(),
        "nothing was written"
    );
    let reminders = init_reminders(&harness);
    assert_eq!(reminders.len(), 1);
    assert!(reminders[0].contains("was not written"));
}

#[tokio::test]
async fn refuses_to_start_while_a_turn_is_running() {
    let harness = create_harness(HarnessOptions {
        tokens_per_second: Some(20.0),
        ..HarnessOptions::default()
    });
    harness.set_responses(vec![reply("a long answer the model is still streaming out")]);

    let session = std::sync::Arc::clone(&harness.session);
    let running =
        tokio::spawn(async move { session.prompt("hello", PromptOptions::default()).await });
    harness.wait_until_streaming().await;

    let error = harness
        .session
        .run_init()
        .await
        .expect_err("a busy session refuses");
    assert!(error.contains("busy"), "{error}");

    let _ = running.await;
}
