//! Scenario 3 — tool display.
//! A scripted tool call runs through the real tool and shows up as a tool row
//! in the transcript. Both halves of the row exist on main: `render_call` and
//! `render_result` of the sixteen tools (C-17) and `ToolExecutionComponent`
//! (A-22) — what the scenario adds is that the interactive mode builds the row
//! and puts it in front of the user while the turn runs.

use notagent::config::APP_NAME;
use serde_json::json;

use super::harness::{InteractiveE2e, run_local};
use crate::app_runtime::{reply, tool_call_reply};

/// An assistant message that says what it is about to do and makes the call in
/// the same breath — the shape a model actually produces when it narrates.
fn narrated_tool_call(
    text: &str,
    tool: &str,
    id: &str,
    argument: &str,
) -> notagent_ai::providers::faux::FauxResponseStep {
    use notagent_ai::providers::faux::{faux_assistant_message, faux_text, faux_tool_call};
    use notagent_ai::types::StopReason;

    let arguments = match tool {
        "grep" => json!({ "pattern": argument }),
        _ => json!({ "path": argument }),
    };
    faux_assistant_message(
        vec![
            faux_text(text),
            faux_tool_call(tool, arguments, Some(id.to_string())),
        ],
        StopReason::ToolUse,
    )
    .into()
}

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
                "patch",
                "call-1",
                json!({
                    "path": e2e.path("does-not-exist.txt"),
                    "edits": [{ "old_string": "before", "new_string": "after" }]
                }),
            ),
            reply("The file is missing."),
        ]);
        let mut driver = e2e.start().await;
        driver.wait_for(APP_NAME).await;

        driver.submit("edit the missing file").await;

        // The row names the tool, its target and the error the tool returned.
        // The path is the one assertion that has to survive the wrap: it is
        // longer than the 80 columns of the terminal. The badge style (the
        // default) names the tool uppercase in its state badge. A read-only
        // tool would be grouped into the explore block instead, so this uses
        // one that keeps its own row.
        driver.wait_for("PATCH").await;
        driver.wait_for_across_wraps("does-not-exist.txt").await;
        driver.assert_shows("Could not edit file");
        driver.wait_for("The file is missing.").await;
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

/// Consecutive exploration calls belong to one block. Each call arrives in its
/// own assistant message, which is the ordinary shape of a turn, and none of
/// those boundaries may end the run — the block exists precisely so that
/// looking around costs one block instead of a row per call.
#[tokio::test(flavor = "current_thread")]
async fn consecutive_exploration_calls_stay_in_one_block() {
    run_local(async {
        let e2e = InteractiveE2e::new().await;
        std::fs::write(e2e.path("notes.txt"), "hello\n").expect("fixture written");
        e2e.faux().set_responses(vec![
            tool_call_reply("ls", "call-1", json!({ "path": e2e.path(".") })),
            tool_call_reply("grep", "call-2", json!({ "pattern": "hello" })),
            tool_call_reply("read", "call-3", json!({ "path": e2e.path("notes.txt") })),
            reply("Had a look."),
        ]);
        let mut driver = e2e.start().await;
        driver.wait_for(APP_NAME).await;

        driver.submit("look around").await;
        driver.wait_for("Had a look.").await;

        let screen = driver.screen();
        assert_eq!(
            screen.matches("EXPLORED").count(),
            1,
            "one block for the whole run:\n{screen}"
        );
        // The summary groups by kind, in the reference's order.
        assert!(
            screen.contains("1 search, 1 read, 1 listing"),
            "the summary counts every call:\n{screen}"
        );
    })
    .await;
}

/// A tool that changes the project ends the run: it is not exploration, so it
/// gets its own row and whatever follows starts a fresh block.
#[tokio::test(flavor = "current_thread")]
async fn a_writing_tool_ends_the_exploration_run() {
    run_local(async {
        let e2e = InteractiveE2e::new().await;
        e2e.faux().set_responses(vec![
            tool_call_reply("ls", "call-1", json!({ "path": e2e.path(".") })),
            tool_call_reply(
                "write",
                "call-2",
                json!({ "path": e2e.path("out.txt"), "content": "x\n" }),
            ),
            tool_call_reply("ls", "call-3", json!({ "path": e2e.path(".") })),
            reply("Done."),
        ]);
        let mut driver = e2e.start().await;
        driver.wait_for(APP_NAME).await;

        driver.submit("look, write, look").await;
        driver.wait_for("Done.").await;

        let screen = driver.screen();
        assert_eq!(
            screen.matches("EXPLORED").count(),
            2,
            "the write splits the exploration in two:\n{screen}"
        );
        assert!(
            screen.contains("WRITE"),
            "the write keeps its own row:\n{screen}"
        );
    })
    .await;
}

