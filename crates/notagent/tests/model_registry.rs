//! Port of `packages/coding-agent/test/model-registry.test.ts` (1 982 LOC).
//!
//! The `dynamic provider lifecycle` block is ported only where it does not depend on
//! the extension `registerProvider(name, config)` API — that API is dropped with the
//! extension system (deviation class 2, extension-boundary §6). See
//! `crates/notagent/PARITY.md`, section "B: model layer", for the excluded cases.

use std::collections::BTreeMap;
use std::sync::Arc;

use notagent::core::auth_storage::AuthStorage;
use notagent::core::model_registry::{ModelRegistry, ResolvedRequestAuth, clear_api_key_cache};
use notagent::core::model_runtime::{CreateModelRuntimeOptions, ModelRuntime};
use notagent::core::models_store::InMemoryCodingAgentModelsStore;
use notagent::core::provider_composer::{AuthStatus, AuthStatusSource};
use notagent_ai::auth::types::{ApiKeyCredential, Credential, CredentialStore, OAuthCredential};
use notagent_ai::types::{Model, ModelCompat};
use serde_json::{Value, json};

struct Fixture {
    dir: tempfile::TempDir,
    auth_storage: Arc<AuthStorage>,
}

impl Fixture {
    fn new() -> Self {
        Fixture {
            dir: tempfile::tempdir().expect("tempdir"),
            auth_storage: Arc::new(AuthStorage::in_memory(serde_json::Map::new())),
        }
    }

    fn models_json_path(&self) -> String {
        self.dir
            .path()
            .join("models.json")
            .to_string_lossy()
            .into_owned()
    }

    fn write_raw_models_json(&self, providers: Value) {
        std::fs::write(
            self.models_json_path(),
            serde_json::to_string(&json!({ "providers": providers })).expect("json"),
        )
        .expect("write models.json");
    }

    async fn registry(&self) -> ModelRegistry {
        let runtime = ModelRuntime::create(CreateModelRuntimeOptions {
            credentials: Some(Arc::clone(&self.auth_storage) as Arc<dyn CredentialStore>),
            models_path: Some(Some(self.models_json_path())),
            models_store: Some(Arc::new(InMemoryCodingAgentModelsStore::new())),
            allow_model_network: Some(false),
            ..CreateModelRuntimeOptions::default()
        })
        .await
        .expect("runtime");
        ModelRegistry::new(runtime)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        clear_api_key_cache();
    }
}

/// `providerConfig(baseUrl, models, api?)`
fn provider_config(base_url: &str, model_ids: &[&str], api: &str) -> Value {
    json!({
        "baseUrl": base_url,
        "apiKey": "test-key",
        "api": api,
        "models": model_ids
            .iter()
            .map(|id| json!({
                "id": id,
                "name": id,
                "reasoning": false,
                "input": ["text"],
                "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
                "contextWindow": 100000,
                "maxTokens": 8000,
            }))
            .collect::<Vec<_>>(),
    })
}

fn models_for_provider(registry: &ModelRegistry, provider: &str) -> Vec<Model> {
    registry
        .get_all()
        .into_iter()
        .filter(|model| model.provider == provider)
        .collect()
}

fn compat_value(model: &Model) -> Value {
    model.compat.as_ref().map_or(Value::Null, |compat| {
        serde_json::to_value(compat).expect("compat")
    })
}

fn compat_field(model: &Model, key: &str) -> Value {
    compat_value(model).get(key).cloned().unwrap_or(Value::Null)
}

fn headers_of(auth: &ResolvedRequestAuth) -> BTreeMap<String, String> {
    match auth {
        ResolvedRequestAuth::Ok { headers, .. } => headers
            .clone()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(name, value)| value.map(|value| (name, value)))
            .collect(),
        ResolvedRequestAuth::Err(error) => panic!("expected ok, got {error}"),
    }
}

fn sh_path(value: &str) -> String {
    value.replace('\\', "/").replace('"', "\\\"")
}

// ---------------------------------------------------------------------------
// baseUrl override (no custom models)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn overriding_base_url_keeps_all_built_in_models() {
    let fixture = Fixture::new();
    fixture.write_raw_models_json(json!({
        "anthropic": { "baseUrl": "https://my-proxy.example.com/v1" },
    }));
    let registry = fixture.registry().await;
    let models = models_for_provider(&registry, "anthropic");
    assert!(models.len() > 1);
    assert!(models.iter().any(|model| model.id.contains("claude")));
}

#[tokio::test]
async fn overriding_base_url_changes_url_on_all_built_in_models() {
    let fixture = Fixture::new();
    fixture.write_raw_models_json(json!({
        "anthropic": { "baseUrl": "https://my-proxy.example.com/v1" },
    }));
    let registry = fixture.registry().await;
    for model in models_for_provider(&registry, "anthropic") {
        assert_eq!(model.base_url, "https://my-proxy.example.com/v1");
    }
}

#[tokio::test]
async fn overriding_headers_resolves_at_request_time() {
    let fixture = Fixture::new();
    fixture.write_raw_models_json(json!({
        "anthropic": {
            "baseUrl": "https://my-proxy.example.com/v1",
            "headers": { "X-Custom-Header": "custom-value" },
        },
    }));
    let registry = fixture.registry().await;
    for model in models_for_provider(&registry, "anthropic") {
        let auth = registry.get_api_key_and_headers(&model).await;
        assert_eq!(
            headers_of(&auth).get("X-Custom-Header").map(String::as_str),
            Some("custom-value")
        );
    }
}

#[tokio::test]
async fn headers_only_override_resolves_at_request_time() {
    let fixture = Fixture::new();
    fixture.write_raw_models_json(json!({
        "anthropic": { "headers": { "X-Custom-Header": "custom-value" } },
    }));
    let registry = fixture.registry().await;
    assert_eq!(registry.get_error(), None);
    for model in models_for_provider(&registry, "anthropic") {
        let auth = registry.get_api_key_and_headers(&model).await;
        assert_eq!(
            headers_of(&auth).get("X-Custom-Header").map(String::as_str),
            Some("custom-value")
        );
    }
}

#[tokio::test]
async fn unconfigured_compatibility_auth_includes_static_model_headers() {
    let fixture = Fixture::new();
    let registry = fixture.registry().await;
    let base = registry.get_all().first().cloned().expect("a model");
    let model = Model {
        provider: "missing-provider".to_owned(),
        headers: Some(BTreeMap::from([(
            "X-Static-Model".to_owned(),
            "static-value".to_owned(),
        )])),
        ..base
    };
    let auth = registry.get_api_key_and_headers(&model).await;
    assert_eq!(
        auth,
        ResolvedRequestAuth::Ok {
            api_key: None,
            headers: Some(BTreeMap::from([(
                "X-Static-Model".to_owned(),
                Some("static-value".to_owned())
            )])),
            base_url: None,
            env: None,
        }
    );
}

