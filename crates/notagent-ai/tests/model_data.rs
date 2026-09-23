use std::sync::{Arc, Mutex};

use notagent_ai::api::streams::{AnthropicMessagesApi, OpenAICompletionsApi};
use notagent_ai::env_api_keys::{find_env_keys, get_env_api_key};
use notagent_ai::model_catalog::{get_builtin_model, get_builtin_models};
use notagent_ai::models::get_supported_thinking_levels;
use notagent_ai::types::*;
use notagent_ai::utils::fetch::{FetchFn, FetchFuture, FetchRequest, FetchResponse};
use serde_json::{Value, json};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Captures the request and answers with an empty SSE body, which ends the stream
/// effect.
struct CapturingFetch {
    seen: Arc<Mutex<Option<FetchRequest>>>,
}

impl FetchFn for CapturingFetch {
    fn fetch(&self, request: FetchRequest) -> FetchFuture {
        *self.seen.lock().expect("poisoned") = Some(request);
        Box::pin(async move {
            Ok(FetchResponse {
                status: 200,
                status_text: "OK".to_string(),
                headers: vec![("content-type".to_string(), "text/event-stream".to_string())],
                body: notagent_ai::utils::fetch::FetchBody::Bytes(Vec::new()),
            })
        })
    }
}

/// Streams one turn against [`CapturingFetch`] and returns the request it captured.
async fn capture_request(
    api: &dyn ProviderStreams,
    model: &Model,
    reasoning: Option<ThinkingLevel>,
    cache_retention: Option<CacheRetention>,
    session_id: Option<&str>,
) -> FetchRequest {
    let seen = Arc::new(Mutex::new(None));
    let options = SimpleStreamOptions {
        base: StreamOptions {
            cache_retention,
            session_id: session_id.map(str::to_string),
            base: ProviderRequestOptions {
                api_key: Some("test-key".to_string()),
                fetch: Some(Arc::new(CapturingFetch { seen: seen.clone() })),
                max_retries: Some(0),
                ..Default::default()
            },
            ..Default::default()
        },
        reasoning,
        ..Default::default()
    };
    api.stream_simple(model, &context(), Some(options))
        .result()
        .await;
    seen.lock().expect("poisoned").take().expect("a request")
}

fn body_json(request: &FetchRequest) -> Value {
    serde_json::from_slice(request.body.as_deref().expect("a body")).expect("json body")
}

fn context() -> Context {
    Context {
        messages: vec![Message::User(UserMessage {
            content: UserContent::Text("test".to_string()),
            timestamp: 0,
        })],
        ..Default::default()
    }
}

fn model_ids(provider: &str) -> Vec<String> {
    get_builtin_models(provider)
        .into_iter()
        .map(|model| model.id)
        .collect()
}

fn env(pairs: &[(&str, &str)]) -> ProviderEnv {
    pairs
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
}

// ---------------------------------------------------------------------------
// DeepSeek
// ---------------------------------------------------------------------------

