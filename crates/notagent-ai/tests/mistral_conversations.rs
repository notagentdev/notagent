use std::sync::{Arc, Mutex};

use notagent_ai::api::mistral_conversations::{
    MistralOptions, MistralReasoningEffort, MistralToolCallIdNormalizer, MistralToolChoice,
    build_request_headers, build_request_url, derive_mistral_tool_call_id, stream,
    uses_prompt_mode_reasoning, uses_reasoning_effort,
};
use notagent_ai::types::{
    AssistantMessageEvent, CacheRetention, Context, Model, ProviderHeaders, ProviderRequestOptions,
};
use notagent_ai::utils::fetch::{FetchBody, FetchError, FetchFn, FetchRequest, FetchResponse};
use serde_json::Value;

struct Case {
    name: String,
    model: Model,
    context: Context,
    options: MistralOptions,
    status: u16,
    body: String,
    request_url: Option<String>,
    request_headers: Option<Value>,
    request_body: Option<Value>,
    events: Vec<Value>,
}

fn options_from_fixture(raw: &Value) -> MistralOptions {
    let raw = raw.as_object().cloned().unwrap_or_default();
    MistralOptions {
        tool_choice: raw.get("toolChoice").and_then(|choice| match choice {
            Value::String(text) => match text.as_str() {
                "auto" => Some(MistralToolChoice::Auto),
                "none" => Some(MistralToolChoice::None),
                "any" => Some(MistralToolChoice::Any),
                "required" => Some(MistralToolChoice::Required),
                _ => None,
            },
            Value::Object(object) => object
                .get("function")
                .and_then(|function| function.get("name"))
                .and_then(Value::as_str)
                .map(|name| MistralToolChoice::Function(name.to_string())),
            _ => None,
        }),
        prompt_mode_reasoning: raw.get("promptMode").and_then(Value::as_str) == Some("reasoning"),
        reasoning_effort: match raw.get("reasoningEffort").and_then(Value::as_str) {
            Some("none") => Some(MistralReasoningEffort::None),
            Some("high") => Some(MistralReasoningEffort::High),
            _ => None,
        },
        temperature: raw.get("temperature").and_then(Value::as_f64),
        max_tokens: raw.get("maxTokens").and_then(Value::as_u64),
        cache_retention: match raw.get("cacheRetention").and_then(Value::as_str) {
            Some("none") => Some(CacheRetention::None),
            Some("short") => Some(CacheRetention::Short),
            Some("long") => Some(CacheRetention::Long),
            _ => None,
        },
        session_id: raw
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::to_string),
        headers: headers_from_fixture(&Value::Object(raw)),
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

fn cases() -> Vec<Case> {
    include_str!("fixtures/mistral-conversations.jsonl")
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let raw: Value = serde_json::from_str(line).expect("fixture line");
            let name = raw["name"].as_str().expect("name").to_string();
            let request = raw.get("request").filter(|request| !request.is_null());
            Case {
                model: serde_json::from_value(raw["model"].clone())
                    .unwrap_or_else(|error| panic!("{name}: model: {error}")),
                context: serde_json::from_value(raw["context"].clone())
                    .unwrap_or_else(|error| panic!("{name}: context: {error}")),
                options: options_from_fixture(&raw["options"]),
                status: raw["status"].as_u64().unwrap_or(200) as u16,
                body: raw["body"].as_str().unwrap_or_default().to_string(),
                request_url: request
                    .and_then(|request| request["url"].as_str())
                    .map(str::to_string),
                request_headers: request.map(|request| request["headers"].clone()),
                request_body: request.map(|request| request["body"].clone()),
                events: raw["events"].as_array().cloned().unwrap_or_default(),
                name,
            }
        })
        .collect()
}

