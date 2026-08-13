//! Differential test of the Google Generative AI adapter.
//!
//! `fixtures/google-generative-ai.jsonl` records, for each case, the URL and the body the
//! TS implementation puts on the wire through the `@google/genai` SDK plus the event
//! sequence it emits for a scripted SSE body (see `fixtures/generators`).

use std::sync::{Arc, Mutex};

use notagent_ai::api::google_generative_ai::{
    GoogleOptions, GoogleThinkingOptions, build_request_body, build_request_url, google_budget,
    is_gemini3_flash_model, is_gemini3_pro_model, is_gemma4_model, stream, thinking_level,
};
use notagent_ai::api::google_shared::{
    FunctionCallingConfigMode, convert_tools, map_stop_reason, requires_tool_call_id,
    resolve_google_function_calling_mode, retain_thought_signature,
    supports_google_strict_tool_sampling,
};
use notagent_ai::types::{
    AssistantMessageEvent, Context, Model, ProviderHeaders, ProviderRequestOptions, StopReason,
    ThinkingBudgets, ThinkingLevel, Tool,
};
use notagent_ai::utils::fetch::{FetchBody, FetchError, FetchFn, FetchRequest, FetchResponse};
use serde_json::Value;

fn options_from_fixture(raw: &Value) -> GoogleOptions {
    let raw = raw.as_object().cloned().unwrap_or_default();
    GoogleOptions {
        tool_choice: raw
            .get("toolChoice")
            .and_then(Value::as_str)
            .map(str::to_string),
        thinking: raw
            .get("thinking")
            .and_then(Value::as_object)
            .map(|thinking| GoogleThinkingOptions {
                enabled: thinking
                    .get("enabled")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                budget_tokens: thinking.get("budgetTokens").and_then(Value::as_i64),
                level: thinking
                    .get("level")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            }),
        max_tokens: raw.get("maxTokens").and_then(Value::as_u64),
        temperature: raw.get("temperature").and_then(Value::as_f64),
    }
}

fn headers_from_fixture(raw: &Value) -> Option<ProviderHeaders> {
    raw.get("headers")
        .and_then(Value::as_object)
        .map(|headers| {
            headers
                .iter()
                .map(|(key, value)| (key.clone(), value.as_str().map(str::to_string)))
                .collect()
        })
}

struct Case {
    name: String,
    model: Model,
    context: Context,
    options: GoogleOptions,
    request_headers: Option<ProviderHeaders>,
    request_url: Option<String>,
    request_body: Option<Value>,
    body: String,
    events: Vec<Value>,
}

fn cases() -> Vec<Case> {
    include_str!("fixtures/google-generative-ai.jsonl")
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
                request_headers: headers_from_fixture(&raw["options"]),
                request_url: raw["requestUrl"].as_str().map(str::to_string),
                request_body: raw
                    .get("requestBody")
                    .filter(|body| !body.is_null())
                    .cloned(),
                body: raw["body"].as_str().unwrap_or_default().to_string(),
                events: raw["events"].as_array().cloned().unwrap_or_default(),
                name,
            }
        })
        .collect()
}

const TIMESTAMP: i64 = 1_700_000_000_000;