/// The model narrating its own exploration keeps the run together: a message
/// whose text introduces the call it makes in the same breath is one step, not
/// two. This is the shape that used to split a single look into a block per
/// call.
#[tokio::test(flavor = "current_thread")]
async fn narrating_the_exploration_keeps_it_in_one_block() {
    run_local(async {
        let e2e = InteractiveE2e::new().await;
        let project = e2e.path(".");
        e2e.faux().set_responses(vec![
            narrated_tool_call("Looking at the project root", "ls", "call-1", &project),
            narrated_tool_call("Now searching for a needle", "grep", "call-2", "needle"),
            reply("Had a look."),
        ]);
        let mut driver = e2e.start().await;
        driver.wait_for(APP_NAME).await;

        driver.submit("look around").await;
        driver.wait_for("Had a look.").await;

        let screen = driver.screen();
        assert_eq!(
            screen.matches("EXPLORED").count(),
            1,
            "the narration does not split the run:\n{screen}"
        );
    })
    .await;
}

/// A second turn never joins the block of the first: the run ended with it.
#[tokio::test(flavor = "current_thread")]
async fn a_new_turn_starts_its_own_block() {
    run_local(async {
        let e2e = InteractiveE2e::new().await;
        let project = e2e.path(".");
        e2e.faux().set_responses(vec![
            tool_call_reply("ls", "call-1", json!({ "path": &project })),
            reply("First look done."),
        ]);
        let mut driver = e2e.start().await;
        driver.wait_for(APP_NAME).await;
        driver.submit("look once").await;
        driver.wait_for("First look done.").await;

        driver.faux().set_responses(vec![
            tool_call_reply("ls", "call-2", json!({ "path": &project })),
            reply("Second look done."),
        ]);
        driver.submit("look again").await;
        driver.wait_for("Second look done.").await;

        let screen = driver.screen();
        assert_eq!(
            screen.matches("EXPLORED").count(),
            2,
            "each turn has its own block:\n{screen}"
        );
    })
    .await;
}

/// A search that finished keeps its result, even when the turn it ran in did
/// not.
/// The abort path fails only the calls that never reported back
/// (`ExploreBlockComponent::fail_running_calls`); a completed one is a fact
/// about that search, not about the turn around it. Colouring it red because
/// the provider gave up afterwards would tell the user a search failed that
/// did not, and send them looking for a fault in the wrong place.
#[tokio::test(flavor = "current_thread")]
async fn a_finished_search_keeps_its_result_when_the_turn_fails() {
    use notagent::modes::interactive::theme::theme::{ThemeBg, badge, theme};

    run_local(async {
        let e2e = InteractiveE2e::new().await;
        let project = e2e.path(".");
        e2e.faux().set_responses(vec![
            tool_call_reply("ls", "call-1", json!({ "path": &project })),
            crate::app_runtime::error_reply("the provider gave up"),
        ]);
        let mut driver = e2e.start().await;
        driver.wait_for(APP_NAME).await;

        driver.submit("look around").await;
        driver.wait_for("EXPLORED").await;
        // The turn really did fail — otherwise this proves nothing.
        driver.wait_for("the provider gave up").await;

        // The fill only exists as an escape sequence, so it is read off the raw
        // writes rather than off the screen.
        let raw = driver.terminal().get_writes();
        let theme = theme();
        assert!(
            raw.contains(&badge(&theme, ThemeBg::ToolSuccessBg, "explored")),
            "the search that finished settles green:\n{}",
            driver.screen()
        );
        assert!(
            !raw.contains(&badge(&theme, ThemeBg::ToolErrorBg, "explored")),
            "and is not repainted red by the turn's failure:\n{}",
            driver.screen()
        );
    })
    .await;
}

/// A thought between two explorations splits them into two blocks. The
/// reference closes the active exploration the moment reasoning appears, so
/// the thought stands between a settled block and a fresh one instead of
/// hiding inside a block that keeps running through it.
#[tokio::test(flavor = "current_thread")]
async fn a_thought_closes_the_running_exploration() {
    use notagent_ai::providers::faux::{faux_assistant_message, faux_thinking, faux_tool_call};
    use notagent_ai::types::StopReason;

    run_local(async {
        let e2e = InteractiveE2e::new().await;
        let project = e2e.path(".");
        e2e.faux().set_responses(vec![
            tool_call_reply("ls", "call-1", json!({ "path": &project })),
            faux_assistant_message(
                vec![
                    faux_thinking("that listing needs a second look"),
                    faux_tool_call(
                        "ls",
                        json!({ "path": &project }),
                        Some("call-2".to_string()),
                    ),
                ],
                StopReason::ToolUse,
            )
            .into(),
            reply("Done looking."),
        ]);
        let mut driver = e2e.start().await;
        driver.wait_for(APP_NAME).await;

        driver.submit("look around").await;
        driver.wait_for("Done looking.").await;

        // Two blocks of one listing each — not one block that swallowed both
        // calls and kept exploring through the thought.
        let screen = driver.screen();
        assert!(!screen.contains("2 listings"), "{screen}");
        assert_eq!(screen.matches("1 listing").count(), 2, "{screen}");
    })
    .await;
}
