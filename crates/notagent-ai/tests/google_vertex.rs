//! Tests of the Google Vertex adapter.
//! The endpoint resolution is what sets Vertex apart from the Gemini adapter, so the
//! URLs are checked against the ones the `@google/genai` SDK builds (captured by hand
//! from its `getBaseUrl`/`constructUrl` logic), and the request body against the shared
//! Google conversion the Gemini fixtures already pin down.

use std::sync::{Arc, Mutex};

use notagent_ai::api::google_generative_ai::GoogleThinkingOptions;
use notagent_ai::api::google_vertex::{
    GoogleVertexOptions, VertexEndpoint, VertexTokenProvider, base_url_includes_api_version,
    build_request_body, build_request_url, gemini3_thinking_level, google_budget, resolve_api_key,
    resolve_custom_base_url, resolve_endpoint, resolve_location, resolve_project, stream,
};
use notagent_ai::types::{
    AssistantMessageEvent, Context, Message, Model, ProviderEnv, ProviderRequestOptions,
    ThinkingBudgets, ThinkingLevel, UserContent, UserMessage,
};
use notagent_ai::utils::fetch::{FetchBody, FetchError, FetchFn, FetchRequest, FetchResponse};
use serde_json::{Value, json};

fn model_with(overrides: Value) -> Model {
    let mut raw = json!({
        "id": "gemini-2.5-flash", "name": "gemini-2.5-flash", "api": "google-vertex",
        "provider": "google-vertex", "baseUrl": "", "reasoning": true, "input": ["text", "image"],
        "cost": { "input": 1, "output": 2, "cacheRead": 0, "cacheWrite": 0 },
        "contextWindow": 1000000, "maxTokens": 8192,
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
            content: UserContent::Text("hi".to_string()),
            timestamp: 1,
        })],
        ..Context::default()
    }
}

