//! End-to-end test of the Anthropic request path against an injected fetch, mirroring
//! how `packages/ai/test/anthropic-sse-parsing.test.ts` injects a fake SDK client.

use std::sync::{Arc, Mutex};

use notagent_ai::api::anthropic_messages::stream;
use notagent_ai::api::anthropic_params::AnthropicOptions;
use notagent_ai::types::*;
use notagent_ai::utils::fetch::{FetchBody, FetchError, FetchFn, FetchRequest, FetchResponse};

fn model() -> Model {
    Model {
        id: "claude-opus-4-5".to_string(),
        name: "Claude Opus 4.5".to_string(),
        api: "anthropic-messages".to_string(),
        provider: "anthropic".to_string(),
        base_url: "https://api.anthropic.com".to_string(),
        reasoning: true,
        thinking_level_map: None,
        input: vec![Modality::Text],
        cost: ModelCost {
            input: 5.0,
            output: 25.0,
            cache_read: 0.5,
            cache_write: 6.25,
            tiers: None,
        },
        context_window: 200_000,
        max_tokens: 64_000,
        sampling_params: None,
        headers: None,
        compat: None,
    }
}

fn context() -> Context {
    Context {
        system_prompt: Some("sys".to_string()),
        messages: vec![Message::User(UserMessage {
            content: UserContent::Text("hello".to_string()),
            timestamp: 1,
        })],
        tools: None,
    }
}

/// Serves a canned SSE body and records the request it received.
struct RecordingFetch {
    status: u16,
    body: String,
    /// Chunk size; small values exercise the incremental decoder.
    chunk_size: usize,
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
        let status = self.status;
        let body = self.body.clone();
        let chunk_size = self.chunk_size;
        Box::pin(async move {
            let (sender, receiver) = tokio::sync::mpsc::channel(64);
            tokio::spawn(async move {
                let bytes = body.into_bytes();
                for chunk in bytes.chunks(chunk_size.max(1)) {
                    if sender.send(Ok(chunk.to_vec())).await.is_err() {
                        break;
                    }
                }
            });
            Ok(FetchResponse {
                status,
                status_text: "OK".to_string(),
                headers: vec![("content-type".to_string(), "text/event-stream".to_string())],
                body: FetchBody::Stream(receiver),
            })
        })
    }
}

fn sse_body() -> String {
    [
        "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"usage\":{\"input_tokens\":12}}}\n",
        "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n",
        "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n",
        "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n",
        "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":5}}\n",
        "event: message_stop\ndata: {\"type\":\"message_stop\"}\n",
    ]
    .join("\n")
}

fn request_with(fetch: Arc<dyn FetchFn>, api_key: &str) -> ProviderRequestOptions {
    ProviderRequestOptions {
        api_key: Some(api_key.to_string()),
        fetch: Some(fetch),
        ..Default::default()
    }
}

#[tokio::test]
async fn streams_a_response_over_the_injected_transport() {
    let seen = Arc::new(Mutex::new(None));
    let fetch = Arc::new(RecordingFetch {
        status: 200,
        body: sse_body(),
        chunk_size: 7,
        seen: Arc::clone(&seen),
    });

    let result = stream(
        model(),
        context(),
        request_with(fetch, "sk-ant-key"),
        AnthropicOptions::default(),
    )
    .result()
    .await;

    assert_eq!(result.stop_reason, StopReason::Stop);
    assert_eq!(
        result.content,
        vec![AssistantContent::Text(TextContent::new("Hello"))]
    );
    assert_eq!(result.usage.input, 12);
    assert_eq!(result.usage.output, 5);
    assert_eq!(result.response_id.as_deref(), Some("msg_1"));

    let request = seen
        .lock()
        .expect("poisoned")
        .clone()
        .expect("a request was sent");
    assert_eq!(request.method, "POST");
    assert_eq!(request.url, "https://api.anthropic.com/v1/messages");
    let header = |name: &str| {
        request
            .headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.clone())
    };
    assert_eq!(header("x-api-key").as_deref(), Some("sk-ant-key"));
    assert_eq!(header("anthropic-version").as_deref(), Some("2023-06-01"));
    assert_eq!(header("content-type").as_deref(), Some("application/json"));

    let body: serde_json::Value =
        serde_json::from_slice(&request.body.expect("body")).expect("the body is JSON");
    assert_eq!(body["model"], serde_json::json!("claude-opus-4-5"));
    assert_eq!(body["stream"], serde_json::json!(true));
}

