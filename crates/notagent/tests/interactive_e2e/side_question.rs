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

/// The hint on the frame promises the arrow keys, so they have to work — and
/// only while the editor is empty, where they are not caret movement.
#[tokio::test(flavor = "current_thread")]
async fn the_arrow_keys_scroll_the_panel_the_hint_offers_them_for() {
    run_local(async {
        let e2e = InteractiveE2e::new().await;
        let answer: String = (0..40).map(|index| format!("line {index}\n\n")).collect();
        e2e.faux().set_responses(vec![reply(&answer)]);
        let mut driver = e2e.start().await;
        driver.wait_for(APP_NAME).await;

        driver.submit("/btw a long one").await;
        driver.wait_for("↑↓ scroll").await;

        let before = driver.screen();
        driver.send_keys("\x1b[A").await;
        driver.settle().await;
        assert_ne!(
            before,
            driver.screen(),
            "up scrolled the panel:\n{}",
            driver.screen()
        );
    })
    .await;
}

/// Two steps, like everything else here: stop the answer, then take the panel
/// away. The child survives the first, so a follow-up still reaches it.
#[tokio::test(flavor = "current_thread")]
async fn ctrl_c_stops_the_answer_before_it_closes_the_panel() {
    run_local(async {
        let e2e = InteractiveE2e::new().await;
        e2e.faux().set_responses(vec![
            reply("the side answer"),
            reply("the follow-up answer"),
        ]);
        let mut driver = e2e.start().await;
        driver.wait_for(APP_NAME).await;

        driver.submit("/btw a side question").await;
        driver.wait_for("the side answer").await;

        // Nothing is running, so the first press closes the panel outright.
        driver.send_keys("\x03").await;
        driver.settle().await;
        assert!(
            !driver.screen().contains("Esc close"),
            "the panel is gone:\n{}",
            driver.screen()
        );
    })
    .await;
}

/// A failed start must not leave the previous panel standing over a child that
/// is no longer there.
#[tokio::test(flavor = "current_thread")]
async fn a_second_command_replaces_the_panel_rather_than_stacking_one() {
    run_local(async {
        let e2e = InteractiveE2e::new().await;
        e2e.faux()
            .set_responses(vec![reply("the first answer"), reply("the second answer")]);
        let mut driver = e2e.start().await;
        driver.wait_for(APP_NAME).await;

        driver.submit("/btw first").await;
        driver.wait_for("the first answer").await;

        driver.submit("/btw second").await;
        driver.wait_for("the second answer").await;

        let screen = driver.screen();
        assert_eq!(
            screen.matches("Esc close").count(),
            1,
            "one panel, not two:\n{screen}"
        );
        assert!(
            !screen.contains("Q: first"),
            "and the old exchange went with the old panel:\n{screen}"
        );
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn the_command_without_a_question_opens_the_panel_and_waits() {
    run_local(async {
        let e2e = InteractiveE2e::new().await;
        e2e.faux().set_responses(vec![reply("The answer.")]);
        let mut driver = e2e.start().await;
        driver.wait_for(APP_NAME).await;

        // No question yet, so the panel stands open with nothing asked.
        driver.submit("/btw").await;
        driver.wait_for("BTW").await;
        let screen = driver.screen();
        assert!(
            !screen.contains("Q: "),
            "nothing has been asked yet:\n{screen}"
        );

        // And the line typed under it is the question, exactly as a follow-up is.
        driver.submit("asked afterwards").await;
        driver.wait_for("Q: asked afterwards").await;
        driver.wait_for("The answer.").await;
    })
    .await;
}
