use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use notagent_ai::api::openai_completions::stream;
use notagent_ai::api::openai_completions_compat::get_compat;
use notagent_ai::api::openai_completions_params::{
    OpenAICompletionsOptions, build_client_headers, get_client_api_key,
};
use notagent_ai::types::{
    AssistantMessageEvent, CacheRetention, Context, Message, Model, ProviderHeaders,
    ProviderRequestOptions, UserMessage,
};
use notagent_ai::utils::fetch::{FetchBody, FetchError, FetchFn, FetchRequest, FetchResponse};
use serde_json::{Value, json};

fn model_with(overrides: Value) -> Model {
    let mut raw = json!({
        "id": "gpt-5", "name": "gpt-5", "api": "openai-completions", "provider": "openai",
        "baseUrl": "https://api.openai.com/v1", "reasoning": true, "input": ["text"],
        "cost": { "input": 1, "output": 2, "cacheRead": 0, "cacheWrite": 0 },
        "contextWindow": 128000, "maxTokens": 4096,
    });
    let object = raw.as_object_mut().expect("object");
    for (key, value) in overrides.as_object().expect("overrides") {
        object.insert(key.clone(), value.clone());
    }
    serde_json::from_value(raw).expect("model")
}

fn context() -> Context {
    Context {
        messages: vec![Message::User(UserMessage {
            content: notagent_ai::types::UserContent::Text("hi".to_string()),
            timestamp: 1,
        })],
        ..Context::default()
    }
}

fn headers_for(
    model: &Model,
    session_id: Option<&str>,
    options: Option<ProviderHeaders>,
) -> BTreeMap<String, Option<String>> {
    let compat = get_compat(model);
    build_client_headers(model, &context(), options.as_ref(), session_id, &compat)
}

fn header(headers: &BTreeMap<String, Option<String>>, name: &str) -> Option<String> {
    headers.get(name).cloned().flatten()
}

#[test]
fn session_affinity_headers_follow_the_configured_format() {
    let model = model_with(json!({
        "baseUrl": "https://proxy.example.com/v1",
        "compat": { "sendSessionAffinityHeaders": true },
    }));
    let headers = headers_for(&model, Some("session-affinity"), None);
    assert_eq!(
        header(&headers, "session_id").as_deref(),
        Some("session-affinity")
    );
    assert_eq!(
        header(&headers, "x-client-request-id").as_deref(),
        Some("session-affinity")
    );
    assert_eq!(
        header(&headers, "x-session-affinity").as_deref(),
        Some("session-affinity")
    );
}

#[test]
fn the_openai_nosession_format_drops_only_the_session_id_header() {
    let model = model_with(json!({
        "compat": { "sendSessionAffinityHeaders": true, "sessionAffinityFormat": "openai-nosession" },
    }));
    let headers = headers_for(&model, Some("session-nosession"), None);
    assert_eq!(header(&headers, "session_id"), None);
    assert_eq!(
        header(&headers, "x-client-request-id").as_deref(),
        Some("session-nosession")
    );
    assert_eq!(
        header(&headers, "x-session-affinity").as_deref(),
        Some("session-nosession")
    );
    assert_eq!(header(&headers, "x-session-id"), None);
}

#[test]
fn openrouter_uses_a_single_session_header_and_is_auto_detected() {
    let configured = model_with(json!({
        "baseUrl": "https://proxy.example.com/v1",
        "compat": { "sendSessionAffinityHeaders": true, "sessionAffinityFormat": "openrouter" },
    }));
    let headers = headers_for(&configured, Some("session-proxy"), None);
    assert_eq!(
        header(&headers, "x-session-id").as_deref(),
        Some("session-proxy")
    );
    assert_eq!(header(&headers, "session_id"), None);
    assert_eq!(header(&headers, "x-client-request-id"), None);
    assert_eq!(header(&headers, "x-session-affinity"), None);

    // The format is detected from the provider, so only the flag has to be set.
    let detected = model_with(json!({
        "provider": "openrouter",
        "baseUrl": "https://openrouter.ai/api/v1",
        "compat": { "sendSessionAffinityHeaders": true },
    }));
    let headers = headers_for(&detected, Some("session-openrouter"), None);
    assert_eq!(
        header(&headers, "x-session-id").as_deref(),
        Some("session-openrouter")
    );

    // Without the flag nothing is sent at all.
    let disabled = model_with(json!({
        "provider": "openrouter",
        "baseUrl": "https://openrouter.ai/api/v1",
    }));
    let headers = headers_for(&disabled, Some("session-openrouter"), None);
    assert_eq!(header(&headers, "x-session-id"), None);
}