#[tokio::test]
async fn an_oauth_token_switches_to_bearer_auth() {
    let seen = Arc::new(Mutex::new(None));
    let fetch = Arc::new(RecordingFetch {
        status: 200,
        body: sse_body(),
        chunk_size: 512,
        seen: Arc::clone(&seen),
    });

    stream(
        model(),
        context(),
        request_with(fetch, "sk-ant-oat-token"),
        AnthropicOptions::default(),
    )
    .result()
    .await;

    let request = seen.lock().expect("poisoned").clone().expect("request");
    let header = |name: &str| {
        request
            .headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.clone())
    };
    assert_eq!(
        header("authorization").as_deref(),
        Some("Bearer sk-ant-oat-token")
    );
    assert!(header("x-api-key").is_none());
    assert_eq!(header("user-agent").as_deref(), Some("claude-cli/2.1.75"));

    // The Claude Code identity is the first system block.
    let body: serde_json::Value = serde_json::from_slice(&request.body.expect("body")).unwrap();
    assert_eq!(
        body["system"][0]["text"],
        serde_json::json!("You are Claude Code, Anthropic's official CLI for Claude.")
    );
}

#[tokio::test]
async fn a_missing_api_key_ends_the_stream_with_an_error() {
    let fetch = Arc::new(RecordingFetch {
        status: 200,
        body: sse_body(),
        chunk_size: 512,
        seen: Arc::new(Mutex::new(None)),
    });
    let request = ProviderRequestOptions {
        fetch: Some(fetch),
        ..Default::default()
    };

    let result = stream(model(), context(), request, AnthropicOptions::default())
        .result()
        .await;
    assert_eq!(result.stop_reason, StopReason::Error);
    assert_eq!(
        result.error_message.as_deref(),
        Some("No API key for provider: anthropic")
    );
}

#[tokio::test]
async fn an_http_error_ends_the_stream_with_the_body() {
    let fetch = Arc::new(RecordingFetch {
        status: 429,
        body: "{\"error\":{\"message\":\"rate limited\"}}".to_string(),
        chunk_size: 512,
        seen: Arc::new(Mutex::new(None)),
    });

    let result = stream(
        model(),
        context(),
        request_with(fetch, "key"),
        AnthropicOptions::default(),
    )
    .result()
    .await;
    assert_eq!(result.stop_reason, StopReason::Error);
    let message = result.error_message.expect("error message");
    assert!(message.starts_with("429: "), "{message}");
    assert!(message.contains("rate limited"), "{message}");
}

#[tokio::test]
async fn a_stream_that_ends_before_message_stop_is_an_error() {
    let truncated = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"m\",\"usage\":{}}}\n\n";
    let fetch = Arc::new(RecordingFetch {
        status: 200,
        body: truncated.to_string(),
        chunk_size: 512,
        seen: Arc::new(Mutex::new(None)),
    });

    let result = stream(
        model(),
        context(),
        request_with(fetch, "key"),
        AnthropicOptions::default(),
    )
    .result()
    .await;
    assert_eq!(result.stop_reason, StopReason::Error);
    assert_eq!(
        result.error_message.as_deref(),
        Some("Anthropic stream ended before message_stop")
    );
}

#[tokio::test]
async fn an_error_event_in_the_stream_surfaces_its_payload() {
    let body =
        "event: error\ndata: {\"type\":\"error\",\"error\":{\"message\":\"overloaded\"}}\n\n";
    let fetch = Arc::new(RecordingFetch {
        status: 200,
        body: body.to_string(),
        chunk_size: 512,
        seen: Arc::new(Mutex::new(None)),
    });

    let result = stream(
        model(),
        context(),
        request_with(fetch, "key"),
        AnthropicOptions::default(),
    )
    .result()
    .await;
    assert_eq!(result.stop_reason, StopReason::Error);
    assert!(
        result
            .error_message
            .expect("message")
            .contains("overloaded")
    );
}

