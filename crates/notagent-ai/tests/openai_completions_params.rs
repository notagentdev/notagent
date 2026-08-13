//! Differential test of the openai-completions request builder.
//!
//! Every case in `fixtures/openai-completions-payloads.jsonl` was captured from the TS
//! implementation through its `onPayload` hook (see `fixtures/generators`). The Rust
//! payload has to match byte for byte, key order included.

use std::collections::BTreeMap;

use notagent_ai::api::constrained_sampling::create_grammar_tool_input_properties;
use notagent_ai::api::openai_completions_compat::get_compat;
use notagent_ai::api::openai_completions_params::{
    OpenAICompletionsOptions, build_params, resolve_cache_retention,
};
use notagent_ai::types::{CacheRetention, Context, Model, ThinkingBudgets, ThinkingLevel};
use serde_json::{Map, Value};

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

fn options_from_fixture(raw: &Value) -> OpenAICompletionsOptions {
    let raw = raw.as_object().cloned().unwrap_or_default();
    OpenAICompletionsOptions {
        tool_choice: raw.get("toolChoice").cloned(),
        reasoning_effort: raw
            .get("reasoningEffort")
            .and_then(Value::as_str)
            .map(thinking_level),
        thinking_budgets: raw.get("thinkingBudgets").map(|value| {
            serde_json::from_value::<ThinkingBudgets>(value.clone()).expect("budgets")
        }),
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
    options: OpenAICompletionsOptions,
    payload: Value,
}

fn cases() -> Vec<Case> {
    include_str!("fixtures/openai-completions-payloads.jsonl")
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

/// The generator's fixed `Date.now()` never reaches the payload: `transformMessages`
/// only stamps synthesized tool results, and no case needs one.
const TIMESTAMP: i64 = 1_700_000_000_000;

fn build(case: &Case) -> Value {
    let compat = get_compat(&case.model);
    let grammar_tool_input_properties: BTreeMap<String, String> =
        create_grammar_tool_input_properties(
            case.context.tools.as_deref(),
            compat.supports_openai_grammar_tools,
        )
        .unwrap_or_else(|error| panic!("{}: grammar properties: {error}", case.name));
    let retention =
        resolve_cache_retention(case.options.cache_retention, case.options.env.as_ref());
    build_params(
        &case.model,
        &case.context,
        &case.options,
        &compat,
        retention,
        &grammar_tool_input_properties,
        TIMESTAMP,
    )
    .unwrap_or_else(|error| panic!("{}: build_params: {error}", case.name))
}

#[test]
fn every_captured_payload_is_reproduced_byte_for_byte() {
    let cases = cases();
    assert!(cases.len() >= 100, "expected the full fixture set");
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

fn case_named(name: &str) -> Case {
    cases()
        .into_iter()
        .find(|case| case.name == name)
        .unwrap_or_else(|| panic!("fixture {name} is missing"))
}

fn payload_of(name: &str) -> Value {
    build(&case_named(name))
}

#[test]
fn openai_gets_a_prompt_cache_key_only_when_a_session_id_is_present() {
    assert_eq!(payload_of("openai").get("prompt_cache_key"), None);
    assert_eq!(
        payload_of("prompt-cache-key")["prompt_cache_key"],
        Value::from("session-123")
    );
    // 64 code points, not 80.
    assert_eq!(
        payload_of("prompt-cache-key-clamped")["prompt_cache_key"]
            .as_str()
            .expect("string")
            .len(),
        64
    );
    assert_eq!(
        payload_of("prompt-cache-key-none").get("prompt_cache_key"),
        None
    );
}

#[test]
fn a_long_retention_asks_for_the_24h_prompt_cache() {
    let payload = payload_of("prompt-cache-retention-long");
    assert_eq!(payload["prompt_cache_retention"], Value::from("24h"));
    // Together does not support long retention, so neither key appears.
    let unsupported = payload_of("prompt-cache-long-unsupported");
    assert_eq!(unsupported.get("prompt_cache_retention"), None);
}

#[test]
fn the_max_tokens_field_follows_the_compat_matrix() {
    assert_eq!(payload_of("max-tokens")["max_completion_tokens"], 512);
    assert_eq!(payload_of("max-tokens-field")["max_tokens"], 512);
}

#[test]
fn a_null_off_level_suppresses_the_field_instead_of_falling_back() {
    assert_eq!(payload_of("openai-off-mapped")["reasoning_effort"], "none");
    assert_eq!(payload_of("openai-off-null").get("reasoning_effort"), None);
    assert_eq!(payload_of("deepseek-off")["thinking"]["type"], "disabled");
    assert_eq!(payload_of("deepseek-off-null").get("thinking"), None);
    assert_eq!(payload_of("string-thinking-off")["thinking"], "none");
    assert_eq!(payload_of("string-thinking-off-null").get("thinking"), None);
    assert_eq!(payload_of("openrouter-off-null").get("reasoning"), None);
}

#[test]
fn zai_drops_a_null_level_while_qwen_falls_back_to_the_requested_one() {
    // zai tests `mapped === undefined`, so an explicit null removes the field...
    assert_eq!(payload_of("zai-effort")["reasoning_effort"], "medium");
    assert_eq!(payload_of("zai-effort-null").get("reasoning_effort"), None);
    // ...while qwen uses `??`, which swallows null and falls back to the level.
    assert_eq!(payload_of("qwen-effort")["reasoning_effort"], "low");
    assert_eq!(payload_of("qwen-effort-null")["reasoning_effort"], "low");
}

#[test]
fn ant_ling_only_sends_an_effort_the_model_maps() {
    assert_eq!(payload_of("ant-ling-effort")["reasoning"]["effort"], "high");
    assert_eq!(payload_of("ant-ling-unmapped").get("reasoning"), None);
}

#[test]
fn chat_template_vars_resolve_against_the_thinking_level_map() {
    let payload = payload_of("chat-template-vars");
    let kwargs = &payload["chat_template_kwargs"];
    assert_eq!(kwargs["enable_thinking"], true);
    assert_eq!(kwargs["effort"], "deep");
    assert_eq!(kwargs["only_on"], "deep");
    assert_eq!(kwargs["literal"], "keep");
    assert_eq!(kwargs["num"], 3);
    assert_eq!(kwargs["flag"], false);
    assert_eq!(kwargs["nothing"], Value::Null);

    // `omitWhenOff` drops its key once no effort is requested.
    let off = payload_of("chat-template-vars-off");
    let off_kwargs = &off["chat_template_kwargs"];
    assert_eq!(off_kwargs["enable_thinking"], false);
    assert_eq!(off_kwargs["effort"], "none");
    assert_eq!(off_kwargs.get("only_on"), None);

    // Nothing resolvable at all means no `chat_template_kwargs` key.
    assert_eq!(
        payload_of("chat-template-vars-empty").get("chat_template_kwargs"),
        None
    );
}

#[test]
fn the_thinking_token_budget_always_leaves_room_for_an_answer() {
    assert_eq!(
        payload_of("thinking-token-budget")["thinking_token_budget"],
        8192
    );
    // 2000 - 1024 reserved answer tokens.
    assert_eq!(
        payload_of("thinking-token-budget-clamped")["thinking_token_budget"],
        976
    );
    // Nothing left over, so the field is dropped entirely.
    assert_eq!(
        payload_of("thinking-token-budget-zero").get("thinking_token_budget"),
        None
    );
    assert_eq!(
        payload_of("thinking-token-budget-custom")["thinking_token_budget"],
        555
    );
    // xhigh clamps to the high budget.
    assert_eq!(
        payload_of("thinking-token-budget-xhigh")["thinking_token_budget"],
        16384
    );
}

#[test]
fn tool_history_forces_an_empty_tools_array() {
    let payload = payload_of("tool-history-without-tools");
    assert_eq!(payload["tools"], Value::Array(vec![]));
}

#[test]
fn cache_control_lands_on_the_system_prompt_the_last_tool_and_the_last_message() {
    let payload = payload_of("cache-control");
    let messages = payload["messages"].as_array().expect("messages");
    assert_eq!(
        messages[0]["content"][0]["cache_control"]["type"],
        "ephemeral"
    );
    let tools = payload["tools"].as_array().expect("tools");
    assert_eq!(
        tools.last().expect("tool")["cache_control"]["type"],
        "ephemeral"
    );
    // The last message that can take one is the tool result, not the assistant turn.
    let last = messages.last().expect("message");
    assert_eq!(last["role"], "tool");
    assert_eq!(last["content"][0]["cache_control"]["type"], "ephemeral");

    // A long retention adds the 1h ttl.
    let long = payload_of("cache-control-long");
    assert_eq!(
        long["messages"][0]["content"][0]["cache_control"]["ttl"],
        "1h"
    );
    // Retention "none" turns the whole mechanism off.
    let none = payload_of("cache-control-none");
    assert_eq!(none["messages"][0]["content"], Value::from("sys"));
}

#[test]
fn pipe_separated_tool_call_ids_collapse_into_one_id() {
    // `transformMessages` only rewrites ids of turns that came from another model.
    assert_eq!(
        payload_of("same-model-tool-call-id-untouched")["messages"][1]["tool_calls"][0]["id"],
        Value::from("call_abc|item_def")
    );

    let payload = payload_of("normalize-pipe-tool-call-id");
    let messages = payload["messages"].as_array().expect("messages");
    assert_eq!(
        messages[1]["tool_calls"][0]["id"],
        Value::from("call_abc_item_def")
    );
    assert_eq!(
        messages[2]["tool_call_id"],
        Value::from("call_abc_item_def")
    );

    // Over 40 characters the id becomes the call id plus an 8-character hash; the call id
    // is only cut when it would push the result past 40.
    let long = payload_of("normalize-long-pipe-tool-call-id");
    let id = long["messages"][1]["tool_calls"][0]["id"]
        .as_str()
        .expect("id");
    assert_eq!(id, "call_abc_1ulsa664");

    // Without a pipe, openai ids are simply truncated to 40 characters.
    let truncated = payload_of("normalize-long-openai-tool-call-id");
    assert_eq!(
        truncated["messages"][1]["tool_calls"][0]["id"]
            .as_str()
            .expect("id")
            .len(),
        40
    );
}

#[test]
fn deferred_kimi_tools_move_into_a_system_message() {
    let payload = payload_of("kimi-deferred-tools");
    let messages = payload["messages"].as_array().expect("messages");
    let system = messages.last().expect("message");
    assert_eq!(system["role"], "system");
    assert_eq!(system["tools"][0]["function"]["name"], "write");
    assert_eq!(system.get("content"), None);
    // The deferred tool is filtered out of the top-level tools.
    let tools = payload["tools"].as_array().expect("tools");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["function"]["name"], "read");
}

#[test]
fn tool_result_images_become_a_separate_user_message() {
    let payload = payload_of("tool-result-images");
    let messages = payload["messages"].as_array().expect("messages");
    let last = messages.last().expect("message");
    assert_eq!(last["role"], "user");
    assert_eq!(
        last["content"][0]["text"],
        "Attached image(s) from tool result:"
    );
    assert_eq!(
        last["content"][1]["image_url"]["url"],
        "data:image/png;base64,BBB"
    );

    // For a text-only model `transformMessages` already replaced the image with its
    // downgrade note, so the tool result carries that text and no user message follows.
    let text_only = payload_of("tool-result-images-text-only-model");
    let messages = text_only["messages"].as_array().expect("messages");
    let last = messages.last().expect("message");
    assert_eq!(last["role"], "tool");
    assert_eq!(
        last["content"],
        "(tool image omitted: model does not support images)"
    );
}

#[test]
fn an_empty_tool_result_gets_the_no_output_placeholder() {
    let payload = payload_of("tool-result-empty");
    let messages = payload["messages"].as_array().expect("messages");
    assert_eq!(
        messages.last().expect("message")["content"],
        "(no tool output)"
    );
}

#[test]
fn thinking_blocks_go_out_under_their_signature_key() {
    let payload = payload_of("assistant-thinking");
    let assistant = &payload["messages"][1];
    assert_eq!(assistant["content"], "answer");
    assert_eq!(assistant["reasoning_content"], "hmm");

    // For opencode-go the assistant turn came from another model, so `transformMessages`
    // already folded the thinking into the text and no signature key survives.
    let opencode = payload_of("assistant-thinking-opencode-go");
    assert_eq!(opencode["messages"][1]["content"], "hmmanswer");
    assert_eq!(opencode["messages"][1].get("reasoning_content"), None);

    // With `requiresThinkingAsText` the thinking is prepended as a plain text part.
    let as_text = payload_of("assistant-thinking-as-text");
    let content = as_text["messages"][1]["content"]
        .as_array()
        .expect("content");
    assert_eq!(content[0]["text"], "hmm");
    assert_eq!(content[1]["text"], "answer");
}

#[test]
fn empty_assistant_messages_are_dropped() {
    let payload = payload_of("assistant-empty-skipped");
    let roles: Vec<&str> = payload["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .map(|message| message["role"].as_str().expect("role"))
        .collect();
    assert_eq!(roles, vec!["user", "user"]);
}

#[test]
fn encrypted_reasoning_details_replay_next_to_the_tool_calls() {
    let payload = payload_of("assistant-reasoning-details");
    let details = &payload["messages"][1]["reasoning_details"];
    assert_eq!(details[0]["type"], "reasoning.encrypted");
    assert_eq!(details[0]["id"], "call_1");
}

#[test]
fn sampling_params_override_the_named_fields() {
    let payload = payload_of("sampling-params");
    assert_eq!(payload["top_p"], 0.5);
    // `model` was set first but the custom key wins.
    assert_eq!(payload["model"], "override");
}

#[test]
fn the_developer_role_needs_both_reasoning_and_compat_support() {
    assert_eq!(
        payload_of("system-prompt")["messages"][0]["role"],
        "developer"
    );
    assert_eq!(
        payload_of("system-prompt-no-developer")["messages"][0]["role"],
        "system"
    );
}

#[test]
fn empty_user_content_blocks_skip_the_message() {
    let payload = payload_of("empty-user-blocks");
    assert_eq!(payload["messages"].as_array().expect("messages").len(), 1);
}

#[test]
fn routing_preferences_are_copied_through_verbatim() {
    assert_eq!(
        payload_of("openrouter-routing")["provider"]["order"][0],
        "anthropic"
    );
    // Deviation class 1: TS copies the config object, so its key order is whatever the
    // model file declared. The typed struct emits the interface order instead — the two
    // agree for every catalog and generated config, which all follow the interface.
    // An empty object is truthy in JS, so the key still appears.
    assert_eq!(
        payload_of("openrouter-routing-empty")["provider"],
        Value::Object(Map::new())
    );
    let gateway = payload_of("vercel-gateway-routing");
    assert_eq!(gateway["providerOptions"]["gateway"]["only"][0], "bedrock");
    assert_eq!(
        gateway["providerOptions"]["gateway"]["order"][1],
        "anthropic"
    );
    assert_eq!(
        payload_of("vercel-gateway-routing-partial")["providerOptions"]["gateway"]
            .as_object()
            .expect("gateway")
            .len(),
        1
    );
    // Neither `only` nor `order` means no providerOptions at all.
    assert_eq!(
        payload_of("vercel-gateway-routing-empty").get("providerOptions"),
        None
    );
}

#[test]
fn strict_mode_is_only_sent_where_the_provider_takes_it() {
    let strict = payload_of("tools-strict");
    assert_eq!(strict["tools"][0]["function"]["strict"], true);
    assert_eq!(
        strict["tools"][0]["function"]["parameters"]["additionalProperties"],
        false
    );
    // Moonshot rejects unknown fields, so `strict` is omitted.
    assert_eq!(
        payload_of("tools-no-strict-mode")["tools"][0]["function"].get("strict"),
        None
    );
}

#[test]
fn grammar_tools_become_custom_tools_and_custom_tool_calls() {
    let payload = payload_of("tools-grammar");
    let tool = &payload["tools"][0];
    assert_eq!(tool["type"], "custom");
    assert_eq!(tool["custom"]["format"]["grammar"]["syntax"], "lark");

    let replay = payload_of("tools-grammar-assistant-call");
    let tool_call = &replay["messages"][1]["tool_calls"][0];
    assert_eq!(tool_call["type"], "custom");
    assert_eq!(tool_call["custom"]["input"], "abc");
}
