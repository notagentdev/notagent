use std::collections::BTreeMap;

use notagent_ai::api::constrained_sampling::create_grammar_tool_input_properties;
use notagent_ai::api::openai_responses::{
    OpenAIResponsesOptions, build_params, get_compat, service_tier_cost_multiplier,
};
use notagent_ai::types::{CacheRetention, Context, Model, ThinkingLevel};
use serde_json::Value;

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

fn cache_retention(value: &str) -> CacheRetention {
    match value {
        "none" => CacheRetention::None,
        "short" => CacheRetention::Short,
        "long" => CacheRetention::Long,
        other => panic!("unknown cache retention {other}"),
    }
}

fn options_from_fixture(raw: &Value) -> OpenAIResponsesOptions {
    let raw = raw.as_object().cloned().unwrap_or_default();
    OpenAIResponsesOptions {
        reasoning_effort: raw
            .get("reasoningEffort")
            .and_then(Value::as_str)
            .map(thinking_level),
        // A present-but-null `reasoningSummary` is distinct from an absent one.
        reasoning_summary: raw
            .get("reasoningSummary")
            .map(|value| value.as_str().map(str::to_string)),
        service_tier: raw
            .get("serviceTier")
            .and_then(Value::as_str)
            .map(str::to_string),
        tool_choice: raw.get("toolChoice").cloned(),
        max_tokens: raw.get("maxTokens").and_then(Value::as_u64),
        temperature: raw.get("temperature").and_then(Value::as_f64),
        sampling_params: raw
            .get("samplingParams")
            .and_then(Value::as_object)
            .cloned(),
        cache_retention: raw
            .get("cacheRetention")
            .and_then(Value::as_str)
            .map(cache_retention),
        session_id: raw
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::to_string),
        env: None,
    }
}

struct Case {
    name: String,
    model: Model,
    context: Context,
    options: OpenAIResponsesOptions,
    payload: Value,
}

fn cases() -> Vec<Case> {
    include_str!("fixtures/openai-responses-payloads.jsonl")
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
                name,
            }
        })
        .collect()
}

/// No case needs a synthesized tool result, so the generator's `Date.now()` never
/// reaches a payload.
const TIMESTAMP: i64 = 1_700_000_000_000;

fn build(case: &Case) -> Value {
    let compat = get_compat(&case.model);
    let grammar_tool_input_properties: BTreeMap<String, String> =
        create_grammar_tool_input_properties(
            case.context.tools.as_deref(),
            compat.supports_openai_grammar_tools,
        )
        .unwrap_or_else(|error| panic!("{}: grammar properties: {error}", case.name));
    build_params(
        &case.model,
        &case.context,
        &case.options,
        &compat,
        &grammar_tool_input_properties,
        TIMESTAMP,
    )
    .unwrap_or_else(|error| panic!("{}: build_params: {error}", case.name))
}