#[test]
fn every_captured_request_body_is_reproduced() {
    // Structural comparison, not byte-for-byte: the `@google/genai` SDK reorders the keys
    // of every part according to its own schema before serializing, so the field order on
    // the wire is the SDK's, not the adapter's.
    let cases = cases();
    assert!(cases.len() >= 35, "expected the full fixture set");
    let mut failures = Vec::new();
    let mut compared = 0;
    for case in &cases {
        let Some(expected) = case.request_body.clone() else {
            continue;
        };
        compared += 1;
        let built = build_request_body(&case.model, &case.context, &case.options, TIMESTAMP)
            .unwrap_or_else(|error| panic!("{}: build_request_body: {error}", case.name));
        if built != expected {
            failures.push(format!(
                "{}\n  expected: {}\n  actual:   {}",
                case.name,
                serde_json::to_string(&expected).expect("serialize"),
                serde_json::to_string(&built).expect("serialize"),
            ));
        }
    }
    assert!(compared >= 25, "expected most cases to reach the wire");
    assert!(
        failures.is_empty(),
        "{} of {compared} bodies differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn every_captured_request_url_is_reproduced() {
    for case in cases() {
        let Some(expected) = case.request_url else {
            continue;
        };
        assert_eq!(build_request_url(&case.model), expected, "{}", case.name);
    }
}

/// A fetch that replies with one canned SSE body and records the request.
struct CannedFetch {
    body: String,
    seen: Arc<Mutex<Option<FetchRequest>>>,
}

impl FetchFn for CannedFetch {
    fn fetch(
        &self,
        request: FetchRequest,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<FetchResponse, FetchError>> + Send>,
    > {
        *self.seen.lock().expect("poisoned") = Some(request);
        let body = self.body.clone();
        Box::pin(async move {
            Ok(FetchResponse {
                status: 200,
                status_text: String::new(),
                headers: vec![("content-type".to_string(), "text/event-stream".to_string())],
                body: FetchBody::Bytes(body.into_bytes()),
            })
        })
    }
}

fn normalize(event: &AssistantMessageEvent) -> Value {
    let mut value = serde_json::to_value(event).expect("serialize event");
    if let Some(object) = value.as_object_mut() {
        object.remove("partial");
        for key in ["message", "error"] {
            if let Some(Value::Object(message)) = object.get_mut(key) {
                message.remove("timestamp");
            }
        }
    }
    // Generated tool-call ids carry a wall-clock stamp and a counter.
    scrub_generated_ids(&mut value);
    value
}

fn strip_expected(mut expected: Value) -> Value {
    if let Some(object) = expected.as_object_mut() {
        for key in ["message", "error"] {
            if let Some(Value::Object(message)) = object.get_mut(key) {
                message.remove("timestamp");
                message.remove("role");
            }
        }
    }
    scrub_generated_ids(&mut expected);
    expected
}

/// Replaces `<name>_<millis>_<counter>` ids with a stable marker.
fn scrub_generated_ids(value: &mut Value) {
    match value {
        Value::Object(object) => {
            if let Some(Value::String(id)) = object.get("id") {
                let parts: Vec<&str> = id.rsplitn(3, '_').collect();
                if parts.len() == 3
                    && parts[0].parse::<u64>().is_ok()
                    && parts[1].parse::<u64>().is_ok()
                {
                    let name = parts[2].to_string();
                    object.insert(
                        "id".to_string(),
                        Value::String(format!("{name}_<generated>")),
                    );
                }
            }
            for (_, entry) in object.iter_mut() {
                scrub_generated_ids(entry);
            }
        }
        Value::Array(array) => {
            for entry in array {
                scrub_generated_ids(entry);
            }
        }
        _ => {}
    }
}

async fn run(case: &Case) -> (Vec<Value>, Option<FetchRequest>) {
    let seen = Arc::new(Mutex::new(None));
    let events = stream(
        case.model.clone(),
        case.context.clone(),
        ProviderRequestOptions {
            api_key: Some("k".to_string()),
            max_retries: Some(0),
            headers: case.request_headers.clone(),
            fetch: Some(Arc::new(CannedFetch {
                body: case.body.clone(),
                seen: seen.clone(),
            })),
            ..ProviderRequestOptions::default()
        },
        case.options.clone(),
    );
    let mut collected = Vec::new();
    while let Some(event) = events.next().await {
        collected.push(normalize(&event));
    }
    let request = seen.lock().expect("poisoned").clone();
    (collected, request)
}

#[tokio::test(flavor = "multi_thread")]
async fn every_captured_event_sequence_is_reproduced() {
    let mut failures = Vec::new();
    for case in cases() {
        let (actual, _) = run(&case).await;
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

fn body_of(name: &str) -> Value {
    let case = case_named(name);
    build_request_body(&case.model, &case.context, &case.options, TIMESTAMP).expect("body")
}

#[tokio::test(flavor = "multi_thread")]
async fn the_request_carries_the_api_key_header() {
    let (_, request) = run(&case_named("minimal")).await;
    let request = request.expect("request");
    assert_eq!(request.method, "POST");
    assert!(
        request
            .headers
            .iter()
            .any(|(key, value)| key == "x-goog-api-key" && value == "k")
    );
    // Model and request headers are merged in.
    let (_, request) = run(&case_named("headers")).await;
    let request = request.expect("request");
    assert!(
        request
            .headers
            .iter()
            .any(|(key, value)| key == "x-model" && value == "from-model")
    );
    assert!(
        request
            .headers
            .iter()
            .any(|(key, value)| key == "x-request" && value == "from-request")
    );
}

#[test]
fn thought_signatures_only_survive_within_the_same_model() {
    let same = body_of("assistant-replay");
    let parts = same["contents"][1]["parts"].as_array().expect("parts");
    assert_eq!(parts[0]["thought"], true);
    assert_eq!(parts[0]["thoughtSignature"], "c2ln");
    assert_eq!(parts[1]["thoughtSignature"], "dGV4");
    assert_eq!(parts[2]["thoughtSignature"], "dG9vbA==");

    // A different model turns thinking into plain text and drops every signature.
    let foreign = body_of("assistant-replay-foreign-model");
    let parts = foreign["contents"][1]["parts"].as_array().expect("parts");
    assert_eq!(parts[0]["text"], "hmm");
    assert_eq!(parts[0].get("thought"), None);
    assert_eq!(parts[0].get("thoughtSignature"), None);
    assert_eq!(parts[1].get("thoughtSignature"), None);

    // Non-base64 signatures are rejected even for the same model.
    let invalid = body_of("assistant-invalid-signature");
    assert_eq!(
        invalid["contents"][1]["parts"][0].get("thoughtSignature"),
        None
    );

    // The helper keeps the last non-empty value.
    assert_eq!(
        retain_thought_signature(Some("a".to_string()), None).as_deref(),
        Some("a")
    );
    assert_eq!(
        retain_thought_signature(Some("a".to_string()), Some("")).as_deref(),
        Some("a")
    );
    assert_eq!(
        retain_thought_signature(Some("a".to_string()), Some("b")).as_deref(),
        Some("b")
    );
}

#[test]
fn an_empty_text_block_survives_only_with_a_signature() {
    let with_signature = body_of("assistant-empty-text-with-signature");
    let parts = with_signature["contents"][1]["parts"]
        .as_array()
        .expect("parts");
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0]["text"], "");
    assert_eq!(parts[0]["thoughtSignature"], "c2ln");

    let without = body_of("assistant-empty-text-without-signature");
    let parts = without["contents"][1]["parts"].as_array().expect("parts");
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0]["text"], "kept");
}

