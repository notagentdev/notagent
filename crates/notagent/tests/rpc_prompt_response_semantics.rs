//! Port of `packages/coding-agent/test/rpc-prompt-response-semantics.test.ts`.
//!
//! Exactly one `prompt` response per command, and it says what preflight
//! decided: a rejected prompt fails, an accepted one succeeds even though its
//! run is still going, and a prompt queued during streaming also succeeds.
//!
//! Deviation (class 3): TypeScript mocks the output guard and the JSONL reader
//! to observe the wire; here the RPC run takes an output sink and the lines are
//! fed to `handle_input_line` directly, which is the same seam without module
//! mocking.

mod app_runtime;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use app_runtime::{HeadlessApp, reply};
use notagent::modes::rpc::rpc_mode::{RpcState, handle_input_line};
use serde_json::{Value, json};

fn prompt_responses(records: &Arc<Mutex<Vec<Value>>>, id: &str) -> Vec<Value> {
    records
        .lock()
        .expect("poisoned")
        .iter()
        .filter(|record| {
            record.get("id").and_then(Value::as_str) == Some(id)
                && record.get("type").and_then(Value::as_str) == Some("response")
                && record.get("command").and_then(Value::as_str) == Some("prompt")
        })
        .cloned()
        .collect()
}

async fn wait_for_one_response(records: &Arc<Mutex<Vec<Value>>>, id: &str) -> Value {
    for _ in 0..400 {
        let responses = prompt_responses(records, id);
        if !responses.is_empty() {
            assert_eq!(responses.len(), 1, "exactly one response for {id}");
            return responses[0].clone();
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("no prompt response for {id}");
}

fn start(app: &HeadlessApp) -> (Arc<RpcState>, Arc<Mutex<Vec<Value>>>) {
    let records: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&records);
    let state = RpcState::new(
        app.runtime(),
        Some(Arc::new(move |value: &Value| {
            sink.lock().expect("poisoned").push(value.clone());
        })),
    );
    state.rebind_session();
    (state, records)
}

#[tokio::test]
async fn emits_one_failure_response_when_prompt_preflight_rejects() {
    // No credential for the model's provider: preflight refuses before a
    // request is made.
    let app = HeadlessApp::create_without_auth().await;
    let (state, records) = start(&app);

    handle_input_line(
        &state,
        &json!({ "id": "b1", "type": "prompt", "message": "Hello" }).to_string(),
    )
    .await;

    let response = wait_for_one_response(&records, "b1").await;
    assert_eq!(response["type"], json!("response"));
    assert_eq!(response["command"], json!("prompt"));
    assert_eq!(response["success"], json!(false));
    let error = response["error"].as_str().expect("error");
    assert!(
        error.starts_with("No API key found for fake-provider.\n\nUse /login to log into a provider via OAuth or API key. See:"),
        "unexpected error text: {error}"
    );
}

#[tokio::test]
async fn emits_one_success_response_when_prompt_preflight_succeeds() {
    let app = HeadlessApp::create().await;
    app.faux().set_responses(vec![reply("done")]);
    let (state, records) = start(&app);

    handle_input_line(
        &state,
        &json!({ "id": "b2", "type": "prompt", "message": "Hello" }).to_string(),
    )
    .await;

    let response = wait_for_one_response(&records, "b2").await;
    assert_eq!(response["success"], json!(true));
}

#[tokio::test]
async fn emits_one_success_response_when_prompt_is_queued_during_streaming() {
    // A slow provider keeps the first run in flight while the second prompt
    // arrives, which is the case the queue answer is about.
    let app = HeadlessApp::create_slow(20.0).await;
    app.faux().set_responses(vec![
        reply("first answer with several words"),
        reply("second"),
    ]);
    let (state, records) = start(&app);

    handle_input_line(
        &state,
        &json!({ "id": "b3-start", "type": "prompt", "message": "Start" }).to_string(),
    )
    .await;
    wait_for_one_response(&records, "b3-start").await;

    handle_input_line(
        &state,
        &json!({
            "id": "b3",
            "type": "prompt",
            "message": "Queue this",
            "streamingBehavior": "followUp"
        })
        .to_string(),
    )
    .await;

    let response = wait_for_one_response(&records, "b3").await;
    assert_eq!(response["success"], json!(true));

    app.session().abort().await;
}
