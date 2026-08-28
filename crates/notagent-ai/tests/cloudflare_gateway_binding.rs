use std::sync::{Arc, Mutex};

use notagent_ai::api::cloudflare_gateway_binding::{
    AiGatewayBinding, AiGatewayUniversalRequest, CLOUDFLARE_GATEWAY_BINDING_AUTH_SENTINEL,
    GatewayBindingFetchOptions, create_gateway_binding_fetch,
};
use notagent_ai::utils::fetch::{FetchBody, FetchFuture, FetchRequest, FetchResponse};

const BASE_URL: &str = "https://gateway.ai.cloudflare.com/v1/account-id/my-gateway";

#[derive(Debug, Clone, PartialEq, Eq)]
struct CapturedRun {
    gateway_id: String,
    data: AiGatewayUniversalRequest,
}

#[derive(Default)]
struct FakeBinding {
    runs: Mutex<Vec<CapturedRun>>,
    body: Option<String>,
}

impl AiGatewayBinding for FakeBinding {
    fn run(&self, gateway: &str, data: AiGatewayUniversalRequest) -> FetchFuture {
        self.runs.lock().expect("poisoned").push(CapturedRun {
            gateway_id: gateway.to_string(),
            data,
        });
        let body = self.body.clone().unwrap_or_else(|| "{}".to_string());
        Box::pin(async move {
            Ok(FetchResponse {
                status: 200,
                status_text: String::new(),
                headers: vec![("content-type".to_string(), "application/json".to_string())],
                body: FetchBody::Bytes(body.into_bytes()),
            })
        })
    }
}

fn post(url: &str, body: &str) -> FetchRequest {
    FetchRequest {
        method: "POST".to_string(),
        url: url.to_string(),
        headers: Vec::new(),
        body: Some(body.as_bytes().to_vec()),
    }
}

fn setup() -> (
    Arc<FakeBinding>,
    Arc<dyn notagent_ai::utils::fetch::FetchFn>,
) {
    let binding = Arc::new(FakeBinding::default());
    let fetch = create_gateway_binding_fetch(GatewayBindingFetchOptions {
        binding: binding.clone(),
        base_url: BASE_URL.to_string(),
        gateway: "my-gateway".to_string(),
    });
    (binding, fetch)
}

#[tokio::test]
async fn derives_provider_and_endpoint_from_gateway_passthrough_urls() {
    let (binding, fetch) = setup();

    fetch
        .fetch(post(
            &format!("{BASE_URL}/anthropic/v1/messages"),
            r#"{"model":"claude"}"#,
        ))
        .await
        .expect("binding call");
    fetch
        .fetch(post(
            &format!("{BASE_URL}/openai/responses"),
            r#"{"model":"gpt"}"#,
        ))
        .await
        .expect("binding call");
    fetch
        .fetch(post(
            &format!("{BASE_URL}/workers-ai/v1/chat/completions"),
            r#"{"model":"@cf/meta/llama"}"#,
        ))
        .await
        .expect("binding call");

    let runs = binding.runs.lock().expect("poisoned");
    assert_eq!(
        runs.iter()
            .map(|run| (run.data.provider.as_str(), run.data.endpoint.as_str()))
            .collect::<Vec<_>>(),
        [
            ("anthropic", "v1/messages"),
            ("openai", "responses"),
            ("workers-ai", "v1/chat/completions"),
        ]
    );
    assert!(runs.iter().all(|run| run.gateway_id == "my-gateway"));
    assert_eq!(runs[0].data.query, serde_json::json!({ "model": "claude" }));
}

#[tokio::test]
async fn keeps_the_query_string_in_the_endpoint() {
    let (binding, fetch) = setup();

    fetch
        .fetch(post(
            &format!(
                "{BASE_URL}/google-ai-studio/v1beta/models/gemini:streamGenerateContent?alt=sse"
            ),
            "{}",
        ))
        .await
        .expect("binding call");

    let runs = binding.runs.lock().expect("poisoned");
    assert_eq!(runs[0].data.provider, "google-ai-studio");
    assert_eq!(
        runs[0].data.endpoint,
        "v1beta/models/gemini:streamGenerateContent?alt=sse"
    );
}

#[tokio::test]
async fn lowercases_header_names_so_case_variants_collapse() {
    let (binding, fetch) = setup();

    let mut request = post(&format!("{BASE_URL}/anthropic/v1/messages"), "{}");
    request.headers = vec![
        ("Content-Type".to_string(), "application/json".to_string()),
        ("X-Custom".to_string(), "first".to_string()),
        ("x-custom".to_string(), "second".to_string()),
    ];
    fetch.fetch(request).await.expect("binding call");

    let runs = binding.runs.lock().expect("poisoned");
    assert_eq!(
        runs[0].data.headers.get("content-type").map(String::as_str),
        Some("application/json")
    );
    assert_eq!(
        runs[0].data.headers.get("x-custom").map(String::as_str),
        Some("second"),
        "the later duplicate wins"
    );
    assert!(!runs[0].data.headers.contains_key("X-Custom"));
}

