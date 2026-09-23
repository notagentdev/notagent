//! Scenario 2 — prompt round-trip.
//! A prompt typed at the terminal reaches the session, the scripted answer
//! comes back through the streaming path and both ends up in the transcript on
//! screen. This is the interactive counterpart of the G2 evidence in
//! `tests/headless_end_to_end.rs`, which drives the same session from the print
//! mode.

use notagent::config::APP_NAME;

use super::harness::{InteractiveE2e, run_local};
use crate::app_runtime::reply;

#[tokio::test(flavor = "current_thread")]
async fn a_typed_prompt_reaches_the_session_and_the_answer_reaches_the_screen() {
    run_local(async {
        let e2e = InteractiveE2e::new().await;
        e2e.faux()
            .set_responses(vec![reply("Hello from the faux provider.")]);
        let mut driver = e2e.start().await;
        driver.wait_for(APP_NAME).await;

        driver.submit("say hello").await;

        // The user line stays in the transcript, the answer follows it.
        driver.wait_for("Hello from the faux provider.").await;
        driver.assert_shows("say hello");

        // The same turn below the surface.
        let session = driver.app().session();
        assert_eq!(
            session.get_last_assistant_text().as_deref(),
            Some("Hello from the faux provider.")
        );
        assert!(
            session.messages().iter().any(|message| {
                let notagent_agent::types::AgentMessage::User(user) = message else {
                    return false;
                };
                match &user.content {
                    notagent_ai::types::UserContent::Text(text) => text.contains("say hello"),
                    notagent_ai::types::UserContent::Blocks(blocks) => blocks.iter().any(|block| {
                        matches!(block, notagent_ai::types::TextOrImageContent::Text(text)
                            if text.text.contains("say hello"))
                    }),
                }
            }),
            "the prompt is part of the transcript: {:#?}",
            session.messages()
        );
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_streamed_answer_longer_than_the_screen_enters_scrollback_once() {
    run_local(async {
        let e2e = InteractiveE2e::new().await;
        let answer = (0..40)
            .map(|index| format!("Paragraph number {index} of the long answer."))
            .collect::<Vec<_>>()
            .join("\n\n");
        e2e.faux().set_responses(vec![reply(&answer)]);
        let mut driver = e2e.start().await;
        driver.wait_for(APP_NAME).await;

        driver.submit("write a lot").await;
        driver.wait_for("Paragraph number 39 of").await;
        driver.submit("").await;
        let buffer = driver.scrollback();
        for index in 0..40 {
            let paragraph = format!("Paragraph number {index} of");
            assert_eq!(
                buffer.matches(&paragraph).count(),
                1,
                "{paragraph:?} must be in the terminal exactly once:\n{buffer}"
            );
        }
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn the_editor_is_empty_again_and_takes_the_next_prompt() {
    run_local(async {
        let e2e = InteractiveE2e::new().await;
        e2e.faux()
            .set_responses(vec![reply("first answer"), reply("second answer")]);
        let mut driver = e2e.start().await;
        driver.wait_for(APP_NAME).await;

        driver.submit("first prompt").await;
        driver.wait_for("first answer").await;

        driver.submit("second prompt").await;
        driver.wait_for("second answer").await;

        let session = driver.app().session();
        assert_eq!(
            session.get_last_assistant_text().as_deref(),
            Some("second answer")
        );
    })
    .await;
}
