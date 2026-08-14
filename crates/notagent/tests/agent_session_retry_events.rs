//! Ported from `packages/coding-agent/test/suite/agent-session-retry-events.test.ts`.
//!
//! Not ported: "emits extension events before public event subscribers"
//! (`plans/facts/extension-boundary.md`).

mod suite;

use std::sync::{Arc, Mutex};

use notagent::core::agent_session::{AgentSessionEvent, PromptOptions};
use notagent_agent::types::AgentEvent;
use notagent_ai::providers::faux::{FauxResponseStep, faux_assistant_message, faux_text};
use notagent_ai::types::StopReason;
use serde_json::json;
use suite::{HarnessOptions, create_harness};

fn retry_settings(max_retries: u64, base_delay_ms: u64) -> serde_json::Value {
    json!({ "retry": { "enabled": true, "maxRetries": max_retries, "baseDelayMs": base_delay_ms } })
}

fn reply(text: &str) -> FauxResponseStep {
    faux_assistant_message(vec![faux_text(text)], StopReason::Stop).into()
}

fn transient_error() -> FauxResponseStep {
    let mut message = faux_assistant_message(Vec::new(), StopReason::Error);
    message.error_message = Some("overloaded_error".to_string());
    message.into()
}

/// Records the retry events in the order they arrive.
fn retry_log(harness: &suite::Harness) -> Arc<Mutex<Vec<String>>> {
    let log = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&log);
    // The handle is leaked deliberately: the subscription has to outlive the
    // borrow, and the harness drops the session at the end of the case.
    std::mem::forget(harness.session.subscribe(Arc::new(move |event| {
        match event {
            AgentSessionEvent::AutoRetryStart { attempt, .. } => sink
                .lock()
                .expect("poisoned")
                .push(format!("start:{attempt}")),
            AgentSessionEvent::AutoRetryEnd { success, .. } => sink
                .lock()
                .expect("poisoned")
                .push(format!("end:{success}")),
            _ => {}
        }
    })));
    log
}

