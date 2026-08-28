use std::sync::{Arc, Mutex};

use notagent_ai::api::anthropic_messages::{stream, stream_simple};
use notagent_ai::api::anthropic_params::AnthropicOptions;
use notagent_ai::model_catalog::{get_builtin_model, get_builtin_models, get_builtin_providers};
use notagent_ai::models::Provider;
use notagent_ai::types::*;
use notagent_ai::utils::fetch::{FetchBody, FetchFn, FetchFuture, FetchRequest, FetchResponse};
use serde_json::{Value, json};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Records the request and answers with `body` (empty ends the stream right away).
struct RecordingFetch {
    seen: Arc<Mutex<Option<FetchRequest>>>,
    body: String,
}

impl FetchFn for RecordingFetch {
    fn fetch(&self, request: FetchRequest) -> FetchFuture {
        *self.seen.lock().expect("poisoned") = Some(request);
        let body = self.body.clone();
        Box::pin(async move {
            Ok(FetchResponse {
                status: 200,
                status_text: "OK".to_string(),
                headers: vec![("content-type".to_string(), "text/event-stream".to_string())],
                body: FetchBody::Bytes(body.into_bytes()),
            })
        })
    }
}

fn request_options(seen: &Arc<Mutex<Option<FetchRequest>>>, body: &str) -> ProviderRequestOptions {
    ProviderRequestOptions {
        api_key: Some("fake-key".to_string()),
        fetch: Some(Arc::new(RecordingFetch {
            seen: seen.clone(),
            body: body.to_string(),
        })),
        max_retries: Some(0),
        ..Default::default()
    }
}

fn user_context() -> Context {
    Context {
        messages: vec![Message::User(UserMessage {
            content: UserContent::Text("Hello".to_string()),
            timestamp: 1,
        })],
        ..Default::default()
    }
}

async fn capture_payload(
    model: &Model,
    context: Context,
    reasoning: Option<ThinkingLevel>,
    temperature: Option<f64>,
) -> Value {
    let seen = Arc::new(Mutex::new(None));
    let options = SimpleStreamOptions {
        base: StreamOptions {
            temperature,
            base: request_options(&seen, ""),
            ..Default::default()
        },
        reasoning,
        ..Default::default()
    };
    stream_simple(model.clone(), context, Some(options))
        .result()
        .await;
    let request = seen.lock().expect("poisoned").take().expect("a request");
    serde_json::from_slice(request.body.as_deref().expect("a body")).expect("json body")
}

fn custom_model(id: &str, compat: Option<Value>) -> Model {
    Model {
        id: id.to_string(),
        name: "Vendor Proxy".to_string(),
        api: "anthropic-messages".to_string(),
        provider: "vendor-proxy".to_string(),
        base_url: "http://127.0.0.1:9".to_string(),
        reasoning: true,
        thinking_level_map: None,
        input: vec![Modality::Text],
        cost: ModelCost::default(),
        context_window: 200_000,
        max_tokens: 32_000,
        sampling_params: None,
        headers: None,
        compat: compat.map(|compat| {
            ModelCompat::from_api_value("anthropic-messages", compat).expect("anthropic compat")
        }),
    }
}

// ---------------------------------------------------------------------------
// Adaptive thinking catalog metadata
// ---------------------------------------------------------------------------

#[test]
fn marks_the_builtin_models_that_use_adaptive_thinking() {
    let expected = [
        "anthropic/claude-fable-5",
        "anthropic/claude-opus-4-8",
        "anthropic/claude-opus-5",
        "anthropic/claude-sonnet-5",
        "cloudflare-ai-gateway/claude-fable-5",
        "kimi-coding/kimi-for-coding",
        "kimi-coding/k3",
        "kimi-coding/kimi-for-coding-highspeed",
        "opencode/claude-opus-4-8",
        "opencode/claude-opus-5",
        "vercel-ai-gateway/anthropic/claude-opus-4.8",
        "vercel-ai-gateway/anthropic/claude-opus-5",
        "vercel-ai-gateway/anthropic/claude-sonnet-5",
    ];

    let mut flagged: Vec<String> = get_builtin_providers()
        .into_iter()
        .flat_map(get_builtin_models)
        .filter(|model| model.api == "anthropic-messages")
        .filter(|model| {
            model
                .compat
                .as_ref()
                .and_then(|compat| compat.as_anthropic_messages())
                .and_then(|compat| compat.force_adaptive_thinking)
                == Some(true)
        })
        .map(|model| format!("{}/{}", model.provider, model.id))
        .collect();
    flagged.sort();

    for model_id in expected {
        assert!(flagged.contains(&model_id.to_string()), "{model_id}");
    }
    // No model outside the adaptive families carries the flag.
    let pattern = regex::Regex::new(
        r"(opus[-.](4[-.][678]|5)|sonnet[-.]4[-.]6|sonnet[-.]5|fable[-.]5|kimi-coding/)",
    )
    .expect("valid pattern");
    for model_id in &flagged {
        assert!(
            pattern.is_match(model_id),
            "{model_id} is an adaptive family member"
        );
    }
}