#[test]
fn explicit_headers_override_the_generated_session_affinity() {
    let model = model_with(json!({
        "baseUrl": "https://proxy.example.com/v1",
        "compat": { "sendSessionAffinityHeaders": true },
    }));
    let mut overrides: ProviderHeaders = BTreeMap::new();
    overrides.insert(
        "session_id".to_string(),
        Some("override-session".to_string()),
    );
    overrides.insert(
        "x-client-request-id".to_string(),
        Some("override-request".to_string()),
    );
    let headers = headers_for(&model, Some("session-affinity"), Some(overrides));
    assert_eq!(
        header(&headers, "session_id").as_deref(),
        Some("override-session")
    );
    assert_eq!(
        header(&headers, "x-client-request-id").as_deref(),
        Some("override-request")
    );
    // Untouched keys keep the generated value.
    assert_eq!(
        header(&headers, "x-session-affinity").as_deref(),
        Some("session-affinity")
    );
}

#[test]
fn model_headers_are_the_base_of_the_client_headers() {
    let model = model_with(json!({ "headers": { "x-model": "from-model", "x-both": "model" } }));
    let mut overrides: ProviderHeaders = BTreeMap::new();
    overrides.insert("x-both".to_string(), Some("request".to_string()));
    // A `null` value deletes a default.
    overrides.insert("x-model".to_string(), None);
    let headers = headers_for(&model, None, Some(overrides));
    assert_eq!(header(&headers, "x-both").as_deref(), Some("request"));
    assert_eq!(header(&headers, "x-model"), None);
}

#[test]
fn an_authorization_header_stands_in_for_a_missing_api_key() {
    assert_eq!(
        get_client_api_key("openai", Some("sk-1"), None).expect("key"),
        "sk-1"
    );

    let mut headers: ProviderHeaders = BTreeMap::new();
    headers.insert("Authorization".to_string(), Some("Bearer x".to_string()));
    assert_eq!(
        get_client_api_key("openai", None, Some(&headers)).expect("key"),
        "unused"
    );

    // Cloudflare's gateway header counts too.
    let mut gateway: ProviderHeaders = BTreeMap::new();
    gateway.insert(
        "cf-aig-authorization".to_string(),
        Some("Bearer y".to_string()),
    );
    assert_eq!(
        get_client_api_key("cloudflare-ai-gateway", None, Some(&gateway)).expect("key"),
        "unused"
    );

    // A blank header value does not.
    let mut blank: ProviderHeaders = BTreeMap::new();
    blank.insert("authorization".to_string(), Some("   ".to_string()));
    assert_eq!(
        get_client_api_key("openai", None, Some(&blank))
            .expect_err("no key")
            .to_string(),
        "No API key for provider: openai"
    );
}

// ---------------------------------------------------------------------------
// Retry
// ---------------------------------------------------------------------------

struct FlakyFetch {
    /// Status per attempt; the last entry is reused once the list runs out.
    statuses: Vec<u16>,
    retry_after: Option<String>,
    attempts: Arc<Mutex<Vec<FetchRequest>>>,
}

