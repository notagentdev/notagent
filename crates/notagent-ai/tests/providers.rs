use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use notagent_ai::auth::types::{
    ApiKeyAuth, ApiKeyAuthInput, ApiKeyCredential, AuthContext, AuthError, AuthEvent, AuthPrompt,
    AuthPromptKind, BoxFuture, Credential, ProviderAuthInteraction,
};
use notagent_ai::models::{CreateModelsOptions, Provider, create_models};
use notagent_ai::providers::all::{builtin_models, builtin_providers};
use notagent_ai::providers::amazon_bedrock::amazon_bedrock_provider;
use notagent_ai::providers::anthropic::anthropic_provider;
use notagent_ai::providers::cloudflare_ai_gateway::cloudflare_ai_gateway_provider;
use notagent_ai::providers::cloudflare_stream::{cloudflare_streams, resolve_cloudflare_model};
use notagent_ai::providers::cloudflare_workers_ai::cloudflare_workers_ai_provider;
use notagent_ai::providers::google_vertex::google_vertex_provider;
use notagent_ai::types::{
    Context, Message, Modality, Model, ModelCost, ProviderEnv, ProviderStreams,
    SimpleStreamOptions, StreamOptions, UserContent, UserMessage,
};
use notagent_ai::utils::event_stream::create_assistant_message_event_stream;
use serde_json::{Value, json};

// ---------------------------------------------------------------------------
// Test doubles
// ---------------------------------------------------------------------------

/// `fakeAuthContext(env, files)`
struct FakeAuthContext {
    env: ProviderEnv,
    files: Vec<String>,
}

impl FakeAuthContext {
    fn shared(env: &[(&str, &str)], files: &[&str]) -> Arc<dyn AuthContext> {
        Arc::new(FakeAuthContext {
            env: env
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect(),
            files: files.iter().map(|path| path.to_string()).collect(),
        })
    }
}

impl AuthContext for FakeAuthContext {
    fn env(&self, name: &str) -> BoxFuture<'_, Option<String>> {
        let value = self.env.get(name).cloned();
        Box::pin(async move { value })
    }

    fn file_exists(&self, path: &str) -> BoxFuture<'_, bool> {
        let exists = self.files.iter().any(|file| file == path);
        Box::pin(async move { exists })
    }
}

/// Answers prompts from a queue and records notifications, like the arrow functions the
struct ScriptedInteraction {
    answers: Mutex<Vec<String>>,
    events: Mutex<Vec<AuthEvent>>,
    prompts: Mutex<Vec<AuthPromptKind>>,
}

impl ScriptedInteraction {
    fn new(answers: &[&str]) -> Arc<ScriptedInteraction> {
        Arc::new(ScriptedInteraction {
            answers: Mutex::new(
                answers
                    .iter()
                    .rev()
                    .map(|answer| answer.to_string())
                    .collect(),
            ),
            events: Mutex::new(Vec::new()),
            prompts: Mutex::new(Vec::new()),
        })
    }
}

impl notagent_ai::auth::types::AuthInteraction for ScriptedInteraction {
    fn signal(&self) -> Option<tokio_util::sync::CancellationToken> {
        None
    }

    fn prompt(&self, prompt: AuthPrompt) -> BoxFuture<'_, Result<String, AuthError>> {
        self.prompts.lock().expect("prompts").push(prompt.kind);
        let answer = self.answers.lock().expect("answers").pop();
        Box::pin(
            async move { answer.ok_or_else(|| AuthError("no scripted answer left".to_string())) },
        )
    }

    fn notify(&self, event: AuthEvent) {
        self.events.lock().expect("events").push(event);
    }
}

async fn login(
    auth: &dyn ApiKeyAuth,
    interaction: &Arc<ScriptedInteraction>,
) -> Result<ApiKeyCredential, AuthError> {
    let provider_interaction = ProviderAuthInteraction {
        interaction: interaction.as_ref(),
        signal: tokio_util::sync::CancellationToken::new(),
    };
    auth.login(&provider_interaction)
        .expect("the provider offers a login flow")
        .await
}

fn context() -> Context {
    Context {
        messages: vec![Message::User(UserMessage {
            content: UserContent::Text("hi".to_string()),
            timestamp: 1,
        })],
        ..Default::default()
    }
}

/// A `fetch` that fails immediately, so the dispatch check never leaves the process.
#[derive(Debug)]
struct FailingFetch;