// ---------------------------------------------------------------------------
// Temperature
// ---------------------------------------------------------------------------

#[tokio::test]
async fn omits_temperature_for_models_that_do_not_support_it() {
    for (model_id, temperature) in [
        ("claude-opus-4-7", 0.0),
        ("claude-opus-4-8", 0.0),
        ("claude-opus-4-7", 1.0),
    ] {
        let model = get_builtin_model("anthropic", model_id).expect(model_id);
        let payload = capture_payload(&model, user_context(), None, Some(temperature)).await;
        assert_eq!(payload.get("temperature"), None, "{model_id}/{temperature}");
    }

    for model_id in ["claude-opus-4-6", "claude-sonnet-4-6"] {
        let model = get_builtin_model("anthropic", model_id).expect(model_id);
        let payload = capture_payload(&model, user_context(), None, Some(0.0)).await;
        assert_eq!(payload["temperature"], json!(0.0), "{model_id}");
    }

    let model = custom_model(
        "vendor--claude-opus-4-7",
        Some(json!({ "supportsTemperature": false })),
    );
    let payload = capture_payload(&model, user_context(), None, Some(0.0)).await;
    assert_eq!(payload.get("temperature"), None);
}

// ---------------------------------------------------------------------------
// forceAdaptiveThinking
// ---------------------------------------------------------------------------

#[tokio::test]
async fn force_adaptive_thinking_overrides_the_payload_shape() {
    // Custom ids keep the legacy payload by default.
    let payload = capture_payload(
        &custom_model("vendor--claude-opus-latest", None),
        user_context(),
        Some(ThinkingLevel::Medium),
        None,
    )
    .await;
    assert_eq!(payload["thinking"]["type"], json!("enabled"));
    assert_eq!(payload.get("output_config"), None);

    let payload = capture_payload(
        &custom_model(
            "vendor--claude-opus-latest",
            Some(json!({ "forceAdaptiveThinking": true })),
        ),
        user_context(),
        Some(ThinkingLevel::Medium),
        None,
    )
    .await;
    assert_eq!(
        payload["thinking"],
        json!({ "type": "adaptive", "display": "summarized" })
    );
    assert_eq!(payload["output_config"], json!({ "effort": "medium" }));

    // Fable 5 uses native xhigh effort.
    let fable = get_builtin_model("anthropic", "claude-fable-5").expect("fable");
    let payload = capture_payload(&fable, user_context(), Some(ThinkingLevel::Xhigh), None).await;
    assert_eq!(
        payload["thinking"],
        json!({ "type": "adaptive", "display": "summarized" })
    );
    assert_eq!(payload["output_config"], json!({ "effort": "xhigh" }));

    for (model_id, reasoning, effort) in [
        ("kimi-for-coding", ThinkingLevel::Medium, "medium"),
        ("k3", ThinkingLevel::Max, "max"),
        ("kimi-for-coding-highspeed", ThinkingLevel::Medium, "medium"),
    ] {
        let model = get_builtin_model("kimi-coding", model_id).expect(model_id);
        let payload = capture_payload(&model, user_context(), Some(reasoning), None).await;
        assert_eq!(
            payload["thinking"],
            json!({ "type": "adaptive", "display": "summarized" }),
            "{model_id}"
        );
        assert_eq!(
            payload["output_config"],
            json!({ "effort": effort }),
            "{model_id}"
        );
    }

    // A built-in adaptive model can opt out.
    let mut opus = get_builtin_model("anthropic", "claude-opus-4-8").expect("opus");
    opus.compat = Some(
        ModelCompat::from_api_value(
            "anthropic-messages",
            json!({ "forceAdaptiveThinking": false }),
        )
        .expect("anthropic compat"),
    );
    let payload = capture_payload(&opus, user_context(), Some(ThinkingLevel::Medium), None).await;
    assert_eq!(payload["thinking"]["type"], json!("enabled"));
    assert_eq!(payload.get("output_config"), None);

    // Reasoning off keeps `thinking.type = disabled` regardless of the override.
    let payload = capture_payload(
        &custom_model(
            "vendor--claude-opus-latest",
            Some(json!({ "forceAdaptiveThinking": true })),
        ),
        user_context(),
        None,
        None,
    )
    .await;
    assert_eq!(payload["thinking"], json!({ "type": "disabled" }));
    assert_eq!(payload.get("output_config"), None);
}

