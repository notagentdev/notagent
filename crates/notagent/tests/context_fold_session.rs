mod suite;

use std::sync::{Arc, Mutex};

use notagent::core::agent_session::{AgentSessionEvent, PromptOptions};
use notagent_ai::providers::faux::{
    FauxResponseStep, faux_assistant_message, faux_text, faux_tool_call,
};
use notagent_ai::types::{AssistantMessage, Context, StopReason};
use serde_json::json;
use suite::{HarnessOptions, create_harness};

/// About 12k estimated tokens once `read` has truncated it: large enough that
/// two of them pass the compaction threshold below and one does not.
fn write_large_file(directory: &std::path::Path, name: &str, marker: &str) {
    let body: String = (0..1500)
        .map(|line| format!("{marker} line {line:04} padding padding padding\n"))
        .collect();
    std::fs::write(directory.join(name), body).expect("write");
}

fn call(name: &str, arguments: serde_json::Value, id: &str) -> AssistantMessage {
    faux_assistant_message(
        vec![faux_tool_call(name, arguments, Some(id.to_owned()))],
        StopReason::ToolUse,
    )
}

/// The first step id a `status` result lists.
fn first_step_id(context: &Context) -> String {
    let text = serde_json::to_string(context).expect("json");
    let marker = "steps, oldest first";
    let after = &text[text.find(marker).expect("a status result") + marker.len()..];
    let start = after.find("- ").expect("a step line") + 2;
    after[start..start + 8].to_owned()
}

#[tokio::test]
async fn a_fold_lands_before_the_compaction_check_so_compaction_does_not_fire() {
    let harness = create_harness(HarnessOptions {
        settings: Some(json!({
            "compaction": { "enabled": true },
            "retry": { "enabled": false }
        })),
        // Compaction fires above 20k estimated tokens.
        runtime_window_override: Some(16_384 + 20_000),
        initial_active_tool_names: Some(vec!["read".to_owned(), "context_fold".to_owned()]),
        ..HarnessOptions::default()
    });
    write_large_file(harness.temp.path(), "first.txt", "first-file");
    write_large_file(harness.temp.path(), "second.txt", "second-file");

    let requests: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let record = |requests: &Arc<Mutex<Vec<String>>>, context: &Context| {
        requests
            .lock()
            .expect("requests")
            .push(serde_json::to_string(context).expect("json"));
    };
    let responses: Vec<FauxResponseStep> = vec![
        {
            let requests = Arc::clone(&requests);
            FauxResponseStep::Factory(Arc::new(move |context, _| {
                record(&requests, context);
                call("read", json!({ "path": "first.txt" }), "read-first")
            }))
        },
        {
            let requests = Arc::clone(&requests);
            FauxResponseStep::Factory(Arc::new(move |context, _| {
                record(&requests, context);
                call("context_fold", json!({ "action": "status" }), "status")
            }))
        },
        {
            let requests = Arc::clone(&requests);
            FauxResponseStep::Factory(Arc::new(move |context, _| {
                record(&requests, context);
                let step = first_step_id(context);
                faux_assistant_message(
                    vec![
                        faux_tool_call(
                            "read",
                            json!({ "path": "second.txt" }),
                            Some("read-second".to_owned()),
                        ),
                        faux_tool_call(
                            "context_fold",
                            json!({
                                "action": "fold",
                                "from": step,
                                "to": step,
                                "summary": "first.txt holds 1500 numbered first-file lines; nothing else",
                            }),
                            Some("fold".to_owned()),
                        ),
                    ],
                    StopReason::ToolUse,
                )
            }))
        },
        {
            let requests = Arc::clone(&requests);
            FauxResponseStep::Factory(Arc::new(move |context, _| {
                record(&requests, context);
                faux_assistant_message(vec![faux_text("done")], StopReason::Stop)
            }))
        },
    ];
    harness.set_responses(responses);

    tokio::time::timeout(
        std::time::Duration::from_secs(20),
        harness
            .session
            .prompt("look at both files", PromptOptions::default()),
    )
    .await
    .expect("in time")
    .expect("prompt");

    assert_eq!(harness.pending_response_count(), 0);
    let requests = requests.lock().expect("requests");
    assert_eq!(requests.len(), 4);
    assert!(
        requests[2].contains("first-file line 0001"),
        "before the fold the file is in the context"
    );
    assert!(
        requests[3].contains("Folded 1 step(s)"),
        "the fold succeeded"
    );
    assert!(
        !requests[3].contains("first-file line 0001"),
        "the request after the fold no longer carries the folded file"
    );
    assert!(requests[3].contains("first.txt holds 1500 numbered first-file lines"));
    assert!(
        requests[3].contains("second-file"),
        "the step in flight is untouched"
    );
    assert!(
        !harness
            .events()
            .iter()
            .any(|event| matches!(event, AgentSessionEvent::CompactionStart { .. })),
        "the fold brought the context under the threshold, so compaction had nothing to do"
    );
}

