//! Differential tests for the `packages/ai` modules that carry no dedicated TS
//! suite of their own.
//!
//! `packages/ai/test/` has no file for `utils/{hash,headers,sanitize-unicode,
//! provider-env,deferred-tools,diagnostics}.ts`, `api/github-copilot-headers.ts`,
//! `auth/context.ts` or `image-models.ts` — they are only touched indirectly by
//! the provider and stream suites. There is therefore nothing to *port*; instead
//! every expectation below is generated from the TypeScript original by
//! `tests/fixtures/generators/ai-utils.mts` and read from
//! `tests/fixtures/ai-utils.jsonl`, so the oracle stays the TS code, not a
//! hand-written guess.

use std::collections::BTreeMap;

use notagent_ai::api::github_copilot_headers::{
    build_copilot_dynamic_headers, has_copilot_vision_input, infer_copilot_initiator,
};
use notagent_ai::auth::context::default_provider_auth_context;
use notagent_ai::images::{built_in_image_models, image_models};
use notagent_ai::types::{
    AssistantContent, AssistantMessage, Context, ImageContent, Message, ProviderEnv,
    ProviderHeaders, StopReason, TextContent, TextOrImageContent, Tool, ToolCall,
    ToolResultMessage, Usage, UserContent, UserMessage,
};
use notagent_ai::utils::deferred_tools::split_deferred_tools;
use notagent_ai::utils::diagnostics::{
    append_assistant_message_diagnostic, create_assistant_message_diagnostic,
    extract_diagnostic_error, format_thrown_value, thrown_value_diagnostic,
};
use notagent_ai::utils::hash::short_hash;
use notagent_ai::utils::headers::{headers_to_record, provider_headers_to_record};
use notagent_ai::utils::provider_env::get_provider_env_value;
use notagent_ai::utils::sanitize_unicode::{sanitize_surrogates, sanitize_surrogates_utf16};
use serde_json::{Value, json};

const FIXTURE: &str = include_str!("fixtures/ai-utils.jsonl");

fn oracle() -> Vec<Value> {
    FIXTURE
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("fixture line is JSON"))
        .collect()
}

fn of_kind(kind: &str) -> Vec<Value> {
    let cases: Vec<Value> = oracle()
        .into_iter()
        .filter(|case| case["kind"] == kind)
        .collect();
    assert!(!cases.is_empty(), "fixture has no case of kind {kind}");
    cases
}

fn str_at(case: &Value, key: &str) -> String {
    case[key].as_str().expect("string field").to_owned()
}

fn units_at(case: &Value, key: &str) -> Vec<u16> {
    case[key]
        .as_array()
        .expect("array field")
        .iter()
        .map(|unit| u16::try_from(unit.as_u64().expect("code unit")).expect("fits in u16"))
        .collect()
}

/// The oracle ran with these variables in `process.env`. Both tests that depend
/// on them go through this `Once`, so every write happens before any read — two
/// test threads calling `set_var` concurrently would be undefined behaviour.
fn setup_oracle_env() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        std::env::set_var("NOTAGENT_ORACLE_SET", "from-process");
        std::env::set_var("NOTAGENT_ORACLE_EMPTY", "");
        std::env::set_var("NOTAGENT_ORACLE_BLANK", "   ");
        std::env::set_var("NOTAGENT_ORACLE_PADDED", "  value  ");
        std::env::remove_var("NOTAGENT_ORACLE_MISSING");
    });
}

fn names_at(case: &Value, key: &str) -> Vec<String> {
    case[key]
        .as_array()
        .expect("array field")
        .iter()
        .map(|name| name.as_str().expect("string").to_owned())
        .collect()
}

// --- utils/hash.ts ---------------------------------------------------------

#[test]
fn short_hash_matches_the_typescript_original() {
    for case in of_kind("shortHash") {
        let input = str_at(&case, "input");
        assert_eq!(
            short_hash(&input),
            str_at(&case, "output"),
            "shortHash({input:?})"
        );
    }
    for case in of_kind("shortHashUnits") {
        let input = String::from_utf16(&units_at(&case, "input")).expect("valid UTF-16");
        assert_eq!(
            short_hash(&input),
            str_at(&case, "output"),
            "shortHash over UTF-16 units"
        );
    }
}

// --- utils/sanitize-unicode.ts ---------------------------------------------