#[tokio::test]
async fn base_url_only_override_does_not_affect_other_providers() {
    let fixture = Fixture::new();
    fixture.write_raw_models_json(json!({
        "anthropic": { "baseUrl": "https://my-proxy.example.com/v1" },
    }));
    let registry = fixture.registry().await;
    let google = models_for_provider(&registry, "google");
    assert!(!google.is_empty());
    assert_ne!(google[0].base_url, "https://my-proxy.example.com/v1");
}

#[tokio::test]
async fn can_mix_base_url_override_and_models_merge() {
    let fixture = Fixture::new();
    fixture.write_raw_models_json(json!({
        "anthropic": { "baseUrl": "https://anthropic-proxy.example.com/v1" },
        "google": provider_config(
            "https://google-proxy.example.com/v1",
            &["gemini-custom"],
            "google-generative-ai",
        ),
    }));
    let registry = fixture.registry().await;
    let anthropic = models_for_provider(&registry, "anthropic");
    assert!(anthropic.len() > 1);
    assert_eq!(
        anthropic[0].base_url,
        "https://anthropic-proxy.example.com/v1"
    );
    let google = models_for_provider(&registry, "google");
    assert!(google.len() > 1);
    assert!(google.iter().any(|model| model.id == "gemini-custom"));
}

#[tokio::test]
async fn refresh_picks_up_base_url_override_changes() {
    let fixture = Fixture::new();
    fixture.write_raw_models_json(json!({
        "anthropic": { "baseUrl": "https://first-proxy.example.com/v1" },
    }));
    let registry = fixture.registry().await;
    assert_eq!(
        models_for_provider(&registry, "anthropic")[0].base_url,
        "https://first-proxy.example.com/v1"
    );

    fixture.write_raw_models_json(json!({
        "anthropic": { "baseUrl": "https://second-proxy.example.com/v1" },
    }));
    registry.refresh(None).await;

    assert_eq!(
        models_for_provider(&registry, "anthropic")[0].base_url,
        "https://second-proxy.example.com/v1"
    );
}

// ---------------------------------------------------------------------------
// custom models merge behavior
// ---------------------------------------------------------------------------

#[tokio::test]
async fn built_in_provider_custom_models_inherit_api_and_base_url() {
    let fixture = Fixture::new();
    fixture.write_raw_models_json(json!({
        "openrouter": {
            "models": [{
                "id": "fake-provider/fake-model",
                "name": "Fake model",
                "reasoning": true,
                "input": ["text"],
            }],
        },
    }));
    let registry = fixture.registry().await;
    assert_eq!(registry.get_error(), None);
    let model = registry
        .find("openrouter", "fake-provider/fake-model")
        .expect("model");
    assert_eq!(model.api, "openai-completions");
    assert_eq!(model.base_url, "https://openrouter.ai/api/v1");
}

#[tokio::test]
async fn non_built_in_provider_custom_models_still_require_base_url() {
    let fixture = Fixture::new();
    fixture.write_raw_models_json(json!({
        "my-custom-provider": {
            "apiKey": "test-key",
            "models": [{ "id": "my-model", "api": "openai-completions", "reasoning": false, "input": ["text"] }],
        },
    }));
    let registry = fixture.registry().await;
    assert!(registry.get_error().expect("error").contains("baseUrl"));
}

#[tokio::test]
async fn reports_every_provider_composition_error() {
    let fixture = Fixture::new();
    fixture.write_raw_models_json(json!({
        "broken-one": { "api": "openai-completions", "models": [{ "id": "one" }] },
        "broken-two": { "api": "openai-completions", "models": [{ "id": "two" }] },
    }));
    let registry = fixture.registry().await;
    let error = registry.get_error().expect("error");
    assert!(error.contains("Provider \"broken-one\""));
    assert!(error.contains("Provider \"broken-two\""));
}

#[tokio::test]
async fn custom_provider_with_same_name_as_built_in_merges_with_built_in_models() {
    let fixture = Fixture::new();
    fixture.write_raw_models_json(json!({
        "anthropic": provider_config("https://my-proxy.example.com/v1", &["claude-custom"], "anthropic-messages"),
    }));
    let registry = fixture.registry().await;
    let models = models_for_provider(&registry, "anthropic");
    assert!(models.len() > 1);
    assert!(models.iter().any(|model| model.id == "claude-custom"));
    assert!(models.iter().any(|model| model.id.contains("claude")));
}

#[tokio::test]
async fn custom_model_with_same_id_replaces_built_in_model_by_id() {
    let fixture = Fixture::new();
    fixture.write_raw_models_json(json!({
        "openrouter": provider_config(
            "https://my-proxy.example.com/v1",
            &["anthropic/claude-sonnet-4"],
            "openai-completions",
        ),
    }));
    let registry = fixture.registry().await;
    let sonnet: Vec<Model> = models_for_provider(&registry, "openrouter")
        .into_iter()
        .filter(|model| model.id == "anthropic/claude-sonnet-4")
        .collect();
    assert_eq!(sonnet.len(), 1);
    assert_eq!(sonnet[0].base_url, "https://my-proxy.example.com/v1");
}

#[tokio::test]
async fn custom_provider_with_same_name_does_not_affect_other_built_in_providers() {
    let fixture = Fixture::new();
    fixture.write_raw_models_json(json!({
        "anthropic": provider_config("https://my-proxy.example.com/v1", &["claude-custom"], "anthropic-messages"),
    }));
    let registry = fixture.registry().await;
    assert!(!models_for_provider(&registry, "google").is_empty());
    assert!(!models_for_provider(&registry, "openai").is_empty());
}

#[tokio::test]
async fn provider_level_base_url_applies_to_built_in_and_custom_models() {
    let fixture = Fixture::new();
    fixture.write_raw_models_json(json!({
        "anthropic": provider_config("https://merged-proxy.example.com/v1", &["claude-custom"], "anthropic-messages"),
    }));
    let registry = fixture.registry().await;
    for model in models_for_provider(&registry, "anthropic") {
        assert_eq!(model.base_url, "https://merged-proxy.example.com/v1");
    }
}

fn demo_provider(compat: Value, model_compat: Option<Value>) -> Value {
    let mut model = json!({
        "id": "demo-model",
        "reasoning": false,
        "input": ["text"],
        "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
        "contextWindow": 1000,
        "maxTokens": 100,
    });
    if let Some(model_compat) = model_compat {
        model["compat"] = model_compat;
    }
    json!({
        "baseUrl": "https://example.com/v1",
        "apiKey": "DEMO_KEY",
        "api": "openai-completions",
        "compat": compat,
        "models": [model],
    })
}