#[tokio::test]
async fn switched_off_the_tool_is_not_offered() {
    let harness = create_harness(HarnessOptions {
        settings: Some(json!({ "contextFolding": false })),
        initial_active_tool_names: Some(vec!["read".to_owned(), "context_fold".to_owned()]),
        ..HarnessOptions::default()
    });
    assert!(
        !harness
            .session
            .get_active_tool_names()
            .iter()
            .any(|name| name == "context_fold"),
        "with folding off the tool does not exist"
    );

    harness.settings_manager.set_context_folding_enabled(true);
    harness.session.apply_context_folding_setting();
    assert!(
        harness
            .session
            .get_all_tools()
            .iter()
            .any(|tool| tool.name == "context_fold"),
        "switching it on brings the tool back"
    );
}

#[tokio::test]
async fn the_model_is_reminded_once_when_enough_finished_work_has_built_up() {
    let harness = create_harness(HarnessOptions {
        settings: Some(json!({ "retry": { "enabled": false } })),
        // A quarter of this window, 12.5k tokens, is the growth that earns a
        // reminder. Each part is about 6k: the reminder comes once three
        // finished parts are unfolded, and the five never reach a second one
        // or compaction.
        runtime_window_override: Some(50_000),
        initial_active_tool_names: Some(vec!["read".to_owned(), "context_fold".to_owned()]),
        ..HarnessOptions::default()
    });
    for index in 0..5 {
        let body: String = (0..530)
            .map(|line| format!("part-{index} line {line:04} padding padding padding\n"))
            .collect();
        std::fs::write(harness.temp.path().join(format!("part-{index}.txt")), body).expect("write");
    }
    let requests: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let mut responses: Vec<FauxResponseStep> = (0..5)
        .map(|index| {
            let requests = Arc::clone(&requests);
            FauxResponseStep::Factory(Arc::new(move |context: &Context, _| {
                requests
                    .lock()
                    .expect("requests")
                    .push(serde_json::to_string(context).expect("json"));
                call(
                    "read",
                    json!({ "path": format!("part-{index}.txt") }),
                    &format!("read-{index}"),
                )
            }))
        })
        .collect();
    {
        let requests = Arc::clone(&requests);
        responses.push(FauxResponseStep::Factory(Arc::new(
            move |context: &Context, _| {
                requests
                    .lock()
                    .expect("requests")
                    .push(serde_json::to_string(context).expect("json"));
                faux_assistant_message(vec![faux_text("done")], StopReason::Stop)
            },
        )));
    }
    harness.set_responses(responses);

    tokio::time::timeout(
        std::time::Duration::from_secs(20),
        harness
            .session
            .prompt("read all parts", PromptOptions::default()),
    )
    .await
    .expect("in time")
    .expect("prompt");

    let requests = requests.lock().expect("requests");
    let last = requests.last().expect("a final request");
    assert_eq!(
        last.matches("Context reminder").count(),
        1,
        "the reminder arrives once and stays in the context"
    );
    assert!(
        !requests[0].contains("Context reminder"),
        "an empty context earns no reminder"
    );
    assert!(
        !requests[2].contains("Context reminder"),
        "two finished parts are too little to be worth a fold"
    );
    let reminder = &last[last.find("Context reminder").expect("reminder")..];
    let reminder = &reminder[..reminder.find(']').expect("end of the reminder")];
    assert!(
        reminder.contains("action \\\"fold\\\", from \\\""),
        "the reminder names where to start: {reminder}"
    );
    assert!(
        !reminder.contains('%'),
        "the reminder does not tell the fill level: {reminder}"
    );
}