#[test]
fn sanitize_surrogates_matches_the_typescript_original() {
    for case in of_kind("sanitizeSurrogates") {
        let input = units_at(&case, "input");
        let expected = units_at(&case, "output");
        let sanitized = sanitize_surrogates_utf16(&input);
        assert_eq!(
            sanitized.encode_utf16().collect::<Vec<u16>>(),
            expected,
            "sanitizeSurrogates over {input:?}"
        );
        // A Rust `&str` can never hold a lone surrogate, so the `&str` overload is
        // the identity — which is exactly what the UTF-16 form yields for input
        // that was already free of unpaired surrogates.
        if input == expected {
            let text = String::from_utf16(&input).expect("valid UTF-16");
            assert_eq!(sanitize_surrogates(&text), text);
        }
    }
}

// --- utils/headers.ts ------------------------------------------------------

#[test]
fn headers_to_record_matches_the_typescript_original() {
    for case in of_kind("headersToRecord") {
        let input: Vec<(String, String)> = case["input"]
            .as_array()
            .expect("array")
            .iter()
            .map(|pair| {
                let pair = pair.as_array().expect("pair");
                (
                    pair[0].as_str().expect("name").to_owned(),
                    pair[1].as_str().expect("value").to_owned(),
                )
            })
            .collect();
        let expected: BTreeMap<String, String> =
            serde_json::from_value(case["output"].clone()).expect("record");
        assert_eq!(headers_to_record(&input), expected);
    }
}

#[test]
fn provider_headers_to_record_matches_the_typescript_original() {
    for case in of_kind("providerHeadersToRecord") {
        let input: Option<ProviderHeaders> = match &case["input"] {
            Value::Null => None,
            value => Some(serde_json::from_value(value.clone()).expect("provider headers")),
        };
        let expected: Option<BTreeMap<String, String>> = match &case["output"] {
            Value::Null => None,
            value => Some(serde_json::from_value(value.clone()).expect("record")),
        };
        assert_eq!(
            provider_headers_to_record(input.as_ref()),
            expected,
            "providerHeadersToRecord({:?})",
            case["input"]
        );
    }
}

// --- utils/provider-env.ts -------------------------------------------------

#[test]
fn get_provider_env_value_matches_the_typescript_original() {
    setup_oracle_env();
    for case in of_kind("getProviderEnvValue") {
        let name = str_at(&case, "name");
        let env: Option<ProviderEnv> = match &case["env"] {
            Value::Null => None,
            value => Some(serde_json::from_value(value.clone()).expect("env map")),
        };
        let expected = case["output"].as_str().map(str::to_owned);
        assert_eq!(
            get_provider_env_value(&name, env.as_ref()),
            expected,
            "getProviderEnvValue({name:?}, {:?})",
            case["env"]
        );
    }
}

// --- utils/deferred-tools.ts -----------------------------------------------

fn tool(name: &str) -> Tool {
    Tool {
        name: name.to_owned(),
        description: name.to_owned(),
        parameters: json!({ "type": "object" }),
        constrained_sampling: None,
    }
}