fn env(pairs: &[(&str, &str)]) -> ProviderEnv {
    pairs
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

const TIMESTAMP: i64 = 1_700_000_000_000;

#[test]
fn the_api_key_endpoint_uses_the_global_host_without_a_project_path() {
    let endpoint = VertexEndpoint::ApiKey {
        api_key: "k".to_string(),
    };
    assert_eq!(
        build_request_url(&model_with(json!({})), &endpoint).expect("url"),
        "https://aiplatform.googleapis.com/v1/publishers/google/models/gemini-2.5-flash:streamGenerateContent?alt=sse"
    );
}

#[test]
fn a_project_and_location_produce_the_regional_host_and_the_project_path() {
    let endpoint = VertexEndpoint::ProjectLocation {
        project: "my-project".to_string(),
        location: "us-central1".to_string(),
    };
    assert_eq!(
        build_request_url(&model_with(json!({})), &endpoint).expect("url"),
        "https://us-central1-aiplatform.googleapis.com/v1/projects/my-project/locations/us-central1/publishers/google/models/gemini-2.5-flash:streamGenerateContent?alt=sse"
    );

    // The global location falls back to the unprefixed host.
    let global = VertexEndpoint::ProjectLocation {
        project: "my-project".to_string(),
        location: "global".to_string(),
    };
    assert_eq!(
        build_request_url(&model_with(json!({})), &global).expect("url"),
        "https://aiplatform.googleapis.com/v1/projects/my-project/locations/global/publishers/google/models/gemini-2.5-flash:streamGenerateContent?alt=sse"
    );

    // The two multi-regional locations get their own host shape.
    for location in ["us", "eu"] {
        let endpoint = VertexEndpoint::ProjectLocation {
            project: "p".to_string(),
            location: location.to_string(),
        };
        let url = build_request_url(&model_with(json!({})), &endpoint).expect("url");
        assert!(
            url.starts_with(&format!(
                "https://aiplatform.{location}.rep.googleapis.com/v1/"
            )),
            "{url}"
        );
    }
}

#[test]
fn a_custom_base_url_suppresses_the_project_path() {
    let model = model_with(json!({ "baseUrl": "https://proxy.example.com/v1" }));
    let endpoint = VertexEndpoint::ProjectLocation {
        project: "my-project".to_string(),
        location: "us-central1".to_string(),
    };
    // The base URL already carries the version, so no version segment is appended and
    // the COLLECTION scope drops the project path.
    assert_eq!(
        build_request_url(&model, &endpoint).expect("url"),
        "https://proxy.example.com/v1/publishers/google/models/gemini-2.5-flash:streamGenerateContent?alt=sse"
    );

    // Without a version in the path the default one is appended.
    let model = model_with(json!({ "baseUrl": "https://proxy.example.com" }));
    assert_eq!(
        build_request_url(&model, &endpoint).expect("url"),
        "https://proxy.example.com/v1/publishers/google/models/gemini-2.5-flash:streamGenerateContent?alt=sse"
    );

    // A `{location}` template is not a usable base URL.
    assert_eq!(
        resolve_custom_base_url("https://{location}-aiplatform.googleapis.com"),
        None
    );
    assert_eq!(resolve_custom_base_url("   "), None);
}

#[test]
fn the_api_version_is_detected_in_the_base_url_path() {
    assert!(base_url_includes_api_version("https://x.example.com/v1"));
    assert!(base_url_includes_api_version(
        "https://x.example.com/v1beta"
    ));
    assert!(base_url_includes_api_version(
        "https://x.example.com/v1beta1/more"
    ));
    assert!(!base_url_includes_api_version("https://x.example.com/api"));
    // The fallback path for values that do not parse as a URL.
    assert!(base_url_includes_api_version("/v1/"));
    assert!(!base_url_includes_api_version("not a url"));
}

#[test]
fn the_credentials_marker_and_placeholders_fall_back_to_adc() {
    assert_eq!(
        resolve_api_key(Some("real-key")).as_deref(),
        Some("real-key")
    );
    assert_eq!(
        resolve_api_key(Some("  spaced  ")).as_deref(),
        Some("spaced")
    );
    assert_eq!(resolve_api_key(Some("gcp-vertex-credentials")), None);
    assert_eq!(resolve_api_key(Some("<your key here>")), None);
    assert_eq!(resolve_api_key(Some("")), None);
    assert_eq!(resolve_api_key(None), None);
}

#[test]
fn the_project_and_location_come_from_the_options_or_the_environment() {
    assert_eq!(
        resolve_project(Some("from-option"), None).expect("project"),
        "from-option"
    );
    let environment = env(&[("GOOGLE_CLOUD_PROJECT", "from-env")]);
    assert_eq!(
        resolve_project(None, Some(&environment)).expect("project"),
        "from-env"
    );
    // GCLOUD_PROJECT is the fallback.
    let environment = env(&[("GCLOUD_PROJECT", "from-gcloud")]);
    assert_eq!(
        resolve_project(None, Some(&environment)).expect("project"),
        "from-gcloud"
    );
    assert_eq!(
        resolve_project(None, None).expect_err("missing").message,
        "Vertex AI requires a project ID. Set GOOGLE_CLOUD_PROJECT/GCLOUD_PROJECT or pass project in options."
    );

    let environment = env(&[("GOOGLE_CLOUD_LOCATION", "europe-west4")]);
    assert_eq!(
        resolve_location(None, Some(&environment)).expect("location"),
        "europe-west4"
    );
    assert_eq!(
        resolve_location(None, None).expect_err("missing").message,
        "Vertex AI requires a location. Set GOOGLE_CLOUD_LOCATION or pass location in options."
    );
}

#[test]
fn the_endpoint_prefers_an_api_key_then_project_and_location() {
    let model = model_with(json!({}));
    let options = GoogleVertexOptions {
        project: Some("p".to_string()),
        location: Some("l".to_string()),
        ..GoogleVertexOptions::default()
    };
    assert_eq!(
        resolve_endpoint(&model, Some("key"), &options).expect("endpoint"),
        VertexEndpoint::ApiKey {
            api_key: "key".to_string()
        }
    );
    assert_eq!(
        resolve_endpoint(&model, None, &options).expect("endpoint"),
        VertexEndpoint::ProjectLocation {
            project: "p".to_string(),
            location: "l".to_string()
        }
    );
    // A custom base URL alone is enough.
    let proxied = model_with(json!({ "baseUrl": "https://proxy.example.com/v1" }));
    assert_eq!(
        resolve_endpoint(&proxied, None, &GoogleVertexOptions::default()).expect("endpoint"),
        VertexEndpoint::CustomBaseUrl
    );
    // Nothing at all reports the missing project.
    assert!(
        resolve_endpoint(&model, None, &GoogleVertexOptions::default())
            .expect_err("missing")
            .message
            .contains("requires a project ID")
    );
}

#[test]
fn the_request_body_matches_the_shared_google_conversion() {
    let body = build_request_body(
        &model_with(json!({})),
        &Context {
            system_prompt: Some("be nice".to_string()),
            ..context()
        },
        &GoogleVertexOptions {
            temperature: Some(0.4),
            max_tokens: Some(256),
            ..GoogleVertexOptions::default()
        },
        TIMESTAMP,
    )
    .expect("body");
    assert_eq!(body["contents"][0]["parts"][0]["text"], "hi");
    assert_eq!(body["systemInstruction"]["parts"][0]["text"], "be nice");
    assert_eq!(body["generationConfig"]["temperature"], 0.4);
    assert_eq!(body["generationConfig"]["maxOutputTokens"], 256);
}

#[test]
fn vertex_has_no_gemma_branch_and_no_flash_lite_budget() {
    // Thinking off: Gemini 3 gets the lowest level, everything else a zero budget.
    let disabled = |id: &str| {
        build_request_body(
            &model_with(json!({ "id": id })),
            &context(),
            &GoogleVertexOptions {
                thinking: Some(GoogleThinkingOptions {
                    enabled: false,
                    ..GoogleThinkingOptions::default()
                }),
                ..GoogleVertexOptions::default()
            },
            TIMESTAMP,
        )
        .expect("body")["generationConfig"]["thinkingConfig"]
            .clone()
    };
    assert_eq!(disabled("gemini-3-pro"), json!({ "thinkingLevel": "LOW" }));
    assert_eq!(
        disabled("gemini-3-flash"),
        json!({ "thinkingLevel": "MINIMAL" })
    );
    // Gemma has no branch here, unlike the Gemini adapter.
    assert_eq!(disabled("gemma-4-27b"), json!({ "thinkingBudget": 0 }));
    assert_eq!(disabled("gemini-2.5-flash"), json!({ "thinkingBudget": 0 }));

    assert_eq!(
        gemini3_thinking_level(ThinkingLevel::Low, "gemini-3-pro"),
        "LOW"
    );
    assert_eq!(
        gemini3_thinking_level(ThinkingLevel::High, "gemini-3-pro"),
        "HIGH"
    );
    assert_eq!(
        gemini3_thinking_level(ThinkingLevel::Medium, "gemini-3-flash"),
        "MEDIUM"
    );

    // Flash-Lite falls into the flash table here, unlike the Gemini adapter.
    assert_eq!(
        google_budget("gemini-2.5-flash-lite", ThinkingLevel::Minimal, None),
        128
    );
    assert_eq!(
        google_budget("gemini-2.5-pro", ThinkingLevel::High, None),
        32768
    );
    assert_eq!(google_budget("gemini-3-pro", ThinkingLevel::High, None), -1);
    assert_eq!(
        google_budget(
            "gemini-2.5-pro",
            ThinkingLevel::Low,
            Some(ThinkingBudgets {
                low: Some(7),
                ..ThinkingBudgets::default()
            })
        ),
        7
    );
}

/// A fetch that records the request and answers with a complete stream.
struct RecordingFetch {
    seen: Arc<Mutex<Option<FetchRequest>>>,
}

impl FetchFn for RecordingFetch {
    fn fetch(
        &self,
        request: FetchRequest,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<FetchResponse, FetchError>> + Send>,
    > {
        *self.seen.lock().expect("poisoned") = Some(request);
        Box::pin(async move {
            Ok(FetchResponse {
                status: 200,
                status_text: String::new(),
                headers: vec![("content-type".to_string(), "text/event-stream".to_string())],
                body: FetchBody::Bytes(
                    "data: {\"responseId\":\"r\",\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"hi\"}]},\"finishReason\":\"STOP\"}]}\n\n"
                        .as_bytes()
                        .to_vec(),
                ),
            })
        })
    }
}

async fn run(
    model: Model,
    options: GoogleVertexOptions,
    request: ProviderRequestOptions,
    token_provider: Option<VertexTokenProvider>,
) -> (Option<AssistantMessageEvent>, Option<FetchRequest>) {
    let seen = Arc::new(Mutex::new(None));
    let events = stream(
        model,
        context(),
        ProviderRequestOptions {
            fetch: Some(Arc::new(RecordingFetch { seen: seen.clone() })),
            ..request
        },
        options,
        token_provider,
    );
    let mut last = None;
    while let Some(event) = events.next().await {
        last = Some(event);
    }
    let request = seen.lock().expect("poisoned").clone();
    (last, request)
}

#[tokio::test(flavor = "multi_thread")]
async fn an_api_key_request_sends_the_goog_header() {
    let (last, request) = run(
        model_with(json!({})),
        GoogleVertexOptions::default(),
        ProviderRequestOptions {
            api_key: Some("vertex-key".to_string()),
            ..ProviderRequestOptions::default()
        },
        None,
    )
    .await;
    assert!(matches!(last, Some(AssistantMessageEvent::Done { .. })));
    let request = request.expect("request");
    assert!(
        request
            .headers
            .iter()
            .any(|(key, value)| key == "x-goog-api-key" && value == "vertex-key")
    );
    assert!(
        request
            .url
            .starts_with("https://aiplatform.googleapis.com/v1/publishers/")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_adc_request_sends_the_bearer_token_the_provider_supplies() {
    let token_provider: VertexTokenProvider =
        Arc::new(|| Box::pin(async { Ok("adc-token".to_string()) }));
    let (last, request) = run(
        model_with(json!({})),
        GoogleVertexOptions {
            project: Some("my-project".to_string()),
            location: Some("us-central1".to_string()),
            ..GoogleVertexOptions::default()
        },
        ProviderRequestOptions::default(),
        Some(token_provider),
    )
    .await;
    assert!(matches!(last, Some(AssistantMessageEvent::Done { .. })));
    let request = request.expect("request");
    assert!(
        request
            .headers
            .iter()
            .any(|(key, value)| key == "authorization" && value == "Bearer adc-token")
    );
    assert!(
        request
            .url
            .starts_with("https://us-central1-aiplatform.googleapis.com/v1/projects/my-project/")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn without_a_token_provider_an_adc_request_reports_the_missing_credentials() {
    let (last, request) = run(
        model_with(json!({})),
        GoogleVertexOptions {
            project: Some("p".to_string()),
            location: Some("l".to_string()),
            ..GoogleVertexOptions::default()
        },
        ProviderRequestOptions::default(),
        None,
    )
    .await;
    let Some(AssistantMessageEvent::Error { error, .. }) = last else {
        panic!("expected an error event");
    };
    assert_eq!(
        error.error_message.as_deref(),
        Some("Could not load the default credentials.")
    );
    // The api field is pinned to the Vertex literal.
    assert_eq!(error.api, "google-vertex");
    // Nothing went out.
    assert!(request.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_token_provider_failure_surfaces_as_the_stream_error() {
    let token_provider: VertexTokenProvider =
        Arc::new(|| Box::pin(async { Err("metadata server unreachable".to_string()) }));
    let (last, _) = run(
        model_with(json!({})),
        GoogleVertexOptions {
            project: Some("p".to_string()),
            location: Some("l".to_string()),
            ..GoogleVertexOptions::default()
        },
        ProviderRequestOptions::default(),
        Some(token_provider),
    )
    .await;
    let Some(AssistantMessageEvent::Error { error, .. }) = last else {
        panic!("expected an error event");
    };
    assert_eq!(
        error.error_message.as_deref(),
        Some("metadata server unreachable")
    );
}