#[test]
fn tool_results_share_one_user_turn() {
    let merged = body_of("tool-results-merged-into-one-turn");
    let contents = merged["contents"].as_array().expect("contents");
    assert_eq!(contents.len(), 3);
    let parts = contents[2]["parts"].as_array().expect("parts");
    assert_eq!(parts.len(), 2);
    assert!(
        parts
            .iter()
            .all(|part| part.get("functionResponse").is_some())
    );
}

#[test]
fn a_failed_tool_result_uses_the_error_key() {
    let body = body_of("tool-result-error");
    let response = &body["contents"][2]["parts"][0]["functionResponse"]["response"];
    assert_eq!(response["error"], "file body");
    assert_eq!(response.get("output"), None);
}

#[test]
fn images_go_into_a_second_turn_below_gemini_3() {
    let below = body_of("tool-result-images");
    let contents = below["contents"].as_array().expect("contents");
    // functionResponse turn, then a separate user turn with the image.
    let last = contents.last().expect("content");
    assert_eq!(last["parts"][0]["text"], "Tool result image:");
    assert_eq!(last["parts"][1]["inlineData"]["mimeType"], "image/png");
    assert_eq!(
        contents[2]["parts"][0]["functionResponse"].get("parts"),
        None
    );

    // Gemini 3 nests them inside the functionResponse instead.
    let gemini3 = body_of("tool-result-images-gemini-3");
    let contents = gemini3["contents"].as_array().expect("contents");
    let function_response = &contents[2]["parts"][0]["functionResponse"];
    assert_eq!(
        function_response["parts"][0]["inlineData"]["mimeType"],
        "image/png"
    );
    assert_eq!(contents.len(), 3);
    // Gemini 3 also demands the ids.
    assert_eq!(function_response["id"], "call_1");
}

#[test]
fn tool_call_ids_are_sanitized_only_where_they_are_required() {
    assert!(requires_tool_call_id("gemini-3-pro"));
    assert!(requires_tool_call_id("claude-sonnet-4"));
    assert!(requires_tool_call_id("gpt-oss-120b"));
    assert!(!requires_tool_call_id("gemini-2.5-flash"));
    assert!(requires_tool_call_id("gemini-live-3-flash"));

    let body = body_of("tool-call-id-normalized");
    assert_eq!(
        body["contents"][1]["parts"][0]["functionCall"]["id"],
        "call_weird_id"
    );
    assert_eq!(
        body["contents"][2]["parts"][0]["functionResponse"]["id"],
        "call_weird_id"
    );
}