// ---------------------------------------------------------------------------
// allowEmptySignature
// ---------------------------------------------------------------------------

fn thinking_context(
    thinking_signature: &str,
    thinking: &str,
    provider: &str,
    model: &str,
) -> Context {
    Context {
        messages: vec![
            Message::User(UserMessage {
                content: UserContent::Text("first".to_string()),
                timestamp: 1,
            }),
            Message::Assistant(AssistantMessage {
                content: vec![AssistantContent::Thinking(ThinkingContent {
                    thinking: thinking.to_string(),
                    thinking_signature: Some(thinking_signature.to_string()),
                    redacted: None,
                    extra: Default::default(),
                })],
                api: "anthropic-messages".to_string(),
                provider: provider.to_string(),
                model: model.to_string(),
                response_model: None,
                response_id: None,
                diagnostics: None,
                usage: Usage::default(),
                stop_reason: StopReason::Stop,
                deferred: None,
                error_message: None,
                raw_stop_reason: None,
                end_turn: None,
                timestamp: 1,
            }),
            Message::User(UserMessage {
                content: UserContent::Text("second".to_string()),
                timestamp: 1,
            }),
        ],
        ..Default::default()
    }
}

fn signature_model(allow_empty_signature: Option<bool>) -> Model {
    Model {
        id: "mimo-v2.5-pro".to_string(),
        name: "MiMo".to_string(),
        api: "anthropic-messages".to_string(),
        provider: "xiaomi-token-plan-ams".to_string(),
        base_url: "http://127.0.0.1:9".to_string(),
        reasoning: true,
        thinking_level_map: None,
        input: vec![Modality::Text],
        cost: ModelCost::default(),
        context_window: 128_000,
        max_tokens: 1024,
        sampling_params: None,
        headers: None,
        compat: allow_empty_signature.map(|allow| {
            ModelCompat::from_api_value(
                "anthropic-messages",
                json!({ "allowEmptySignature": allow }),
            )
            .expect("compat")
        }),
    }
}

