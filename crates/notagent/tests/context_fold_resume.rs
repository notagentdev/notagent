mod app_runtime;

use app_runtime::HeadlessApp;
use notagent::core::context_fold::{FOLD_ENTRY_TYPE, plan_fold};
use notagent::core::session_manager::{SessionEntry, SessionManager};
use notagent_agent::types::AgentMessage;
use serde_json::json;

fn message(value: serde_json::Value) -> AgentMessage {
    serde_json::from_value(value).expect("message")
}

/// A session file with three grep steps, the first two folded.
fn session_with_a_fold(app: &HeadlessApp) -> String {
    let mut manager =
        SessionManager::create(&app.cwd(), Some(&app.path("sessions")), None).expect("manager");
    manager
        .append_message(&message(
            json!({ "role": "user", "content": "find the bug", "timestamp": 1 }),
        ))
        .expect("user");
    let mut steps = Vec::new();
    for index in 0..3 {
        let call = format!("call-{index}");
        steps.push(
            manager
                .append_message(&message(json!({
                    "role": "assistant",
                    "content": [{ "type": "toolCall", "id": call, "name": "grep", "arguments": {} }],
                    "api": "faux", "provider": "faux", "model": "faux",
                    "usage": { "input": 1, "output": 1, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 2,
                        "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0 } },
                    "stopReason": "toolUse", "timestamp": 2,
                })))
                .expect("assistant"),
        );
        manager
            .append_message(&message(json!({
                "role": "toolResult", "toolCallId": call, "toolName": "grep",
                "content": [{ "type": "text", "text": format!("needle-{index}") }],
                "isError": false, "timestamp": 3,
            })))
            .expect("result");
    }
    let entries = manager.build_context_entries();
    let refs: Vec<&SessionEntry> = entries.iter().collect();
    let planned = plan_fold(&refs, &steps[0], &steps[1], "folded summary text").expect("plan");
    manager
        .append_custom_entry(
            FOLD_ENTRY_TYPE,
            Some(serde_json::to_value(planned.record).expect("record")),
        )
        .expect("fold");
    manager.get_session_file().expect("file").to_owned()
}

fn context(app: &HeadlessApp) -> String {
    serde_json::to_string(&app.session().messages()).expect("json")
}

#[tokio::test]
async fn a_resumed_session_starts_unfolded_when_folding_is_off() {
    let app = HeadlessApp::create().await;
    let file = session_with_a_fold(&app);

    app.runtime()
        .switch_session(&file, None)
        .await
        .expect("resume");
    assert!(
        context(&app).contains("folded summary text") && !context(&app).contains("needle-0"),
        "with folding on the resumed context is folded"
    );

    app.session()
        .settings_manager()
        .set_context_folding_enabled(false);
    app.runtime()
        .switch_session(&file, None)
        .await
        .expect("resume again");
    assert!(
        context(&app).contains("needle-0") && !context(&app).contains("folded summary text"),
        "with folding off the first context built for the resumed session is the full history"
    );
}
