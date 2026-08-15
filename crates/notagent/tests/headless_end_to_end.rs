//! End-to-end evidence for gate G2 (master plan, section Gates): the headless
//! modes drive a real session against the faux provider, from the prompt through
//! a tool call to the answer.
//!
//! Nothing below the mode is a stand-in. The model runtime is the real one with
//! the faux provider registered as a native provider, the services are the ones
//! `main.ts` builds, the session comes out of `create_agent_session_from_services`
//! and the tools are the built-in ones — only the provider is scripted.

mod app_runtime;

use std::sync::Arc;

use app_runtime::{HeadlessApp, reply, tool_call_reply};
use notagent::core::agent_session::PromptOptions;
use notagent::modes::print_mode::{PrintModeOptions, PrintOutputMode, run_print_mode};
use notagent::modes::rpc::rpc_mode::{RpcState, handle_input_line};
use serde_json::{Value, json};

#[tokio::test]
async fn print_mode_runs_a_prompt_through_a_tool_call_to_the_answer() {
    let app = HeadlessApp::create().await;
    let target = app.path("notes.txt");
    app.faux().set_responses(vec![
        tool_call_reply(
            "write",
            "call-1",
            json!({ "path": target, "content": "written by the agent\n" }),
        ),
        reply("Wrote the file."),
    ]);

    let exit_code = run_print_mode(
        app.runtime(),
        PrintModeOptions {
            mode: Some(PrintOutputMode::Text),
            initial_message: Some("Write the note".to_owned()),
            ..PrintModeOptions::default()
        },
    )
    .await;

    assert_eq!(exit_code, 0);
    assert_eq!(
        std::fs::read_to_string(&target).expect("written file"),
        "written by the agent\n"
    );
    let messages = app.session().messages();
    assert!(
        messages.iter().any(|message| matches!(
            message,
            notagent_agent::types::AgentMessage::ToolResult(result) if result.tool_name == "write"
        )),
        "the tool result is part of the transcript: {messages:#?}"
    );
    assert_eq!(
        app.session().get_last_assistant_text().as_deref(),
        Some("Wrote the file.")
    );
}

#[tokio::test]
async fn print_mode_reports_an_assistant_error_as_a_non_zero_exit_code() {
    let app = HeadlessApp::create().await;
    app.faux()
        .set_responses(vec![app_runtime::error_reply("provider failure")]);

    let exit_code = run_print_mode(
        app.runtime(),
        PrintModeOptions {
            mode: Some(PrintOutputMode::Text),
            initial_message: Some("Say something".to_owned()),
            ..PrintModeOptions::default()
        },
    )
    .await;

    assert_eq!(exit_code, 1);
}

#[tokio::test]
async fn json_mode_streams_the_session_events_as_json_lines() {
    let app = HeadlessApp::create().await;
    app.faux()
        .set_responses(vec![reply("Hello from json mode")]);

    let events = Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
    let sink = Arc::clone(&events);
    let subscription = app.session().subscribe(Arc::new(move |event| {
        sink.lock()
            .expect("poisoned")
            .push(notagent::modes::json_event::to_json_event(&event));
    }));

    app.session()
        .prompt("Say hello", PromptOptions::default())
        .await
        .expect("prompt");
    drop(subscription);

    let events = events.lock().expect("poisoned").clone();
    let types: Vec<&str> = events
        .iter()
        .filter_map(|event| event.get("type").and_then(Value::as_str))
        .collect();
    assert!(types.contains(&"agent_start"), "{types:?}");
    assert!(types.contains(&"message_end"), "{types:?}");
    assert!(types.contains(&"agent_settled"), "{types:?}");

    // The cumulative snapshot of a streaming delta never goes on the wire.
    for event in &events {
        if event.get("type").and_then(Value::as_str) == Some("message_update") {
            let assistant_event = event
                .get("assistantMessageEvent")
                .and_then(Value::as_object)
                .expect("assistantMessageEvent");
            assert!(
                !assistant_event.contains_key("partial"),
                "partial snapshot leaked: {event}"
            );
        }
    }
}

#[tokio::test]
async fn rpc_mode_answers_commands_and_streams_events() {
    let app = HeadlessApp::create().await;
    app.faux().set_responses(vec![reply("Hello from rpc")]);

    let records = Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
    let sink = Arc::clone(&records);
    let state = RpcState::new(
        app.runtime(),
        Some(Arc::new(move |value: &Value| {
            sink.lock().expect("poisoned").push(value.clone());
        })),
    );
    state.rebind_session();

    handle_input_line(
        &state,
        &json!({ "id": "1", "type": "get_state" }).to_string(),
    )
    .await;
    handle_input_line(
        &state,
        &json!({ "id": "2", "type": "prompt", "message": "Say hello" }).to_string(),
    )
    .await;
    // The prompt answers from its own task, so the client waits for the settle
    // event on the wire rather than for the session it cannot see.
    for _ in 0..200 {
        let settled = records
            .lock()
            .expect("poisoned")
            .iter()
            .any(|record| record.get("type").and_then(Value::as_str) == Some("agent_settled"));
        if settled {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    let records = records.lock().expect("poisoned").clone();
    let state_response = records
        .iter()
        .find(|record| record.get("id").and_then(Value::as_str) == Some("1"))
        .expect("get_state response");
    assert_eq!(state_response["command"], json!("get_state"));
    assert_eq!(state_response["success"], json!(true));
    assert_eq!(state_response["data"]["isStreaming"], json!(false));
    assert_eq!(state_response["data"]["messageCount"], json!(0));

    let prompt_responses: Vec<&Value> = records
        .iter()
        .filter(|record| {
            record.get("id").and_then(Value::as_str) == Some("2")
                && record.get("type").and_then(Value::as_str) == Some("response")
        })
        .collect();
    assert_eq!(prompt_responses.len(), 1, "exactly one prompt response");
    assert_eq!(prompt_responses[0]["success"], json!(true));

    assert!(
        records
            .iter()
            .any(|record| record.get("type").and_then(Value::as_str) == Some("agent_settled")),
        "the run settles on the wire"
    );
}

#[tokio::test]
async fn rpc_mode_answers_an_unknown_command_and_a_malformed_line() {
    let app = HeadlessApp::create().await;
    let records = Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
    let sink = Arc::clone(&records);
    let state = RpcState::new(
        app.runtime(),
        Some(Arc::new(move |value: &Value| {
            sink.lock().expect("poisoned").push(value.clone());
        })),
    );

    handle_input_line(&state, "{not json").await;
    handle_input_line(&state, &json!({ "id": "7", "type": "nope" }).to_string()).await;

    let records = records.lock().expect("poisoned").clone();
    assert_eq!(records[0]["command"], json!("parse"));
    assert_eq!(records[0]["success"], json!(false));
    assert!(
        records[0]["error"]
            .as_str()
            .expect("error")
            .starts_with("Failed to parse command: ")
    );
    assert_eq!(records[1]["id"], json!("7"));
    assert_eq!(records[1]["command"], json!("nope"));
    assert_eq!(records[1]["error"], json!("Unknown command: nope"));
}