impl FetchFn for FlakyFetch {
    fn fetch(
        &self,
        request: FetchRequest,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<FetchResponse, FetchError>> + Send>,
    > {
        let attempt = {
            let mut attempts = self.attempts.lock().expect("poisoned");
            attempts.push(request);
            attempts.len() - 1
        };
        let status = *self
            .statuses
            .get(attempt)
            .or_else(|| self.statuses.last())
            .expect("at least one status");
        let retry_after = self.retry_after.clone();
        Box::pin(async move {
            let mut headers = vec![("content-type".to_string(), "text/event-stream".to_string())];
            if let Some(retry_after) = retry_after
                && status != 200
            {
                headers.push(("retry-after".to_string(), retry_after));
            }
            let body = if status == 200 {
                "data: {\"id\":\"1\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n".to_string()
            } else {
                "{\"error\":{\"message\":\"rate limited\"}}".to_string()
            };
            Ok(FetchResponse {
                status,
                status_text: String::new(),
                headers,
                body: FetchBody::Bytes(body.into_bytes()),
            })
        })
    }
}

async fn run_with(
    fetch: Arc<FlakyFetch>,
    request: ProviderRequestOptions,
) -> AssistantMessageEvent {
    let events = stream(
        model_with(json!({})),
        context(),
        ProviderRequestOptions {
            api_key: Some("k".to_string()),
            fetch: Some(fetch),
            ..request
        },
        OpenAICompletionsOptions::default(),
    );
    let mut last = None;
    while let Some(event) = events.next().await {
        last = Some(event);
    }
    last.expect("at least one event")
}

#[tokio::test(flavor = "multi_thread")]
async fn without_a_retry_budget_a_failure_is_reported_immediately() {
    let attempts = Arc::new(Mutex::new(Vec::new()));
    let last = run_with(
        Arc::new(FlakyFetch {
            statuses: vec![429],
            retry_after: None,
            attempts: attempts.clone(),
        }),
        ProviderRequestOptions::default(),
    )
    .await;
    assert!(matches!(last, AssistantMessageEvent::Error { .. }));
    assert_eq!(attempts.lock().expect("poisoned").len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_retryable_status_is_retried_up_to_the_budget() {
    let attempts = Arc::new(Mutex::new(Vec::new()));
    let last = run_with(
        Arc::new(FlakyFetch {
            statuses: vec![429, 429, 200],
            retry_after: Some("0".to_string()),
            attempts: attempts.clone(),
        }),
        ProviderRequestOptions {
            max_retries: Some(2),
            ..ProviderRequestOptions::default()
        },
    )
    .await;
    assert!(matches!(last, AssistantMessageEvent::Done { .. }));
    assert_eq!(attempts.lock().expect("poisoned").len(), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_requested_delay_beyond_the_limit_fails_without_waiting() {
    let attempts = Arc::new(Mutex::new(Vec::new()));
    let last = run_with(
        Arc::new(FlakyFetch {
            statuses: vec![429],
            // 120 s, twice the default cap.
            retry_after: Some("120".to_string()),
            attempts: attempts.clone(),
        }),
        ProviderRequestOptions {
            max_retries: Some(3),
            max_retry_delay_ms: Some(60_000),
            ..ProviderRequestOptions::default()
        },
    )
    .await;
    let AssistantMessageEvent::Error { error, .. } = last else {
        panic!("expected an error event");
    };
    let message = error.error_message.expect("message");
    assert!(
        message.contains("Server requested 120s retry delay"),
        "{message}"
    );
    assert_eq!(attempts.lock().expect("poisoned").len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_request_carries_the_bearer_token_and_the_completions_path() {
    let attempts = Arc::new(Mutex::new(Vec::new()));
    run_with(
        Arc::new(FlakyFetch {
            statuses: vec![200],
            retry_after: None,
            attempts: attempts.clone(),
        }),
        ProviderRequestOptions::default(),
    )
    .await;
    let attempts = attempts.lock().expect("poisoned");
    let request = attempts.first().expect("request");
    assert_eq!(request.method, "POST");
    assert_eq!(request.url, "https://api.openai.com/v1/chat/completions");
    assert!(
        request
            .headers
            .iter()
            .any(|(key, value)| key == "authorization" && value == "Bearer k")
    );
    let body: Value = serde_json::from_slice(request.body.as_ref().expect("body")).expect("json");
    assert_eq!(body["stream"], true);
}

#[test]
fn a_cache_retention_of_none_suppresses_the_affinity_session_id() {
    // The stream path passes `undefined` for the session id in that case; the header
    // builder then has nothing to add.
    let model = model_with(json!({
        "baseUrl": "https://proxy.example.com/v1",
        "compat": { "sendSessionAffinityHeaders": true },
    }));
    let retention = CacheRetention::None;
    let session_id = if retention == CacheRetention::None {
        None
    } else {
        Some("session-affinity")
    };
    let headers = headers_for(&model, session_id, None);
    assert_eq!(header(&headers, "session_id"), None);
    assert_eq!(header(&headers, "x-client-request-id"), None);
    assert_eq!(header(&headers, "x-session-affinity"), None);
}