#[test]
fn every_captured_payload_is_reproduced_byte_for_byte() {
    let cases = cases();
    assert!(cases.len() >= 50, "expected the full fixture set");
    let mut failures = Vec::new();
    for case in &cases {
        let built = build(case);
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
        "{} of {} payloads differ:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
}

fn payload_of(name: &str) -> Value {
    let case = cases()
        .into_iter()
        .find(|case| case.name == name)
        .unwrap_or_else(|| panic!("fixture {name} is missing"));
    build(&case)
}

#[test]
fn the_developer_role_needs_reasoning_and_compat_support() {
    assert_eq!(payload_of("system-prompt")["input"][0]["role"], "developer");
    assert_eq!(
        payload_of("system-prompt-no-developer")["input"][0]["role"],
        "system"
    );
    assert_eq!(
        payload_of("system-prompt-no-reasoning")["input"][0]["role"],
        "system"
    );
}

#[test]
fn assistant_text_blocks_get_a_message_id_from_their_signature() {
    let payload = payload_of("assistant-text-signature-legacy");
    assert_eq!(payload["input"][1]["id"], "msg_legacy");

    let phase = payload_of("assistant-text-signature-phase");
    assert_eq!(phase["input"][1]["phase"], "final_answer");

    // Without a signature the index-based fallback id is used.
    let missing = payload_of("assistant-text-signature-missing");
    assert_eq!(missing["input"][1]["id"], "msg_pi_1");
    assert_eq!(missing["input"][2]["id"], "msg_pi_1_1");

    // Over 64 characters the id collapses to a hash.
    let long = payload_of("assistant-text-signature-too-long");
    let id = long["input"][1]["id"].as_str().expect("id");
    assert!(id.starts_with("msg_"), "{id}");
    assert!(id.len() <= 64, "{id}");
}

#[test]
fn reasoning_items_are_replayed_verbatim_from_their_signature() {
    let payload = payload_of("assistant-reasoning-replay");
    let item = &payload["input"][1];
    assert_eq!(item["type"], "reasoning");
    assert_eq!(item["id"], "rs_1");
    assert_eq!(item["encrypted_content"], "enc");
    assert_eq!(item["summary"][0]["text"], "thinking");
}

#[test]
fn tool_call_item_ids_are_dropped_when_they_cannot_be_paired() {
    // Same model: the fc_ item id is replayed.
    assert_eq!(
        payload_of("assistant-tool-call")["input"][2]["id"],
        "fc_item1"
    );

    // A different model of the same provider: dropping the id avoids the pairing check.
    let different = payload_of("assistant-different-model");
    assert_eq!(different["input"][2].get("id"), None);

    // A ctc_ id cannot go on a function_call item.
    let non_fc = payload_of("assistant-non-fc-item-id");
    assert_eq!(non_fc["input"][1].get("id"), None);

    // No pipe at all means no item id either.
    let without = payload_of("assistant-tool-call-without-item-id");
    assert_eq!(without["input"][1].get("id"), None);
    assert_eq!(without["input"][1]["call_id"], "call_4");
}

#[test]
fn a_foreign_provider_gets_a_hashed_item_id() {
    let payload = payload_of("assistant-foreign-provider");
    let item = &payload["input"][2];
    let id = item["id"].as_str().expect("id");
    assert!(id.starts_with("fc_"), "{id}");
    assert_ne!(id, "fc_item1");
    // The tool result references the same call id.
    assert_eq!(payload["input"][3]["call_id"], item["call_id"]);
}

#[test]
fn the_namespace_is_only_replayed_for_the_same_model() {
    assert_eq!(
        payload_of("assistant-namespace")["input"][1]["namespace"],
        "files"
    );
}

#[test]
fn tool_results_carry_text_or_image_parts() {
    let images = payload_of("tool-result-images");
    let output = &images["input"][3]["output"];
    assert_eq!(output[0]["type"], "input_text");
    assert_eq!(output[0]["text"], "shot");
    assert_eq!(output[1]["type"], "input_image");
    assert_eq!(output[1]["detail"], "auto");

    // A text-only model gets the downgrade note as a plain string.
    let text_only = payload_of("tool-result-images-text-only");
    assert_eq!(
        text_only["input"][3]["output"],
        "(tool image omitted: model does not support images)"
    );

    let empty = payload_of("tool-result-empty");
    assert_eq!(empty["input"][3]["output"], "(no tool output)");
}

#[test]
fn strict_is_only_sent_when_the_model_supports_it() {
    let plain = payload_of("tools");
    assert_eq!(plain["tools"][0]["type"], "function");
    assert_eq!(plain["tools"][0].get("strict"), None);

    let strict_mode = payload_of("tools-strict-mode");
    assert_eq!(strict_mode["tools"][0]["strict"], false);

    // A tool that requires constrained sampling turns it on.
    let constrained = payload_of("tools-strict-constrained");
    assert_eq!(constrained["tools"][0]["strict"], true);
    assert_eq!(
        constrained["tools"][0]["parameters"]["additionalProperties"],
        false
    );
}

#[test]
fn grammar_tools_become_custom_tools_and_custom_tool_calls() {
    let payload = payload_of("tools-grammar");
    assert_eq!(payload["tools"][0]["type"], "custom");
    assert_eq!(payload["tools"][0]["format"]["syntax"], "lark");

    let replay = payload_of("tools-grammar-replay");
    assert_eq!(replay["input"][1]["type"], "custom_tool_call");
    assert_eq!(replay["input"][1]["input"], "abc");
    assert_eq!(replay["input"][2]["type"], "custom_tool_call_output");
}

#[test]
fn deferred_tools_are_loaded_through_the_configured_mechanism() {
    let additional = payload_of("deferred-additional-tools");
    let item = additional["input"]
        .as_array()
        .expect("input")
        .last()
        .expect("item");
    assert_eq!(item["type"], "additional_tools");
    assert_eq!(item["role"], "developer");
    assert_eq!(item["tools"][0]["name"], "write");
    // The deferred tool is not in the top-level list.
    assert_eq!(additional["tools"].as_array().expect("tools").len(), 1);

    let search = payload_of("deferred-tool-search");
    let input = search["input"].as_array().expect("input");
    let call = &input[input.len() - 2];
    let output = &input[input.len() - 1];
    assert_eq!(call["type"], "tool_search_call");
    assert_eq!(call["arguments"]["query"], "write");
    assert_eq!(call["arguments"]["limit"], 1);
    assert_eq!(output["type"], "tool_search_output");
    assert_eq!(output["call_id"], call["call_id"]);
    assert_eq!(output["tools"][0]["defer_loading"], true);

    // A tool that was already loaded is not searched for again.
    let twice = payload_of("deferred-tool-search-twice");
    let searches = twice["input"]
        .as_array()
        .expect("input")
        .iter()
        .filter(|item| item["type"] == "tool_search_call")
        .count();
    assert_eq!(searches, 1);
}

#[test]
fn reasoning_effort_and_summary_travel_together() {
    let effort = payload_of("reasoning-effort");
    assert_eq!(effort["reasoning"]["effort"], "high");
    assert_eq!(effort["reasoning"]["summary"], "auto");
    assert_eq!(effort["include"][0], "reasoning.encrypted_content");

    assert_eq!(
        payload_of("reasoning-effort-mapped")["reasoning"]["effort"],
        "detailed"
    );

    // A summary alone still turns reasoning on, at medium.
    let summary = payload_of("reasoning-summary-only");
    assert_eq!(summary["reasoning"]["effort"], "medium");
    assert_eq!(summary["reasoning"]["summary"], "detailed");

    // An explicit null summary falls back to auto.
    assert_eq!(
        payload_of("reasoning-summary-null")["reasoning"]["summary"],
        "auto"
    );
}

#[test]
fn without_an_effort_the_off_level_decides() {
    assert_eq!(payload_of("reasoning-off")["reasoning"]["effort"], "none");
    assert_eq!(
        payload_of("reasoning-off-mapped")["reasoning"]["effort"],
        "minimal"
    );
    // An explicit null suppresses the whole field.
    assert_eq!(payload_of("reasoning-off-null").get("reasoning"), None);
    // Copilot never gets the off value.
    assert_eq!(payload_of("reasoning-copilot").get("reasoning"), None);
    // A non-reasoning model gets nothing at all.
    assert_eq!(
        payload_of("reasoning-disabled-model").get("reasoning"),
        None
    );
}

#[test]
fn xai_always_asks_for_encrypted_reasoning() {
    let payload = payload_of("reasoning-xai");
    assert_eq!(payload["include"][0], "reasoning.encrypted_content");
}

#[test]
fn prompt_cache_fields_follow_the_retention() {
    assert_eq!(
        payload_of("prompt-cache-key")["prompt_cache_key"],
        "session-1"
    );
    assert_eq!(
        payload_of("prompt-cache-key-clamped")["prompt_cache_key"]
            .as_str()
            .expect("key")
            .len(),
        64
    );
    assert_eq!(
        payload_of("prompt-cache-none").get("prompt_cache_key"),
        None
    );
    assert_eq!(
        payload_of("prompt-cache-explicit-mode")["prompt_cache_options"]["mode"],
        "explicit"
    );
    assert_eq!(
        payload_of("prompt-cache-long")["prompt_cache_retention"],
        "24h"
    );
    assert_eq!(
        payload_of("prompt-cache-long-unsupported").get("prompt_cache_retention"),
        None
    );
}

#[test]
fn max_output_tokens_never_drops_below_the_api_minimum() {
    assert_eq!(payload_of("max-tokens")["max_output_tokens"], 512);
    assert_eq!(
        payload_of("max-tokens-below-minimum")["max_output_tokens"],
        16
    );
}

#[test]
fn sampling_params_override_the_named_fields() {
    let payload = payload_of("sampling-params");
    assert_eq!(payload["top_p"], 0.4);
    // `store: false` was set first; the custom key wins.
    assert_eq!(payload["store"], true);
}

#[test]
fn the_service_tier_multiplier_matches_the_price_list() {
    assert_eq!(service_tier_cost_multiplier("gpt-5", None), 1.0);
    assert_eq!(service_tier_cost_multiplier("gpt-5", Some("auto")), 1.0);
    assert_eq!(service_tier_cost_multiplier("gpt-5", Some("flex")), 0.5);
    assert_eq!(service_tier_cost_multiplier("gpt-5", Some("priority")), 2.0);
    // gpt-5.5 is the exception.
    assert_eq!(
        service_tier_cost_multiplier("gpt-5.5", Some("priority")),
        2.5
    );
}