#[tokio::test]
async fn provider_level_compat_applies_to_custom_models() {
    let fixture = Fixture::new();
    fixture.write_raw_models_json(json!({
        "demo": demo_provider(
            json!({ "supportsUsageInStreaming": false, "maxTokensField": "max_tokens" }),
            None,
        ),
    }));
    let registry = fixture.registry().await;
    let model = registry.find("demo", "demo-model").expect("model");
    assert_eq!(
        compat_field(&model, "supportsUsageInStreaming"),
        json!(false)
    );
    assert_eq!(compat_field(&model, "maxTokensField"), json!("max_tokens"));
}

#[tokio::test]
async fn model_level_compat_overrides_provider_level_compat() {
    let fixture = Fixture::new();
    fixture.write_raw_models_json(json!({
        "demo": demo_provider(
            json!({ "supportsUsageInStreaming": false, "maxTokensField": "max_tokens" }),
            Some(json!({ "supportsUsageInStreaming": true, "maxTokensField": "max_completion_tokens" })),
        ),
    }));
    let registry = fixture.registry().await;
    let model = registry.find("demo", "demo-model").expect("model");
    assert_eq!(
        compat_field(&model, "supportsUsageInStreaming"),
        json!(true)
    );
    assert_eq!(
        compat_field(&model, "maxTokensField"),
        json!("max_completion_tokens")
    );
}

#[tokio::test]
async fn provider_level_compat_applies_to_built_in_models() {
    let fixture = Fixture::new();
    fixture.write_raw_models_json(json!({
        "openrouter": { "compat": { "supportsUsageInStreaming": false, "supportsStrictMode": false } },
    }));
    let registry = fixture.registry().await;
    let models = models_for_provider(&registry, "openrouter");
    assert!(!models.is_empty());
    for model in models {
        assert_eq!(
            compat_field(&model, "supportsUsageInStreaming"),
            json!(false)
        );
        assert_eq!(compat_field(&model, "supportsStrictMode"), json!(false));
    }
}

#[tokio::test]
async fn model_schema_accepts_thinking_level_map_and_compat_flags() {
    let fixture = Fixture::new();
    fixture.write_raw_models_json(json!({
        "demo": {
            "baseUrl": "https://example.com/v1",
            "apiKey": "DEMO_KEY",
            "api": "openai-completions",
            "models": [{
                "id": "demo-model",
                "reasoning": true,
                "input": ["text"],
                "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
                "contextWindow": 1000,
                "maxTokens": 100,
                "thinkingLevelMap": { "minimal": null, "high": "max" },
                "compat": { "supportsStrictMode": false, "cacheControlFormat": "anthropic" },
            }],
        },
    }));
    let registry = fixture.registry().await;
    assert_eq!(registry.get_error(), None);
    let model = registry.find("demo", "demo-model").expect("model");
    assert_eq!(
        serde_json::to_value(model.thinking_level_map.clone()).expect("map"),
        json!({ "minimal": null, "high": "max" })
    );
    assert_eq!(compat_field(&model, "supportsStrictMode"), json!(false));
    assert_eq!(
        compat_field(&model, "cacheControlFormat"),
        json!("anthropic")
    );
}

#[tokio::test]
async fn compat_schema_accepts_chat_template_thinking_configuration() {
    let fixture = Fixture::new();
    fixture.write_raw_models_json(json!({
        "demo": {
            "baseUrl": "https://example.com/v1",
            "apiKey": "DEMO_KEY",
            "api": "openai-completions",
            "models": [
                {
                    "id": "kwargs-model",
                    "reasoning": true,
                    "input": ["text"],
                    "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
                    "contextWindow": 1000,
                    "maxTokens": 100,
                    "compat": {
                        "thinkingFormat": "chat-template",
                        "chatTemplateKwargs": {
                            "preserve_thinking": true,
                            "thinking": { "$var": "thinking.enabled" },
                        },
                    },
                },
                {
                    "id": "args-model",
                    "reasoning": true,
                    "input": ["text"],
                    "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
                    "contextWindow": 1000,
                    "maxTokens": 100,
                    "compat": {
                        "thinkingFormat": "baseten",
                        "chatTemplateArgs": { "enable_thinking": { "$var": "thinking.enabled" } },
                    },
                },
            ],
        },
    }));
    let registry = fixture.registry().await;
    assert_eq!(registry.get_error(), None);
    let kwargs = registry.find("demo", "kwargs-model").expect("model");
    let args = registry.find("demo", "args-model").expect("model");
    assert_eq!(
        compat_field(&kwargs, "thinkingFormat"),
        json!("chat-template")
    );
    assert_eq!(
        compat_field(&kwargs, "chatTemplateKwargs"),
        json!({ "preserve_thinking": true, "thinking": { "$var": "thinking.enabled" } })
    );
    assert_eq!(compat_field(&args, "thinkingFormat"), json!("baseten"));
    assert_eq!(
        compat_field(&args, "chatTemplateArgs"),
        json!({ "enable_thinking": { "$var": "thinking.enabled" } })
    );
}

#[tokio::test]
async fn compat_schema_accepts_anthropic_eager_tool_input_streaming_flag() {
    let fixture = Fixture::new();
    fixture.write_raw_models_json(json!({
        "demo": {
            "baseUrl": "https://example.com",
            "apiKey": "DEMO_KEY",
            "api": "anthropic-messages",
            "compat": { "supportsEagerToolInputStreaming": false },
            "models": [{
                "id": "demo-model",
                "reasoning": true,
                "input": ["text"],
                "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
                "contextWindow": 1000,
                "maxTokens": 100,
            }],
        },
    }));
    let registry = fixture.registry().await;
    assert_eq!(registry.get_error(), None);
    let model = registry.find("demo", "demo-model").expect("model");
    assert_eq!(
        compat_field(&model, "supportsEagerToolInputStreaming"),
        json!(false)
    );
}

#[tokio::test]
async fn compat_schema_accepts_long_cache_retention_flag() {
    let fixture = Fixture::new();
    fixture.write_raw_models_json(json!({
        "demo": {
            "baseUrl": "https://example.com",
            "apiKey": "DEMO_KEY",
            "api": "anthropic-messages",
            "compat": { "supportsLongCacheRetention": false },
            "models": [{
                "id": "demo-model",
                "reasoning": true,
                "input": ["text"],
                "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
                "contextWindow": 1000,
                "maxTokens": 100,
            }],
        },
    }));
    let registry = fixture.registry().await;
    assert_eq!(registry.get_error(), None);
    let model = registry.find("demo", "demo-model").expect("model");
    assert_eq!(
        compat_field(&model, "supportsLongCacheRetention"),
        json!(false)
    );
}

