//! Scenario 6 — resize.
//!
//! A SIGWINCH reaches the TUI as the resize handler of the terminal; the screen
//! is laid out again at the new width and nothing wraps past the edge. The
//! renderer's own resize behaviour is pinned in the TUI crate
//! (`tests/tui_shrink.rs`); what this scenario adds is the whole interactive
//! screen — header, transcript, editor, footer — over a real turn.

use notagent::config::APP_NAME;

use super::harness::{InteractiveE2e, run_local};
use crate::app_runtime::reply;

#[tokio::test(flavor = "current_thread")]
async fn the_screen_is_laid_out_again_at_the_new_width() {
    run_local(async {
        let e2e = InteractiveE2e::with_size(100, 30).await;
        e2e.faux().set_responses(vec![reply(
            "An answer long enough to need more than one line once the terminal gets narrow.",
        )]);
        let mut driver = e2e.start().await;
        driver.wait_for(APP_NAME).await;
        driver.submit("say something long").await;
        driver.wait_for("An answer long enough").await;
        driver.assert_fits(100);

        driver.resize(50, 20).await;

        // The transcript is still there, now inside 50 columns.
        driver.assert_fits(50);
        driver.wait_for("An answer long enough").await;
        assert_eq!(driver.terminal().get_viewport().len(), 20);

        // And back — growing the terminal must not lose the transcript either.
        driver.resize(100, 30).await;
        driver.assert_fits(100);
        driver.wait_for("An answer long enough").await;
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn the_editor_keeps_its_text_across_a_resize() {
    run_local(async {
        let e2e = InteractiveE2e::with_size(100, 30).await;
        let mut driver = e2e.start().await;
        driver.wait_for(APP_NAME).await;
        driver
            .send_keys("a prompt that is still being written")
            .await;
        driver.assert_shows("a prompt that is still being written");

        driver.resize(60, 20).await;

        driver.wait_for("a prompt that is still").await;
        driver.assert_fits(60);
    })
    .await;
}