struct CannedFetch {
    status: u16,
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
        let status = self.status;
        Box::pin(async move {
            Ok(FetchResponse {
                status,
                status_text: String::new(),
                headers: vec![(
                    "content-type".to_string(),
                    if status == 200 {
                        "text/event-stream".to_string()
                    } else {
                        "application/json".to_string()
                    },
                )],
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
    expected
}

async fn run(case: &Case) -> (Vec<Value>, Option<FetchRequest>) {
    let seen = Arc::new(Mutex::new(None));
    let events = stream(
        case.model.clone(),
        case.context.clone(),
        ProviderRequestOptions {
            api_key: Some("k".to_string()),
            max_retries: Some(0),
            fetch: Some(Arc::new(CannedFetch {
                status: case.status,
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

#[tokio::test]
async fn every_captured_request_is_reproduced() {
    let cases = cases();
    assert!(cases.len() >= 40, "expected the full fixture set");

    let mut failures = Vec::new();
    for case in &cases {
        let (_, request) = run(case).await;
        let Some(request) = request else {
            failures.push(format!("{}: no request was sent", case.name));
            continue;
        };

        if let Some(expected_url) = &case.request_url
            && &request.url != expected_url
        {
            failures.push(format!(
                "{}: url\n  expected: {expected_url}\n  actual:   {}",
                case.name, request.url
            ));
        }

        if let Some(expected_headers) = case.request_headers.as_ref().and_then(Value::as_object) {
            let actual: serde_json::Map<String, Value> = request
                .headers
                .iter()
                .map(|(key, value)| (key.clone(), Value::String(value.clone())))
                .collect();
            if &actual != expected_headers {
                failures.push(format!(
                    "{}: headers\n  expected: {}\n  actual:   {}",
                    case.name,
                    serde_json::to_string(expected_headers).expect("serialize"),
                    serde_json::to_string(&actual).expect("serialize"),
                ));
            }
        }

        if let Some(expected_body) = &case.request_body {
            let actual: Value = serde_json::from_slice(request.body.as_deref().unwrap_or(b"null"))
                .expect("request body is JSON");
            if &actual != expected_body {
                failures.push(format!(
                    "{}: body\n  expected: {}\n  actual:   {}",
                    case.name,
                    serde_json::to_string(expected_body).expect("serialize"),
                    serde_json::to_string(&actual).expect("serialize"),
                ));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "{} of {} requests differ:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
}

#[tokio::test]
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
        "{} event sequences differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn derives_stable_tool_call_ids() {
    // A nine-character alphanumeric id passes through unchanged.
    assert_eq!(derive_mistral_tool_call_id("abcdefghi", 0), "abcdefghi");
    // Everything else is hashed down to nine characters.
    let hashed = derive_mistral_tool_call_id("toolu_01ABCDEF", 0);
    assert_eq!(hashed.chars().count(), 9);
    assert!(
        hashed
            .chars()
            .all(|character| character.is_ascii_alphanumeric())
    );

    let mut normalizer = MistralToolCallIdNormalizer::new();
    let first = normalizer.normalize("call_one");
    assert_eq!(normalizer.normalize("call_one"), first, "ids are stable");
    assert_ne!(normalizer.normalize("call_two"), first, "ids stay unique");
}

#[test]
fn selects_the_reasoning_style_per_model() {
    let cases = cases();
    let model = &cases[0].model;
    assert!(uses_reasoning_effort(model), "mistral-medium-3.5");
    assert!(!uses_prompt_mode_reasoning(model));

    let mut other = model.clone();
    other.id = "mistral-large-latest".to_string();
    assert!(!uses_reasoning_effort(&other));
    assert!(uses_prompt_mode_reasoning(&other), "reasoning model");
}

#[test]
fn builds_the_expected_url_and_headers() {
    let cases = cases();
    let mut model = cases[0].model.clone();
    assert_eq!(
        build_request_url(&model),
        "https://api.mistral.ai/v1/chat/completions"
    );

    // A base URL with a trailing slash resolves to the same endpoint.
    model.base_url = "https://api.mistral.ai/".to_string();
    assert_eq!(
        build_request_url(&model),
        "https://api.mistral.ai/v1/chat/completions"
    );

    let headers = build_request_headers(&model, "k", &MistralOptions::default());
    assert_eq!(
        headers,
        vec![
            ("accept".to_string(), "text/event-stream".to_string()),
            ("authorization".to_string(), "Bearer k".to_string()),
            ("content-type".to_string(), "application/json".to_string()),
        ]
    );
}