impl notagent_ai::utils::fetch::FetchFn for FailingFetch {
    fn fetch(
        &self,
        _request: notagent_ai::utils::fetch::FetchRequest,
    ) -> notagent_ai::utils::fetch::FetchFuture {
        Box::pin(async {
            Err(notagent_ai::utils::fetch::FetchError {
                message: "offline dispatch check".to_string(),
            })
        })
    }
}

// ---------------------------------------------------------------------------
// builtin providers
// ---------------------------------------------------------------------------

#[tokio::test]
async fn builtin_models_registers_every_builtin_provider_with_models() {
    let models = builtin_models(None);
    let providers = models.get_providers();
    assert_eq!(providers.len(), builtin_providers().len());
    assert!(
        providers
            .iter()
            .any(|provider| provider.id() == "anthropic")
    );

    let anthropic = models.get_model("anthropic", "claude-haiku-4-5");
    assert_eq!(anthropic.expect("model").api, "anthropic-messages");

    assert!(models.get_models(None).len() > 500);

    // Static providers list models immediately. The rest ask a server what it
    // holds, so a static entry would name a model that server may not have
    // loaded — for the local runtimes, may not even have been pulled.
    for provider in &providers {
        let list = models.get_models(Some(provider.id()));
        if DYNAMIC_PROVIDERS.contains(&provider.id()) {
            assert!(list.is_empty(), "{} lists nothing up front", provider.id());
        } else {
            assert!(!list.is_empty(), "{} has models", provider.id());
        }
        assert!(list.iter().all(|model| model.provider == provider.id()));
    }
}

/// Providers whose catalogue comes from a server rather than from a snapshot.
const DYNAMIC_PROVIDERS: [&str; 5] = ["radius", "mtplx", "ollama", "lmstudio", "custom"];

/// They are checked on their own (see `cline_pass_is_an_openai_compatible_provider`,
/// `tests/mtplx_provider.rs` and `tests/openai_compatible_providers.rs`).
const PORT_ADDED_PROVIDERS: [&str; 5] = ["cline-pass", "mtplx", "ollama", "lmstudio", "custom"];

#[test]
fn every_builtin_provider_matches_the_catalog_fixture() {
    let fixture: Vec<Value> = include_str!("fixtures/providers.jsonl")
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("fixture line"))
        .collect();
    let providers: Vec<_> = builtin_providers()
        .into_iter()
        .filter(|provider| !PORT_ADDED_PROVIDERS.contains(&provider.id()))
        .collect();
    assert_eq!(providers.len(), fixture.len());

    for (provider, expected) in providers.iter().zip(&fixture) {
        let id = provider.id();
        assert_eq!(id, expected["id"].as_str().expect("id"));
        assert_eq!(provider.name(), expected["name"].as_str().expect("name"));
        assert_eq!(
            provider.base_url(),
            expected["baseUrl"].as_str(),
            "{id} base url"
        );
        assert_eq!(provider.headers().is_none(), expected["headers"].is_null());
        assert_eq!(
            provider.auth().api_key.as_ref().map(|auth| auth.name()),
            expected["apiKey"]["name"].as_str(),
            "{id} api key auth"
        );
        match provider.auth().oauth.as_ref() {
            Some(oauth) => {
                assert_eq!(oauth.name(), expected["oauth"]["name"], "{id} oauth name");
                assert_eq!(
                    oauth.is_subscription(),
                    expected["oauth"]["isSubscription"] == json!(true),
                    "{id} subscription"
                );
                assert_eq!(
                    oauth.login_label(),
                    expected["oauth"]["loginLabel"].as_str(),
                    "{id} login label"
                );
            }
            None => assert!(expected["oauth"].is_null(), "{id} has no oauth"),
        }
        assert_eq!(
            provider.is_dynamic(),
            expected["dynamic"] == json!(true),
            "{id} dynamic"
        );

        // is: they are named here rather than written into it (v0.1.16).
        let port_added: &[&str] = match id {
            "zai" => &["glm-5.3", "glm-5.3-flash"],
            "zai-coding-cn" => &["glm-5.3"],
            _ => &[],
        };
        let models: Vec<_> = provider
            .get_models()
            .into_iter()
            .filter(|model| !port_added.contains(&model.id.as_str()))
            .collect();
        assert_eq!(
            models.len(),
            expected["modelCount"].as_u64().expect("count") as usize,
            "{id} model count"
        );
        let ids: Vec<&str> = models.iter().map(|model| model.id.as_str()).collect();
        let expected_ids: Vec<&str> = expected["modelIds"]
            .as_array()
            .expect("ids")
            .iter()
            .map(|id| id.as_str().expect("id"))
            .collect();
        assert_eq!(ids, expected_ids, "{id} model ids");
    }
}