fn agent_end_will_retry(harness: &suite::Harness) -> Vec<bool> {
    harness
        .events()
        .into_iter()
        .filter_map(|event| match event {
            AgentSessionEvent::AgentEnd { will_retry, .. } => Some(will_retry),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn retries_after_a_transient_error_and_succeeds() {
    let harness = create_harness(HarnessOptions {
        settings: Some(retry_settings(3, 1)),
        ..HarnessOptions::default()
    });
    let log = retry_log(&harness);
    harness.set_responses(vec![transient_error(), reply("recovered")]);

    harness
        .session
        .prompt("test", PromptOptions::default())
        .await
        .expect("prompt");

    assert_eq!(
        *log.lock().expect("poisoned"),
        vec!["start:1".to_string(), "end:true".to_string()]
    );
    assert_eq!(agent_end_will_retry(&harness), vec![true, false]);
    assert_eq!(
        harness
            .faux
            .state
            .call_count
            .load(std::sync::atomic::Ordering::SeqCst),
        2
    );
    assert!(!harness.session.is_retrying());
}

#[tokio::test]
async fn retries_several_transient_failures_and_succeeds_on_the_last_attempt() {
    let harness = create_harness(HarnessOptions {
        settings: Some(retry_settings(3, 1)),
        ..HarnessOptions::default()
    });
    let log = retry_log(&harness);
    harness.set_responses(vec![transient_error(), transient_error(), reply("success")]);

    harness
        .session
        .prompt("test", PromptOptions::default())
        .await
        .expect("prompt");

    assert_eq!(
        *log.lock().expect("poisoned"),
        vec![
            "start:1".to_string(),
            "start:2".to_string(),
            "end:true".to_string()
        ]
    );
    assert_eq!(
        harness
            .faux
            .state
            .call_count
            .load(std::sync::atomic::Ordering::SeqCst),
        3
    );
    assert_eq!(
        harness.assistant_texts().last().map(String::as_str),
        Some("success")
    );
}

#[tokio::test]
async fn exhausts_the_retry_budget_and_reports_the_failure() {
    let harness = create_harness(HarnessOptions {
        settings: Some(retry_settings(2, 1)),
        ..HarnessOptions::default()
    });
    let log = retry_log(&harness);
    harness.set_responses(vec![
        transient_error(),
        transient_error(),
        transient_error(),
    ]);

    harness
        .session
        .prompt("test", PromptOptions::default())
        .await
        .expect("prompt");

    let log = log.lock().expect("poisoned").clone();
    assert_eq!(
        log,
        vec![
            "start:1".to_string(),
            "start:2".to_string(),
            "end:false".to_string()
        ]
    );
    assert_eq!(
        harness
            .faux
            .state
            .call_count
            .load(std::sync::atomic::Ordering::SeqCst),
        3
    );
    assert!(!harness.session.is_retrying());
}

#[tokio::test]
async fn does_not_retry_when_retry_is_disabled() {
    let harness = create_harness(HarnessOptions {
        settings: Some(json!({ "retry": { "enabled": false } })),
        ..HarnessOptions::default()
    });
    harness.set_responses(vec![transient_error(), reply("never reached")]);

    harness
        .session
        .prompt("test", PromptOptions::default())
        .await
        .expect("prompt");

    assert_eq!(
        harness
            .faux
            .state
            .call_count
            .load(std::sync::atomic::Ordering::SeqCst),
        1
    );
}

#[tokio::test]
async fn does_not_retry_an_error_that_will_not_get_better() {
    let harness = create_harness(HarnessOptions {
        settings: Some(retry_settings(3, 1)),
        ..HarnessOptions::default()
    });
    let mut message = faux_assistant_message(Vec::new(), StopReason::Error);
    message.error_message = Some("invalid_request_error: bad tool schema".to_string());
    harness.set_responses(vec![message.into(), reply("never reached")]);

    harness
        .session
        .prompt("test", PromptOptions::default())
        .await
        .expect("prompt");

    assert_eq!(
        harness
            .faux
            .state
            .call_count
            .load(std::sync::atomic::Ordering::SeqCst),
        1
    );
}

/// The TypeScript `normalizeEventOrder`: message events carry their role, and a
/// run of `message_update` collapses to one.
fn normalize_event_order(events: Vec<AgentSessionEvent>) -> Vec<String> {
    let mut normalized: Vec<String> = Vec::new();
    for event in events {
        let label = match &event {
            AgentSessionEvent::Agent(AgentEvent::MessageStart { message }) => {
                format!("message_start:{}", message_role(message))
            }
            AgentSessionEvent::Agent(AgentEvent::MessageEnd { message }) => {
                format!("message_end:{}", message_role(message))
            }
            AgentSessionEvent::Agent(AgentEvent::ToolExecutionStart { tool_name, .. }) => {
                format!("tool_execution_start:{tool_name}")
            }
            AgentSessionEvent::Agent(AgentEvent::ToolExecutionEnd { tool_name, .. }) => {
                format!("tool_execution_end:{tool_name}")
            }
            AgentSessionEvent::Agent(event) => event.type_name().to_string(),
            AgentSessionEvent::AgentEnd { .. } => "agent_end".to_string(),
            AgentSessionEvent::AgentSettled => "agent_settled".to_string(),
            _ => continue,
        };
        if label == "message_update"
            && normalized.last().map(String::as_str) == Some("message_update")
        {
            continue;
        }
        normalized.push(label);
    }
    normalized
}

fn message_role(message: &notagent_agent::types::AgentMessage) -> &'static str {
    use notagent_agent::types::AgentMessage;
    match message {
        AgentMessage::User(_) => "user",
        AgentMessage::Assistant(_) => "assistant",
        AgentMessage::ToolResult(_) => "toolResult",
        AgentMessage::BashExecution(_) => "bashExecution",
        AgentMessage::Custom(_) => "custom",
        AgentMessage::BranchSummary(_) => "branchSummary",
        AgentMessage::CompactionSummary(_) => "compactionSummary",
    }
}

#[tokio::test]
async fn emits_the_expected_event_order_for_a_single_prompt() {
    let harness = create_harness(HarnessOptions::default());
    harness.set_responses(vec![reply("hello")]);

    harness
        .session
        .prompt("hi", PromptOptions::default())
        .await
        .expect("prompt");

    assert_eq!(
        normalize_event_order(harness.events()),
        vec![
            "agent_start",
            "turn_start",
            "message_start:user",
            "message_end:user",
            "message_start:assistant",
            "message_update",
            "message_end:assistant",
            "turn_end",
            "agent_end",
            "agent_settled",
        ]
    );
}

#[tokio::test]
async fn emits_agent_end_for_error_responses() {
    let harness = create_harness(HarnessOptions {
        settings: Some(json!({ "retry": { "enabled": false } })),
        ..HarnessOptions::default()
    });
    let mut message = faux_assistant_message(Vec::new(), StopReason::Error);
    message.error_message = Some("boom".to_string());
    harness.set_responses(vec![message.into()]);

    harness
        .session
        .prompt("hi", PromptOptions::default())
        .await
        .expect("prompt");

    assert_eq!(agent_end_will_retry(&harness), vec![false]);
}

#[tokio::test]
async fn streams_message_updates_while_the_response_arrives() {
    let harness = create_harness(HarnessOptions::default());
    harness.set_responses(vec![reply("a longer answer that arrives in pieces")]);

    harness
        .session
        .prompt("hi", PromptOptions::default())
        .await
        .expect("prompt");

    let updates = harness
        .events()
        .into_iter()
        .filter(|event| {
            matches!(
                event,
                AgentSessionEvent::Agent(AgentEvent::MessageUpdate { .. })
            )
        })
        .count();
    assert!(updates >= 1);
}
