use std::sync::Arc;

use notagent_ai::api::openai_completions::stream as stream_completions;
use notagent_ai::api::openai_completions_params::OpenAICompletionsOptions;
use notagent_ai::api::openai_responses::stream as stream_responses;
use notagent_ai::model_catalog::get_builtin_model;
use notagent_ai::types::*;
use notagent_ai::utils::fetch::{FetchBody, FetchFn, FetchFuture, FetchRequest, FetchResponse};
use serde_json::{Value, json};

/// Answers every request with one canned non-2xx response.
struct ErrorFetch {
    status: u16,
    status_text: String,
    body: String,
}

impl FetchFn for ErrorFetch {
    fn fetch(&self, _request: FetchRequest) -> FetchFuture {
        let status = self.status;
        let status_text = self.status_text.clone();
        let body = self.body.clone();
        Box::pin(async move {
            Ok(FetchResponse {
                status,
                status_text,
                headers: vec![("content-type".to_string(), "application/json".to_string())],
                body: FetchBody::Bytes(body.into_bytes()),
            })
        })
    }
}

fn request_options(status: u16, body: Value) -> ProviderRequestOptions {
    ProviderRequestOptions {
        api_key: Some("test".to_string()),
        fetch: Some(Arc::new(ErrorFetch {
            status,
            status_text: "Forbidden".to_string(),
            body: body.to_string(),
        })),
        max_retries: Some(0),
        ..Default::default()
    }
}

fn context() -> Context {
    Context {
        messages: vec![Message::User(UserMessage {
            content: UserContent::Text("hello".to_string()),
            timestamp: 1,
        })],
        ..Default::default()
    }
}

#[tokio::test]
async fn openai_completions_surfaces_the_status_and_the_body() {
    let model = get_builtin_model("openai", "gpt-4o-mini").expect("model");
    let mut completions_model = model.clone();
    completions_model.api = "openai-completions".to_string();

    let result = stream_completions(
        completions_model.clone(),
        context(),
        request_options(403, json!({ "error": "blocked by gateway WAF" })),
        OpenAICompletionsOptions::default(),
    )
    .result()
    .await;

    assert_eq!(result.stop_reason, StopReason::Error);
    let message = result.error_message.expect("an error message");
    assert!(message.contains("403"), "{message}");
    assert!(message.contains("blocked by gateway WAF"), "{message}");
    assert_ne!(message, "403 status code (no body)");
}

#[tokio::test]
async fn the_openrouter_metadata_extra_is_not_printed_twice() {
    let model = get_builtin_model("openrouter", "~anthropic/claude-opus-latest").expect("model");
    let result = stream_completions(
        model,
        context(),
        request_options(
            403,
            json!({
                "error": {
                    "message": "Provider returned error",
                    "code": 403,
                    "metadata": { "raw": "upstream WAF blocked policy XYZ" },
                },
            }),
        ),
        OpenAICompletionsOptions::default(),
    )
    .result()
    .await;

    let message = result.error_message.expect("an error message");
    assert!(
        message.contains("upstream WAF blocked policy XYZ"),
        "{message}"
    );
    assert_eq!(
        message.matches("upstream WAF blocked policy XYZ").count(),
        1,
        "{message}"
    );
}

#[tokio::test]
async fn openai_responses_keeps_its_prefix_and_surfaces_the_body() {
    let model = get_builtin_model("openai", "gpt-4o-mini").expect("model");
    let result = stream_responses(
        model,
        context(),
        request_options(403, json!({ "error": "blocked by gateway WAF" })),
        notagent_ai::api::openai_responses::OpenAIResponsesOptions::default(),
    )
    .result()
    .await;

    assert_eq!(result.stop_reason, StopReason::Error);
    let message = result.error_message.expect("an error message");
    assert!(message.contains("403"), "{message}");
    assert!(message.contains("blocked by gateway WAF"), "{message}");
}