#[tokio::test]
async fn model_level_base_url_overrides_provider_level_base_url() {
    let fixture = Fixture::new();
    fixture.write_raw_models_json(json!({
        "opencode-go": {
            "baseUrl": "https://opencode.ai/zen/go/v1",
            "apiKey": "TEST_KEY",
            "models": [
                {
                    "id": "minimax-m2.5",
                    "api": "anthropic-messages",
                    "baseUrl": "https://opencode.ai/zen/go",
                    "reasoning": true,
                    "input": ["text"],
                    "cost": { "input": 0.3, "output": 1.2, "cacheRead": 0.03, "cacheWrite": 0 },
                    "contextWindow": 204800,
                    "maxTokens": 131072,
                },
                {
                    "id": "glm-5",
                    "api": "openai-completions",
                    "reasoning": true,
                    "input": ["text"],
                    "cost": { "input": 1, "output": 3.2, "cacheRead": 0.2, "cacheWrite": 0 },
                    "contextWindow": 204800,
                    "maxTokens": 131072,
                },
            ],
        },
    }));
    let registry = fixture.registry().await;
    assert_eq!(
        registry
            .find("opencode-go", "minimax-m2.5")
            .expect("model")
            .base_url,
        "https://opencode.ai/zen/go"
    );
    assert_eq!(
        registry
            .find("opencode-go", "glm-5")
            .expect("model")
            .base_url,
        "https://opencode.ai/zen/go/v1"
    );
}

#[tokio::test]
async fn model_overrides_still_apply_when_provider_also_defines_models() {
    let fixture = Fixture::new();
    fixture.write_raw_models_json(json!({
        "openrouter": {
            "baseUrl": "https://my-proxy.example.com/v1",
            "apiKey": "OPENROUTER_API_KEY",
            "api": "openai-completions",
            "models": [{
                "id": "custom/openrouter-model",
                "name": "Custom OpenRouter Model",
                "reasoning": false,
                "input": ["text"],
                "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
                "contextWindow": 128000,
                "maxTokens": 16384,
            }],
            "modelOverrides": {
                "anthropic/claude-sonnet-4": { "name": "Overridden Built-in Sonnet" },
            },
        },
    }));
    let registry = fixture.registry().await;
    let models = models_for_provider(&registry, "openrouter");
    assert!(
        models
            .iter()
            .any(|model| model.id == "custom/openrouter-model")
    );
    assert!(
        models
            .iter()
            .any(|model| model.id == "anthropic/claude-sonnet-4"
                && model.name == "Overridden Built-in Sonnet")
    );
}

#[tokio::test]
async fn refresh_reloads_merged_custom_models_from_disk() {
    let fixture = Fixture::new();
    fixture.write_raw_models_json(json!({
        "anthropic": provider_config("https://first-proxy.example.com/v1", &["claude-custom"], "anthropic-messages"),
    }));
    let registry = fixture.registry().await;
    assert!(
        models_for_provider(&registry, "anthropic")
            .iter()
            .any(|model| model.id == "claude-custom")
    );

    fixture.write_raw_models_json(json!({
        "anthropic": provider_config("https://second-proxy.example.com/v1", &["claude-custom-2"], "anthropic-messages"),
    }));
    registry.refresh(None).await;

    let models = models_for_provider(&registry, "anthropic");
    assert!(!models.iter().any(|model| model.id == "claude-custom"));
    assert!(models.iter().any(|model| model.id == "claude-custom-2"));
    assert!(models.iter().any(|model| model.id.contains("claude")));
}

#[tokio::test]
async fn removing_custom_models_keeps_built_in_provider_models() {
    let fixture = Fixture::new();
    fixture.write_raw_models_json(json!({
        "anthropic": provider_config("https://proxy.example.com/v1", &["claude-custom"], "anthropic-messages"),
    }));
    let registry = fixture.registry().await;
    assert!(
        models_for_provider(&registry, "anthropic")
            .iter()
            .any(|model| model.id == "claude-custom")
    );

    fixture.write_raw_models_json(json!({}));
    registry.refresh(None).await;

    let models = models_for_provider(&registry, "anthropic");
    assert!(!models.iter().any(|model| model.id == "claude-custom"));
    assert!(models.iter().any(|model| model.id.contains("claude")));
}

// ---------------------------------------------------------------------------
// modelOverrides (per-model customization)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn model_override_applies_to_a_single_built_in_model() {
    let fixture = Fixture::new();
    fixture.write_raw_models_json(json!({
        "openrouter": {
            "modelOverrides": { "anthropic/claude-sonnet-4": { "name": "Custom Sonnet Name" } },
        },
    }));
    let registry = fixture.registry().await;
    let models = models_for_provider(&registry, "openrouter");
    let sonnet = models
        .iter()
        .find(|model| model.id == "anthropic/claude-sonnet-4")
        .expect("sonnet");
    assert_eq!(sonnet.name, "Custom Sonnet Name");
    let opus = models
        .iter()
        .find(|model| model.id == "anthropic/claude-opus-4")
        .expect("opus");
    assert_ne!(opus.name, "Custom Sonnet Name");
}

#[tokio::test]
async fn custom_model_and_model_override_carry_sampling_params() {
    let fixture = Fixture::new();
    fixture.write_raw_models_json(json!({
        "openrouter": {
            "baseUrl": "https://my-proxy.example.com/v1",
            "api": "openai-completions",
            "models": [{
                "id": "custom/sampling-model",
                "samplingParams": { "temperature": 1, "top_p": 0.95, "top_k": 0 },
            }],
            "modelOverrides": { "anthropic/claude-sonnet-4": { "samplingParams": { "top_p": 0.9 } } },
        },
    }));
    let registry = fixture.registry().await;
    let models = models_for_provider(&registry, "openrouter");
    let custom = models
        .iter()
        .find(|model| model.id == "custom/sampling-model")
        .expect("custom");
    assert_eq!(
        serde_json::to_value(custom.sampling_params.clone()).expect("params"),
        json!({ "temperature": 1, "top_p": 0.95, "top_k": 0 })
    );
    let sonnet = models
        .iter()
        .find(|model| model.id == "anthropic/claude-sonnet-4")
        .expect("sonnet");
    assert_eq!(
        serde_json::to_value(sonnet.sampling_params.clone()).expect("params"),
        json!({ "top_p": 0.9 })
    );
    let opus = models
        .iter()
        .find(|model| model.id == "anthropic/claude-opus-4")
        .expect("opus");
    assert!(opus.sampling_params.is_none());
}

#[tokio::test]
async fn model_override_with_compat_open_router_routing() {
    let fixture = Fixture::new();
    fixture.write_raw_models_json(json!({
        "openrouter": {
            "modelOverrides": {
                "anthropic/claude-sonnet-4": { "compat": { "openRouterRouting": { "only": ["amazon-bedrock"] } } },
            },
        },
    }));
    let registry = fixture.registry().await;
    let sonnet = registry
        .find("openrouter", "anthropic/claude-sonnet-4")
        .expect("sonnet");
    assert_eq!(
        compat_field(&sonnet, "openRouterRouting"),
        json!({ "only": ["amazon-bedrock"] })
    );
}