#[tokio::test]
async fn on_payload_can_replace_the_request_body() {
    let seen = Arc::new(Mutex::new(None));
    let fetch = Arc::new(RecordingFetch {
        status: 200,
        body: sse_body(),
        chunk_size: 512,
        seen: Arc::clone(&seen),
    });
    let request = ProviderRequestOptions {
        api_key: Some("key".to_string()),
        fetch: Some(fetch),
        on_payload: Some(Arc::new(
            |mut payload: serde_json::Value, _model: &Model| {
                payload["model"] = serde_json::json!("replaced");
                Box::pin(async move { Some(payload) })
            },
        )),
        ..Default::default()
    };

    stream(model(), context(), request, AnthropicOptions::default())
        .result()
        .await;
    let sent = seen.lock().expect("poisoned").clone().expect("request");
    let body: serde_json::Value = serde_json::from_slice(&sent.body.expect("body")).unwrap();
    assert_eq!(body["model"], serde_json::json!("replaced"));
}

// ---------------------------------------------------------------------------
// streamSimple
// ---------------------------------------------------------------------------

use notagent_ai::api::anthropic_messages::stream_simple;

fn simple_options(
    fetch: Arc<dyn FetchFn>,
    reasoning: Option<ThinkingLevel>,
) -> SimpleStreamOptions {
    SimpleStreamOptions {
        base: StreamOptions {
            base: ProviderRequestOptions {
                api_key: Some("key".to_string()),
                fetch: Some(fetch),
                ..Default::default()
            },
            ..Default::default()
        },
        reasoning,
        ..Default::default()
    }
}

#[tokio::test]
async fn stream_simple_disables_thinking_without_a_reasoning_level() {
    let seen = Arc::new(Mutex::new(None));
    let fetch = Arc::new(RecordingFetch {
        status: 200,
        body: sse_body(),
        chunk_size: 512,
        seen: Arc::clone(&seen),
    });

    stream_simple(model(), context(), Some(simple_options(fetch, None)))
        .result()
        .await;

    let sent = seen.lock().expect("poisoned").clone().expect("request");
    let body: serde_json::Value = serde_json::from_slice(&sent.body.expect("body")).unwrap();
    assert_eq!(body["thinking"], serde_json::json!({"type": "disabled"}));
}

#[tokio::test]
async fn stream_simple_maps_a_reasoning_level_to_a_thinking_budget() {
    let seen = Arc::new(Mutex::new(None));
    let fetch = Arc::new(RecordingFetch {
        status: 200,
        body: sse_body(),
        chunk_size: 512,
        seen: Arc::clone(&seen),
    });

    stream_simple(
        model(),
        context(),
        Some(simple_options(fetch, Some(ThinkingLevel::Medium))),
    )
    .result()
    .await;

    let sent = seen.lock().expect("poisoned").clone().expect("request");
    let body: serde_json::Value = serde_json::from_slice(&sent.body.expect("body")).unwrap();
    assert_eq!(body["thinking"]["type"], serde_json::json!("enabled"));
    assert_eq!(
        body["thinking"]["budget_tokens"],
        serde_json::json!(8192),
        "the medium default budget"
    );
    assert_eq!(body["thinking"]["display"], serde_json::json!("summarized"));
}

#[tokio::test]
async fn stream_simple_uses_effort_for_adaptive_thinking_models() {
    let seen = Arc::new(Mutex::new(None));
    let fetch = Arc::new(RecordingFetch {
        status: 200,
        body: sse_body(),
        chunk_size: 512,
        seen: Arc::clone(&seen),
    });
    let adaptive = Model {
        compat: Some(
            ModelCompat::from_api_value(
                "anthropic-messages",
                serde_json::json!({"forceAdaptiveThinking": true}),
            )
            .unwrap(),
        ),
        ..model()
    };

    stream_simple(
        adaptive,
        context(),
        Some(simple_options(fetch, Some(ThinkingLevel::High))),
    )
    .result()
    .await;

    let sent = seen.lock().expect("poisoned").clone().expect("request");
    let body: serde_json::Value = serde_json::from_slice(&sent.body.expect("body")).unwrap();
    assert_eq!(body["thinking"]["type"], serde_json::json!("adaptive"));
    assert_eq!(body["output_config"]["effort"], serde_json::json!("high"));
    assert!(body.get("budget_tokens").is_none());
}
