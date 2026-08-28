use notagent_ai::api::anthropic_params::{
    AnthropicOptions, build_params as build_anthropic_params, is_oauth_token,
};
use notagent_ai::api::openai_completions_compat::get_compat;
use notagent_ai::api::openai_completions_params::{
    OpenAICompletionsOptions, build_params as build_completions_params,
};
use notagent_ai::api::simple_options::build_base_options;
use notagent_ai::types::{
    CacheRetention, Context, Message, Modality, Model, ModelCost, ProviderRequestOptions,
    SimpleStreamOptions, StreamOptions, UserContent, UserMessage,
};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

const TIMESTAMP: i64 = 1_700_000_000_000;

fn make_context() -> Context {
    Context {
        messages: vec![Message::User(UserMessage {
            content: UserContent::Text("Hello".to_string()),
            timestamp: TIMESTAMP,
        })],
        ..Context::default()
    }
}

fn make_completions_model(sampling_params: Option<Map<String, Value>>) -> Model {
    Model {
        id: "custom-model".to_string(),
        name: "Custom Model".to_string(),
        api: "openai-completions".to_string(),
        provider: "custom-provider".to_string(),
        base_url: "http://127.0.0.1:9/v1".to_string(),
        reasoning: false,
        thinking_level_map: None,
        input: vec![Modality::Text],
        cost: ModelCost::default(),
        context_window: 128_000,
        max_tokens: 16_384,
        sampling_params,
        headers: None,
        compat: None,
    }
}

fn make_anthropic_model() -> Model {
    Model {
        id: "vendor--claude".to_string(),
        name: "Vendor Proxy Claude".to_string(),
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
        compat: None,
    }
}

fn sampling(pairs: &[(&str, Value)]) -> Map<String, Value> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), value.clone()))
        .collect()
}

fn simple_options(
    temperature: Option<f64>,
    sampling_params: Option<Map<String, Value>>,
) -> SimpleStreamOptions {
    SimpleStreamOptions {
        base: StreamOptions {
            temperature,
            sampling_params,
            base: ProviderRequestOptions {
                api_key: Some("fake-key".to_string()),
                ..ProviderRequestOptions::default()
            },
            ..StreamOptions::default()
        },
        ..SimpleStreamOptions::default()
    }
}

fn capture_completions_payload(model: &Model, options: &SimpleStreamOptions) -> Value {
    let context = make_context();
    let base = build_base_options(model, &context, Some(options), None);
    let compat = get_compat(model);
    build_completions_params(
        model,
        &context,
        &OpenAICompletionsOptions {
            max_tokens: base.max_tokens,
            temperature: base.temperature,
            sampling_params: base.sampling_params.clone(),
            cache_retention: base.cache_retention,
            session_id: base.session_id.clone(),
            ..OpenAICompletionsOptions::default()
        },
        &compat,
        base.cache_retention.unwrap_or(CacheRetention::Short),
        &BTreeMap::new(),
        TIMESTAMP,
    )
    .expect("params")
}

fn capture_anthropic_payload(model: &Model, options: &SimpleStreamOptions) -> Value {
    let context = make_context();
    let base = build_base_options(model, &context, Some(options), None);
    build_anthropic_params(
        model,
        &context,
        is_oauth_token(base.base.api_key.as_deref().unwrap_or_default()),
        &AnthropicOptions {
            max_tokens: base.max_tokens,
            temperature: base.temperature,
            cache_retention: base.cache_retention,
            session_id: base.session_id.clone(),
            thinking_enabled: Some(false),
            ..AnthropicOptions::default()
        },
        TIMESTAMP,
    )
}

#[test]
fn merges_stream_option_sampling_params_into_the_request_body() {
    let payload = capture_completions_payload(
        &make_completions_model(None),
        &simple_options(
            None,
            Some(sampling(&[
                ("top_p", json!(0.95)),
                ("top_k", json!(0)),
                ("min_p", json!(0)),
            ])),
        ),
    );

    assert_eq!(payload["top_p"], json!(0.95));
    assert_eq!(payload["top_k"], json!(0));
    assert_eq!(payload["min_p"], json!(0));
}

#[test]
fn omits_sampling_params_when_neither_options_nor_model_set_them() {
    let payload =
        capture_completions_payload(&make_completions_model(None), &simple_options(None, None));

    assert_eq!(payload.get("temperature"), None);
    assert_eq!(payload.get("top_p"), None);
}

#[test]
fn applies_model_level_sampling_params() {
    let payload = capture_completions_payload(
        &make_completions_model(Some(sampling(&[
            ("temperature", json!(1)),
            ("top_p", json!(0.95)),
        ]))),
        &simple_options(None, None),
    );

    assert_eq!(payload["temperature"], json!(1));
    assert_eq!(payload["top_p"], json!(0.95));
}

#[test]
fn merges_stream_option_keys_over_model_level_keys() {
    let payload = capture_completions_payload(
        &make_completions_model(Some(sampling(&[
            ("top_p", json!(0.95)),
            ("min_p", json!(0.05)),
        ]))),
        &simple_options(None, Some(sampling(&[("top_p", json!(0.5))]))),
    );

    assert_eq!(payload["top_p"], json!(0.5));
    assert_eq!(payload["min_p"], json!(0.05));
}

#[test]
fn overrides_named_request_fields() {
    let payload = capture_completions_payload(
        &make_completions_model(None),
        &simple_options(Some(0.0), Some(sampling(&[("temperature", json!(1))]))),
    );

    assert_eq!(payload["temperature"], json!(1));
}

#[test]
fn is_ignored_by_non_openai_compatible_apis() {
    let payload = capture_anthropic_payload(
        &make_anthropic_model(),
        &simple_options(
            None,
            Some(sampling(&[("top_p", json!(0.9)), ("top_k", json!(40))])),
        ),
    );

    assert_eq!(payload.get("top_p"), None);
    assert_eq!(payload.get("top_k"), None);
}