#[test]
fn the_thinking_config_follows_the_model_family() {
    assert_eq!(
        body_of("thinking-enabled-budget")["generationConfig"]["thinkingConfig"],
        serde_json::json!({ "includeThoughts": true, "thinkingBudget": 4096 })
    );
    assert_eq!(
        body_of("thinking-enabled-level")["generationConfig"]["thinkingConfig"],
        serde_json::json!({ "includeThoughts": true, "thinkingLevel": "HIGH" })
    );
    assert_eq!(
        body_of("thinking-disabled-25")["generationConfig"]["thinkingConfig"],
        serde_json::json!({ "thinkingBudget": 0 })
    );
    assert_eq!(
        body_of("thinking-disabled-3-pro")["generationConfig"]["thinkingConfig"],
        serde_json::json!({ "thinkingLevel": "LOW" })
    );
    for name in [
        "thinking-disabled-3-flash",
        "thinking-disabled-flash-latest",
        "thinking-disabled-gemma4",
    ] {
        assert_eq!(
            body_of(name)["generationConfig"]["thinkingConfig"],
            serde_json::json!({ "thinkingLevel": "MINIMAL" }),
            "{name}"
        );
    }
    // A model that cannot reason never gets a thinking config.
    assert_eq!(
        body_of("thinking-on-non-reasoning-model")["generationConfig"].get("thinkingConfig"),
        None
    );
}

#[test]
fn the_model_family_predicates_match_the_documented_ids() {
    assert!(is_gemini3_pro_model("gemini-3-pro"));
    assert!(is_gemini3_pro_model("gemini-3.1-pro-preview"));
    assert!(!is_gemini3_pro_model("gemini-2.5-pro"));
    assert!(is_gemini3_flash_model("gemini-3-flash"));
    assert!(is_gemini3_flash_model("gemini-flash-latest"));
    assert!(is_gemini3_flash_model("gemini-flash-lite-latest"));
    assert!(!is_gemini3_flash_model("gemini-2.5-flash"));
    assert!(is_gemma4_model("gemma-4-27b"));
    assert!(is_gemma4_model("gemma4"));
    assert!(supports_google_strict_tool_sampling("gemini-3-pro"));
    assert!(!supports_google_strict_tool_sampling("gemini-2.5-pro"));
}

#[test]
fn the_thinking_level_and_budget_tables_match() {
    assert_eq!(thinking_level(ThinkingLevel::Low, "gemini-3-pro"), "LOW");
    assert_eq!(
        thinking_level(ThinkingLevel::Medium, "gemini-3-pro"),
        "HIGH"
    );
    assert_eq!(thinking_level(ThinkingLevel::Low, "gemma-4-27b"), "MINIMAL");
    assert_eq!(
        thinking_level(ThinkingLevel::Medium, "gemini-2.5-flash"),
        "MEDIUM"
    );

    assert_eq!(
        google_budget("gemini-2.5-pro", ThinkingLevel::High, None),
        32768
    );
    assert_eq!(
        google_budget("gemini-2.5-flash-lite", ThinkingLevel::Minimal, None),
        512
    );
    assert_eq!(
        google_budget("gemini-2.5-flash", ThinkingLevel::Minimal, None),
        128
    );
    // Anything else asks for a dynamic budget.
    assert_eq!(google_budget("gemini-3-pro", ThinkingLevel::High, None), -1);
    // A custom budget wins.
    assert_eq!(
        google_budget(
            "gemini-2.5-pro",
            ThinkingLevel::High,
            Some(ThinkingBudgets {
                high: Some(99),
                ..ThinkingBudgets::default()
            })
        ),
        99
    );
}

#[test]
fn the_function_calling_mode_follows_the_tool_choice_and_strictness() {
    let tool = Tool {
        name: "read".to_string(),
        description: "Reads".to_string(),
        parameters: serde_json::json!({ "type": "object", "properties": {} }),
        constrained_sampling: None,
    };
    let strict = Tool {
        name: "strict".to_string(),
        constrained_sampling: Some(
            serde_json::from_value(serde_json::json!({
                "type": "json_schema", "strict": "require"
            }))
            .expect("config"),
        ),
        ..tool.clone()
    };
    let tools = [tool.clone()];
    assert_eq!(
        resolve_google_function_calling_mode(&tools, Some("none"), true).expect("mode"),
        Some(FunctionCallingConfigMode::None)
    );
    assert_eq!(
        resolve_google_function_calling_mode(&tools, Some("any"), true).expect("mode"),
        Some(FunctionCallingConfigMode::Any)
    );
    assert_eq!(
        resolve_google_function_calling_mode(&tools, Some("auto"), true).expect("mode"),
        Some(FunctionCallingConfigMode::Auto)
    );
    assert_eq!(
        resolve_google_function_calling_mode(&tools, None, true).expect("mode"),
        None
    );
    // A tool that demands strict sampling forces the validated mode.
    assert_eq!(
        resolve_google_function_calling_mode(std::slice::from_ref(&strict), None, true)
            .expect("mode"),
        Some(FunctionCallingConfigMode::Validated)
    );
    // ... unless the tool choice already pins it.
    assert_eq!(
        resolve_google_function_calling_mode(&[strict], Some("none"), true).expect("mode"),
        Some(FunctionCallingConfigMode::None)
    );
}