#[tokio::test]
async fn model_override_deep_merges_compat_settings() {
    let fixture = Fixture::new();
    fixture.write_raw_models_json(json!({
        "openrouter": {
            "modelOverrides": {
                "anthropic/claude-sonnet-4": {
                    "compat": { "openRouterRouting": { "order": ["anthropic", "together"] } },
                },
            },
        },
    }));
    let registry = fixture.registry().await;
    let sonnet = registry
        .find("openrouter", "anthropic/claude-sonnet-4")
        .expect("sonnet");
    assert_eq!(
        compat_field(&sonnet, "openRouterRouting"),
        json!({ "order": ["anthropic", "together"] })
    );
}

#[tokio::test]
async fn multiple_model_overrides_on_same_provider() {
    let fixture = Fixture::new();
    fixture.write_raw_models_json(json!({
        "openrouter": {
            "modelOverrides": {
                "anthropic/claude-sonnet-4": { "compat": { "openRouterRouting": { "only": ["amazon-bedrock"] } } },
                "anthropic/claude-opus-4": { "compat": { "openRouterRouting": { "only": ["anthropic"] } } },
            },
        },
    }));
    let registry = fixture.registry().await;
    let sonnet = registry
        .find("openrouter", "anthropic/claude-sonnet-4")
        .expect("sonnet");
    let opus = registry
        .find("openrouter", "anthropic/claude-opus-4")
        .expect("opus");
    assert_eq!(
        compat_field(&sonnet, "openRouterRouting"),
        json!({ "only": ["amazon-bedrock"] })
    );
    assert_eq!(
        compat_field(&opus, "openRouterRouting"),
        json!({ "only": ["anthropic"] })
    );
}

#[tokio::test]
async fn model_override_combined_with_base_url_override() {
    let fixture = Fixture::new();
    fixture.write_raw_models_json(json!({
        "openrouter": {
            "baseUrl": "https://my-proxy.example.com/v1",
            "modelOverrides": { "anthropic/claude-sonnet-4": { "name": "Proxied Sonnet" } },
        },
    }));
    let registry = fixture.registry().await;
    let sonnet = registry
        .find("openrouter", "anthropic/claude-sonnet-4")
        .expect("sonnet");
    assert_eq!(sonnet.base_url, "https://my-proxy.example.com/v1");
    assert_eq!(sonnet.name, "Proxied Sonnet");
    let opus = registry
        .find("openrouter", "anthropic/claude-opus-4")
        .expect("opus");
    assert_eq!(opus.base_url, "https://my-proxy.example.com/v1");
    assert_ne!(opus.name, "Proxied Sonnet");
}

#[tokio::test]
async fn model_override_for_non_existent_model_id_is_ignored() {
    let fixture = Fixture::new();
    fixture.write_raw_models_json(json!({
        "openrouter": {
            "modelOverrides": { "nonexistent/model-id": { "name": "This should not appear" } },
        },
    }));
    let registry = fixture.registry().await;
    assert!(
        !models_for_provider(&registry, "openrouter")
            .iter()
            .any(|model| model.id == "nonexistent/model-id")
    );
    assert_eq!(registry.get_error(), None);
}

#[tokio::test]
async fn model_override_can_change_cost_fields_partially() {
    let fixture = Fixture::new();
    fixture.write_raw_models_json(json!({
        "openrouter": {
            "modelOverrides": { "anthropic/claude-sonnet-4": { "cost": { "input": 99 } } },
        },
    }));
    let registry = fixture.registry().await;
    let sonnet = registry
        .find("openrouter", "anthropic/claude-sonnet-4")
        .expect("sonnet");
    assert_eq!(sonnet.cost.input, 99.0);
    assert!(sonnet.cost.output > 0.0);
}

#[tokio::test]
async fn model_override_can_add_headers_at_request_time() {
    let fixture = Fixture::new();
    fixture.write_raw_models_json(json!({
        "openrouter": {
            "modelOverrides": {
                "anthropic/claude-sonnet-4": { "headers": { "X-Custom-Model-Header": "value" } },
            },
        },
    }));
    let registry = fixture.registry().await;
    let sonnet = registry
        .find("openrouter", "anthropic/claude-sonnet-4")
        .expect("sonnet");
    let auth = registry.get_api_key_and_headers(&sonnet).await;
    assert_eq!(
        headers_of(&auth)
            .get("X-Custom-Model-Header")
            .map(String::as_str),
        Some("value")
    );
}

#[tokio::test]
async fn refresh_picks_up_model_override_changes() {
    let fixture = Fixture::new();
    fixture.write_raw_models_json(json!({
        "openrouter": { "modelOverrides": { "anthropic/claude-sonnet-4": { "name": "First Name" } } },
    }));
    let registry = fixture.registry().await;
    assert_eq!(
        registry
            .find("openrouter", "anthropic/claude-sonnet-4")
            .expect("sonnet")
            .name,
        "First Name"
    );

    fixture.write_raw_models_json(json!({
        "openrouter": { "modelOverrides": { "anthropic/claude-sonnet-4": { "name": "Second Name" } } },
    }));
    registry.refresh(None).await;

    assert_eq!(
        registry
            .find("openrouter", "anthropic/claude-sonnet-4")
            .expect("sonnet")
            .name,
        "Second Name"
    );
}

#[tokio::test]
async fn removing_model_override_restores_built_in_values() {
    let fixture = Fixture::new();
    fixture.write_raw_models_json(json!({
        "openrouter": { "modelOverrides": { "anthropic/claude-sonnet-4": { "name": "Custom Name" } } },
    }));
    let registry = fixture.registry().await;
    assert_eq!(
        registry
            .find("openrouter", "anthropic/claude-sonnet-4")
            .expect("sonnet")
            .name,
        "Custom Name"
    );

    fixture.write_raw_models_json(json!({}));
    registry.refresh(None).await;

    assert_ne!(
        registry
            .find("openrouter", "anthropic/claude-sonnet-4")
            .expect("sonnet")
            .name,
        "Custom Name"
    );
}

// ---------------------------------------------------------------------------
// provider display names
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_provider_display_name_resolves_built_in_and_fallback_names() {
    let fixture = Fixture::new();
    let registry = fixture.registry().await;
    assert_eq!(registry.get_provider_display_name("openai"), "OpenAI");
    assert_eq!(
        registry.get_provider_display_name("github-copilot"),
        "GitHub Copilot"
    );
    assert_eq!(registry.get_provider_display_name("zai"), "Z.AI");
    assert_eq!(
        registry.get_provider_display_name("unknown-provider"),
        "unknown-provider"
    );
}

#[tokio::test]
async fn models_json_name_wins_over_the_built_in_provider_name() {
    let fixture = Fixture::new();
    fixture.write_raw_models_json(json!({
        "openai": { "name": "Named Provider", "baseUrl": "https://provider.test/v1" },
    }));
    let registry = fixture.registry().await;
    assert_eq!(
        registry.get_provider_display_name("openai"),
        "Named Provider"
    );
}