fn assistant_content(payload: &Value) -> Value {
    payload["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .find(|message| message["role"] == json!("assistant"))
        .expect("an assistant message")["content"]
        .clone()
}

#[tokio::test]
async fn empty_thinking_signatures_become_text_unless_allowed() {
    let payload = capture_payload(
        &signature_model(None),
        thinking_context(
            "",
            "internal reasoning",
            "xiaomi-token-plan-ams",
            "mimo-v2.5-pro",
        ),
        None,
        None,
    )
    .await;
    assert_eq!(
        assistant_content(&payload),
        json!([{ "type": "text", "text": "internal reasoning" }])
    );

    let payload = capture_payload(
        &signature_model(None),
        thinking_context(
            "signed-thinking",
            "",
            "xiaomi-token-plan-ams",
            "mimo-v2.5-pro",
        ),
        None,
        None,
    )
    .await;
    assert_eq!(
        assistant_content(&payload),
        json!([{ "type": "thinking", "thinking": "", "signature": "signed-thinking" }])
    );

    let payload = capture_payload(
        &signature_model(Some(true)),
        thinking_context(
            " ",
            "internal reasoning",
            "xiaomi-token-plan-ams",
            "mimo-v2.5-pro",
        ),
        None,
        None,
    )
    .await;
    assert_eq!(
        assistant_content(&payload),
        json!([{ "type": "thinking", "thinking": "internal reasoning", "signature": "" }])
    );

    let model = get_builtin_model("kimi-coding", "k3").expect("k3");
    assert_eq!(
        model
            .compat
            .as_ref()
            .and_then(|compat| compat.as_anthropic_messages())
            .and_then(|compat| compat.allow_empty_signature),
        Some(true)
    );
    let payload = capture_payload(
        &model,
        thinking_context(" ", "internal reasoning", "kimi-coding", "k3"),
        None,
        None,
    )
    .await;
    assert_eq!(
        assistant_content(&payload),
        json!([{ "type": "thinking", "thinking": "internal reasoning", "signature": "" }])
    );
}

// ---------------------------------------------------------------------------
// Eager tool input streaming
// ---------------------------------------------------------------------------

fn tool(name: &str, parameters: Value, constrained: Option<Value>) -> Tool {
    Tool {
        name: name.to_string(),
        description: "Look up a value".to_string(),
        parameters,
        constrained_sampling: constrained
            .map(|value| serde_json::from_value(value).expect("constrained sampling")),
    }
}

fn tool_context(tools: Vec<Tool>) -> Context {
    Context {
        messages: vec![Message::User(UserMessage {
            content: UserContent::Text("Use the tool".to_string()),
            timestamp: 1,
        })],
        tools: (!tools.is_empty()).then_some(tools),
        ..Default::default()
    }
}

/// the injected fetch observes the same request.
async fn capture_anthropic_request(compat: Option<Value>, context: Context) -> FetchRequest {
    let mut model = custom_model("claude-opus-4-8", None);
    model.provider = "test-anthropic".to_string();
    model.name = "Claude Opus 4.8".to_string();
    let mut merged = json!({ "forceAdaptiveThinking": true });
    if let Some(compat) = compat {
        for (key, value) in compat.as_object().expect("compat object") {
            merged[key] = value.clone();
        }
    }
    model.compat =
        Some(ModelCompat::from_api_value("anthropic-messages", merged).expect("anthropic compat"));

    let seen = Arc::new(Mutex::new(None));
    stream(
        model,
        context,
        request_options(&seen, ""),
        AnthropicOptions {
            cache_retention: Some(CacheRetention::None),
            ..Default::default()
        },
    )
    .result()
    .await;
    seen.lock().expect("poisoned").take().expect("a request")
}

fn body_json(request: &FetchRequest) -> Value {
    serde_json::from_slice(request.body.as_deref().expect("a body")).expect("json body")
}

fn header(request: &FetchRequest, name: &str) -> Option<String> {
    request
        .headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.clone())
}

#[tokio::test]
async fn sends_per_tool_eager_input_streaming_by_default() {
    let lookup = tool(
        "lookup",
        json!({ "type": "object", "properties": { "value": { "type": "string" } }, "required": ["value"] }),
        None,
    );
    let request = capture_anthropic_request(None, tool_context(vec![lookup.clone()])).await;
    let body = body_json(&request);
    assert_eq!(body["tools"][0]["eager_input_streaming"], json!(true));
    assert_eq!(header(&request, "anthropic-beta"), None);

    let request = capture_anthropic_request(
        Some(json!({ "supportsEagerToolInputStreaming": false })),
        tool_context(vec![lookup]),
    )
    .await;
    let body = body_json(&request);
    assert_eq!(body["tools"][0].get("eager_input_streaming"), None);
    assert_eq!(
        header(&request, "anthropic-beta").as_deref(),
        Some("fine-grained-tool-streaming-2025-05-14")
    );

    // Without tools the legacy beta header stays off.
    let request = capture_anthropic_request(
        Some(json!({ "supportsEagerToolInputStreaming": false })),
        tool_context(vec![]),
    )
    .await;
    assert_eq!(body_json(&request).get("tools"), None);
    assert_eq!(header(&request, "anthropic-beta"), None);
}

