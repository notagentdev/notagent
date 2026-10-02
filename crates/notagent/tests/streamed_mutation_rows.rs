//! A file-mutating call carries a whole file (or whole edits) in its arguments.
//! Its row belongs to the moment the tool starts, with the arguments complete;
//! a row painted from the streamed partial message would repaint on every
//! token and count its diff up while the model is still writing.

mod app_runtime;

use std::time::Duration;

use app_runtime::{HeadlessApp, reply, tool_call_reply};
use notagent::modes::interactive::interactive_mode::{
    InteractiveModeHandle, InteractiveModeOptions, InteractiveTerminal, create_interactive_mode,
};
use notagent_agent::types::AgentMessage;
use notagent_ai::types::StopReason;
use notagent_tui::test_terminal::VirtualTerminal;
use notagent_tui::tui::run_until;
use serde_json::json;

async fn local<F: std::future::Future>(body: F) -> F::Output {
    tokio::task::LocalSet::new().run_until(body).await
}

fn long_text(lines: usize) -> String {
    (0..lines)
        .map(|index| format!("let value_{index:04} = {index};\n"))
        .collect()
}

/// What the screen showed while the model was still streaming the call.
struct Observation {
    samples: usize,
    rows_while_streaming: Vec<String>,
}

async fn watch_streamed_call(
    call: notagent_ai::providers::faux::FauxResponseStep,
    heading: &str,
) -> Observation {
    let app = HeadlessApp::create_slow(600.0).await;
    std::fs::write(app.path("target.rs"), "let seed = 0;\n").expect("fixture");
    app.faux().set_responses(vec![call, reply("Done.")]);
    let terminal = VirtualTerminal::new(100, 40);
    let InteractiveModeHandle {
        renderer,
        pump,
        run,
        ..
    } = create_interactive_mode(
        app.runtime(),
        InteractiveModeOptions {
            terminal: Some(InteractiveTerminal {
                terminal: Box::new(terminal.clone()),
                pump: Box::new(terminal.pump_handle()),
            }),
            ..InteractiveModeOptions::default()
        },
    );
    let mut renderer = renderer;
    let mut pump = pump;
    let mut run = Box::pin(run);

    let mut settle = async |millis: u64| {
        run_until(renderer.as_mut(), pump.as_mut(), async {
            tokio::select! {
                _ = &mut run => {}
                () = tokio::time::sleep(Duration::from_millis(millis)) => {}
            }
        })
        .await;
    };

    settle(300).await;
    terminal.send_input("change the file");
    settle(50).await;
    terminal.send_input("\r");

    let mut observation = Observation {
        samples: 0,
        rows_while_streaming: Vec::new(),
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        settle(20).await;
        // The window closes with the assistant message: from then on the
        // arguments are complete and the tool starts.
        let message_complete = app.session().messages().iter().any(|message| {
            matches!(
                message,
                AgentMessage::Assistant(assistant)
                    if assistant.stop_reason == StopReason::ToolUse
            )
        });
        let buffer = terminal.get_scroll_buffer().join("\n");
        if message_complete {
            break;
        }
        observation.samples += 1;
        for row in buffer.lines().filter(|row| row.contains(heading)) {
            let row = row.trim().to_owned();
            if !observation.rows_while_streaming.contains(&row) {
                observation.rows_while_streaming.push(row);
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the message never completed:\n{buffer}"
        );
    }
    observation
}

#[tokio::test(flavor = "current_thread")]
async fn a_streamed_write_paints_no_row_before_it_executes() {
    local(async {
        let seen = watch_streamed_call(
            tool_call_reply(
                "write",
                "write-1",
                json!({ "path": "out.rs", "content": long_text(300) }),
            ),
            "Write",
        )
        .await;
        assert!(seen.samples > 5, "the stream must be observable");
        assert!(
            seen.rows_while_streaming.is_empty(),
            "a streamed write must not paint a row while the model is still streaming it: {:#?}",
            seen.rows_while_streaming
        );
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_streamed_patch_minified_paints_no_row_before_it_executes() {
    local(async {
        let seen = watch_streamed_call(
            tool_call_reply(
                "patch_minified",
                "patch-1",
                json!({ "path": "target.rs", "old_string": "seed", "new_string": long_text(300) }),
            ),
            "Patch",
        )
        .await;
        assert!(seen.samples > 5, "the stream must be observable");
        assert!(
            seen.rows_while_streaming.is_empty(),
            "a streamed patch_minified must not paint a row while the model is still streaming it: {:#?}",
            seen.rows_while_streaming
        );
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_streamed_multi_patch_minified_paints_no_row_before_it_executes() {
    local(async {
        let edits: Vec<_> = (0..60)
            .map(|index| json!({ "old_string": format!("seed{index}"), "new_string": format!("let grown_{index} = {index};\nlet more_{index} = {index};") }))
            .collect();
        let seen = watch_streamed_call(
            tool_call_reply(
                "multi_patch_minified",
                "multi-1",
                json!({ "path": "target.rs", "edits": edits }),
            ),
            "Patch",
        )
        .await;
        assert!(seen.samples > 5, "the stream must be observable");
        assert!(
            seen.rows_while_streaming.is_empty(),
            "a streamed multi_patch_minified must not paint a row while the model is still streaming it: {:#?}",
            seen.rows_while_streaming
        );
    })
    .await;
}