// ---------------------------------------------------------------------------
// API key resolution
// ---------------------------------------------------------------------------

fn provider_with_api_key(api_key: &str) -> Value {
    json!({
        "baseUrl": "https://example.com/v1",
        "apiKey": api_key,
        "api": "anthropic-messages",
        "models": [{
            "id": "test-model",
            "name": "Test Model",
            "reasoning": false,
            "input": ["text"],
            "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
            "contextWindow": 100000,
            "maxTokens": 8000,
        }],
    })
}

async fn api_key_for(api_key: &str) -> Option<String> {
    let fixture = Fixture::new();
    fixture.write_raw_models_json(json!({ "custom-provider": provider_with_api_key(api_key) }));
    let registry = fixture.registry().await;
    registry.get_api_key_for_provider("custom-provider").await
}

#[tokio::test]
async fn api_key_with_bang_prefix_executes_command_and_uses_stdout() {
    assert_eq!(
        api_key_for("!echo test-api-key-from-command")
            .await
            .as_deref(),
        Some("test-api-key-from-command")
    );
}

#[tokio::test]
async fn api_key_with_bang_prefix_trims_whitespace() {
    assert_eq!(
        api_key_for("!echo '  spaced-key  '").await.as_deref(),
        Some("spaced-key")
    );
}

#[tokio::test]
async fn api_key_with_bang_prefix_handles_multiline_output() {
    assert_eq!(
        api_key_for("!printf 'line1\\nline2'").await.as_deref(),
        Some("line1\nline2")
    );
}

#[tokio::test]
async fn api_key_with_bang_prefix_returns_none_on_command_failure() {
    assert_eq!(api_key_for("!exit 1").await, None);
}

#[tokio::test]
async fn api_key_with_bang_prefix_returns_none_on_nonexistent_command() {
    assert_eq!(api_key_for("!nonexistent-command-12345").await, None);
}

#[tokio::test]
async fn api_key_with_bang_prefix_returns_none_on_empty_output() {
    assert_eq!(api_key_for("!printf ''").await, None);
}

#[tokio::test]
async fn api_key_with_dollar_prefix_resolves_to_env_value() {
    unsafe { std::env::set_var("TEST_API_KEY_RS_DOLLAR", "env-api-key-value") };
    assert_eq!(
        api_key_for("$TEST_API_KEY_RS_DOLLAR").await.as_deref(),
        Some("env-api-key-value")
    );
    unsafe { std::env::remove_var("TEST_API_KEY_RS_DOLLAR") };
}

#[tokio::test]
async fn api_key_with_braced_env_syntax_resolves_to_env_value() {
    unsafe { std::env::set_var("TEST_BRACED_API_KEY_RS", "braced-env-api-key-value") };
    assert_eq!(
        api_key_for("${TEST_BRACED_API_KEY_RS}").await.as_deref(),
        Some("braced-env-api-key-value")
    );
    unsafe { std::env::remove_var("TEST_BRACED_API_KEY_RS") };
}

#[tokio::test]
async fn api_key_interpolates_braced_env_references_inside_literals() {
    unsafe { std::env::set_var("TEST_INTERPOLATED_PART_A_RS", "left") };
    unsafe { std::env::set_var("TEST_INTERPOLATED_PART_B_RS", "right") };
    assert_eq!(
        api_key_for("${TEST_INTERPOLATED_PART_A_RS}_${TEST_INTERPOLATED_PART_B_RS}")
            .await
            .as_deref(),
        Some("left_right")
    );
    unsafe { std::env::remove_var("TEST_INTERPOLATED_PART_A_RS") };
    unsafe { std::env::remove_var("TEST_INTERPOLATED_PART_B_RS") };
}

#[tokio::test]
async fn api_key_with_double_dollar_escapes_a_leading_dollar() {
    assert_eq!(
        api_key_for("$$TEST_API_KEY_RS_ESCAPED").await.as_deref(),
        Some("$TEST_API_KEY_RS_ESCAPED")
    );
}

#[tokio::test]
async fn api_key_with_dollar_bang_escapes_a_literal_bang() {
    unsafe { std::env::set_var("TEST_API_KEY_RS_BANG", "env-api-key-value") };
    assert_eq!(
        api_key_for("$!literal-$TEST_API_KEY_RS_BANG")
            .await
            .as_deref(),
        Some("!literal-env-api-key-value")
    );
    unsafe { std::env::remove_var("TEST_API_KEY_RS_BANG") };
}

#[tokio::test]
async fn plain_api_key_is_used_directly_even_when_it_matches_an_env_var() {
    unsafe { std::env::set_var("TEST_API_KEY_RS_PLAIN", "env-api-key-value") };
    assert_eq!(
        api_key_for("TEST_API_KEY_RS_PLAIN").await.as_deref(),
        Some("TEST_API_KEY_RS_PLAIN")
    );
    unsafe { std::env::remove_var("TEST_API_KEY_RS_PLAIN") };
}

#[tokio::test]
async fn api_key_as_literal_value_is_used_directly_when_not_an_env_var() {
    assert_eq!(
        api_key_for("literal_api_key_value").await.as_deref(),
        Some("literal_api_key_value")
    );
}

#[tokio::test]
async fn api_key_command_can_use_shell_features_like_pipes() {
    assert_eq!(
        api_key_for("!echo 'hello world' | tr ' ' '-'")
            .await
            .as_deref(),
        Some("hello-world")
    );
}

// -- request-time resolution ------------------------------------------------

fn counting_command(counter: &std::path::Path, tail: &str) -> String {
    let path = sh_path(&counter.to_string_lossy());
    format!("!sh -c 'count=$(cat \"{path}\"); echo $((count + 1)) > \"{path}\"; {tail}'")
}

fn counter_value(counter: &std::path::Path) -> i64 {
    std::fs::read_to_string(counter)
        .expect("counter")
        .trim()
        .parse()
        .expect("number")
}

#[tokio::test]
async fn command_is_executed_on_every_provider_lookup() {
    let fixture = Fixture::new();
    let counter = fixture.dir.path().join("counter");
    std::fs::write(&counter, "0").expect("seed");
    fixture.write_raw_models_json(json!({
        "custom-provider": provider_with_api_key(&counting_command(&counter, "echo \"key-value\"")),
    }));
    let registry = fixture.registry().await;
    for _ in 0..3 {
        registry.get_api_key_for_provider("custom-provider").await;
    }
    assert_eq!(counter_value(&counter), 3);
}

#[tokio::test]
async fn commands_are_re_executed_across_registry_instances() {
    let fixture = Fixture::new();
    let counter = fixture.dir.path().join("counter");
    std::fs::write(&counter, "0").expect("seed");
    fixture.write_raw_models_json(json!({
        "custom-provider": provider_with_api_key(&counting_command(&counter, "echo \"key-value\"")),
    }));
    let first = fixture.registry().await;
    first.get_api_key_for_provider("custom-provider").await;
    let second = fixture.registry().await;
    second.get_api_key_for_provider("custom-provider").await;
    assert_eq!(counter_value(&counter), 2);
}