#[tokio::test]
async fn strips_gateway_auth_and_derived_headers_and_forwards_the_rest() {
    let (binding, fetch) = setup();

    let mut request = post(&format!("{BASE_URL}/anthropic/v1/messages"), "{}");
    request.headers = vec![
        (
            "cf-aig-authorization".to_string(),
            format!("Bearer {CLOUDFLARE_GATEWAY_BINDING_AUTH_SENTINEL}"),
        ),
        ("content-length".to_string(), "2".to_string()),
        ("host".to_string(), "gateway.ai.cloudflare.com".to_string()),
        ("anthropic-version".to_string(), "2023-06-01".to_string()),
    ];
    fetch.fetch(request).await.expect("binding call");

    let runs = binding.runs.lock().expect("poisoned");
    assert_eq!(
        runs[0].data.headers.keys().collect::<Vec<_>>(),
        ["anthropic-version"]
    );
}

#[tokio::test]
async fn rejects_in_prefix_requests_the_universal_endpoint_cannot_express() {
    let (binding, fetch) = setup();

    let mut get = post(&format!("{BASE_URL}/anthropic/v1/messages"), "{}");
    get.method = "GET".to_string();
    let error = fetch.fetch(get).await.expect_err("rejects");
    assert!(error.to_string().contains("cannot express GET"), "{error}");

    let error = fetch
        .fetch(post(
            &format!("{BASE_URL}/anthropic/v1/messages"),
            "not json",
        ))
        .await
        .expect_err("rejects");
    assert!(error.to_string().contains("non-JSON body"), "{error}");

    let error = fetch
        .fetch(post(&format!("{BASE_URL}/anthropic"), "{}"))
        .await
        .expect_err("rejects");
    assert!(
        error.to_string().contains("missing provider/endpoint path"),
        "{error}"
    );

    assert!(binding.runs.lock().expect("poisoned").is_empty());
}

#[tokio::test]
async fn rejects_urls_outside_the_gateway_prefix() {
    let (binding, fetch) = setup();

    for url in [
        "https://api.openai.com/v1/chat/completions",
        "https://gateway.ai.cloudflare.com/v1/other-account/my-gateway/anthropic/v1/messages",
    ] {
        let error = fetch.fetch(post(url, "{}")).await.expect_err("rejects");
        assert!(
            error
                .to_string()
                .contains("outside the configured gateway prefix"),
            "{error}"
        );
    }

    assert!(binding.runs.lock().expect("poisoned").is_empty());
}

#[tokio::test]
async fn matches_and_splits_on_the_url_normalized_path() {
    let (binding, fetch) = setup();

    // Dot segments normalize away before the provider/endpoint split.
    fetch
        .fetch(post(
            &format!("{BASE_URL}/anthropic/../anthropic/v1/./messages"),
            r#"{"model":"claude"}"#,
        ))
        .await
        .expect("binding call");
    assert_eq!(
        binding
            .runs
            .lock()
            .expect("poisoned")
            .iter()
            .map(|run| (run.data.provider.clone(), run.data.endpoint.clone()))
            .collect::<Vec<_>>(),
        [("anthropic".to_string(), "v1/messages".to_string())]
    );

    // A dot-segment URL that resolves outside the prefix is rejected even though it
    // starts with the prefix as a raw string.
    let error = fetch
        .fetch(post(
            &format!("{BASE_URL}/../other-gateway/anthropic/v1/messages"),
            "{}",
        ))
        .await
        .expect_err("rejects");
    assert!(
        error
            .to_string()
            .contains("outside the configured gateway prefix"),
        "{error}"
    );
    assert_eq!(binding.runs.lock().expect("poisoned").len(), 1);
}

#[tokio::test]
async fn returns_the_binding_response_untouched() {
    let binding = Arc::new(FakeBinding {
        runs: Mutex::new(Vec::new()),
        body: Some("data: chunk\n\n".to_string()),
    });
    let fetch = create_gateway_binding_fetch(GatewayBindingFetchOptions {
        binding: binding.clone(),
        base_url: BASE_URL.to_string(),
        gateway: "my-gateway".to_string(),
    });

    let response = fetch
        .fetch(post(&format!("{BASE_URL}/anthropic/v1/messages"), "{}"))
        .await
        .expect("binding call");
    assert_eq!(response.status, 200);
    let FetchBody::Bytes(bytes) = response.body else {
        panic!("expected a buffered body");
    };
    assert_eq!(String::from_utf8_lossy(&bytes), "data: chunk\n\n");
}
