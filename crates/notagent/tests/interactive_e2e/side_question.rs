//! `/btw` from the user's side.
//!
//! The child and the cache invariants are covered by `tests/side_question.rs`,
//! the frame by `tests/side_question_panel.rs`. What runs here is the path a
//! user takes: the typed command opens the panel, an answer lands in it, a
//! plain line becomes a follow-up rather than a prompt, and Escape takes the
//! panel away without disturbing the conversation underneath.

use notagent::config::APP_NAME;
use notagent_ai::providers::faux::{faux_assistant_message, faux_text};
use notagent_ai::types::StopReason;

use super::harness::{InteractiveE2e, run_local};

fn reply(text: &str) -> notagent_ai::providers::faux::FauxResponseStep {
    faux_assistant_message(vec![faux_text(text)], StopReason::Stop).into()
}

#[tokio::test(flavor = "current_thread")]
async fn the_command_opens_a_panel_and_answers_in_it() {
    run_local(async {
        let e2e = InteractiveE2e::new().await;
        e2e.faux()
            .set_responses(vec![reply("Because the prefix stays intact.")]);
        let mut driver = e2e.start().await;
        driver.wait_for(APP_NAME).await;

        driver.submit("/btw why does this not cost context?").await;
        driver.wait_for("BTW").await;
        driver.wait_for("Q: why does this not cost context?").await;
        driver.wait_for("Because the prefix stays intact.").await;

        // The exchange is framed, which is what says it is not the transcript.
        let screen = driver.screen();
        assert!(screen.contains('╭'), "the panel is framed:\n{screen}");
        assert!(
            screen.contains("Esc close"),
            "and says how to leave it:\n{screen}"
        );
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_plain_line_under_an_open_panel_is_a_follow_up() {
    run_local(async {
        let e2e = InteractiveE2e::new().await;
        e2e.faux().set_responses(vec![
            reply("The first answer."),
            reply("The second answer."),
        ]);
        let mut driver = e2e.start().await;
        driver.wait_for(APP_NAME).await;

        driver.submit("/btw first question").await;
        driver.wait_for("The first answer.").await;

        // Not a prompt for the main agent: the panel has the editor.
        driver.submit("second question").await;
        driver.wait_for("Q: second question").await;
        driver.wait_for("The second answer.").await;

        let messages = driver.app().session().messages();
        assert!(
            messages.is_empty(),
            "the main conversation stayed empty: {messages:?}"
        );
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn escape_takes_the_panel_away_and_leaves_the_conversation_alone() {
    run_local(async {
        let e2e = InteractiveE2e::new().await;
        e2e.faux()
            .set_responses(vec![reply("the main answer"), reply("the side answer")]);
        let mut driver = e2e.start().await;
        driver.wait_for(APP_NAME).await;

        driver.submit("hello").await;
        driver.wait_for("the main answer").await;
        let before = driver.app().session().messages().len();

        driver.submit("/btw a side question").await;
        driver.wait_for("the side answer").await;

        driver.send_keys("\x1b").await;
        driver.settle().await;
        assert!(
            !driver.screen().contains("Esc close"),
            "the panel is gone:\n{}",
            driver.screen()
        );

        assert_eq!(
            driver.app().session().messages().len(),
            before,
            "not one message reached the main conversation"
        );
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn the_command_says_what_it_needs_without_a_question() {
    run_local(async {
        let e2e = InteractiveE2e::new().await;
        let mut driver = e2e.start().await;
        driver.wait_for(APP_NAME).await;

        driver.submit("/btw").await;
        driver.wait_for("Ask something").await;
    })
    .await;
}