#[tokio::test]
async fn sends_the_full_input_schema_only_for_strict_json_schema_tools() {
    let compatibility_tool = tool(
        "lookup",
        json!({
            "type": "object",
            "properties": { "value": { "type": "string" } },
            "required": ["value"],
            "additionalProperties": false,
            "title": "LookupInput",
        }),
        None,
    );
    let request = capture_anthropic_request(
        Some(json!({ "supportsStrictTools": true })),
        tool_context(vec![compatibility_tool]),
    )
    .await;
    assert_eq!(
        body_json(&request)["tools"][0]["input_schema"],
        json!({
            "type": "object",
            "properties": { "value": { "type": "string" } },
            "required": ["value"],
        })
    );

    let strict_tool = tool(
        "lookup",
        json!({
            "type": "object",
            "properties": { "value": { "type": "string" }, "optional": { "type": "number" } },
            "required": ["value"],
            "title": "StrictLookupInput",
        }),
        Some(json!({ "type": "json_schema", "strict": "prefer" })),
    );
    let request = capture_anthropic_request(
        Some(json!({ "supportsStrictTools": true })),
        tool_context(vec![strict_tool]),
    )
    .await;
    let body = body_json(&request);
    assert_eq!(body["tools"][0]["strict"], json!(true));
    let schema = &body["tools"][0]["input_schema"];
    assert_eq!(schema["additionalProperties"], json!(false));
    assert_eq!(schema["required"], json!(["value", "optional"]));
    assert_eq!(
        schema["properties"]["optional"],
        json!({ "anyOf": [{ "type": "number" }, { "type": "null" }] })
    );
    assert_eq!(schema["title"], json!("StrictLookupInput"));
}

// ---------------------------------------------------------------------------
// 1h cache write cost
// ---------------------------------------------------------------------------

fn cache_creation_sse(cache_creation: Option<Value>) -> String {
    let mut start_usage = json!({
        "input_tokens": 100,
        "output_tokens": 0,
        "cache_read_input_tokens": 0,
        "cache_creation_input_tokens": 1_000_000,
    });
    if let Some(cache_creation) = cache_creation {
        start_usage["cache_creation"] = cache_creation;
    }
    let events = [
        (
            "message_start",
            json!({ "type": "message_start", "message": { "id": "msg_test", "usage": start_usage } }),
        ),
        (
            "content_block_start",
            json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "text", "text": "" } }),
        ),
        (
            "content_block_delta",
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": "Hi" } }),
        ),
        (
            "content_block_stop",
            json!({ "type": "content_block_stop", "index": 0 }),
        ),
        (
            "message_delta",
            json!({
                "type": "message_delta",
                "delta": { "stop_reason": "end_turn" },
                "usage": {
                    "input_tokens": 100,
                    "output_tokens": 5,
                    "cache_read_input_tokens": 0,
                    "cache_creation_input_tokens": 1_000_000,
                },
            }),
        ),
        ("message_stop", json!({ "type": "message_stop" })),
    ];
    events
        .iter()
        .map(|(name, data)| format!("event: {name}\ndata: {data}\n"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn prices_the_1h_cache_write_portion_at_twice_the_input_rate() {
    let model = get_builtin_model("anthropic", "claude-opus-4-8").expect("model");
    let context = Context {
        messages: vec![Message::User(UserMessage {
            content: UserContent::Text("hi".to_string()),
            timestamp: 1,
        })],
        ..Default::default()
    };

    let seen = Arc::new(Mutex::new(None));
    let body = cache_creation_sse(Some(
        json!({ "ephemeral_5m_input_tokens": 600_000, "ephemeral_1h_input_tokens": 400_000 }),
    ));
    let result = stream(
        model.clone(),
        context.clone(),
        request_options(&seen, &body),
        AnthropicOptions::default(),
    )
    .result()
    .await;
    assert_eq!(result.usage.cache_write, 1_000_000);
    assert_eq!(result.usage.cache_write1h, Some(400_000));
    // 600k * 6.25/Mtok + 400k * 10/Mtok = 3.75 + 4.0 = 7.75
    assert!((result.usage.cost.cache_write - 7.75).abs() < 1e-10);

    let seen = Arc::new(Mutex::new(None));
    let body = cache_creation_sse(None);
    let result = stream(
        model,
        context,
        request_options(&seen, &body),
        AnthropicOptions::default(),
    )
    .result()
    .await;
    assert_eq!(result.usage.cache_write, 1_000_000);
    assert_eq!(result.usage.cache_write1h.unwrap_or(0), 0);
    assert!((result.usage.cost.cache_write - 6.25).abs() < 1e-10);
}

// ---------------------------------------------------------------------------
// ANTHROPIC_AUTH_TOKEN / ANTHROPIC_OAUTH_TOKEN
//
// SDK client's constructor options; here the request headers the adapter builds are the
// equivalent observation point.
// ---------------------------------------------------------------------------

struct EnvAuthContext {
    env: ProviderEnv,
}

impl notagent_ai::auth::types::AuthContext for EnvAuthContext {
    fn env(&self, name: &str) -> notagent_ai::auth::types::BoxFuture<'_, Option<String>> {
        let value = self.env.get(name).cloned();
        Box::pin(async move { value })
    }

    fn file_exists(&self, _path: &str) -> notagent_ai::auth::types::BoxFuture<'_, bool> {
        Box::pin(async { false })
    }
}