#[tokio::test]
async fn different_commands_resolve_independently() {
    let fixture = Fixture::new();
    fixture.write_raw_models_json(json!({
        "provider-a": provider_with_api_key("!echo key-a"),
        "provider-b": provider_with_api_key("!echo key-b"),
    }));
    let registry = fixture.registry().await;
    assert_eq!(
        registry
            .get_api_key_for_provider("provider-a")
            .await
            .as_deref(),
        Some("key-a")
    );
    assert_eq!(
        registry
            .get_api_key_for_provider("provider-b")
            .await
            .as_deref(),
        Some("key-b")
    );
}

#[tokio::test]
async fn failed_commands_are_retried() {
    let fixture = Fixture::new();
    let counter = fixture.dir.path().join("counter");
    std::fs::write(&counter, "0").expect("seed");
    fixture.write_raw_models_json(json!({
        "custom-provider": provider_with_api_key(&counting_command(&counter, "exit 1")),
    }));
    let registry = fixture.registry().await;
    assert_eq!(
        registry.get_api_key_for_provider("custom-provider").await,
        None
    );
    assert_eq!(
        registry.get_api_key_for_provider("custom-provider").await,
        None
    );
    assert_eq!(counter_value(&counter), 2);
}

#[tokio::test]
async fn provider_auth_status_reports_api_key_environment_variables() {
    unsafe { std::env::set_var("TEST_API_KEY_STATUS_RS", "status-test-key") };
    let fixture = Fixture::new();
    fixture.write_raw_models_json(json!({
        "custom-provider": provider_with_api_key("$TEST_API_KEY_STATUS_RS"),
    }));
    let registry = fixture.registry().await;
    assert_eq!(
        registry.get_provider_auth_status("custom-provider"),
        AuthStatus {
            configured: true,
            source: Some(AuthStatusSource::Environment),
            label: Some("TEST_API_KEY_STATUS_RS".to_owned()),
        }
    );
    unsafe { std::env::remove_var("TEST_API_KEY_STATUS_RS") };
}

#[tokio::test]
async fn provider_auth_status_reports_interpolated_api_key_environment_variables() {
    unsafe { std::env::set_var("TEST_API_KEY_STATUS_PART_A_RS", "left") };
    unsafe { std::env::set_var("TEST_API_KEY_STATUS_PART_B_RS", "right") };
    let fixture = Fixture::new();
    fixture.write_raw_models_json(json!({
        "custom-provider": provider_with_api_key(
            "${TEST_API_KEY_STATUS_PART_A_RS}_${TEST_API_KEY_STATUS_PART_B_RS}",
        ),
    }));
    let registry = fixture.registry().await;
    assert_eq!(
        registry.get_provider_auth_status("custom-provider"),
        AuthStatus {
            configured: true,
            source: Some(AuthStatusSource::Environment),
            label: Some("TEST_API_KEY_STATUS_PART_A_RS, TEST_API_KEY_STATUS_PART_B_RS".to_owned()),
        }
    );
    unsafe { std::env::remove_var("TEST_API_KEY_STATUS_PART_A_RS") };
    unsafe { std::env::remove_var("TEST_API_KEY_STATUS_PART_B_RS") };
}

#[tokio::test]
async fn provider_auth_status_reports_non_env_api_key_values_as_a_config_key() {
    let fixture = Fixture::new();
    fixture.write_raw_models_json(json!({
        "custom-provider": provider_with_api_key("literal_api_key_value"),
    }));
    let registry = fixture.registry().await;
    assert_eq!(
        registry.get_provider_auth_status("custom-provider"),
        AuthStatus::configured(AuthStatusSource::ModelsJsonKey)
    );
}

#[tokio::test]
async fn missing_explicit_env_api_key_keeps_provider_unavailable() {
    unsafe { std::env::remove_var("TEST_API_KEY_MISSING_RS") };
    let fixture = Fixture::new();
    fixture.write_raw_models_json(json!({
        "custom-provider": provider_with_api_key("$TEST_API_KEY_MISSING_RS"),
    }));
    let registry = fixture.registry().await;
    assert_eq!(
        registry.get_provider_auth_status("custom-provider"),
        AuthStatus::unconfigured()
    );
    assert!(
        !registry
            .get_available()
            .iter()
            .any(|model| model.provider == "custom-provider")
    );
}

#[tokio::test]
async fn provider_auth_status_reports_command_api_key_values_without_executing_them() {
    let fixture = Fixture::new();
    let counter = fixture.dir.path().join("status-counter");
    std::fs::write(&counter, "0").expect("seed");
    let path = sh_path(&counter.to_string_lossy());
    fixture.write_raw_models_json(json!({
        "custom-provider": provider_with_api_key(&format!("!sh -c 'echo 1 > \"{path}\"; echo key-value'")),
    }));
    let registry = fixture.registry().await;
    assert_eq!(
        registry.get_provider_auth_status("custom-provider"),
        AuthStatus::configured(AuthStatusSource::ModelsJsonCommand)
    );
    assert_eq!(std::fs::read_to_string(&counter).expect("counter"), "0");
}

#[tokio::test]
async fn environment_variables_are_not_cached() {
    unsafe { std::env::set_var("TEST_API_KEY_CACHE_RS", "first-value") };
    let fixture = Fixture::new();
    fixture.write_raw_models_json(json!({
        "custom-provider": provider_with_api_key("$TEST_API_KEY_CACHE_RS"),
    }));
    let registry = fixture.registry().await;
    assert_eq!(
        registry
            .get_api_key_for_provider("custom-provider")
            .await
            .as_deref(),
        Some("first-value")
    );
    unsafe { std::env::set_var("TEST_API_KEY_CACHE_RS", "second-value") };
    assert_eq!(
        registry
            .get_api_key_for_provider("custom-provider")
            .await
            .as_deref(),
        Some("second-value")
    );
    unsafe { std::env::remove_var("TEST_API_KEY_CACHE_RS") };
}

#[tokio::test]
async fn get_available_does_not_execute_command_backed_api_key_resolution() {
    let fixture = Fixture::new();
    let counter = fixture.dir.path().join("counter");
    std::fs::write(&counter, "0").expect("seed");
    fixture.write_raw_models_json(json!({
        "custom-provider": provider_with_api_key(&counting_command(&counter, "echo \"key-value\"")),
    }));
    let registry = fixture.registry().await;
    assert!(
        registry
            .get_available()
            .iter()
            .any(|model| model.provider == "custom-provider")
    );
    assert_eq!(counter_value(&counter), 0);
}