#[test]
fn tool_conversion_can_fall_back_to_the_openapi_field() {
    let tool = Tool {
        name: "read".to_string(),
        description: "Reads".to_string(),
        parameters: serde_json::json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "$defs": { "x": {} },
            "type": "object",
            "properties": { "path": { "type": "string" } },
        }),
        constrained_sampling: None,
    };
    let json_schema = convert_tools(std::slice::from_ref(&tool), false, false)
        .expect("tools")
        .expect("some");
    let declaration = &json_schema[0]["functionDeclarations"][0];
    // The full JSON Schema goes out unchanged.
    assert!(declaration["parametersJsonSchema"]["$defs"].is_object());

    let openapi = convert_tools(std::slice::from_ref(&tool), true, false)
        .expect("tools")
        .expect("some");
    let declaration = &openapi[0]["functionDeclarations"][0];
    // The OpenAPI form drops the meta declarations.
    assert_eq!(declaration["parameters"].get("$defs"), None);
    assert_eq!(declaration["parameters"].get("$schema"), None);
    assert_eq!(declaration["parameters"]["type"], "object");

    // No tools at all means no field.
    assert_eq!(convert_tools(&[], false, false).expect("tools"), None);
}

#[test]
fn only_stop_and_max_tokens_are_not_errors() {
    assert_eq!(map_stop_reason("STOP"), StopReason::Stop);
    assert_eq!(map_stop_reason("MAX_TOKENS"), StopReason::Length);
    for reason in [
        "SAFETY",
        "RECITATION",
        "BLOCKLIST",
        "PROHIBITED_CONTENT",
        "MALFORMED_FUNCTION_CALL",
        "OTHER",
        "FINISH_REASON_UNSPECIFIED",
    ] {
        assert_eq!(map_stop_reason(reason), StopReason::Error, "{reason}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_duplicate_function_call_id_gets_a_generated_one() {
    let (events, _) = run(&case_named("stream-function-call-duplicate-id")).await;
    let last = events.last().expect("event");
    let content = last["message"]["content"].as_array().expect("content");
    assert_eq!(content.len(), 2);
    assert_eq!(content[0]["id"], "dup");
    // The second one is generated, so the scrubbed marker shows up.
    assert_eq!(content[1]["id"], "read_<generated>");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_missing_finish_reason_is_an_error() {
    let (events, _) = run(&case_named("stream-no-finish-reason")).await;
    let last = events.last().expect("event");
    assert_eq!(last["type"], "error");
    assert_eq!(
        last["error"]["errorMessage"],
        "Google stream ended without a finish reason"
    );

    // A safety stop reports the provider's raw reason.
    let (events, _) = run(&case_named("stream-safety")).await;
    let last = events.last().expect("event");
    assert_eq!(
        last["error"]["errorMessage"],
        "Provider stopped with: SAFETY"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn usage_subtracts_the_cached_tokens_and_adds_the_thoughts() {
    let (events, _) = run(&case_named("stream-text")).await;
    let usage = &events.last().expect("event")["message"]["usage"];
    assert_eq!(usage["input"], 8);
    assert_eq!(usage["output"], 7);
    assert_eq!(usage["cacheRead"], 2);
    assert_eq!(usage["reasoning"], 3);
    assert_eq!(usage["totalTokens"], 17);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_missing_api_key_ends_the_stream_with_an_error() {
    let case = case_named("minimal");
    let events = stream(
        case.model.clone(),
        case.context.clone(),
        ProviderRequestOptions::default(),
        case.options.clone(),
    );
    let mut last = None;
    while let Some(event) = events.next().await {
        last = Some(event);
    }
    let Some(AssistantMessageEvent::Error { error, .. }) = last else {
        panic!("expected an error event");
    };
    assert_eq!(
        error.error_message.as_deref(),
        Some("No API key for provider: google")
    );
    assert_eq!(error.api, "google-generative-ai");
}
