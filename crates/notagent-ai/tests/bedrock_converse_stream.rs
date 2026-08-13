//! Differential test of the Amazon Bedrock Converse-Stream adapter.
//!
//! `fixtures/bedrock-converse-stream.jsonl` records, for each case, the
//! `ConverseStreamCommand` input the TS implementation builds (through its `onPayload`
//! hook, with the AWS SDK client mocked) plus the event sequence it emits for a scripted
//! stream (see `fixtures/generators`).

use std::collections::BTreeMap;

use notagent_ai::api::bedrock_converse_stream::{
    BedrockOptions, BedrockStreamState, BedrockThinkingDisplay, BedrockToolChoice,
    bedrock_failure_diagnostic_details, build_client_config, build_command_input,
    configured_credentials, format_bedrock_error, is_anthropic_claude_model,
    is_gov_cloud_bedrock_target, is_reserved_header, map_stop_reason, normalize_diagnostic_value,
    should_use_explicit_endpoint, standard_endpoint_region, supports_adaptive_thinking,
    supports_native_xhigh_effort, supports_prompt_caching,
};
use notagent_ai::types::{
    AssistantMessageEvent, CacheRetention, Context, Model, ProviderEnv, StopReason,
    ThinkingBudgets, ThinkingLevel,
};
use serde_json::{Value, json};

fn thinking_level(value: &str) -> ThinkingLevel {
    match value {
        "minimal" => ThinkingLevel::Minimal,
        "low" => ThinkingLevel::Low,
        "medium" => ThinkingLevel::Medium,
        "high" => ThinkingLevel::High,
        "xhigh" => ThinkingLevel::Xhigh,
        "max" => ThinkingLevel::Max,
        other => panic!("unknown thinking level {other}"),
    }
}

fn options_from_fixture(raw: &Value) -> BedrockOptions {
    let raw = raw.as_object().cloned().unwrap_or_default();
    BedrockOptions {
        tool_choice: raw.get("toolChoice").map(|choice| match choice {
            Value::String(value) if value == "auto" => BedrockToolChoice::Auto,
            Value::String(value) if value == "any" => BedrockToolChoice::Any,
            Value::String(value) if value == "none" => BedrockToolChoice::None,
            Value::Object(object) => BedrockToolChoice::Tool {
                name: object["name"].as_str().expect("name").to_string(),
            },
            other => panic!("unknown tool choice {other}"),
        }),
        reasoning: raw
            .get("reasoning")
            .and_then(Value::as_str)
            .map(thinking_level),
        thinking_budgets: raw.get("thinkingBudgets").map(|budgets| {
            serde_json::from_value::<ThinkingBudgets>(budgets.clone()).expect("budgets")
        }),
        interleaved_thinking: raw.get("interleavedThinking").and_then(Value::as_bool),
        thinking_display: raw
            .get("thinkingDisplay")
            .and_then(Value::as_str)
            .map(|display| match display {
                "omitted" => BedrockThinkingDisplay::Omitted,
                _ => BedrockThinkingDisplay::Summarized,
            }),
        request_metadata: raw
            .get("requestMetadata")
            .and_then(Value::as_object)
            .map(|metadata| {
                metadata
                    .iter()
                    .filter_map(|(key, value)| {
                        value.as_str().map(|value| (key.clone(), value.to_string()))
                    })
                    .collect::<BTreeMap<String, String>>()
            }),
        max_tokens: raw.get("maxTokens").and_then(Value::as_u64),
        temperature: raw.get("temperature").and_then(Value::as_f64),
        cache_retention: raw.get("cacheRetention").and_then(Value::as_str).map(
            |value| match value {
                "none" => CacheRetention::None,
                "long" => CacheRetention::Long,
                _ => CacheRetention::Short,
            },
        ),
        // The generator pins these so no ambient AWS configuration leaks in.
        env: Some(
            [
                ("AWS_REGION", "us-east-1"),
                ("AWS_ACCESS_KEY_ID", "x"),
                ("AWS_SECRET_ACCESS_KEY", "y"),
            ]
            .into_iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect::<ProviderEnv>(),
        ),
        ..BedrockOptions::default()
    }
}

struct Case {
    name: String,
    model: Model,
    context: Context,
    options: BedrockOptions,
    payload: Value,
    events: Vec<Value>,
}

fn cases() -> Vec<Case> {
    include_str!("fixtures/bedrock-converse-stream.jsonl")
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let raw: Value = serde_json::from_str(line).expect("fixture line");
            let name = raw["name"].as_str().expect("name").to_string();
            Case {
                model: serde_json::from_value(raw["model"].clone())
                    .unwrap_or_else(|error| panic!("{name}: model: {error}")),
                context: serde_json::from_value(raw["context"].clone())
                    .unwrap_or_else(|error| panic!("{name}: context: {error}")),
                options: options_from_fixture(&raw["options"]),
                payload: raw["payload"].clone(),
                events: raw["events"].as_array().cloned().unwrap_or_default(),
                name,
            }
        })
        .collect()
}

