//! Regression — an all-completed todo list hides after five seconds and the
//! editor keeps accepting input after the turn.
//! Both halves broke in v0.1.1: `TodoVisibility::hide_deadline` only woke the
//! loop (nothing called `tick`), so a finished list stayed on screen forever —
//! the same missing-half pattern as the autocomplete pump. The user report
//! also saw dead input after a long tool turn, so this scenario types after
//! the turn and asserts the keystrokes land.

use serde_json::json;

use super::harness::{InteractiveE2e, run_local};
use crate::app_runtime::{reply, tool_call_reply};

#[tokio::test(flavor = "current_thread")]
async fn a_completed_todo_list_hides_after_five_seconds() {
    run_local(async {
        let e2e = InteractiveE2e::new().await;
        e2e.faux().set_responses(vec![
            tool_call_reply(
                "todo_write",
                "call-1",
                json!({ "todos": [
                    { "content": "Run checks", "activeForm": "Running checks", "status": "completed" },
                    { "content": "Clean up", "activeForm": "Cleaning up", "status": "completed" },
                ] }),
            ),
            reply("Alles erledigt."),
        ]);
        let mut driver = e2e.start().await;
        driver.wait_for("notagent").await;

        driver.submit("erledige alles").await;
        // The all-completed list arrives from the tool call and is shown once
        // — in the dock panel only: since 2026-08-16 `todo_write` gets no
        // transcript row (user decision, like notagent-main-rust).
        driver.wait_for("Clean up").await;
        driver.wait_for("Alles erledigt.").await;
        assert!(
            !driver.screen().contains("todo_write"),
            "the tool call has no transcript row:\n{}",
            driver.screen()
        );

        // The editor still accepts input after the turn.
        driver.send_keys("tippt noch").await;
        driver.wait_for("tippt noch").await;

        // The dock renders completed rows with a filled bullet, and it is the
        // only place the list appears — so the bullet is the marker that must
        // leave the visible screen when the five-second hide fires.
        assert!(
            driver.screen().contains('●'),
            "the completed list is shown once before it hides:\n{}",
            driver.screen()
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            driver.settle().await;
            if !driver.screen().contains('●') {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the completed todo list never hid:\n{}",
                driver.screen()
            );
        }
    })
    .await;
}