#[tokio::test]
async fn get_available_filters_github_copilot_oauth_models() {
    let fixture = Fixture::new();
    fixture
        .auth_storage
        .modify(
            "github-copilot",
            Box::new(|_current| {
                Box::pin(async {
                    let mut extra = serde_json::Map::new();
                    extra.insert("availableModelIds".to_owned(), json!(["gpt-4.1"]));
                    Ok(Some(Credential::OAuth(OAuthCredential {
                        refresh: "github-access-token".to_owned(),
                        access:
                            "tid=test;exp=9999999999;proxy-ep=proxy.individual.githubcopilot.com;"
                                .to_owned(),
                        expires: 4_102_444_800_000,
                        extra,
                    })))
                })
            }),
            None,
        )
        .await
        .expect("modify");

    let registry = fixture.registry().await;
    let ids: Vec<String> = registry
        .get_available()
        .into_iter()
        .filter(|model| model.provider == "github-copilot")
        .map(|model| model.id)
        .collect();
    assert_eq!(ids, vec!["gpt-4.1"]);
}

#[tokio::test]
async fn get_api_key_and_headers_resolves_auth_header_on_every_request() {
    let fixture = Fixture::new();
    let token = fixture.dir.path().join("token");
    std::fs::write(&token, "token-1").expect("seed");
    let path = sh_path(&token.to_string_lossy());
    let mut provider = provider_with_api_key(&format!("!sh -c 'cat \"{path}\"'"));
    provider["authHeader"] = json!(true);
    fixture.write_raw_models_json(json!({ "custom-provider": provider }));
    let registry = fixture.registry().await;
    let model = registry
        .find("custom-provider", "test-model")
        .expect("model");

    assert_eq!(
        registry.get_api_key_and_headers(&model).await,
        ResolvedRequestAuth::Ok {
            api_key: Some("token-1".to_owned()),
            headers: Some(BTreeMap::from([(
                "Authorization".to_owned(),
                Some("Bearer token-1".to_owned())
            )])),
            base_url: None,
            env: None,
        }
    );

    std::fs::write(&token, "token-2").expect("rewrite");
    assert_eq!(
        registry.get_api_key_and_headers(&model).await,
        ResolvedRequestAuth::Ok {
            api_key: Some("token-2".to_owned()),
            headers: Some(BTreeMap::from([(
                "Authorization".to_owned(),
                Some("Bearer token-2".to_owned())
            )])),
            base_url: None,
            env: None,
        }
    );
}

#[tokio::test]
async fn get_api_key_and_headers_resolves_configured_auth_exactly_once() {
    let fixture = Fixture::new();
    let counter = fixture.dir.path().join("auth-counter");
    std::fs::write(&counter, "0").expect("seed");
    let path = sh_path(&counter.to_string_lossy());
    let mut provider = provider_with_api_key(&format!(
        "!sh -c 'count=$(cat \"{path}\"); count=$((count + 1)); echo \"$count\" > \"{path}\"; echo \"token-$count\"'"
    ));
    provider["authHeader"] = json!(true);
    fixture.write_raw_models_json(json!({ "custom-provider": provider }));
    let registry = fixture.registry().await;
    let model = registry
        .find("custom-provider", "test-model")
        .expect("model");
    assert_eq!(
        registry.get_api_key_and_headers(&model).await,
        ResolvedRequestAuth::Ok {
            api_key: Some("token-1".to_owned()),
            headers: Some(BTreeMap::from([(
                "Authorization".to_owned(),
                Some("Bearer token-1".to_owned())
            )])),
            base_url: None,
            env: None,
        }
    );
    assert_eq!(counter_value(&counter), 1);
}

#[tokio::test]
async fn stored_credentials_bypass_lower_priority_configured_auth_commands() {
    let fixture = Fixture::new();
    let counter = fixture.dir.path().join("fallback-counter");
    std::fs::write(&counter, "0").expect("seed");
    let path = sh_path(&counter.to_string_lossy());
    fixture.write_raw_models_json(json!({
        "custom-provider": provider_with_api_key(&format!("!sh -c 'echo 1 > \"{path}\"; echo fallback-key'")),
    }));
    fixture
        .auth_storage
        .modify(
            "custom-provider",
            Box::new(|_current| {
                Box::pin(async {
                    Ok(Some(Credential::ApiKey(ApiKeyCredential {
                        key: Some("stored-key".to_owned()),
                        env: None,
                    })))
                })
            }),
            None,
        )
        .await
        .expect("modify");

    let registry = fixture.registry().await;
    let model = registry
        .find("custom-provider", "test-model")
        .expect("model");
    match registry.get_api_key_and_headers(&model).await {
        ResolvedRequestAuth::Ok { api_key, .. } => {
            assert_eq!(api_key.as_deref(), Some("stored-key"));
        }
        ResolvedRequestAuth::Err(error) => panic!("expected ok, got {error}"),
    }
    assert_eq!(
        std::fs::read_to_string(&counter).expect("counter").trim(),
        "0"
    );
}

#[tokio::test]
async fn get_api_key_and_headers_preserves_the_legacy_missing_key_auth_header_error() {
    let fixture = Fixture::new();
    fixture.write_raw_models_json(json!({
        "custom-provider": {
            "baseUrl": "https://example.test/v1",
            "api": "openai-completions",
            "authHeader": true,
            "models": [{ "id": "test-model" }],
        },
    }));
    let registry = fixture.registry().await;
    let model = registry
        .find("custom-provider", "test-model")
        .expect("model");
    assert_eq!(
        registry.get_api_key_and_headers(&model).await,
        ResolvedRequestAuth::Err("No API key found for \"custom-provider\"".to_owned())
    );
}

#[tokio::test]
async fn get_api_key_and_headers_returns_an_error_for_failed_auth_header_resolution() {
    let fixture = Fixture::new();
    let mut provider = provider_with_api_key("!exit 1");
    provider["authHeader"] = json!(true);
    fixture.write_raw_models_json(json!({ "custom-provider": provider }));
    let registry = fixture.registry().await;
    let model = registry
        .find("custom-provider", "test-model")
        .expect("model");
    match registry.get_api_key_and_headers(&model).await {
        ResolvedRequestAuth::Err(error) => assert!(
            error.contains("Failed to resolve API key for provider \"custom-provider\""),
            "unexpected error: {error}"
        ),
        ResolvedRequestAuth::Ok { .. } => panic!("expected an error"),
    }
}

/// Keeps the `ModelCompat` import honest: `compat_value` round-trips through it.
#[allow(dead_code)]
fn compat_kind(compat: &ModelCompat) -> &'static str {
    match compat {
        ModelCompat::OpenAICompletions(_) => "openai-completions",
        ModelCompat::OpenAIResponses(_) => "openai-responses",
        ModelCompat::AnthropicMessages(_) => "anthropic-messages",
        ModelCompat::Bedrock(_) => "bedrock",
        ModelCompat::Other(_) => "other",
    }
}