#[tokio::test]
async fn deepseek_flash_sends_the_native_model_id_and_supported_effort_parameters() {
    let model = get_builtin_model("deepseek", "deepseek-flash").expect("DeepSeek V4.1 Flash");
    assert_eq!(
        get_supported_thinking_levels(&model),
        vec![
            ModelThinkingLevel::Off,
            ModelThinkingLevel::Low,
            ModelThinkingLevel::High,
            ModelThinkingLevel::Max,
        ]
    );
    for (reasoning, expected_effort) in [
        (None, None),
        (Some(ThinkingLevel::Low), Some("low")),
        (Some(ThinkingLevel::High), Some("high")),
        (Some(ThinkingLevel::Max), Some("max")),
    ] {
        let request = capture_request(&OpenAICompletionsApi, &model, reasoning, None, None).await;
        assert_eq!(request.url, "https://api.deepseek.com/chat/completions");
        let body = body_json(&request);
        assert_eq!(body["model"], "deepseek-flash");
        assert_eq!(
            body["thinking"]["type"],
            if reasoning.is_some() {
                "enabled"
            } else {
                "disabled"
            }
        );
        assert_eq!(
            body.get("reasoning_effort").and_then(Value::as_str),
            expected_effort
        );
        assert!(
            body.get("max_tokens").is_some(),
            "DeepSeek requires max_tokens: {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Xiaomi
// ---------------------------------------------------------------------------

#[test]
fn xiaomi_keeps_api_billing_models_off_the_token_plans() {
    for model_id in ["mimo-v2-flash", "mimo-v2-omni"] {
        assert!(
            get_builtin_model("xiaomi", model_id).is_some(),
            "{model_id}"
        );
    }
    for provider in [
        "xiaomi-token-plan-cn",
        "xiaomi-token-plan-ams",
        "xiaomi-token-plan-sgp",
    ] {
        let ids = model_ids(provider);
        assert!(!ids.contains(&"mimo-v2-flash".to_string()), "{provider}");
        assert!(!ids.contains(&"mimo-v2-omni".to_string()), "{provider}");
    }
}

// ---------------------------------------------------------------------------
// OpenRouter
// ---------------------------------------------------------------------------

#[test]
fn openrouter_anthropic_latest_models_enable_cache_control() {
    for model_id in [
        "~anthropic/claude-fable-latest",
        "~anthropic/claude-haiku-latest",
        "~anthropic/claude-opus-latest",
        "~anthropic/claude-sonnet-latest",
    ] {
        let model = get_builtin_model("openrouter", model_id).expect(model_id);
        let compat = model.compat.expect("compat");
        assert_eq!(
            serde_json::to_value(
                compat
                    .as_openai_completions()
                    .expect("openai-completions compat")
                    .cache_control_format
            )
            .unwrap(),
            json!("anthropic"),
            "{model_id}"
        );
    }
}

// ---------------------------------------------------------------------------
// Together
// ---------------------------------------------------------------------------

#[test]
fn together_registers_kimi_k26_on_the_completions_api() {
    let model = get_builtin_model("together", "moonshotai/Kimi-K2.6").expect("model");
    assert_eq!(model.api, "openai-completions");
    assert_eq!(model.provider, "together");
    assert_eq!(model.base_url, "https://api.together.ai/v1");
    assert!(model.reasoning);
    assert_eq!(
        model.thinking_level_map,
        Some(
            serde_json::from_value(json!({ "minimal": null, "low": null, "medium": null }))
                .unwrap()
        )
    );
    assert_eq!(model.input, vec![Modality::Text, Modality::Image]);
    assert_eq!(model.context_window, 262_144);
    assert_eq!(model.max_tokens, 131_000);
    assert_eq!(
        model.cost,
        ModelCost {
            input: 1.2,
            output: 4.5,
            cache_read: 0.2,
            cache_write: 0.0,
            tiers: None,
        }
    );
    assert_eq!(
        serde_json::to_value(model.compat.expect("compat")).unwrap(),
        json!({
            "supportsStore": false,
            "supportsDeveloperRole": false,
            "supportsReasoningEffort": false,
            "maxTokensField": "max_tokens",
            "thinkingFormat": "together",
            "supportsStrictMode": false,
            "supportsLongCacheRetention": false,
        })
    );
}

#[test]
fn together_models_its_reasoning_controls_after_the_together_api() {
    let gpt_oss = get_builtin_model("together", "openai/gpt-oss-120b").expect("model");
    assert_eq!(
        serde_json::to_value(gpt_oss.thinking_level_map).unwrap(),
        json!({ "off": null, "minimal": null, "low": "low", "medium": "medium", "high": "high", "max": null, "xhigh": null })
    );
    let compat = serde_json::to_value(gpt_oss.compat.expect("compat")).unwrap();
    assert_eq!(compat["supportsReasoningEffort"], json!(true));
    assert_eq!(compat["thinkingFormat"], json!("openai"));

    let deepseek = get_builtin_model("together", "deepseek-ai/DeepSeek-V4-Pro").expect("model");
    assert_eq!(
        serde_json::to_value(deepseek.thinking_level_map).unwrap(),
        json!({ "minimal": null, "low": null, "medium": null, "high": "high", "xhigh": null })
    );
    let compat = serde_json::to_value(deepseek.compat.expect("compat")).unwrap();
    assert_eq!(compat["supportsReasoningEffort"], json!(true));
    assert_eq!(compat["thinkingFormat"], json!("together"));

    let minimax = get_builtin_model("together", "MiniMaxAI/MiniMax-M2.7").expect("model");
    assert_eq!(
        serde_json::to_value(minimax.thinking_level_map).unwrap(),
        json!({ "off": null, "minimal": null, "low": null, "medium": null })
    );
    let compat = serde_json::to_value(minimax.compat.expect("compat")).unwrap();
    assert_eq!(compat.get("thinkingFormat"), None);
    assert_eq!(compat["supportsReasoningEffort"], json!(false));
}

#[test]
fn together_resolves_its_api_key_from_the_environment() {
    let env = env(&[("TOGETHER_API_KEY", "test-together-key")]);
    assert_eq!(
        find_env_keys("together", Some(&env)),
        Some(vec!["TOGETHER_API_KEY".to_string()])
    );
    assert_eq!(
        get_env_api_key("together", Some(&env)).as_deref(),
        Some("test-together-key")
    );
}

// ---------------------------------------------------------------------------
// Amazon Bedrock
// ---------------------------------------------------------------------------

#[test]
fn bedrock_exposes_claude_opus_5_through_an_inference_profile_only() {
    let ids = model_ids("amazon-bedrock");
    assert!(!ids.is_empty());
    assert!(ids.contains(&"global.anthropic.claude-opus-5".to_string()));
    assert!(!ids.contains(&"anthropic.claude-opus-5".to_string()));
}

// ---------------------------------------------------------------------------
// Baseten
// ---------------------------------------------------------------------------

#[test]
fn baseten_registers_glm_52_as_its_default_reasoning_model() {
    let model = get_builtin_model("baseten", "zai-org/GLM-5.2").expect("model");
    assert_eq!(model.api, "openai-completions");
    assert_eq!(model.provider, "baseten");
    assert_eq!(model.base_url, "https://inference.baseten.co/v1");
    assert!(model.reasoning);
    assert_eq!(
        serde_json::to_value(model.thinking_level_map).unwrap(),
        json!({ "off": "none", "minimal": null, "low": null, "medium": null, "high": "high", "xhigh": null, "max": "max" })
    );
    assert_eq!(model.input, vec![Modality::Text, Modality::Image]);
    assert_eq!(model.context_window, 1_048_576);
    assert_eq!(model.max_tokens, 262_144);
    assert_eq!(
        model.cost,
        ModelCost {
            input: 1.4,
            output: 4.4,
            cache_read: 0.3,
            cache_write: 0.0,
            tiers: None,
        }
    );
    assert_eq!(
        serde_json::to_value(model.compat.expect("compat")).unwrap(),
        json!({
            "supportsStore": false,
            "supportsDeveloperRole": false,
            "supportsReasoningEffort": true,
            "supportsUsageInStreaming": true,
            "maxTokensField": "max_tokens",
            "supportsStrictMode": true,
            "supportsLongCacheRetention": false,
            "thinkingFormat": "baseten",
            "chatTemplateArgs": { "enable_thinking": { "$var": "thinking.enabled" } },
        })
    );
}

#[tokio::test]
async fn baseten_models_kimi_k26_reasoning_as_an_explicit_toggle() {
    let model = get_builtin_model("baseten", "moonshotai/Kimi-K2.6").expect("model");
    assert_eq!(
        serde_json::to_value(model.thinking_level_map.clone()).unwrap(),
        json!({ "off": "off", "minimal": null, "low": null, "medium": null, "high": "high", "xhigh": null, "max": null })
    );
    let compat = serde_json::to_value(model.compat.clone().expect("compat")).unwrap();
    assert_eq!(compat["supportsReasoningEffort"], json!(false));
    assert_eq!(compat["thinkingFormat"], json!("baseten"));
    assert_eq!(
        compat["chatTemplateArgs"],
        json!({ "enable_thinking": { "$var": "thinking.enabled" } })
    );
    assert_eq!(
        get_supported_thinking_levels(&model),
        vec![ModelThinkingLevel::Off, ModelThinkingLevel::High]
    );

    let request = capture_request(
        &OpenAICompletionsApi,
        &model,
        Some(ThinkingLevel::High),
        None,
        None,
    )
    .await;
    let body = body_json(&request);
    assert_eq!(
        body["chat_template_args"],
        json!({ "enable_thinking": true })
    );
    assert_eq!(body.get("reasoning_effort"), None);
}

#[tokio::test]
async fn baseten_sends_chat_template_args_with_and_without_reasoning() {
    let model = get_builtin_model("baseten", "zai-org/GLM-5.2").expect("model");

    let request = capture_request(
        &OpenAICompletionsApi,
        &model,
        Some(ThinkingLevel::High),
        None,
        None,
    )
    .await;
    let body = body_json(&request);
    assert_eq!(
        body["chat_template_args"],
        json!({ "enable_thinking": true })
    );
    assert_eq!(body["reasoning_effort"], json!("high"));

    let request = capture_request(&OpenAICompletionsApi, &model, None, None, None).await;
    let body = body_json(&request);
    assert_eq!(
        body["chat_template_args"],
        json!({ "enable_thinking": false })
    );
    assert_eq!(body["reasoning_effort"], json!("none"));
}

#[test]
fn baseten_resolves_its_api_key_from_the_environment() {
    let env = env(&[("BASETEN_API_KEY", "test-baseten-key")]);
    assert_eq!(
        find_env_keys("baseten", Some(&env)),
        Some(vec!["BASETEN_API_KEY".to_string()])
    );
    assert_eq!(
        get_env_api_key("baseten", Some(&env)).as_deref(),
        Some("test-baseten-key")
    );
}

// ---------------------------------------------------------------------------
// Qwen Token Plan
// ---------------------------------------------------------------------------

const QWEN_TEXT_MODELS: [&str; 15] = [
    "MiniMax-M2.5",
    "deepseek-v3.2",
    "deepseek-v4-flash",
    "deepseek-v4-pro",
    "glm-5",
    "glm-5.1",
    "glm-5.2",
    "kimi-k2.5",
    "kimi-k2.6",
    "kimi-k2.7-code",
    "qwen3.6-flash",
    "qwen3.6-plus",
    "qwen3.7-max",
    "qwen3.7-plus",
    "qwen3.8-max",
];

const QWEN_INDIVIDUAL_TEXT_MODELS: [&str; 7] = [
    "deepseek-v4-flash-0731",
    "deepseek-v4-pro",
    "glm-5.2",
    "qwen3.6-flash",
    "qwen3.7-max",
    "qwen3.7-plus",
    "qwen3.8-max",
];

const QWEN_IMAGE_MODELS: [&str; 4] = [
    "qwen-image-2.0",
    "qwen-image-2.0-pro",
    "wan2.7-image",
    "wan2.7-image-pro",
];

const QWEN_THINKING_MODELS: [&str; 14] = [
    "deepseek-v3.2",
    "deepseek-v4-flash",
    "deepseek-v4-pro",
    "glm-5",
    "glm-5.1",
    "glm-5.2",
    "kimi-k2.5",
    "kimi-k2.6",
    "kimi-k2.7-code",
    "qwen3.6-flash",
    "qwen3.6-plus",
    "qwen3.7-max",
    "qwen3.7-plus",
    "qwen3.8-max",
];

const QWEN_REASONING_EFFORT_MODELS: [&str; 5] = [
    "deepseek-v4-flash",
    "deepseek-v4-pro",
    "glm-5",
    "glm-5.1",
    "glm-5.2",
];

/// `QWEN_THINKING_MODEL_CASES` / `QWEN_REASONING_EFFORT_MODEL_CASES`
fn qwen_cases(shared: &[&'static str], individual: &[&'static str]) -> Vec<(&'static str, String)> {
    let mut cases: Vec<(&'static str, String)> = Vec::new();
    for provider in ["qwen-token-plan", "qwen-token-plan-cn"] {
        for model_id in shared {
            cases.push((provider, (*model_id).to_string()));
        }
    }
    for model_id in individual {
        cases.push(("qwen-token-plan-individual", (*model_id).to_string()));
    }
    cases
}

#[test]
fn qwen_individual_exposes_exactly_the_documented_text_models() {
    let mut ids = model_ids("qwen-token-plan-individual");
    ids.sort();
    let mut expected: Vec<String> = QWEN_INDIVIDUAL_TEXT_MODELS
        .iter()
        .map(|id| (*id).to_string())
        .collect();
    expected.sort();
    assert_eq!(ids, expected);
}

#[test]
fn qwen_individual_reuses_the_international_env_var() {
    assert_eq!(
        find_env_keys(
            "qwen-token-plan-individual",
            Some(&env(&[("QWEN_TOKEN_PLAN_API_KEY", "test")]))
        ),
        Some(vec!["QWEN_TOKEN_PLAN_API_KEY".to_string()])
    );
}

#[test]
fn qwen_token_plans_carry_the_text_models_and_no_image_models() {
    for provider in ["qwen-token-plan", "qwen-token-plan-cn"] {
        let ids = model_ids(provider);
        for expected in QWEN_TEXT_MODELS {
            assert!(ids.contains(&expected.to_string()), "{provider}/{expected}");
        }
        for excluded in QWEN_IMAGE_MODELS {
            assert!(
                !ids.contains(&excluded.to_string()),
                "{provider}/{excluded}"
            );
        }
    }
    for provider in [
        "qwen-token-plan",
        "qwen-token-plan-cn",
        "qwen-token-plan-individual",
    ] {
        assert!(!model_ids(provider).contains(&"qwen3.8-max-preview".to_string()));
    }
}

#[test]
fn qwen_reasoning_effort_models_expose_their_levels() {
    for (provider, model_id) in qwen_cases(
        &QWEN_REASONING_EFFORT_MODELS,
        &["deepseek-v4-flash-0731", "deepseek-v4-pro", "glm-5.2"],
    ) {
        let model = get_builtin_model(provider, &model_id).expect(&model_id);
        let map = serde_json::to_value(model.thinking_level_map).unwrap();
        for (level, expected) in [
            ("minimal", json!(null)),
            ("low", json!(null)),
            ("medium", json!(null)),
            ("high", json!("high")),
            ("xhigh", json!(null)),
            ("max", json!("max")),
        ] {
            assert_eq!(map[level], expected, "{provider}/{model_id}.{level}");
        }
    }

    for provider in [
        "qwen-token-plan",
        "qwen-token-plan-cn",
        "qwen-token-plan-individual",
    ] {
        let model = get_builtin_model(provider, "qwen3.8-max").expect("qwen3.8-max");
        let map = serde_json::to_value(model.thinking_level_map).unwrap();
        for (level, expected) in [
            ("minimal", json!(null)),
            ("low", json!("low")),
            ("medium", json!("medium")),
            ("high", json!(null)),
            ("xhigh", json!("xhigh")),
            ("max", json!(null)),
        ] {
            assert_eq!(map[level], expected, "{provider}/qwen3.8-max.{level}");
        }
    }
}

#[tokio::test]
async fn qwen_sends_thinking_and_reasoning_effort_fields() {
    for (provider, model_id) in qwen_cases(&QWEN_THINKING_MODELS, &QWEN_INDIVIDUAL_TEXT_MODELS) {
        let model = get_builtin_model(provider, &model_id).expect(&model_id);
        let request = capture_request(
            &OpenAICompletionsApi,
            &model,
            Some(ThinkingLevel::High),
            None,
            None,
        )
        .await;
        let body = body_json(&request);
        assert_eq!(
            body["enable_thinking"],
            json!(true),
            "{provider}/{model_id}"
        );
        assert_eq!(body.get("thinking"), None, "{provider}/{model_id}");
    }

    for (provider, model_id) in qwen_cases(
        &QWEN_REASONING_EFFORT_MODELS,
        &["deepseek-v4-flash-0731", "deepseek-v4-pro", "glm-5.2"],
    ) {
        let model = get_builtin_model(provider, &model_id).expect(&model_id);
        let request = capture_request(
            &OpenAICompletionsApi,
            &model,
            Some(ThinkingLevel::High),
            None,
            None,
        )
        .await;
        assert_eq!(
            body_json(&request)["reasoning_effort"],
            json!("high"),
            "{provider}/{model_id}"
        );
    }

    for provider in [
        "qwen-token-plan",
        "qwen-token-plan-cn",
        "qwen-token-plan-individual",
    ] {
        let model = get_builtin_model(provider, "qwen3.8-max").expect("qwen3.8-max");
        let request = capture_request(
            &OpenAICompletionsApi,
            &model,
            Some(ThinkingLevel::Xhigh),
            None,
            None,
        )
        .await;
        let body = body_json(&request);
        assert_eq!(body["enable_thinking"], json!(true), "{provider}");
        assert_eq!(body["reasoning_effort"], json!("xhigh"), "{provider}");
        assert_eq!(body.get("thinking"), None, "{provider}");
    }
}

// ---------------------------------------------------------------------------
// Fireworks
// ---------------------------------------------------------------------------

#[test]
fn fireworks_registers_kimi_k26_on_the_anthropic_api() {
    let model =
        get_builtin_model("fireworks", "accounts/fireworks/models/kimi-k2p6").expect("model");
    assert_eq!(model.api, "anthropic-messages");
    assert_eq!(model.provider, "fireworks");
    assert_eq!(model.base_url, "https://api.fireworks.ai/inference");
    assert!(model.reasoning);
    assert_eq!(model.input, vec![Modality::Text, Modality::Image]);
    assert_eq!(model.context_window, 262_000);
    assert_eq!(model.max_tokens, 262_000);
    assert_eq!(
        model.cost,
        ModelCost {
            input: 0.95,
            output: 4.0,
            cache_read: 0.16,
            cache_write: 0.0,
            tiers: None,
        }
    );

    let compat = serde_json::to_value(model.compat.expect("compat")).unwrap();
    assert_eq!(compat["sendSessionAffinityHeaders"], json!(true));
    assert_eq!(compat["supportsEagerToolInputStreaming"], json!(false));
    assert_eq!(compat["supportsCacheControlOnTools"], json!(false));
    assert_eq!(compat["supportsLongCacheRetention"], json!(false));
}

#[test]
fn fireworks_aligns_glm_52_fast_with_glm_52() {
    let base = get_builtin_model("fireworks", "accounts/fireworks/models/glm-5p2").expect("base");
    let fast =
        get_builtin_model("fireworks", "accounts/fireworks/routers/glm-5p2-fast").expect("fast");
    assert_eq!(fast.api, base.api);
    assert_eq!(fast.base_url, base.base_url);
    assert_eq!(fast.compat, base.compat);
    assert_eq!(fast.thinking_level_map, base.thinking_level_map);
}

#[tokio::test]
async fn fireworks_omits_unsupported_long_cache_retention() {
    for model_id in [
        "accounts/fireworks/models/glm-5p2",
        "accounts/fireworks/routers/glm-5p2-fast",
    ] {
        let model = get_builtin_model("fireworks", model_id).expect(model_id);
        let request = capture_request(
            &OpenAICompletionsApi,
            &model,
            None,
            Some(CacheRetention::Long),
            Some("test-fireworks-session"),
        )
        .await;
        assert_eq!(
            body_json(&request).get("prompt_cache_retention"),
            None,
            "{model_id}"
        );
    }
}

#[tokio::test]
async fn fireworks_routes_kimi_k3_through_the_completions_api() {
    let base = get_builtin_model("fireworks", "accounts/fireworks/models/kimi-k3").expect("base");
    let fast =
        get_builtin_model("fireworks", "accounts/fireworks/routers/kimi-k3-fast").expect("fast");
    let expected_compat = json!({
        "supportsStore": false,
        "supportsDeveloperRole": false,
        "requiresReasoningContentOnAssistantMessages": true,
        "thinkingFormat": "openai",
        "deferredToolsMode": "kimi",
        "sendSessionAffinityHeaders": true,
        "supportsLongCacheRetention": false,
    });
    let expected_levels = json!({
        "off": null, "minimal": null, "low": "low", "medium": "medium",
        "high": "high", "xhigh": null, "max": "max",
    });

    assert_eq!(base.api, "openai-completions");
    assert_eq!(base.base_url, "https://api.fireworks.ai/inference/v1");
    assert_eq!(
        serde_json::to_value(base.compat.clone().expect("compat")).unwrap(),
        expected_compat
    );
    assert_eq!(
        serde_json::to_value(base.thinking_level_map.clone()).unwrap(),
        expected_levels
    );
    assert_eq!(fast.api, base.api);
    assert_eq!(fast.base_url, base.base_url);
    assert_eq!(
        serde_json::to_value(fast.compat.expect("compat")).unwrap(),
        expected_compat
    );
    assert_eq!(
        serde_json::to_value(fast.thinking_level_map).unwrap(),
        expected_levels
    );

    let request = capture_request(
        &OpenAICompletionsApi,
        &base,
        Some(ThinkingLevel::Max),
        None,
        None,
    )
    .await;
    assert_eq!(body_json(&request)["reasoning_effort"], json!("max"));
}

#[test]
fn fireworks_resolves_its_api_key_from_the_environment() {
    let env = env(&[("FIREWORKS_API_KEY", "test-fireworks-key")]);
    assert_eq!(
        find_env_keys("fireworks", Some(&env)),
        Some(vec!["FIREWORKS_API_KEY".to_string()])
    );
    assert_eq!(
        get_env_api_key("fireworks", Some(&env)).as_deref(),
        Some("test-fireworks-key")
    );
}

// ---------------------------------------------------------------------------
// Fireworks session affinity and tool compat (Anthropic transport)
// ---------------------------------------------------------------------------

fn tool() -> Tool {
    Tool {
        name: "lookup".to_string(),
        description: "Look up a value".to_string(),
        parameters: json!({
            "type": "object",
            "properties": { "value": { "type": "string" } },
            "required": ["value"],
        }),
        constrained_sampling: None,
    }
}

fn tool_context() -> Context {
    Context {
        messages: vec![Message::User(UserMessage {
            content: UserContent::Text("Use the tool".to_string()),
            timestamp: 1,
        })],
        tools: Some(vec![tool()]),
        ..Default::default()
    }
}

async fn capture_anthropic_request(
    model: &Model,
    cache_retention: CacheRetention,
    session_id: Option<&str>,
) -> FetchRequest {
    let seen = Arc::new(Mutex::new(None));
    let options = SimpleStreamOptions {
        base: StreamOptions {
            cache_retention: Some(cache_retention),
            session_id: session_id.map(str::to_string),
            base: ProviderRequestOptions {
                api_key: Some("test-key".to_string()),
                fetch: Some(Arc::new(CapturingFetch { seen: seen.clone() })),
                max_retries: Some(0),
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    };
    AnthropicMessagesApi
        .stream_simple(model, &tool_context(), Some(options))
        .result()
        .await;
    seen.lock().expect("poisoned").take().expect("a request")
}

fn header(request: &FetchRequest, name: &str) -> Option<String> {
    request
        .headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.clone())
}

#[tokio::test]
async fn fireworks_sends_session_affinity_and_omits_anthropic_only_tool_fields() {
    let fireworks =
        get_builtin_model("fireworks", "accounts/fireworks/models/kimi-k2p6").expect("model");

    let request = capture_anthropic_request(
        &fireworks,
        CacheRetention::Short,
        Some("fireworks-session-1"),
    )
    .await;
    assert_eq!(
        header(&request, "x-session-affinity").as_deref(),
        Some("fireworks-session-1")
    );
    let tools = body_json(&request)["tools"]
        .as_array()
        .cloned()
        .expect("tools");
    assert_eq!(tools.last().expect("tool").get("cache_control"), None);
    for tool in &tools {
        assert_eq!(tool.get("eager_input_streaming"), None);
    }

    // `cacheRetention: "none"` drops the affinity header.
    let request = capture_anthropic_request(
        &fireworks,
        CacheRetention::None,
        Some("fireworks-session-2"),
    )
    .await;
    assert_eq!(header(&request, "x-session-affinity"), None);
}

#[tokio::test]
async fn native_anthropic_models_keep_cache_control_and_eager_tool_input() {
    let anthropic = get_builtin_model("anthropic", "claude-opus-4-8").expect("model");
    let request = capture_anthropic_request(
        &anthropic,
        CacheRetention::Short,
        Some("anthropic-session-1"),
    )
    .await;

    assert_eq!(header(&request, "x-session-affinity"), None);
    let tools = body_json(&request)["tools"]
        .as_array()
        .cloned()
        .expect("tools");
    assert_eq!(
        tools.last().expect("tool")["cache_control"]["type"],
        json!("ephemeral")
    );
    assert_eq!(tools[0]["eager_input_streaming"], json!(true));
}

// ---------------------------------------------------------------------------
// Thinking levels
//
// opt-in, and the Codex models that support it advertise both `xhigh` and `max`.
// ---------------------------------------------------------------------------

#[test]
fn gpt_6_astra_uses_the_context_limit_of_each_openai_endpoint() {
    let api = get_builtin_model("openai", "gpt-6-astra").expect("OpenAI model");
    let codex = get_builtin_model("openai-codex", "gpt-6-astra").expect("Codex model");

    assert_eq!(api.context_window, 1_050_000);
    assert_eq!(codex.context_window, 872_000);
    assert_eq!(api.max_tokens, 128_000);
    assert_eq!(codex.max_tokens, 128_000);
    assert_eq!(
        get_supported_thinking_levels(&api),
        vec![
            ModelThinkingLevel::Low,
            ModelThinkingLevel::Medium,
            ModelThinkingLevel::High,
            ModelThinkingLevel::Xhigh,
            ModelThinkingLevel::Max,
        ]
    );
    assert_eq!(
        get_supported_thinking_levels(&codex),
        get_supported_thinking_levels(&api)
    );
}

#[test]
fn gpt_6_sol_and_luna_are_offered_through_the_api_and_the_codex_plan() {
    for (model_id, input, output) in [("gpt-6-sol", 2.0, 10.0), ("gpt-6-luna", 0.1, 0.5)] {
        let api = get_builtin_model("openai", model_id).expect(model_id);
        let codex = get_builtin_model("openai-codex", model_id).expect(model_id);

        assert_eq!(api.context_window, 1_050_000, "{model_id}");
        assert_eq!(codex.context_window, 872_000, "{model_id}");
        assert_eq!(api.max_tokens, 128_000, "{model_id}");
        assert_eq!(codex.max_tokens, 128_000, "{model_id}");
        assert_eq!(
            (api.cost.input, api.cost.output),
            (input, output),
            "{model_id}"
        );
        for model in [&api, &codex] {
            let levels = get_supported_thinking_levels(model);
            assert!(
                levels.contains(&ModelThinkingLevel::Xhigh)
                    && levels.contains(&ModelThinkingLevel::Max),
                "{model_id} on {} offers xhigh and max: {levels:?}",
                model.provider
            );
        }
    }
}

#[test]
fn claude_opus_5_5_is_in_the_anthropic_catalog() {
    let model = get_builtin_model("anthropic", "claude-opus-5-5").expect("Opus 5.5");

    assert_eq!(model.context_window, 1_000_000);
    assert_eq!(model.max_tokens, 128_000);
    assert_eq!((model.cost.input, model.cost.output), (4.0, 20.0));
    assert_eq!((model.cost.cache_read, model.cost.cache_write), (0.2, 5.0));
    assert_eq!(
        get_supported_thinking_levels(&model),
        get_supported_thinking_levels(
            &get_builtin_model("anthropic", "claude-opus-5").expect("Opus 5")
        ),
        "Opus 5.5 thinks like Opus 5: adaptive, up to max"
    );
}

#[test]
fn max_thinking_is_opt_in_and_codex_models_advertise_it() {
    use notagent_ai::models::{clamp_thinking_level, get_supported_thinking_levels};

    let ordinary = Model {
        reasoning: true,
        ..test_catalog_model("ordinary-reasoning")
    };
    assert_eq!(
        get_supported_thinking_levels(&ordinary),
        vec![
            ModelThinkingLevel::Off,
            ModelThinkingLevel::Minimal,
            ModelThinkingLevel::Low,
            ModelThinkingLevel::Medium,
            ModelThinkingLevel::High,
        ]
    );
    assert_eq!(
        clamp_thinking_level(&ordinary, ModelThinkingLevel::Max),
        ModelThinkingLevel::High
    );

    for model_id in ["gpt-5.6-luna", "gpt-5.6-sol", "gpt-5.6-terra"] {
        let model = get_builtin_model("openai-codex", model_id).expect(model_id);
        let levels = serde_json::to_value(model.thinking_level_map.clone()).unwrap();
        assert_eq!(levels["xhigh"], json!("xhigh"), "{model_id}");
        assert_eq!(levels["max"], json!("max"), "{model_id}");
        assert_eq!(
            get_supported_thinking_levels(&model),
            vec![
                ModelThinkingLevel::Off,
                ModelThinkingLevel::Minimal,
                ModelThinkingLevel::Low,
                ModelThinkingLevel::Medium,
                ModelThinkingLevel::High,
                ModelThinkingLevel::Xhigh,
                ModelThinkingLevel::Max,
            ],
            "{model_id}"
        );
    }

    // A hole between `high` and `max` is allowed.
    let mut holed = test_catalog_model("high-and-max");
    holed.reasoning = true;
    holed.thinking_level_map =
        Some(serde_json::from_value(json!({ "xhigh": null, "max": "max" })).expect("levels"));
    assert_eq!(
        get_supported_thinking_levels(&holed),
        vec![
            ModelThinkingLevel::Off,
            ModelThinkingLevel::Minimal,
            ModelThinkingLevel::Low,
            ModelThinkingLevel::Medium,
            ModelThinkingLevel::High,
            ModelThinkingLevel::Max,
        ]
    );
    assert_eq!(
        clamp_thinking_level(&holed, ModelThinkingLevel::Xhigh),
        ModelThinkingLevel::Max
    );
}

fn test_catalog_model(id: &str) -> Model {
    Model {
        id: id.to_string(),
        name: id.to_string(),
        api: "openai-completions".to_string(),
        provider: "test".to_string(),
        base_url: "https://example.com/v1".to_string(),
        reasoning: false,
        thinking_level_map: None,
        input: vec![Modality::Text],
        cost: ModelCost::default(),
        context_window: 128_000,
        max_tokens: 4096,
        sampling_params: None,
        headers: None,
        compat: None,
    }
}
