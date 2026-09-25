//! A `!command` row keeps receiving its output after it enters the transcript.
//! The row is appended before the command produces anything, so a transcript
//! that treated it as settled would move the empty row into scrollback and
//! show nothing until the command ended.

use std::time::{Duration, Instant};

use notagent::config::APP_NAME;

use super::harness::{InteractiveE2e, run_local};

#[tokio::test(flavor = "current_thread")]
async fn a_shell_command_row_shows_its_output_while_the_command_runs() {
    run_local(async {
        let e2e = InteractiveE2e::new().await;
        let mut driver = e2e.start().await;
        driver.wait_for(APP_NAME).await;

        let started = Instant::now();
        // The printed text is assembled by the shell, so the command line
        // echoed in the transcript cannot satisfy the wait.
        driver
            .submit("!sleep 0.2; printf 'early-%s\\n' output; sleep 3")
            .await;

        driver.wait_for("early-output").await;
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "the output must appear while the command still runs, not when it ends"
        );
        let buffer = driver.scrollback();
        assert_eq!(
            buffer.matches("early-output").count(),
            1,
            "the output must appear exactly once: {buffer}"
        );
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_quiet_bash_call_keeps_counting_its_running_time() {
    use crate::app_runtime::{reply, tool_call_reply};
    use serde_json::json;

    run_local(async {
        let e2e = InteractiveE2e::new().await;
        e2e.faux().set_responses(vec![
            tool_call_reply("bash", "call-1", json!({ "command": "sleep 4" })),
            reply("Done waiting."),
        ]);
        let mut driver = e2e.start().await;
        driver.wait_for(APP_NAME).await;

        driver.submit("wait a moment").await;
        // Nothing is printed, so only the elapsed tick can move the time on.
        driver.wait_for("2s /").await;
        driver.wait_for("Done waiting.").await;
    })
    .await;
}
