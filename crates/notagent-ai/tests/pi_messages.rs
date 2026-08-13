//! Differential test of the pi-messages adapter.
//!
//! `fixtures/pi-messages.jsonl` records, for each case, the URL, the headers and the
//! body the TS implementation puts on the wire plus the event sequence it emits for a
//! scripted SSE body (see `fixtures/generators`).

use std::sync::{Arc, Mutex};

use notagent_ai::api::pi_messages::{
    PiMessagesOptions, PiMessagesToolChoice, build_request_url, stream,
};
use notagent_ai::types::{
    AssistantMessageEvent, CacheRetention, Context, Model, ProviderEnv, ProviderHeaders,
    ProviderRequestOptions, ThinkingLevel,
};
use notagent_ai::utils::fetch::{FetchBody, FetchError, FetchFn, FetchRequest, FetchResponse};
use serde_json::Value;

struct Case {
    name: String,
    model: Model,
    context: Context,
    options: PiMessagesOptions,
    status: u16,
    status_text: String,
    body: String,
    request_url: Option<String>,
    request_headers: Option<Value>,
    request_body: Option<Value>,
    events: Vec<Value>,
}

fn options_from_fixture(raw: &Value) -> PiMessagesOptions {
    let raw = raw.as_object().cloned().unwrap_or_default();
    PiMessagesOptions {
        reasoning: raw
            .get("reasoning")
            .cloned()
            .and_then(|level| serde_json::from_value::<ThinkingLevel>(level).ok()),
        tool_choice: raw.get("toolChoice").and_then(|choice| match choice {
            Value::String(text) => match text.as_str() {
                "auto" => Some(PiMessagesToolChoice::Auto),
                "none" => Some(PiMessagesToolChoice::None),
                "required" => Some(PiMessagesToolChoice::Required),
                _ => None,
            },
            Value::Object(object) => object
                .get("function")
                .and_then(|function| function.get("name"))
                .and_then(Value::as_str)
                .map(|name| PiMessagesToolChoice::Function(name.to_string())),
            _ => None,
        }),
        debug: raw.get("debug").and_then(Value::as_bool).unwrap_or(false),
        temperature: raw.get("temperature").and_then(Value::as_f64),
        max_tokens: raw.get("maxTokens").and_then(Value::as_u64),
        cache_retention: raw
            .get("cacheRetention")
            .cloned()
            .and_then(|value| serde_json::from_value::<CacheRetention>(value).ok()),
        session_id: raw
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::to_string),
        headers: raw
            .get("headers")
            .and_then(Value::as_object)
            .map(|headers| {
                headers
                    .iter()
                    .map(|(key, value)| (key.clone(), value.as_str().map(str::to_string)))
                    .collect::<ProviderHeaders>()
            }),
        env: raw.get("env").and_then(Value::as_object).map(|env| {
            env.iter()
                .filter_map(|(key, value)| {
                    value.as_str().map(|value| (key.clone(), value.to_string()))
                })
                .collect::<ProviderEnv>()
        }),
    }
}

fn cases() -> Vec<Case> {
    include_str!("fixtures/pi-messages.jsonl")
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
                status_text: raw["statusText"].as_str().unwrap_or_default().to_string(),
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
    status_text: String,
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
        let status_text = self.status_text.clone();
        Box::pin(async move {
            Ok(FetchResponse {
                status,
                status_text,
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
                scrub_diagnostic_timestamps(message);
            }
        }
    }
    value
}

fn strip_expected(mut expected: Value) -> Value {
    if let Some(object) = expected.as_object_mut() {
        // `{ ...event, partial }` spreads the wire event, so the emitted object carries
        // fields the declared `AssistantMessageEvent` type does not have and no typed
        // consumer can read. The port's enum only carries the declared fields; the data
        // itself reaches the consumer through `partial.content` (deviation class 1).
        for key in [
            "contentSignature",
            "redacted",
            "id",
            "toolName",
            "usage",
            "rewrite",
        ] {
            object.remove(key);
        }
        for key in ["message", "error"] {
            if let Some(Value::Object(message)) = object.get_mut(key) {
                message.remove("timestamp");
                message.remove("role");
                scrub_diagnostic_timestamps(message);
            }
        }
    }
    expected
}

/// Diagnostics stamp `Date.now()`; only their content is compared.
fn scrub_diagnostic_timestamps(message: &mut serde_json::Map<String, Value>) {
    if let Some(Value::Array(diagnostics)) = message.get_mut("diagnostics") {
        for diagnostic in diagnostics {
            if let Some(object) = diagnostic.as_object_mut() {
                object.remove("timestamp");
                if let Some(Value::Object(details)) = object.get_mut("details") {
                    details.remove("timestampMs");
                }
                // JS errors carry a stack trace; Rust errors do not.
                if let Some(Value::Object(error)) = object.get_mut("error") {
                    error.remove("stack");
                }
            }
        }
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
            fetch: Some(Arc::new(CannedFetch {
                status: case.status,
                status_text: case.status_text.clone(),
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
    assert!(cases.len() >= 18, "expected the full fixture set");

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
                .map(|(key, value)| (key.to_lowercase(), Value::String(value.clone())))
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
fn appends_the_debug_flag_and_strips_trailing_slashes() {
    let model = cases()[0].model.clone();
    assert_eq!(
        build_request_url(&model, false),
        "https://gateway.example.com/v1/messages"
    );
    assert_eq!(
        build_request_url(&model, true),
        "https://gateway.example.com/v1/messages?debug=1"
    );

    let mut trailing = model;
    trailing.base_url = "https://gateway.example.com/v1///".to_string();
    assert_eq!(
        build_request_url(&trailing, false),
        "https://gateway.example.com/v1/messages"
    );
}
