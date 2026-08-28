use std::sync::{Arc, Mutex};

use notagent_ai::api::openai_responses::{OpenAIResponsesOptions, stream};
use notagent_ai::model_catalog::get_builtin_model;
use notagent_ai::models::get_supported_thinking_levels;
use notagent_ai::types::{
    CacheRetention, Context, Message, Model, ModelThinkingLevel, ProviderRequestOptions,
    StopReason, ThinkingLevel, UserContent, UserMessage,
};
use notagent_ai::utils::fetch::{FetchBody, FetchError, FetchFn, FetchRequest, FetchResponse};
use serde_json::{Value, json};

/// `response.completed` with an empty output, so the stream ends cleanly.
fn completed_response_body() -> String {
    let event = json!({
        "type": "response.completed",
        "sequence_number": 0,
        "response": {
            "id": "resp_xai_test",
            "status": "completed",
            "output": [],
            "usage": {
                "input_tokens": 1,
                "output_tokens": 1,
                "total_tokens": 2,
                "input_tokens_details": { "cached_tokens": 0 },
            },
        },
    });
    format!("data: {event}\n\ndata: [DONE]\n\n")
}

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
                body: FetchBody::Bytes(completed_response_body().into_bytes()),
            })
        })
    }
}

struct Captured {
    url: String,
    headers: Vec<(String, String)>,
    body: Value,
}

impl Captured {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

fn xai_model(id: &str) -> Model {
    get_builtin_model("xai", id).unwrap_or_else(|| panic!("xai/{id} is missing from the catalog"))
}

async fn capture_request(
    model: Model,
    context: Context,
    options: OpenAIResponsesOptions,
    api_key: &str,
) -> Captured {
    let seen = Arc::new(Mutex::new(None));
    let result = stream(
        model,
        context,
        ProviderRequestOptions {
            api_key: Some(api_key.to_string()),
            fetch: Some(Arc::new(RecordingFetch { seen: seen.clone() })),
            ..ProviderRequestOptions::default()
        },
        options,
    )
    .result()
    .await;
    assert_eq!(
        result.stop_reason,
        StopReason::Stop,
        "{:?}",
        result.error_message
    );

    let request = seen.lock().expect("poisoned").take().expect("a request");
    Captured {
        url: request.url,
        headers: request.headers,
        body: serde_json::from_slice(&request.body.expect("body")).expect("json body"),
    }
}

#[test]
fn excludes_retired_and_redundant_models_from_the_builtin_catalog() {
    for model_id in [
        "grok-3",
        "grok-3-fast",
        "grok-4.20-0309-non-reasoning",
        "grok-4.20-0309-reasoning",
        "grok-code-fast-1",
    ] {
        assert!(
            get_builtin_model("xai", model_id).is_none(),
            "{model_id} should not be in the catalog"
        );
    }
}

#[test]
fn uses_responses_with_low_medium_high_efforts_only_for_grok_4_5() {
    assert_eq!(xai_model("grok-4.5").api, "openai-responses");
    assert_eq!(
        get_supported_thinking_levels(&xai_model("grok-4.5")),
        vec![
            ModelThinkingLevel::Low,
            ModelThinkingLevel::Medium,
            ModelThinkingLevel::High,
        ]
    );
    assert_eq!(xai_model("grok-4.3").api, "openai-completions");
}

#[tokio::test(flavor = "multi_thread")]
async fn uses_responses_with_bearer_auth_and_xai_compatible_request_fields() {
    let captured = capture_request(
        xai_model("grok-4.5"),
        Context {
            system_prompt: Some("You are a careful coding assistant.".to_string()),
            messages: vec![Message::User(UserMessage {
                content: UserContent::Text("hello".to_string()),
                timestamp: 1,
            })],
            ..Context::default()
        },
        OpenAIResponsesOptions {
            session_id: Some("notagent-session-123".to_string()),
            cache_retention: Some(CacheRetention::Long),
            reasoning_effort: Some(ThinkingLevel::Medium),
            ..OpenAIResponsesOptions::default()
        },
        "xai-test-token",
    )
    .await;

    assert_eq!(captured.url, "https://api.x.ai/v1/responses");
    assert_eq!(
        captured.header("authorization"),
        Some("Bearer xai-test-token")
    );
    assert_eq!(captured.header("session_id"), Some("notagent-session-123"));
    assert_eq!(captured.body["model"], "grok-4.5");
    assert_eq!(captured.body["store"], json!(false));
    assert_eq!(captured.body["stream"], json!(true));
    assert_eq!(captured.body["prompt_cache_key"], "notagent-session-123");
    assert_eq!(captured.body["reasoning"]["effort"], "medium");
    assert_eq!(
        captured.body["include"],
        json!(["reasoning.encrypted_content"])
    );
    assert_eq!(captured.body.get("prompt_cache_retention"), None);
    let input = captured.body["input"].as_array().expect("input");
    assert!(
        input.iter().any(|item| {
            item.get("role").and_then(Value::as_str) == Some("developer")
                && item.get("content").and_then(Value::as_str)
                    == Some("You are a careful coding assistant.")
        }),
        "{input:?}"
    );
}