fn auth_context(pairs: &[(&str, &str)]) -> Arc<dyn notagent_ai::auth::types::AuthContext> {
    Arc::new(EnvAuthContext {
        env: pairs
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect(),
    })
}

fn plain_anthropic_model() -> Model {
    Model {
        id: "claude-test".to_string(),
        name: "Claude Test".to_string(),
        api: "anthropic-messages".to_string(),
        provider: "anthropic".to_string(),
        base_url: "https://api.anthropic.com".to_string(),
        reasoning: false,
        thinking_level_map: None,
        input: vec![Modality::Text],
        cost: ModelCost::default(),
        context_window: 100_000,
        max_tokens: 4096,
        sampling_params: None,
        headers: None,
        compat: None,
    }
}

fn system_context() -> Context {
    Context {
        system_prompt: Some("System prompt.".to_string()),
        messages: vec![Message::User(UserMessage {
            content: UserContent::Text("Hello".to_string()),
            timestamp: 1,
        })],
        ..Default::default()
    }
}

#[tokio::test]
async fn resolves_the_auth_token_env_as_a_bearer_header() {
    use notagent_ai::auth::types::{ApiKeyAuthInput, ModelAuth};
    use notagent_ai::env_api_keys::{ANTHROPIC_AUTH_TOKEN_ENV, ANTHROPIC_OAUTH_TOKEN_ENV};

    let provider = notagent_ai::providers::anthropic::anthropic_provider();
    let auth = provider.auth().api_key.clone().expect("api key auth");

    let ctx = auth_context(&[
        ("ANTHROPIC_AUTH_TOKEN", "auth-token"),
        ("ANTHROPIC_OAUTH_TOKEN", "oauth-token"),
        ("ANTHROPIC_API_KEY", "api-key"),
    ]);
    let resolved = auth
        .resolve(ApiKeyAuthInput {
            ctx: ctx.as_ref(),
            credential: None,
            signal: tokio_util::sync::CancellationToken::new(),
        })
        .await
        .expect("resolve")
        .expect("configured");
    assert_eq!(
        resolved.auth,
        ModelAuth {
            headers: Some(
                [(
                    "Authorization".to_string(),
                    Some("Bearer auth-token".to_string())
                )]
                .into()
            ),
            ..Default::default()
        }
    );
    assert_eq!(resolved.env, None);
    assert_eq!(resolved.source.as_deref(), Some(ANTHROPIC_AUTH_TOKEN_ENV));

    let ctx = auth_context(&[
        ("ANTHROPIC_OAUTH_TOKEN", "oauth-token"),
        ("ANTHROPIC_API_KEY", "api-key"),
    ]);
    let resolved = auth
        .resolve(ApiKeyAuthInput {
            ctx: ctx.as_ref(),
            credential: None,
            signal: tokio_util::sync::CancellationToken::new(),
        })
        .await
        .expect("resolve")
        .expect("configured");
    assert_eq!(
        resolved.auth,
        ModelAuth {
            api_key: Some("oauth-token".to_string()),
            ..Default::default()
        }
    );
    assert_eq!(resolved.source.as_deref(), Some(ANTHROPIC_OAUTH_TOKEN_ENV));
}