/// Every api a built-in model declares must dispatch. A fake `fetch` keeps the check
/// offline; Bedrock goes through the AWS SDK instead of `fetch` and has its own test.
#[tokio::test]
async fn every_declared_model_api_dispatches() {
    for provider in builtin_providers() {
        let mut seen: Vec<String> = Vec::new();
        for model in provider.get_models() {
            if seen.contains(&model.api) || model.api == "bedrock-converse-stream" {
                continue;
            }
            seen.push(model.api.clone());
            let options = SimpleStreamOptions {
                base: StreamOptions {
                    base: notagent_ai::types::ProviderRequestOptions {
                        api_key: Some("test-key".to_string()),
                        fetch: Some(Arc::new(FailingFetch)),
                        max_retries: Some(0),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                ..Default::default()
            };
            let result = provider
                .stream_simple(&model, &context(), Some(options))
                .result()
                .await;
            let message = result.error_message.unwrap_or_default();
            assert!(
                !message.contains("no API implementation"),
                "{}/{} dispatches ({message})",
                provider.id(),
                model.api
            );
        }

        // so only the api-map providers reject an unknown api.
        if seen.len() > 1 {
            let ghost = Model {
                api: "api-ghost".to_string(),
                ..provider.get_models()[0].clone()
            };
            let result = provider
                .stream_simple(&ghost, &context(), None)
                .result()
                .await;
            assert!(
                result
                    .error_message
                    .unwrap_or_default()
                    .contains("no API implementation"),
                "{} rejects an unknown api",
                provider.id()
            );
        }
    }
}

fn test_model(api: &str, id: &str) -> Model {
    Model {
        id: id.to_string(),
        name: id.to_string(),
        api: api.to_string(),
        provider: "mixed".to_string(),
        base_url: "https://example.test/v1".to_string(),
        reasoning: false,
        thinking_level_map: None,
        input: vec![Modality::Text],
        cost: ModelCost::default(),
        context_window: 10000,
        max_tokens: 1000,
        sampling_params: None,
        headers: None,
        compat: None,
    }
}

#[test]
fn stores_native_constrained_sampling_capabilities_in_model_metadata() {
    let gpt4o = notagent_ai::model_catalog::get_builtin_model("openai", "gpt-4o").expect("gpt-4o");
    let compat = gpt4o.compat.expect("compat");
    let compat = compat.as_openai_responses().expect("openai compat");
    assert_eq!(compat.supports_strict_mode, Some(true));
    assert_eq!(compat.supports_openai_grammar_tools, None);

    let gpt54 =
        notagent_ai::model_catalog::get_builtin_model("openai", "gpt-5.4").expect("gpt-5.4");
    let compat = gpt54.compat.expect("compat");
    let compat = compat.as_openai_responses().expect("openai compat");
    assert_eq!(compat.supports_strict_mode, Some(true));
    assert_eq!(compat.supports_openai_grammar_tools, Some(true));

    let haiku = notagent_ai::model_catalog::get_builtin_model("anthropic", "claude-haiku-4-5")
        .expect("haiku");
    let compat = haiku.compat.expect("compat");
    assert_eq!(
        compat
            .as_anthropic_messages()
            .expect("anthropic compat")
            .supports_strict_tools,
        Some(true)
    );
}

#[test]
fn uses_official_kimi_k3_pricing_for_moonshot_providers() {
    let models = builtin_models(None);
    for provider in ["moonshotai", "moonshotai-cn"] {
        let model = models.get_model(provider, "kimi-k3").expect("kimi-k3");
        assert_eq!(
            model.cost,
            ModelCost {
                input: 3.0,
                output: 15.0,
                cache_read: 0.3,
                cache_write: 0.0,
                tiers: None,
            }
        );
    }
}

#[test]
fn uses_api_equivalent_implied_pricing_for_kimi_coding_subscription_models() {
    let models = builtin_models(None);
    let expected = [
        ("k3", (3.0, 15.0, 0.3, 0.0)),
        ("kimi-for-coding-highspeed", (1.9, 8.0, 0.38, 0.0)),
    ];
    for (model_id, (input, output, cache_read, cache_write)) in expected {
        let model = models.get_model("kimi-coding", model_id).expect("model");
        assert_eq!(
            model.cost,
            ModelCost {
                input,
                output,
                cache_read,
                cache_write,
                tiers: None,
            },
            "{model_id}"
        );
    }
}

// ---------------------------------------------------------------------------
// Anthropic
// ---------------------------------------------------------------------------

#[tokio::test]
async fn resolves_anthropic_bearer_auth_from_env_with_auth_token_precedence() {
    let models = create_models(Some(CreateModelsOptions {
        auth_context: Some(FakeAuthContext::shared(
            &[
                ("ANTHROPIC_AUTH_TOKEN", "auth-token"),
                ("ANTHROPIC_OAUTH_TOKEN", "oauth-token"),
                ("ANTHROPIC_API_KEY", "api-key"),
            ],
            &[],
        )),
        ..Default::default()
    }));
    models.set_provider(anthropic_provider());

    let result = models
        .get_auth_for_provider("anthropic", None)
        .await
        .expect("auth")
        .expect("configured");
    assert_eq!(result.auth.api_key, None);
    assert_eq!(
        result.auth.headers,
        Some(BTreeMap::from([(
            "Authorization".to_string(),
            Some("Bearer auth-token".to_string())
        )]))
    );
    assert_eq!(result.source.as_deref(), Some("ANTHROPIC_AUTH_TOKEN"));
}

#[tokio::test]
async fn preserves_anthropic_oauth_token_precedence_over_the_api_key() {
    let models = create_models(Some(CreateModelsOptions {
        auth_context: Some(FakeAuthContext::shared(
            &[
                ("ANTHROPIC_API_KEY", "key"),
                ("ANTHROPIC_OAUTH_TOKEN", "oauth-token"),
            ],
            &[],
        )),
        ..Default::default()
    }));
    models.set_provider(anthropic_provider());

    let result = models
        .get_auth_for_provider("anthropic", None)
        .await
        .expect("auth")
        .expect("configured");
    assert_eq!(result.auth.api_key.as_deref(), Some("oauth-token"));
    assert_eq!(result.source.as_deref(), Some("ANTHROPIC_OAUTH_TOKEN"));
}

// ---------------------------------------------------------------------------
// Amazon Bedrock
// ---------------------------------------------------------------------------

#[tokio::test]
async fn runs_provider_owned_bedrock_bearer_token_and_aws_profile_login_flows() {
    let provider = amazon_bedrock_provider();
    let auth = provider.auth().api_key.clone().expect("api key auth");

    let bearer = ScriptedInteraction::new(&["bearer-token", "bedrock-token"]);
    assert_eq!(
        login(auth.as_ref(), &bearer).await.expect("credential"),
        ApiKeyCredential {
            key: Some("bedrock-token".to_string()),
            env: None,
        }
    );

    let profile = ScriptedInteraction::new(&["aws-profile", "work"]);
    assert_eq!(
        login(auth.as_ref(), &profile).await.expect("credential"),
        ApiKeyCredential {
            key: None,
            env: Some(ProviderEnv::from([(
                "AWS_PROFILE".to_string(),
                "work".to_string()
            )])),
        }
    );
    {
        let events = profile.events.lock().expect("events");
        assert_eq!(events.len(), 1);
        match &events[0] {
            AuthEvent::Info { links, .. } => assert_eq!(
                links[0].label.as_deref(),
                Some("AWS credential provider chain")
            ),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    let resolved = auth
        .resolve(ApiKeyAuthInput {
            ctx: FakeAuthContext::shared(&[], &[]).as_ref(),
            credential: Some(ApiKeyCredential {
                key: None,
                env: Some(ProviderEnv::from([(
                    "AWS_PROFILE".to_string(),
                    "work".to_string(),
                )])),
            }),
            signal: tokio_util::sync::CancellationToken::new(),
        })
        .await
        .expect("resolve")
        .expect("configured");
    assert_eq!(resolved.auth, Default::default());
    assert_eq!(
        resolved.env,
        Some(ProviderEnv::from([(
            "AWS_PROFILE".to_string(),
            "work".to_string()
        )]))
    );
}

#[tokio::test]
async fn reports_bedrock_as_configured_from_ambient_aws_credentials_without_an_api_key() {
    let models = create_models(Some(CreateModelsOptions {
        auth_context: Some(FakeAuthContext::shared(&[("AWS_PROFILE", "dev")], &[])),
        ..Default::default()
    }));
    models.set_provider(amazon_bedrock_provider());
    let model = models.get_models(Some("amazon-bedrock"))[0].clone();

    let result = models
        .get_auth_for_provider(&model.provider, None)
        .await
        .expect("auth")
        .expect("configured");
    assert_eq!(result.auth, Default::default());
    assert_eq!(result.source.as_deref(), Some("AWS_PROFILE"));

    let unconfigured = create_models(Some(CreateModelsOptions {
        auth_context: Some(FakeAuthContext::shared(&[], &[])),
        ..Default::default()
    }));
    unconfigured.set_provider(amazon_bedrock_provider());
    assert!(
        unconfigured
            .get_auth_for_provider(&model.provider, None)
            .await
            .expect("auth")
            .is_none()
    );
}

// ---------------------------------------------------------------------------
// Cloudflare
// ---------------------------------------------------------------------------

#[tokio::test]
async fn requires_cloudflare_workers_ai_account_config_and_returns_scoped_env() {
    let missing_account = create_models(Some(CreateModelsOptions {
        auth_context: Some(FakeAuthContext::shared(
            &[("CLOUDFLARE_API_KEY", "cf-key")],
            &[],
        )),
        ..Default::default()
    }));
    missing_account.set_provider(cloudflare_workers_ai_provider());
    let model = missing_account.get_models(Some("cloudflare-workers-ai"))[0].clone();
    assert!(
        missing_account
            .get_auth_for_provider(&model.provider, None)
            .await
            .expect("auth")
            .is_none()
    );

    let configured = create_models(Some(CreateModelsOptions {
        auth_context: Some(FakeAuthContext::shared(
            &[
                ("CLOUDFLARE_API_KEY", "cf-key"),
                ("CLOUDFLARE_ACCOUNT_ID", "account-id"),
            ],
            &[],
        )),
        ..Default::default()
    }));
    configured.set_provider(cloudflare_workers_ai_provider());
    let result = configured
        .get_auth_for_provider(&model.provider, None)
        .await
        .expect("auth")
        .expect("configured");
    assert_eq!(result.auth.api_key.as_deref(), Some("cf-key"));
    assert_eq!(result.auth.headers, None);
    assert_eq!(
        result.env,
        Some(ProviderEnv::from([(
            "CLOUDFLARE_ACCOUNT_ID".to_string(),
            "account-id".to_string()
        )]))
    );
}

#[tokio::test]
async fn requires_cloudflare_ai_gateway_account_and_gateway_config_and_returns_scoped_env_headers()
{
    let missing_gateway = create_models(Some(CreateModelsOptions {
        auth_context: Some(FakeAuthContext::shared(
            &[
                ("CLOUDFLARE_API_KEY", "cf-key"),
                ("CLOUDFLARE_ACCOUNT_ID", "account-id"),
            ],
            &[],
        )),
        ..Default::default()
    }));
    missing_gateway.set_provider(cloudflare_ai_gateway_provider());
    let model = missing_gateway.get_models(Some("cloudflare-ai-gateway"))[0].clone();
    assert!(
        missing_gateway
            .get_auth_for_provider(&model.provider, None)
            .await
            .expect("auth")
            .is_none()
    );

    let configured = create_models(Some(CreateModelsOptions {
        auth_context: Some(FakeAuthContext::shared(
            &[
                ("CLOUDFLARE_API_KEY", "cf-key"),
                ("CLOUDFLARE_ACCOUNT_ID", "account-id"),
                ("CLOUDFLARE_GATEWAY_ID", "gateway-id"),
            ],
            &[],
        )),
        ..Default::default()
    }));
    configured.set_provider(cloudflare_ai_gateway_provider());
    let result = configured
        .get_auth_for_provider(&model.provider, None)
        .await
        .expect("auth")
        .expect("configured");
    assert_eq!(result.auth.api_key, None);
    assert_eq!(
        result.auth.headers,
        Some(BTreeMap::from([
            (
                "cf-aig-authorization".to_string(),
                Some("Bearer cf-key".to_string())
            ),
            ("Authorization".to_string(), None),
            ("x-api-key".to_string(), None),
        ]))
    );
    assert_eq!(
        result.env,
        Some(ProviderEnv::from([
            (
                "CLOUDFLARE_ACCOUNT_ID".to_string(),
                "account-id".to_string()
            ),
            (
                "CLOUDFLARE_GATEWAY_ID".to_string(),
                "gateway-id".to_string()
            ),
        ]))
    );
}

#[tokio::test]
async fn materializes_the_model_endpoint_before_dispatch() {
    #[derive(Default)]
    struct RecordingStreams {
        captured: Mutex<Vec<String>>,
    }

    impl ProviderStreams for RecordingStreams {
        fn stream(
            &self,
            model: &Model,
            _context: &Context,
            _options: Option<StreamOptions>,
        ) -> notagent_ai::utils::event_stream::AssistantMessageEventStream {
            self.captured
                .lock()
                .expect("captured")
                .push(model.base_url.clone());
            create_assistant_message_event_stream()
        }

        fn stream_simple(
            &self,
            model: &Model,
            _context: &Context,
            _options: Option<SimpleStreamOptions>,
        ) -> notagent_ai::utils::event_stream::AssistantMessageEventStream {
            self.captured
                .lock()
                .expect("captured")
                .push(model.base_url.clone());
            create_assistant_message_event_stream()
        }
    }

    let model = Model {
        provider: "cloudflare-ai-gateway".to_string(),
        base_url:
            "https://gateway.ai.cloudflare.com/v1/{CLOUDFLARE_ACCOUNT_ID}/{CLOUDFLARE_GATEWAY_ID}/openai"
                .to_string(),
        ..test_model("openai-completions", "model")
    };
    let recording = Arc::new(RecordingStreams::default());
    let streams = cloudflare_streams(recording.clone());
    let env = ProviderEnv::from([
        ("CLOUDFLARE_ACCOUNT_ID".to_string(), "account".to_string()),
        ("CLOUDFLARE_GATEWAY_ID".to_string(), "gateway".to_string()),
    ]);

    streams.stream(
        &model,
        &Context::default(),
        Some(StreamOptions {
            base: notagent_ai::types::ProviderRequestOptions {
                env: Some(env.clone()),
                ..Default::default()
            },
            ..Default::default()
        }),
    );
    streams.stream_simple(
        &model,
        &Context::default(),
        Some(SimpleStreamOptions {
            base: StreamOptions {
                base: notagent_ai::types::ProviderRequestOptions {
                    env: Some(env),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        }),
    );

    assert_eq!(
        *recording.captured.lock().expect("captured"),
        vec![
            "https://gateway.ai.cloudflare.com/v1/account/gateway/openai".to_string(),
            "https://gateway.ai.cloudflare.com/v1/account/gateway/openai".to_string(),
        ]
    );

    // Without a provider env the placeholders survive.
    assert_eq!(
        resolve_cloudflare_model(&model, None).base_url,
        model.base_url
    );
    assert_eq!(
        resolve_cloudflare_model(&model, Some(&ProviderEnv::new())).base_url,
        model.base_url
    );
}

// ---------------------------------------------------------------------------
// Google Vertex AI
// ---------------------------------------------------------------------------

#[tokio::test]
async fn runs_provider_owned_vertex_api_key_and_adc_login_flows() {
    let provider = google_vertex_provider();
    let auth = provider.auth().api_key.clone().expect("api key auth");

    let keyed = ScriptedInteraction::new(&["api-key", "vertex-key"]);
    assert_eq!(
        login(auth.as_ref(), &keyed).await.expect("credential"),
        ApiKeyCredential {
            key: Some("vertex-key".to_string()),
            env: None,
        }
    );

    let adc = ScriptedInteraction::new(&["adc", "project-id", "us-central1"]);
    assert_eq!(
        login(auth.as_ref(), &adc).await.expect("credential"),
        ApiKeyCredential {
            key: None,
            env: Some(ProviderEnv::from([
                ("GOOGLE_CLOUD_PROJECT".to_string(), "project-id".to_string()),
                (
                    "GOOGLE_CLOUD_LOCATION".to_string(),
                    "us-central1".to_string()
                ),
            ])),
        }
    );
    {
        let events = adc.events.lock().expect("events");
        assert_eq!(events.len(), 1);
        match &events[0] {
            AuthEvent::Info { links, .. } => assert_eq!(
                links[0].label.as_deref(),
                Some("Application Default Credentials")
            ),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    let resolved = auth
        .resolve(ApiKeyAuthInput {
            ctx: FakeAuthContext::shared(
                &[],
                &["~/.config/gcloud/application_default_credentials.json"],
            )
            .as_ref(),
            credential: Some(ApiKeyCredential {
                key: None,
                env: Some(ProviderEnv::from([
                    ("GOOGLE_CLOUD_PROJECT".to_string(), "project-id".to_string()),
                    (
                        "GOOGLE_CLOUD_LOCATION".to_string(),
                        "us-central1".to_string(),
                    ),
                ])),
            }),
            signal: tokio_util::sync::CancellationToken::new(),
        })
        .await
        .expect("resolve")
        .expect("configured");
    assert_eq!(resolved.auth, Default::default());
    assert_eq!(
        resolved.env,
        Some(ProviderEnv::from([
            ("GOOGLE_CLOUD_PROJECT".to_string(), "project-id".to_string()),
            (
                "GOOGLE_CLOUD_LOCATION".to_string(),
                "us-central1".to_string()
            ),
        ]))
    );
}

#[tokio::test]
async fn resolves_vertex_via_adc_file_plus_project_and_location() {
    let adc = "~/.config/gcloud/application_default_credentials.json";
    let configured = create_models(Some(CreateModelsOptions {
        auth_context: Some(FakeAuthContext::shared(
            &[
                ("GOOGLE_CLOUD_PROJECT", "proj"),
                ("GOOGLE_CLOUD_LOCATION", "us-central1"),
            ],
            &[adc],
        )),
        ..Default::default()
    }));
    configured.set_provider(google_vertex_provider());
    let model = configured.get_models(Some("google-vertex"))[0].clone();

    let result = configured
        .get_auth_for_provider(&model.provider, None)
        .await
        .expect("auth")
        .expect("configured");
    assert_eq!(result.auth, Default::default());
    assert!(
        result
            .source
            .as_deref()
            .expect("source")
            .contains("application default")
    );

    // ADC without project/location is not configured.
    let partial = create_models(Some(CreateModelsOptions {
        auth_context: Some(FakeAuthContext::shared(
            &[("GOOGLE_CLOUD_PROJECT", "proj")],
            &[adc],
        )),
        ..Default::default()
    }));
    partial.set_provider(google_vertex_provider());
    assert!(
        partial
            .get_auth_for_provider(&model.provider, None)
            .await
            .expect("auth")
            .is_none()
    );

    // An explicit key wins over ADC.
    let keyed = create_models(Some(CreateModelsOptions {
        auth_context: Some(FakeAuthContext::shared(
            &[("GOOGLE_CLOUD_API_KEY", "vertex-key")],
            &[],
        )),
        ..Default::default()
    }));
    keyed.set_provider(google_vertex_provider());
    assert_eq!(
        keyed
            .get_auth_for_provider(&model.provider, None)
            .await
            .expect("auth")
            .expect("configured")
            .auth
            .api_key
            .as_deref(),
        Some("vertex-key")
    );
}

// ---------------------------------------------------------------------------
// GitHub Copilot
// ---------------------------------------------------------------------------

#[test]
fn github_copilot_filters_models_by_the_credentials_available_ids() {
    use notagent_ai::providers::github_copilot::filter_models;

    let models = vec![
        test_model("openai-completions", "gpt-4.1"),
        test_model("openai-completions", "gpt-4o"),
    ];
    let oauth = |available: Value| {
        Credential::OAuth(notagent_ai::auth::types::OAuthCredential {
            refresh: "r".to_string(),
            access: "a".to_string(),
            expires: 0,
            extra: serde_json::Map::from_iter([("availableModelIds".to_string(), available)]),
        })
    };

    // An api-key credential and a missing list keep the whole catalog.
    assert_eq!(filter_models(models.clone(), None).len(), 2);
    assert_eq!(
        filter_models(
            models.clone(),
            Some(&Credential::ApiKey(ApiKeyCredential::default()))
        )
        .len(),
        2
    );
    assert_eq!(
        filter_models(models.clone(), Some(&oauth(json!("not-an-array")))).len(),
        2
    );
    // A list with a non-string entry is ignored as a whole.
    assert_eq!(
        filter_models(models.clone(), Some(&oauth(json!(["gpt-4.1", 7])))).len(),
        2
    );
    assert_eq!(
        filter_models(models.clone(), Some(&oauth(json!(["gpt-4.1"]))))
            .iter()
            .map(|model| model.id.clone())
            .collect::<Vec<_>>(),
        vec!["gpt-4.1".to_string()]
    );
    assert!(filter_models(models, Some(&oauth(json!([])))).is_empty());
}

// ---------------------------------------------------------------------------
// Amazon Bedrock transport
// ---------------------------------------------------------------------------

/// The Bedrock adapter builds its client from the resolved config and turns a transport
/// failure into an in-band error message. `AWS_BEDROCK_SKIP_AUTH` supplies the dummy
/// credentials, and the endpoint points at a closed port, so nothing leaves the machine.
#[tokio::test]
async fn bedrock_dispatches_through_the_aws_sdk_and_reports_transport_failures() {
    let model = Model {
        provider: "amazon-bedrock".to_string(),
        base_url: "http://127.0.0.1:1".to_string(),
        ..test_model(
            "bedrock-converse-stream",
            "anthropic.claude-haiku-4-5-20251001-v1:0",
        )
    };
    let env = ProviderEnv::from([
        ("AWS_BEDROCK_SKIP_AUTH".to_string(), "1".to_string()),
        ("AWS_REGION".to_string(), "us-east-1".to_string()),
    ]);
    let payloads = Arc::new(Mutex::new(Vec::<Value>::new()));
    let captured = payloads.clone();

    let provider = amazon_bedrock_provider();
    let options = SimpleStreamOptions {
        base: StreamOptions {
            base: notagent_ai::types::ProviderRequestOptions {
                env: Some(env),
                max_retries: Some(0),
                on_payload: Some(Arc::new(move |payload: Value, _model: &Model| {
                    captured.lock().expect("payloads").push(payload);
                    Box::pin(async { None })
                })),
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    };
    let result = provider
        .stream_simple(&model, &context(), Some(options))
        .result()
        .await;

    {
        let payloads = payloads.lock().expect("payloads");
        assert_eq!(payloads.len(), 1, "onPayload sees the command input");
        assert_eq!(
            payloads[0]["modelId"],
            json!("anthropic.claude-haiku-4-5-20251001-v1:0")
        );
    }
    assert_eq!(result.stop_reason, notagent_ai::types::StopReason::Error);
    let message = result.error_message.expect("error message");
    assert!(
        !message.contains("no API implementation"),
        "the provider dispatches into the adapter: {message}"
    );
    assert!(
        message.contains("dispatch failure") || message.contains("error"),
        "the transport failure surfaces in-band: {message}"
    );
}

/// a provider entry in `crates/notagent_repo/src/provider/provider.json`
/// (user decision 2026-08-17): an OpenAI-compatible endpoint on an API key,
/// with the plan covering the tokens — hence no per-token price.
#[test]
fn cline_pass_is_an_openai_compatible_provider_without_per_token_cost() {
    let provider = builtin_providers()
        .into_iter()
        .find(|provider| provider.id() == "cline-pass")
        .expect("cline-pass is registered");

    assert_eq!(provider.name(), "ClinePass");
    assert_eq!(provider.base_url(), Some("https://api.cline.bot/api/v1"));

    let models = builtin_models(None);
    let list = models.get_models(Some("cline-pass"));
    assert_eq!(list.len(), 11, "every ClinePass model is present");
    for model in &list {
        assert!(model.id.starts_with("cline-pass/"), "{}", model.id);
        assert_eq!(model.api, "openai-completions");
        assert!(model.reasoning, "{} reasons", model.id);
        assert_eq!(model.cost.input, 0.0, "{} is covered by the plan", model.id);
        assert_eq!(
            model.cost.output, 0.0,
            "{} is covered by the plan",
            model.id
        );
        assert!(
            model.thinking_level_map.is_some(),
            "{} carries its level map",
            model.id
        );
    }
}

/// GLM-5.3 joins z.ai with the values of GLM-5.2 (user decision 2026-08-17).
/// ClinePass does not offer it yet, so it is not in that catalog.
#[test]
fn glm_5_3_matches_glm_5_2_on_zai_and_is_absent_from_cline_pass() {
    let models = builtin_models(None);

    for provider in ["zai", "zai-coding-cn"] {
        let older = models.get_model(provider, "glm-5.2").expect("glm-5.2");
        let newer = models.get_model(provider, "glm-5.3").expect("glm-5.3");

        assert_eq!(newer.name, "GLM-5.3");
        assert_eq!(newer.context_window, older.context_window);
        assert_eq!(newer.max_tokens, older.max_tokens);
        assert_eq!(newer.reasoning, older.reasoning);
        assert_eq!(newer.thinking_level_map, older.thinking_level_map);
        assert_eq!(newer.compat, older.compat);
        assert_eq!(newer.base_url, older.base_url);
    }

    assert!(
        models
            .get_model("cline-pass", "cline-pass/glm-5.3")
            .is_none(),
        "ClinePass does not carry GLM-5.3 yet"
    );
}