const TIMESTAMP: i64 = 1_700_000_000_000;

#[test]
fn every_captured_command_input_is_reproduced_byte_for_byte() {
    let cases = cases();
    assert!(cases.len() >= 45, "expected the full fixture set");
    let mut failures = Vec::new();
    for case in &cases {
        let built = build_command_input(&case.model, &case.context, &case.options, TIMESTAMP)
            .unwrap_or_else(|error| panic!("{}: build_command_input: {error}", case.name));
        if serde_json::to_string(&built).expect("serialize")
            != serde_json::to_string(&case.payload).expect("serialize")
        {
            failures.push(format!(
                "{}\n  expected: {}\n  actual:   {}",
                case.name,
                serde_json::to_string(&case.payload).expect("serialize"),
                serde_json::to_string(&built).expect("serialize"),
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} inputs differ:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
}

/// The stream items the generator fed to the mocked SDK, recovered from the fixture's
/// case name: the generator's scripted items live in the generator, so the sequences are
/// replayed from the recorded events instead — the state machine is driven by the same
/// items here.
fn stream_items(name: &str) -> Option<Vec<Value>> {
    Some(match name {
        "stream-text" => vec![
            json!({ "messageStart": { "role": "assistant" } }),
            json!({ "contentBlockDelta": { "contentBlockIndex": 0, "delta": { "text": "Hel" } } }),
            json!({ "contentBlockDelta": { "contentBlockIndex": 0, "delta": { "text": "lo" } } }),
            json!({ "contentBlockStop": { "contentBlockIndex": 0 } }),
            json!({ "messageStop": { "stopReason": "end_turn" } }),
            json!({ "metadata": { "usage": { "inputTokens": 10, "outputTokens": 4, "cacheReadInputTokens": 2, "cacheWriteInputTokens": 1, "totalTokens": 17 } } }),
        ],
        "stream-thinking" => vec![
            json!({ "messageStart": { "role": "assistant" } }),
            json!({ "contentBlockDelta": { "contentBlockIndex": 0, "delta": { "reasoningContent": { "text": "think" } } } }),
            json!({ "contentBlockDelta": { "contentBlockIndex": 0, "delta": { "reasoningContent": { "signature": "sig-a" } } } }),
            json!({ "contentBlockDelta": { "contentBlockIndex": 0, "delta": { "reasoningContent": { "signature": "sig-b" } } } }),
            json!({ "contentBlockStop": { "contentBlockIndex": 0 } }),
            json!({ "contentBlockDelta": { "contentBlockIndex": 1, "delta": { "text": "answer" } } }),
            json!({ "contentBlockStop": { "contentBlockIndex": 1 } }),
            json!({ "messageStop": { "stopReason": "end_turn" } }),
        ],
        "stream-tool-use" => vec![
            json!({ "messageStart": { "role": "assistant" } }),
            json!({ "contentBlockStart": { "contentBlockIndex": 0, "start": { "toolUse": { "toolUseId": "call_1", "name": "read" } } } }),
            json!({ "contentBlockDelta": { "contentBlockIndex": 0, "delta": { "toolUse": { "input": "{\"path\":" } } } }),
            json!({ "contentBlockDelta": { "contentBlockIndex": 0, "delta": { "toolUse": { "input": "\"a.txt\"}" } } } }),
            json!({ "contentBlockStop": { "contentBlockIndex": 0 } }),
            json!({ "messageStop": { "stopReason": "tool_use" } }),
        ],
        "stream-truncated-tool-arguments" => vec![
            json!({ "messageStart": { "role": "assistant" } }),
            json!({ "contentBlockStart": { "contentBlockIndex": 0, "start": { "toolUse": { "toolUseId": "call_1", "name": "read" } } } }),
            json!({ "contentBlockDelta": { "contentBlockIndex": 0, "delta": { "toolUse": { "input": "{\"path\":\"a" } } } }),
            json!({ "contentBlockStop": { "contentBlockIndex": 0 } }),
            json!({ "messageStop": { "stopReason": "tool_use" } }),
        ],
        "stream-max-tokens" => vec![
            json!({ "messageStart": { "role": "assistant" } }),
            json!({ "messageStop": { "stopReason": "max_tokens" } }),
        ],
        "stream-context-window-exceeded" => vec![
            json!({ "messageStart": { "role": "assistant" } }),
            json!({ "messageStop": { "stopReason": "model_context_window_exceeded" } }),
        ],
        "stream-stop-sequence" => vec![
            json!({ "messageStart": { "role": "assistant" } }),
            json!({ "messageStop": { "stopReason": "stop_sequence" } }),
        ],
        "stream-unknown-stop-reason" => vec![
            json!({ "messageStart": { "role": "assistant" } }),
            json!({ "messageStop": { "stopReason": "guardrail_intervened" } }),
        ],
        "stream-no-stop-reason" => vec![json!({ "messageStart": { "role": "assistant" } })],
        "stream-user-message-start" => vec![json!({ "messageStart": { "role": "user" } })],
        _ => vec![
            json!({ "messageStart": { "role": "assistant" } }),
            json!({ "messageStop": { "stopReason": "end_turn" } }),
        ],
    })
}

fn normalize(event: &AssistantMessageEvent) -> Value {
    let mut value = serde_json::to_value(event).expect("serialize event");
    if let Some(object) = value.as_object_mut() {
        object.remove("partial");
        for key in ["message", "error"] {
            if let Some(Value::Object(message)) = object.get_mut(key) {
                message.remove("timestamp");
                strip_diagnostic_timestamps(message);
            }
        }
    }
    value
}

/// The diagnostic carries a wall-clock stamp on both sides.
fn strip_diagnostic_timestamps(message: &mut serde_json::Map<String, Value>) {
    if let Some(Value::Array(diagnostics)) = message.get_mut("diagnostics") {
        for diagnostic in diagnostics {
            if let Some(object) = diagnostic.as_object_mut() {
                object.remove("timestamp");
            }
        }
    }
}

fn strip_expected(mut expected: Value) -> Value {
    if let Some(object) = expected.as_object_mut() {
        for key in ["message", "error"] {
            if let Some(Value::Object(message)) = object.get_mut(key) {
                message.remove("timestamp");
                message.remove("role");
                strip_diagnostic_timestamps(message);
            }
        }
    }
    expected
}

/// Drives the state machine over the scripted items and mirrors the `stream` wrapper's
/// terminal handling.
fn run(case: &Case) -> Vec<Value> {
    let mut state = BedrockStreamState::new(&case.model, TIMESTAMP);
    let mut collected = Vec::new();
    let mut failure = None;
    for item in stream_items(&case.name).expect("items") {
        match state.process_item(&item) {
            Ok(emitted) => collected.extend(emitted.iter().map(normalize)),
            Err(error) => {
                failure = Some(error.to_string());
                break;
            }
        }
    }
    let outcome = match failure {
        Some(message) => Err(message),
        None => state.finish().map_err(|error| error.to_string()),
    };
    match outcome {
        Ok(reason) => {
            state.output.stop_reason = match reason {
                notagent_ai::types::DoneReason::Length => StopReason::Length,
                notagent_ai::types::DoneReason::ToolUse => StopReason::ToolUse,
                notagent_ai::types::DoneReason::Deferred => StopReason::Deferred,
                notagent_ai::types::DoneReason::Stop => StopReason::Stop,
            };
            collected.push(normalize(&AssistantMessageEvent::Done {
                reason,
                message: state.output.clone(),
            }));
        }
        Err(message) => {
            state.output.stop_reason = StopReason::Error;
            state.output.error_message = Some(format_bedrock_error(None, None, None, &message));
            // The generator's mocked SDK reports this request id on every response.
            if let Some(details) =
                bedrock_failure_diagnostic_details(None, None, None, Some("req-1"))
            {
                notagent_ai::utils::diagnostics::append_assistant_message_diagnostic(
                    &mut state.output.diagnostics,
                    notagent_ai::utils::diagnostics::AssistantMessageDiagnostic {
                        r#type: "bedrock_response_failure".to_string(),
                        timestamp: TIMESTAMP,
                        error: None,
                        details: Some(details),
                    },
                );
            }
            collected.push(normalize(&AssistantMessageEvent::Error {
                reason: notagent_ai::types::ErrorReason::Error,
                error: state.output.clone(),
            }));
        }
    }
    collected
}

#[test]
fn every_captured_event_sequence_is_reproduced() {
    let mut failures = Vec::new();
    for case in cases() {
        if !case.name.starts_with("stream-") && !case.events.is_empty() {
            // Non-stream cases all share the default two-item script.
        }
        let actual = run(&case);
        let expected: Vec<Value> = case.events.iter().cloned().map(strip_expected).collect();
        if actual != expected {
            failures.push(format!(
                "{}\n  expected: {}\n  actual:   {}",
                case.name,
                serde_json::to_string(&expected).expect("serialize"),
                serde_json::to_string(&actual).expect("serialize"),
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} sequences differ:\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

fn case_named(name: &str) -> Case {
    cases()
        .into_iter()
        .find(|case| case.name == name)
        .unwrap_or_else(|| panic!("fixture {name} is missing"))
}

fn payload_of(name: &str) -> Value {
    let case = case_named(name);
    build_command_input(&case.model, &case.context, &case.options, TIMESTAMP).expect("payload")
}

#[test]
fn cache_points_land_on_the_system_prompt_and_the_last_user_message() {
    let payload = payload_of("system-prompt");
    assert_eq!(payload["system"][1]["cachePoint"]["type"], "default");
    let messages = payload["messages"].as_array().expect("messages");
    let last = messages.last().expect("message");
    let content = last["content"].as_array().expect("content");
    assert_eq!(
        content.last().expect("block")["cachePoint"]["type"],
        "default"
    );

    // A long retention adds the 1h ttl.
    let long = payload_of("system-prompt-long-cache");
    assert_eq!(long["system"][1]["cachePoint"]["ttl"], "1h");

    // Retention "none" removes every cache point.
    let none = payload_of("system-prompt-no-cache");
    assert_eq!(none["system"].as_array().expect("system").len(), 1);
    assert_eq!(
        none["messages"][0]["content"]
            .as_array()
            .expect("content")
            .len(),
        1
    );

    // A non-Claude model gets none either.
    let nova = payload_of("system-prompt-non-claude");
    assert_eq!(nova["system"].as_array().expect("system").len(), 1);
}

#[test]
fn only_claude_models_default_to_the_model_cap() {
    assert_eq!(payload_of("minimal")["inferenceConfig"]["maxTokens"], 64000);
    assert_eq!(
        payload_of("non-claude-has-no-default-max-tokens")["inferenceConfig"].get("maxTokens"),
        None
    );
    assert_eq!(
        payload_of("temperature-and-max-tokens")["inferenceConfig"],
        json!({ "maxTokens": 1024, "temperature": 0.4 })
    );
}

#[test]
fn a_thinking_block_without_a_signature_replays_as_text() {
    let with_signature = payload_of("assistant-replay");
    assert_eq!(
        with_signature["messages"][1]["content"][0]["reasoningContent"]["reasoningText"]["signature"],
        "sig"
    );

    let without = payload_of("assistant-thinking-without-signature");
    assert_eq!(without["messages"][1]["content"][0]["text"], "hmm");
    assert_eq!(
        without["messages"][1]["content"][0].get("reasoningContent"),
        None
    );

    // A non-Claude model never gets the signature field.
    let nova = payload_of("assistant-thinking-non-claude");
    let reasoning = &nova["messages"][1]["content"][0]["reasoningContent"]["reasoningText"];
    assert_eq!(reasoning["text"], "hmm");
    assert_eq!(reasoning.get("signature"), None);
}

#[test]
fn consecutive_tool_results_share_one_user_message() {
    let payload = payload_of("consecutive-tool-results-merge");
    let messages = payload["messages"].as_array().expect("messages");
    let last = messages.last().expect("message");
    assert_eq!(last["role"], "user");
    let content = last["content"].as_array().expect("content");
    // Two tool results plus the cache point.
    assert_eq!(content.len(), 3);
    assert_eq!(content[0]["toolResult"]["status"], "success");
    assert_eq!(content[1]["toolResult"]["status"], "error");
}

#[test]
fn an_empty_tool_result_gets_the_placeholder() {
    let payload = payload_of("tool-result-empty");
    let content = payload["messages"]
        .as_array()
        .expect("messages")
        .last()
        .expect("message")["content"][0]["toolResult"]["content"]
        .as_array()
        .expect("content");
    assert_eq!(content[0]["text"], "<empty>");
}

#[test]
fn empty_assistant_messages_are_skipped() {
    for name in ["assistant-empty-skipped", "assistant-only-blank-text"] {
        let payload = payload_of(name);
        let roles: Vec<&str> = payload["messages"]
            .as_array()
            .expect("messages")
            .iter()
            .map(|message| message["role"].as_str().expect("role"))
            .collect();
        assert_eq!(roles, vec!["user", "user"], "{name}");
    }
}

#[test]
fn the_thinking_fields_follow_the_model_generation() {
    // Budget-based Claude.
    let budget = payload_of("reasoning-budget");
    assert_eq!(
        budget["additionalModelRequestFields"]["thinking"],
        json!({ "type": "enabled", "budget_tokens": 8192, "display": "summarized" })
    );
    assert_eq!(
        budget["additionalModelRequestFields"]["anthropic_beta"][0],
        "interleaved-thinking-2025-05-14"
    );
    assert_eq!(
        payload_of("reasoning-budget-custom")["additionalModelRequestFields"]["thinking"]["budget_tokens"],
        777
    );
    // xhigh clamps to the high budget on budget-based models.
    assert_eq!(
        payload_of("reasoning-xhigh-clamped")["additionalModelRequestFields"]["thinking"]["budget_tokens"],
        16384
    );
    assert_eq!(
        payload_of("reasoning-no-interleaved")["additionalModelRequestFields"]
            .get("anthropic_beta"),
        None
    );

    // Adaptive Claude uses an effort instead of a budget and never asks for the beta.
    let adaptive = payload_of("reasoning-adaptive");
    assert_eq!(
        adaptive["additionalModelRequestFields"]["thinking"],
        json!({ "type": "adaptive", "display": "summarized" })
    );
    assert_eq!(
        adaptive["additionalModelRequestFields"]["output_config"]["effort"],
        "high"
    );
    assert_eq!(
        adaptive["additionalModelRequestFields"].get("anthropic_beta"),
        None
    );
    // Only the newer models take xhigh natively.
    assert_eq!(
        payload_of("reasoning-adaptive-xhigh")["additionalModelRequestFields"]["output_config"]["effort"],
        "xhigh"
    );
    assert_eq!(
        payload_of("reasoning-adaptive-omitted")["additionalModelRequestFields"]["thinking"]["display"],
        "omitted"
    );

    // GovCloud rejects the display field.
    assert_eq!(
        payload_of("reasoning-govcloud")["additionalModelRequestFields"]["thinking"].get("display"),
        None
    );
    // Non-Claude and non-reasoning models get no fields at all.
    assert_eq!(
        payload_of("reasoning-non-claude").get("additionalModelRequestFields"),
        None
    );
    assert_eq!(
        payload_of("reasoning-on-non-reasoning-model").get("additionalModelRequestFields"),
        None
    );
}

#[test]
fn the_tool_choice_maps_onto_the_bedrock_shape() {
    assert_eq!(
        payload_of("tool-choice-auto")["toolConfig"]["toolChoice"],
        json!({ "auto": {} })
    );
    assert_eq!(
        payload_of("tool-choice-any")["toolConfig"]["toolChoice"],
        json!({ "any": {} })
    );
    assert_eq!(
        payload_of("tool-choice-tool")["toolConfig"]["toolChoice"],
        json!({ "tool": { "name": "read" } })
    );
    // "none" drops the whole tool config.
    assert_eq!(payload_of("tool-choice-none").get("toolConfig"), None);
    // Without a choice there is no toolChoice key.
    assert_eq!(payload_of("tools")["toolConfig"].get("toolChoice"), None);
    // Strict tools carry the flag and the tightened schema.
    let strict = payload_of("tools-strict");
    assert_eq!(strict["toolConfig"]["tools"][0]["toolSpec"]["strict"], true);
    assert_eq!(
        strict["toolConfig"]["tools"][0]["toolSpec"]["inputSchema"]["json"]["additionalProperties"],
        false
    );
}

#[test]
fn the_model_predicates_match_ids_and_names() {
    assert!(is_anthropic_claude_model("anthropic.claude-sonnet-4-5", ""));
    assert!(is_anthropic_claude_model(
        "arn:aws:bedrock:us-east-1:1:x",
        "Claude Sonnet"
    ));
    assert!(!is_anthropic_claude_model(
        "amazon.nova-pro-v1:0",
        "Nova Pro"
    ));

    assert!(supports_adaptive_thinking(
        "anthropic.claude-opus-4-6-v1:0",
        ""
    ));
    assert!(supports_adaptive_thinking("arn:x", "Claude Sonnet 4.6"));
    assert!(!supports_adaptive_thinking(
        "anthropic.claude-sonnet-4-5",
        ""
    ));
    assert!(supports_native_xhigh_effort(
        "anthropic.claude-opus-4-8-v1:0",
        ""
    ));
    assert!(!supports_native_xhigh_effort(
        "anthropic.claude-opus-4-6-v1:0",
        ""
    ));

    assert!(is_gov_cloud_bedrock_target("us-gov.anthropic.claude", None));
    assert!(is_gov_cloud_bedrock_target(
        "arn:aws-us-gov:bedrock:x",
        None
    ));
    assert!(is_gov_cloud_bedrock_target(
        "anthropic.claude",
        Some("us-gov-west-1")
    ));
    assert!(!is_gov_cloud_bedrock_target(
        "anthropic.claude",
        Some("us-east-1")
    ));
}

#[test]
fn prompt_caching_needs_a_claude_reference_or_the_force_flag() {
    let model = |id: &str, name: &str| -> Model {
        serde_json::from_value(json!({
            "id": id, "name": name, "api": "bedrock-converse-stream", "provider": "amazon-bedrock",
            "baseUrl": "", "reasoning": true, "input": ["text"],
            "cost": { "input": 1, "output": 1, "cacheRead": 0, "cacheWrite": 0 },
            "contextWindow": 1000, "maxTokens": 100,
        }))
        .expect("model")
    };
    assert!(supports_prompt_caching(
        &model("anthropic.claude-sonnet-4-5-v1:0", ""),
        None
    ));
    assert!(supports_prompt_caching(
        &model("x", "Claude 3.7 Sonnet"),
        None
    ));
    assert!(supports_prompt_caching(
        &model("x", "Claude 3.5 Haiku"),
        None
    ));
    assert!(supports_prompt_caching(&model("x", "Claude Opus 5"), None));
    assert!(!supports_prompt_caching(&model("x", "Claude 3 Opus"), None));
    assert!(!supports_prompt_caching(
        &model("amazon.nova-pro", "Nova"),
        None
    ));
    // The escape hatch for application inference profiles.
    let forced: ProviderEnv = [("AWS_BEDROCK_FORCE_CACHE".to_string(), "1".to_string())]
        .into_iter()
        .collect();
    assert!(supports_prompt_caching(
        &model(
            "arn:aws:bedrock:us-east-1:1:application-inference-profile/x",
            ""
        ),
        Some(&forced)
    ));
}

#[test]
fn the_endpoint_is_only_pinned_when_nothing_else_configures_the_region() {
    assert_eq!(
        standard_endpoint_region("https://bedrock-runtime.us-west-2.amazonaws.com").as_deref(),
        Some("us-west-2")
    );
    assert_eq!(
        standard_endpoint_region("https://bedrock-runtime-fips.us-east-1.amazonaws.com").as_deref(),
        Some("us-east-1")
    );
    assert_eq!(
        standard_endpoint_region("https://bedrock-runtime.cn-north-1.amazonaws.com.cn").as_deref(),
        Some("cn-north-1")
    );
    // A custom endpoint has no region, so it is always used explicitly.
    assert_eq!(
        standard_endpoint_region("https://vpc.internal/bedrock"),
        None
    );
    assert!(should_use_explicit_endpoint(
        "https://vpc.internal/bedrock",
        Some("us-east-1"),
        true
    ));
    // A standard endpoint yields to a configured region or an ambient profile.
    assert!(!should_use_explicit_endpoint(
        "https://bedrock-runtime.us-east-1.amazonaws.com",
        Some("us-west-2"),
        false
    ));
    assert!(!should_use_explicit_endpoint(
        "https://bedrock-runtime.us-east-1.amazonaws.com",
        None,
        true
    ));
    assert!(should_use_explicit_endpoint(
        "https://bedrock-runtime.us-east-1.amazonaws.com",
        None,
        false
    ));
}

#[test]
fn the_client_config_resolves_region_and_credentials() {
    let model: Model = serde_json::from_value(json!({
        "id": "anthropic.claude-sonnet-4-5-v1:0", "name": "Claude", "api": "bedrock-converse-stream",
        "provider": "amazon-bedrock", "baseUrl": "https://bedrock-runtime.us-east-1.amazonaws.com",
        "reasoning": true, "input": ["text"],
        "cost": { "input": 1, "output": 1, "cacheRead": 0, "cacheWrite": 0 },
        "contextWindow": 1000, "maxTokens": 100,
    }))
    .expect("model");

    let environment: ProviderEnv = [
        ("AWS_REGION", "eu-central-1"),
        ("AWS_ACCESS_KEY_ID", "AKIA"),
        ("AWS_SECRET_ACCESS_KEY", "secret"),
        ("AWS_SESSION_TOKEN", "token"),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_string(), value.to_string()))
    .collect();
    let config = build_client_config(
        &model,
        &BedrockOptions {
            env: Some(environment.clone()),
            ..BedrockOptions::default()
        },
    );
    assert_eq!(config.region.as_deref(), Some("eu-central-1"));
    let credentials = config.credentials.expect("credentials");
    assert_eq!(credentials.access_key_id, "AKIA");
    assert_eq!(credentials.session_token.as_deref(), Some("token"));
    // A configured region means the standard endpoint is not pinned.
    assert_eq!(config.endpoint, None);

    // An ARN-embedded region wins over everything else.
    let mut arn_model = model.clone();
    arn_model.id = "arn:aws:bedrock:ap-southeast-2:1:inference-profile/x".to_string();
    let config = build_client_config(
        &arn_model,
        &BedrockOptions {
            region: Some("us-east-1".to_string()),
            env: Some(environment.clone()),
            ..BedrockOptions::default()
        },
    );
    assert_eq!(config.region.as_deref(), Some("ap-southeast-2"));

    // A configured profile suppresses the ambient access keys.
    let config = build_client_config(
        &model,
        &BedrockOptions {
            profile: Some("work".to_string()),
            env: Some(environment.clone()),
            ..BedrockOptions::default()
        },
    );
    assert_eq!(config.profile.as_deref(), Some("work"));
    assert_eq!(config.credentials, None);

    // Skip-auth swaps in the dummy credentials.
    let mut skip = environment.clone();
    skip.insert("AWS_BEDROCK_SKIP_AUTH".to_string(), "1".to_string());
    let config = build_client_config(
        &model,
        &BedrockOptions {
            env: Some(skip),
            ..BedrockOptions::default()
        },
    );
    assert_eq!(
        config.credentials.expect("credentials").access_key_id,
        "dummy-access-key"
    );

    // A bearer token is carried through, and skip-auth suppresses it.
    let config = build_client_config(
        &model,
        &BedrockOptions {
            bearer_token: Some("bedrock-token".to_string()),
            env: Some(environment.clone()),
            ..BedrockOptions::default()
        },
    );
    assert_eq!(config.bearer_token.as_deref(), Some("bedrock-token"));

    assert!(configured_credentials(Some(&environment)).is_some());
    assert!(configured_credentials(None).is_none());
}

#[test]
fn reserved_headers_are_never_overwritten() {
    assert!(is_reserved_header("Authorization"));
    assert!(is_reserved_header("host"));
    assert!(is_reserved_header("X-Amz-Date"));
    assert!(!is_reserved_header("x-custom"));
}

#[test]
fn stop_reasons_map_onto_the_shared_vocabulary() {
    assert_eq!(map_stop_reason(Some("end_turn")).0, StopReason::Stop);
    assert_eq!(map_stop_reason(Some("stop_sequence")).0, StopReason::Stop);
    assert_eq!(map_stop_reason(Some("max_tokens")).0, StopReason::Length);
    assert_eq!(
        map_stop_reason(Some("model_context_window_exceeded")).0,
        StopReason::Length
    );
    assert_eq!(map_stop_reason(Some("tool_use")).0, StopReason::ToolUse);
    let (reason, message) = map_stop_reason(Some("guardrail_intervened"));
    assert_eq!(reason, StopReason::Error);
    assert_eq!(
        message.as_deref(),
        Some("Provider stopped with: guardrail_intervened")
    );
    // No reason at all is an error without a message.
    assert_eq!(map_stop_reason(None), (StopReason::Error, None));
}

#[test]
fn errors_get_a_human_readable_prefix_and_the_retention_hint() {
    assert_eq!(
        format_bedrock_error(Some("ThrottlingException"), None, None, "slow down"),
        "Throttling error: slow down"
    );
    assert_eq!(
        format_bedrock_error(Some("SomethingElseException"), None, None, "boom"),
        "SomethingElseException: boom"
    );
    // The body is surfaced with its status when the message does not carry it.
    assert_eq!(
        format_bedrock_error(None, Some(403), Some("Forbidden"), "UnknownError"),
        "403: Forbidden"
    );
    // The data-retention hint is appended.
    let message = format_bedrock_error(
        None,
        None,
        None,
        "data retention mode 'default' is not available for this model",
    );
    assert!(
        message.ends_with("for supported data retention modes."),
        "{message}"
    );
}

#[test]
fn the_failure_diagnostic_omits_what_it_cannot_know() {
    let details = bedrock_failure_diagnostic_details(
        Some(429),
        Some("ThrottlingException"),
        Some("req-1"),
        None,
    )
    .expect("details");
    assert_eq!(details["status"], 429);
    assert_eq!(details["errorCode"], "ThrottlingException");
    assert_eq!(details["requestId"], "req-1");

    // A transport error name is not a modeled error code.
    let details =
        bedrock_failure_diagnostic_details(None, Some("TimeoutError"), None, Some("fallback"))
            .expect("details");
    assert_eq!(details.get("errorCode"), None);
    assert_eq!(details["requestId"], "fallback");

    // Nothing known at all means no diagnostic.
    assert_eq!(
        bedrock_failure_diagnostic_details(None, None, None, None),
        None
    );

    // An over-long value is dropped rather than truncated.
    assert_eq!(normalize_diagnostic_value(Some(&"x".repeat(201))), None);
    assert_eq!(normalize_diagnostic_value(Some("  ")), None);
    assert_eq!(
        normalize_diagnostic_value(Some(" ok ")).as_deref(),
        Some("ok")
    );
}

#[test]
fn a_user_message_start_is_rejected() {
    let case = case_named("stream-user-message-start");
    let mut state = BedrockStreamState::new(&case.model, TIMESTAMP);
    let error = state
        .process_item(&json!({ "messageStart": { "role": "user" } }))
        .expect_err("rejected");
    assert_eq!(
        error.to_string(),
        "Unexpected assistant message start but got user message start instead"
    );
}

#[test]
fn a_mid_stream_exception_ends_the_stream() {
    let case = case_named("minimal");
    for key in [
        "internalServerException",
        "modelStreamErrorException",
        "validationException",
        "throttlingException",
        "serviceUnavailableException",
    ] {
        let mut state = BedrockStreamState::new(&case.model, TIMESTAMP);
        let error = state
            .process_item(&json!({ key: { "message": "boom" } }))
            .expect_err("rejected");
        assert_eq!(error.to_string(), "boom", "{key}");
    }
}

// ---------------------------------------------------------------------------
// SDK mapping
// ---------------------------------------------------------------------------

/// The command input is mapped onto the SDK's typed request; this walks a payload that
/// exercises every block kind and checks the round trip did not lose anything.
#[tokio::test(flavor = "multi_thread")]
async fn the_command_input_maps_onto_the_sdk_request() {
    use notagent_ai::api::bedrock_converse_stream::to_converse_stream_request;

    let config = aws_config::SdkConfig::builder()
        .region(aws_sdk_bedrockruntime::config::Region::new("us-east-1"))
        .behavior_version(aws_sdk_bedrockruntime::config::BehaviorVersion::latest())
        .build();
    let client = aws_sdk_bedrockruntime::Client::new(&config);

    let input = json!({
        "modelId": "anthropic.claude-sonnet-4-5-v1:0",
        "messages": [
            { "role": "user", "content": [
                { "text": "hi" },
                { "image": { "source": { "bytes": { "__bytes__": "AAAA" } }, "format": "png" } },
                { "cachePoint": { "type": "default", "ttl": "1h" } },
            ] },
            { "role": "assistant", "content": [
                { "reasoningContent": { "reasoningText": { "text": "hmm", "signature": "sig" } } },
                { "toolUse": { "toolUseId": "call_1", "name": "read", "input": { "path": "a.txt", "n": 3 } } },
            ] },
            { "role": "user", "content": [
                { "toolResult": { "toolUseId": "call_1", "content": [{ "text": "body" }], "status": "error" } },
            ] },
        ],
        "system": [{ "text": "be nice" }, { "cachePoint": { "type": "default" } }],
        "inferenceConfig": { "maxTokens": 1024, "temperature": 0.5 },
        "toolConfig": {
            "tools": [{ "toolSpec": {
                "name": "read", "description": "Reads",
                "inputSchema": { "json": { "type": "object" } },
            } }],
            "toolChoice": { "tool": { "name": "read" } },
        },
        "additionalModelRequestFields": { "thinking": { "type": "enabled", "budget_tokens": 8192 } },
        "requestMetadata": { "team": "core" },
    });

    let request = to_converse_stream_request(&client, &input).expect("request");
    let built = request.as_input().clone().build().expect("input");
    assert_eq!(built.model_id(), Some("anthropic.claude-sonnet-4-5-v1:0"));
    let messages = built.messages();
    assert_eq!(messages.len(), 3);
    assert_eq!(
        messages[0].role(),
        &aws_sdk_bedrockruntime::types::ConversationRole::User
    );
    assert_eq!(messages[0].content().len(), 3);
    assert_eq!(
        messages[1].role(),
        &aws_sdk_bedrockruntime::types::ConversationRole::Assistant
    );
    assert_eq!(built.system().len(), 2);
    assert_eq!(
        built.inference_config().expect("config").max_tokens(),
        Some(1024)
    );
    assert_eq!(built.tool_config().expect("tools").tools().len(), 1);
    assert!(built.additional_model_request_fields().is_some());
    assert_eq!(
        built.request_metadata().expect("metadata").get("team"),
        Some(&"core".to_string())
    );
}

#[test]
fn an_unknown_content_block_is_rejected_rather_than_dropped() {
    use notagent_ai::api::bedrock_converse_stream::to_converse_stream_request;

    let config = aws_config::SdkConfig::builder()
        .region(aws_sdk_bedrockruntime::config::Region::new("us-east-1"))
        .behavior_version(aws_sdk_bedrockruntime::config::BehaviorVersion::latest())
        .build();
    let client = aws_sdk_bedrockruntime::Client::new(&config);
    let error = to_converse_stream_request(
        &client,
        &json!({
            "modelId": "m",
            "messages": [{ "role": "user", "content": [{ "video": {} }] }],
        }),
    )
    .expect_err("rejected");
    assert!(
        error.to_string().starts_with("Unsupported content block"),
        "{error}"
    );

    // A missing model id is an error too.
    let error = to_converse_stream_request(&client, &json!({})).expect_err("rejected");
    assert_eq!(error.to_string(), "Command input without modelId");
}