#[tokio::test]
async fn an_authorization_header_skips_the_oauth_request_shaping() {
    let seen = Arc::new(Mutex::new(None));
    let mut request = request_options(&seen, "");
    request.api_key = None;
    request.headers = Some(
        [(
            "Authorization".to_string(),
            Some("Bearer gateway-token".to_string()),
        )]
        .into(),
    );
    stream(
        plain_anthropic_model(),
        system_context(),
        request,
        AnthropicOptions::default(),
    )
    .result()
    .await;

    let request = seen.lock().expect("poisoned").take().expect("a request");
    assert_eq!(
        header(&request, "authorization").as_deref(),
        Some("Bearer gateway-token")
    );
    assert_eq!(header(&request, "x-api-key"), None);
    assert!(
        !header(&request, "anthropic-beta")
            .unwrap_or_default()
            .contains("oauth-2025-04-20")
    );
    let body = body_json(&request);
    assert_eq!(body["system"][0]["text"], json!("System prompt."));
}

/// Both env vars reach the request through `Models`, and an explicit request header wins.
#[tokio::test]
async fn threads_the_env_tokens_through_models_into_the_request() {
    use notagent_ai::models::{CreateModelsOptions, create_models};

    for (env_var, token, expects_oauth) in [
        ("ANTHROPIC_AUTH_TOKEN", "ctx-token", false),
        ("ANTHROPIC_OAUTH_TOKEN", "sk-ant-oat-test", true),
    ] {
        let models = create_models(Some(CreateModelsOptions {
            auth_context: Some(auth_context(&[(env_var, token)])),
            ..Default::default()
        }));
        models.set_provider(notagent_ai::providers::anthropic::anthropic_provider());

        let seen = Arc::new(Mutex::new(None));
        let options = SimpleStreamOptions {
            base: StreamOptions {
                base: ProviderRequestOptions {
                    fetch: Some(Arc::new(RecordingFetch {
                        seen: seen.clone(),
                        body: String::new(),
                    })),
                    max_retries: Some(0),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };
        models
            .stream_simple(plain_anthropic_model(), system_context(), Some(options))
            .result()
            .await;

        let request = seen.lock().expect("poisoned").take().expect("a request");
        let beta = header(&request, "anthropic-beta").unwrap_or_default();
        if expects_oauth {
            assert_eq!(
                header(&request, "authorization").as_deref(),
                Some(format!("Bearer {token}").as_str()),
                "{env_var}"
            );
            assert!(beta.contains("oauth-2025-04-20"), "{env_var}: {beta}");
        } else {
            assert_eq!(
                header(&request, "authorization").as_deref(),
                Some("Bearer ctx-token")
            );
            assert_eq!(header(&request, "x-api-key"), None);
            assert!(!beta.contains("oauth-2025-04-20"), "{env_var}: {beta}");
            assert_eq!(
                body_json(&request)["system"][0]["text"],
                json!("System prompt.")
            );
        }
    }

    // An explicit request header overrides the env token.
    let models = create_models(Some(CreateModelsOptions {
        auth_context: Some(auth_context(&[("ANTHROPIC_AUTH_TOKEN", "ctx-token")])),
        ..Default::default()
    }));
    models.set_provider(notagent_ai::providers::anthropic::anthropic_provider());
    let seen = Arc::new(Mutex::new(None));
    let options = SimpleStreamOptions {
        base: StreamOptions {
            base: ProviderRequestOptions {
                fetch: Some(Arc::new(RecordingFetch {
                    seen: seen.clone(),
                    body: String::new(),
                })),
                headers: Some(
                    [(
                        "Authorization".to_string(),
                        Some("Bearer explicit-token".to_string()),
                    )]
                    .into(),
                ),
                max_retries: Some(0),
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    };
    models
        .stream_simple(plain_anthropic_model(), system_context(), Some(options))
        .result()
        .await;
    let request = seen.lock().expect("poisoned").take().expect("a request");
    assert_eq!(
        header(&request, "authorization").as_deref(),
        Some("Bearer explicit-token")
    );
}

// ---------------------------------------------------------------------------
// GitHub Copilot through the Anthropic Messages API
//
// ---------------------------------------------------------------------------

#[test]
fn copilot_claude_models_carry_the_effort_overrides() {
    use notagent_ai::models::get_supported_thinking_levels;

    for (model_id, expects_xhigh) in [("claude-opus-4.7", true), ("claude-opus-5", true)] {
        let model = get_builtin_model("github-copilot", model_id).expect(model_id);
        let levels = serde_json::to_value(model.thinking_level_map.clone()).unwrap();
        assert_eq!(levels["minimal"], json!("low"), "{model_id}");
        assert_eq!(levels["xhigh"], json!("xhigh"), "{model_id}");
        assert_eq!(levels["max"], json!("max"), "{model_id}");
        let supported = get_supported_thinking_levels(&model);
        assert_eq!(
            supported.contains(&ModelThinkingLevel::Xhigh),
            expects_xhigh,
            "{model_id}"
        );
        assert!(supported.contains(&ModelThinkingLevel::Max), "{model_id}");
        if model_id == "claude-opus-5" {
            assert_eq!(model.api, "anthropic-messages");
            assert_eq!(model.context_window, 1_000_000);
        }
    }

    let sonnet = get_builtin_model("github-copilot", "claude-sonnet-4.6").expect("sonnet");
    let levels = serde_json::to_value(sonnet.thinking_level_map.clone()).unwrap();
    assert_eq!(levels["minimal"], json!("low"));
    assert_eq!(levels["max"], json!("max"));
    let supported = get_supported_thinking_levels(&sonnet);
    assert!(supported.contains(&ModelThinkingLevel::Max));
    assert!(!supported.contains(&ModelThinkingLevel::Xhigh));
}

#[tokio::test]
async fn copilot_uses_bearer_auth_its_headers_and_a_valid_payload() {
    let model = get_builtin_model("github-copilot", "claude-sonnet-4.6").expect("model");
    assert_eq!(model.api, "anthropic-messages");

    let seen = Arc::new(Mutex::new(None));
    let mut request = request_options(&seen, "");
    request.api_key = Some("tid_copilot_session_test_token".to_string());
    stream(
        model.clone(),
        system_context(),
        request,
        AnthropicOptions::default(),
    )
    .result()
    .await;
    let request = seen.lock().expect("poisoned").take().expect("a request");

    // Bearer auth instead of the `x-api-key` header.
    assert_eq!(
        header(&request, "authorization").as_deref(),
        Some("Bearer tid_copilot_session_test_token")
    );
    assert_eq!(header(&request, "x-api-key"), None);
    // Static Copilot headers off the model plus the dynamic ones.
    assert!(
        header(&request, "user-agent")
            .unwrap_or_default()
            .contains("GitHubCopilotChat")
    );
    assert_eq!(
        header(&request, "copilot-integration-id").as_deref(),
        Some("vscode-chat")
    );
    assert_eq!(header(&request, "x-initiator").as_deref(), Some("user"));
    assert_eq!(
        header(&request, "openai-intent").as_deref(),
        Some("conversation-edits")
    );
    // Copilot supports neither the fine-grained tool streaming beta …
    let beta = header(&request, "anthropic-beta").unwrap_or_default();
    assert!(!beta.contains("fine-grained-tool-streaming"), "{beta}");

    let body = body_json(&request);
    assert_eq!(body["model"], json!("claude-sonnet-4.6"));
    assert_eq!(body["stream"], json!(true));
    assert_eq!(body["max_tokens"], json!(model.max_tokens));
    assert!(body["messages"].is_array());
}

#[tokio::test]
async fn copilot_adaptive_models_omit_the_interleaved_thinking_beta() {
    let model = get_builtin_model("github-copilot", "claude-sonnet-4.6").expect("model");
    let seen = Arc::new(Mutex::new(None));
    let mut request = request_options(&seen, "");
    request.api_key = Some("tid_copilot_session_test_token".to_string());
    stream(
        model,
        system_context(),
        request,
        AnthropicOptions {
            interleaved_thinking: Some(true),
            ..Default::default()
        },
    )
    .result()
    .await;

    let request = seen.lock().expect("poisoned").take().expect("a request");
    let beta = header(&request, "anthropic-beta").unwrap_or_default();
    assert!(!beta.contains("interleaved-thinking-2025-05-14"), "{beta}");
}
