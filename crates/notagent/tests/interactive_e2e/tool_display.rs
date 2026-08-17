//! Scenario 3 — tool display.
//!
//! A scripted tool call runs through the real tool and shows up as a tool row
//! in the transcript. Both halves of the row exist on main: `render_call` and
//! `render_result` of the sixteen tools (C-17) and `ToolExecutionComponent`
//! (A-22) — what the scenario adds is that the interactive mode builds the row
//! and puts it in front of the user while the turn runs.

use notagent::config::APP_NAME;
use serde_json::json;

use super::harness::{InteractiveE2e, run_local};
use crate::app_runtime::{reply, tool_call_reply};

#[tokio::test(flavor = "current_thread")]
async fn a_tool_call_shows_up_as_a_tool_row_and_does_its_work() {
    run_local(async {
        let e2e = InteractiveE2e::new().await;
        let target = e2e.path("notes.txt");
        e2e.faux().set_responses(vec![
            tool_call_reply(
                "write",
                "call-1",
                json!({ "path": &target, "content": "written by the agent\n" }),
            ),
            reply("Wrote the file."),
        ]);
        let mut driver = e2e.start().await;
        driver.wait_for(APP_NAME).await;

        driver.submit("write the note").await;

        // The row of the `write` tool names the tool and its target; the
        // answer of the next step closes the turn.
        driver.wait_for("write").await;
        driver.wait_for_across_wraps("notes.txt").await;
        driver.wait_for("Wrote the file.").await;

        assert_eq!(
            std::fs::read_to_string(&target).expect("written file"),
            "written by the agent\n"
        );
        assert!(
            driver
                .app()
                .session()
                .messages()
                .iter()
                .any(|message| matches!(
                    message,
                    notagent_agent::types::AgentMessage::ToolResult(result)
                        if result.tool_name == "write"
                )),
            "the tool result is part of the transcript"
        );
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_failing_tool_call_shows_the_error_in_the_row() {
    run_local(async {
        let e2e = InteractiveE2e::new().await;
        e2e.faux().set_responses(vec![
            tool_call_reply(
                "ls",
                "call-1",
                json!({ "path": e2e.path("does-not-exist") }),
            ),
            reply("The directory is missing."),
        ]);
        let mut driver = e2e.start().await;
        driver.wait_for(APP_NAME).await;

        driver.submit("list the missing directory").await;

        // The row names the tool, its target and the error the tool returned.
        // The path is the one assertion that has to survive the wrap: it is
        // longer than the 80 columns of the terminal. The badge style (the
        // default) names the tool uppercase in its state badge.
        driver.wait_for("LS").await;
        driver.wait_for_across_wraps("does-not-exist").await;
        driver.assert_shows("Path not found");
        driver.wait_for("The directory is missing.").await;
    })
    .await;
}

/// Reads join the searches in the compact block instead of getting a row of
/// their own (user decision 2026-08-17, v0.1.11): the block names the file and
/// counts the failure rather than printing the tool's error text.
#[tokio::test(flavor = "current_thread")]
async fn a_failing_read_is_counted_in_the_search_block() {
    run_local(async {
        let e2e = InteractiveE2e::new().await;
        e2e.faux().set_responses(vec![
            tool_call_reply(
                "read",
                "call-1",
                json!({ "path": e2e.path("does-not-exist.txt") }),
            ),
            reply("The file is missing."),
        ]);
        let mut driver = e2e.start().await;
        driver.wait_for(APP_NAME).await;

        driver.submit("read the missing file").await;

        driver.wait_for("Read does-not-exist.txt").await;
        driver.wait_for("1 read, 1 failed").await;
        driver.wait_for("The file is missing.").await;
    })
    .await;
}