fn assistant_tool_call(name: &str) -> Message {
    Message::Assistant(AssistantMessage {
        content: vec![AssistantContent::ToolCall(ToolCall {
            id: format!("call-{name}"),
            name: name.to_owned(),
            arguments: serde_json::Map::new(),
            ..ToolCall::default()
        })],
        api: "anthropic-messages".to_owned(),
        provider: "anthropic".to_owned(),
        model: "model".to_owned(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: StopReason::ToolUse,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: 2,
    })
}

fn tool_result(added: &[&str]) -> Message {
    Message::ToolResult(ToolResultMessage {
        tool_call_id: "call-1".to_owned(),
        tool_name: "read".to_owned(),
        content: vec![],
        details: None,
        usage: None,
        added_tool_names: Some(added.iter().map(|name| (*name).to_owned()).collect()),
        is_error: false,
        timestamp: 3,
    })
}

#[test]
fn split_deferred_tools_matches_the_typescript_original() {
    for case in of_kind("splitDeferredTools") {
        let label = str_at(&case, "label");
        let tools: Vec<Tool> = names_at(&case, "tools")
            .iter()
            .map(|name| tool(name))
            .collect();
        let messages: Vec<Message> = case["messages"]
            .as_array()
            .expect("messages")
            .iter()
            .map(|message| match message["role"].as_str().expect("role") {
                "assistant" => {
                    assistant_tool_call(message["content"][0]["name"].as_str().expect("tool name"))
                }
                "toolResult" => {
                    let added: Vec<&str> = message["addedToolNames"]
                        .as_array()
                        .map(|names| {
                            names
                                .iter()
                                .map(|name| name.as_str().expect("name"))
                                .collect()
                        })
                        .unwrap_or_default();
                    tool_result(&added)
                }
                other => panic!("unexpected role {other} in {label}"),
            })
            .collect();

        let context = Context {
            system_prompt: None,
            messages,
            tools: Some(tools),
        };
        let split = split_deferred_tools(
            &context,
            case["enabled"].as_bool().expect("enabled"),
            |name| name.to_owned(),
        );

        assert_eq!(
            split
                .immediate
                .iter()
                .map(|tool| tool.name.clone())
                .collect::<Vec<String>>(),
            names_at(&case, "immediate"),
            "immediate tools of {label}"
        );
        assert_eq!(
            split.deferred.keys().cloned().collect::<Vec<String>>(),
            names_at(&case, "deferred"),
            "deferred tools of {label}"
        );
    }
}

// --- utils/diagnostics.ts --------------------------------------------------

#[derive(Debug)]
struct CodedError {
    message: String,
}

impl std::fmt::Display for CodedError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CodedError {}

#[test]
fn diagnostics_match_the_typescript_original() {
    for case in of_kind("diagnostics") {
        let label = str_at(&case, "label");
        let formatted = str_at(&case, "formatted");
        let expected_name = case["error"]["name"].as_str().map(str::to_owned);
        let expected_message = str_at(&case["error"], "message");

        // TS splits on `error instanceof Error`; the Rust port has one function per
        // side of that branch, so each case is checked against the matching one.
        if expected_name.as_deref() == Some("ThrownValue") {
            let info = thrown_value_diagnostic(&formatted);
            assert_eq!(info.name.as_deref(), Some("ThrownValue"), "{label}");
            assert_eq!(info.message, expected_message, "{label}");
            assert_eq!(format_thrown_value(&formatted), formatted, "{label}");
        } else {
            let error = CodedError {
                message: formatted.clone(),
            };
            let info = extract_diagnostic_error(&error);
            assert_eq!(info.message, expected_message, "{label}");
            assert_eq!(format_thrown_value(&error), formatted, "{label}");
            // Deviation class 1, pinned rather than hidden: `dyn Error` exposes
            // neither a `name`/`message` pair nor a `code`, so the TS
            // `message || name` fallback and the string/number `code` passthrough
            // have no counterpart — the port reports a fixed name and no code.
            assert_eq!(info.name.as_deref(), Some("Error"), "{label}");
            assert_eq!(info.code, None, "{label}");
            assert_eq!(info.stack, None, "{label}");
            if !case["error"]["code"].is_null() {
                assert!(
                    matches!(case["error"]["code"], Value::String(_) | Value::Number(_)),
                    "the oracle carries a scalar code the port drops: {label}"
                );
            }
        }
    }
}

#[test]
fn assistant_message_diagnostics_accumulate() {
    let mut diagnostics = None;
    let first = create_assistant_message_diagnostic(
        "provider_transport_failure",
        thrown_value_diagnostic(&"boom"),
        None,
        11,
    );
    append_assistant_message_diagnostic(&mut diagnostics, first);
    let second = create_assistant_message_diagnostic(
        "retry",
        extract_diagnostic_error(&CodedError {
            message: "again".to_owned(),
        }),
        Some(serde_json::Map::from_iter([(
            "attempt".to_owned(),
            json!(2),
        )])),
        12,
    );
    append_assistant_message_diagnostic(&mut diagnostics, second);

    let diagnostics = diagnostics.expect("two diagnostics");
    assert_eq!(diagnostics.len(), 2);
    assert_eq!(diagnostics[0].r#type, "provider_transport_failure");
    assert_eq!(diagnostics[0].timestamp, 11);
    assert_eq!(
        diagnostics[1].details.as_ref().expect("details")["attempt"],
        json!(2)
    );
}

// --- api/github-copilot-headers.ts -----------------------------------------

fn copilot_messages(label: &str) -> Vec<Message> {
    let user_text = |text: &str| {
        Message::User(UserMessage {
            content: UserContent::Text(text.to_owned()),
            timestamp: 1,
        })
    };
    let image = || {
        TextOrImageContent::Image(ImageContent {
            data: String::new(),
            mime_type: "image/png".to_owned(),
        })
    };
    let empty_tool_result = |content: Vec<TextOrImageContent>| {
        Message::ToolResult(ToolResultMessage {
            tool_call_id: "call-1".to_owned(),
            tool_name: "read".to_owned(),
            content,
            details: None,
            usage: None,
            added_tool_names: None,
            is_error: false,
            timestamp: 3,
        })
    };
    match label {
        "empty" => vec![],
        "last is user" => vec![user_text("hi")],
        "last is assistant" => vec![user_text("hi"), assistant_tool_call("noop")],
        "last is toolResult" => vec![empty_tool_result(vec![])],
        "user image" => vec![Message::User(UserMessage {
            content: UserContent::Blocks(vec![image()]),
            timestamp: 1,
        })],
        "toolResult image" => vec![empty_tool_result(vec![image()])],
        "user text only" => vec![Message::User(UserMessage {
            content: UserContent::Blocks(vec![TextOrImageContent::Text(TextContent::new("hi"))]),
            timestamp: 1,
        })],
        "string content is not an image" => vec![user_text("hi")],
        other => panic!("unknown copilot case {other}"),
    }
}

#[test]
fn copilot_dynamic_headers_match_the_typescript_original() {
    for case in of_kind("copilotHeaders") {
        let label = str_at(&case, "label");
        let messages = copilot_messages(&label);

        assert_eq!(
            infer_copilot_initiator(&messages),
            str_at(&case, "initiator"),
            "initiator of {label}"
        );
        let has_images = has_copilot_vision_input(&messages);
        assert_eq!(
            has_images,
            case["hasVisionInput"].as_bool().expect("bool"),
            "vision input of {label}"
        );
        let expected: BTreeMap<String, String> =
            serde_json::from_value(case["headers"].clone()).expect("headers");
        assert_eq!(
            build_copilot_dynamic_headers(&messages, has_images),
            expected,
            "headers of {label}"
        );
    }
}

// --- auth/context.ts -------------------------------------------------------

#[tokio::test]
async fn default_auth_context_matches_the_typescript_original() {
    setup_oracle_env();
    let context = default_provider_auth_context();
    for case in of_kind("authContextEnv") {
        let name = str_at(&case, "name");
        let expected = case["output"].as_str().map(str::to_owned);
        assert_eq!(context.env(&name).await, expected, "env({name:?})");
    }
    for case in of_kind("authContextFileExists") {
        let path = str_at(&case, "path");
        assert_eq!(
            context.file_exists(&path).await,
            case["output"].as_bool().expect("bool"),
            "fileExists({path:?})"
        );
    }
}

// --- image-models.ts -------------------------------------------------------

#[test]
fn image_model_registry_matches_the_typescript_original() {
    let providers = of_kind("imageProviders");
    let expected_providers = names_at(&providers[0], "output");
    let mut actual_providers: Vec<String> = image_models().keys().cloned().collect();
    let mut sorted_expected = expected_providers.clone();
    // `getImageProviders()` follows the key order of the generated module; the Rust
    // catalog is a sorted `serde_json::Map`, so the sets are compared.
    actual_providers.sort();
    sorted_expected.sort();
    assert_eq!(actual_providers, sorted_expected);

    for case in of_kind("imageModels") {
        let provider = str_at(&case, "provider");
        let mut expected = names_at(&case, "ids");
        let mut actual: Vec<String> = built_in_image_models(&provider)
            .into_iter()
            .map(|model| model.id)
            .collect();
        expected.sort();
        actual.sort();
        assert_eq!(actual, expected, "models of {provider}");
    }

    for case in of_kind("imageModel") {
        let provider = str_at(&case, "provider");
        let id = str_at(&case, "id");
        let model = built_in_image_models(&provider)
            .into_iter()
            .find(|model| model.id == id)
            .expect("the model of the oracle exists");
        assert_eq!(model.name, str_at(&case, "name"));
        assert_eq!(
            serde_json::to_value(model.api).expect("api serializes"),
            case["api"]
        );
    }

    for case in of_kind("imageModelMissing") {
        let provider = str_at(&case, "provider");
        let id = str_at(&case, "id");
        assert!(
            case["output"].is_null(),
            "the oracle reports no model for {provider}/{id}"
        );
        assert!(
            built_in_image_models(&provider)
                .into_iter()
                .all(|model| model.id != id)
        );
    }
}
