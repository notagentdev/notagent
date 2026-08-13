//! Ports of the core cases of `packages/ai/test/models-runtime.test.ts` (1 159 LOC).
//! The remaining cases (abort behaviour of non-cooperative providers, header transform
//! chain) follow in task 13; see PARITY.md.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use notagent_ai::auth::types::*;
use notagent_ai::auth::{InMemoryCredentialStore, resolve::now_ms};

/// Credential store holding an api key for every provider used in a refresh test.
async fn configured_credentials(provider_ids: &[&str]) -> Arc<InMemoryCredentialStore> {
    let credentials = Arc::new(InMemoryCredentialStore::new());
    for provider_id in provider_ids {
        credentials
            .modify(
                provider_id,
                Box::new(|_| {
                    Box::pin(async {
                        Ok(Some(Credential::ApiKey(ApiKeyCredential {
                            key: Some("k".to_string()),
                            env: None,
                        })))
                    })
                }),
                None,
            )
            .await
            .expect("seed credential");
    }
    credentials
}
use notagent_ai::models::*;
use notagent_ai::models_store::{InMemoryModelsStore, ModelsStore, ModelsStoreEntry};
use notagent_ai::types::*;
use notagent_ai::utils::event_stream::AssistantMessageEventStream;

fn test_model(provider: &str, id: &str) -> Model {
    Model {
        id: id.to_string(),
        name: id.to_string(),
        api: "test-api".to_string(),
        provider: provider.to_string(),
        base_url: "https://example.test".to_string(),
        reasoning: false,
        thinking_level_map: None,
        input: vec![Modality::Text],
        cost: ModelCost::default(),
        context_window: 1000,
        max_tokens: 100,
        sampling_params: None,
        headers: None,
        compat: None,
    }
}

struct TestProvider {
    id: String,
    auth: ProviderAuth,
    models: Mutex<Vec<Model>>,
    dynamic: bool,
    refreshes: Arc<AtomicUsize>,
    /// Models the network phase publishes.
    fetched: Vec<Model>,
}

impl TestProvider {
    fn new(id: &str) -> Self {
        TestProvider {
            id: id.to_string(),
            auth: ProviderAuth {
                api_key: Some(Arc::new(notagent_ai::auth::helpers::EnvApiKeyAuth::new(
                    "test key",
                    ["TEST_API_KEY"],
                ))),
                oauth: None,
            },
            models: Mutex::new(Vec::new()),
            dynamic: false,
            refreshes: Arc::new(AtomicUsize::new(0)),
            fetched: Vec::new(),
        }
    }

    fn with_models(mut self, models: Vec<Model>) -> Self {
        self.models = Mutex::new(models);
        self
    }
}

impl Provider for TestProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        &self.id
    }

    fn auth(&self) -> &ProviderAuth {
        &self.auth
    }

    fn get_models(&self) -> Vec<Model> {
        self.models.lock().expect("models poisoned").clone()
    }

    fn is_dynamic(&self) -> bool {
        self.dynamic
    }

    fn refresh_models<'a>(
        &'a self,
        context: RefreshModelsContext<'a>,
    ) -> Option<BoxFuture<'a, Result<(), String>>> {
        if !self.dynamic {
            return None;
        }
        Some(Box::pin(async move {
            if let Some(stored) = &context.stored {
                let restored = stored.models.clone();
                (context.publish)(ModelsPublication {
                    persist: None,
                    update: Some(Box::new(move || {
                        *self.models.lock().expect("models poisoned") = restored;
                    })),
                })
                .await;
            }
            if !context.allow_network {
                return Ok(());
            }
            self.refreshes.fetch_add(1, Ordering::SeqCst);
            let fetched = self.fetched.clone();
            (context.publish)(ModelsPublication {
                persist: Some(Some(ModelsStoreEntry {
                    models: fetched.clone(),
                    checked_at: Some(now_ms()),
                    ..Default::default()
                })),
                update: Some(Box::new(move || {
                    *self.models.lock().expect("models poisoned") = fetched;
                })),
            })
            .await;
            Ok(())
        }))
    }

    fn stream(
        &self,
        model: &Model,
        _context: &Context,
        _options: Option<StreamOptions>,
    ) -> AssistantMessageEventStream {
        let stream = notagent_ai::utils::event_stream::create_assistant_message_event_stream();
        stream.push(AssistantMessageEvent::Done {
            reason: DoneReason::Stop,
            message: AssistantMessage {
                content: vec![AssistantContent::Text(TextContent::new("streamed"))],
                api: model.api.clone(),
                provider: model.provider.clone(),
                model: model.id.clone(),
                response_model: None,
                response_id: None,
                diagnostics: None,
                usage: Usage::default(),
                stop_reason: StopReason::Stop,
                deferred: None,
                error_message: None,
                raw_stop_reason: None,
                end_turn: None,
                timestamp: 0,
            },
        });
        stream
    }

    fn stream_simple(
        &self,
        model: &Model,
        context: &Context,
        _options: Option<SimpleStreamOptions>,
    ) -> AssistantMessageEventStream {
        self.stream(model, context, None)
    }
}

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

#[test]
fn applies_request_wide_pricing_tiers_above_the_configured_threshold() {
    let mut model = test_model("openai", "gpt-5.6-sol");
    model.cost = ModelCost {
        input: 5.0,
        output: 30.0,
        cache_read: 0.5,
        cache_write: 6.25,
        tiers: Some(vec![ModelCostTier {
            input_tokens_above: 272_000,
            input: 10.0,
            output: 45.0,
            cache_read: 1.0,
            cache_write: 12.5,
        }]),
    };
    let usage_with = |cache_write: u64| Usage {
        input: 200_000,
        output: 100_000,
        cache_read: 72_000,
        cache_write,
        cache_write1h: None,
        reasoning: None,
        total_tokens: Some(372_000 + cache_write),
        cost: UsageCost::default(),
    };

    // 272 000 input tokens do not exceed the threshold: base rates apply.
    let mut usage = usage_with(0);
    let short = calculate_cost(&model, &mut usage);
    assert_eq!(short.input, 1.0);
    assert_eq!(short.output, 3.0);
    assert_eq!(short.cache_read, 0.036);
    assert_eq!(short.cache_write, 0.0);

    let mut usage = usage_with(1);
    let long = calculate_cost(&model, &mut usage);
    assert_eq!(long.input, 2.0);
    assert_eq!(long.output, 4.5);
    assert_eq!(long.cache_read, 0.072);
    assert_eq!(long.cache_write, 0.0000125);
}

#[test]
fn charges_1h_cache_writes_at_twice_the_base_input_rate() {
    let mut model = test_model("anthropic", "claude");
    model.cost = ModelCost {
        input: 3.0,
        output: 15.0,
        cache_read: 0.3,
        cache_write: 3.75,
        tiers: None,
    };
    let mut usage = Usage {
        input: 0,
        output: 0,
        cache_read: 0,
        cache_write: 1_000_000,
        cache_write1h: Some(400_000),
        reasoning: None,
        total_tokens: Some(1_000_000),
        cost: UsageCost::default(),
    };
    let cost = calculate_cost(&model, &mut usage);
    // 600 000 short writes at 3.75 plus 400 000 long writes at 2x3.
    assert_eq!(
        cost.cache_write,
        (3.75 * 600_000.0 + 3.0 * 2.0 * 400_000.0) / 1_000_000.0
    );
}

#[test]
fn thinking_level_support_and_clamping() {
    let mut model = test_model("p", "m");
    assert_eq!(
        get_supported_thinking_levels(&model),
        vec![ModelThinkingLevel::Off]
    );

    model.reasoning = true;
    assert_eq!(
        get_supported_thinking_levels(&model),
        vec![
            ModelThinkingLevel::Off,
            ModelThinkingLevel::Minimal,
            ModelThinkingLevel::Low,
            ModelThinkingLevel::Medium,
            ModelThinkingLevel::High,
        ],
        "xhigh and max need an explicit mapping"
    );

    let mut map: ThinkingLevelMap = BTreeMap::new();
    map.insert(ModelThinkingLevel::Xhigh, Some("xhigh".to_string()));
    map.insert(ModelThinkingLevel::Minimal, None);
    model.thinking_level_map = Some(map);
    let supported = get_supported_thinking_levels(&model);
    assert!(supported.contains(&ModelThinkingLevel::Xhigh));
    assert!(
        !supported.contains(&ModelThinkingLevel::Minimal),
        "null marks a level unsupported"
    );
    assert!(!supported.contains(&ModelThinkingLevel::Max));

    // Clamping walks up first, then down: max is unsupported, xhigh is the nearest.
    assert_eq!(
        clamp_thinking_level(&model, ModelThinkingLevel::Max),
        ModelThinkingLevel::Xhigh
    );
    assert_eq!(
        clamp_thinking_level(&model, ModelThinkingLevel::Minimal),
        ModelThinkingLevel::Low
    );
    assert_eq!(
        clamp_thinking_level(&model, ModelThinkingLevel::High),
        ModelThinkingLevel::High
    );

    let plain = test_model("p", "m");
    assert_eq!(
        clamp_thinking_level(&plain, ModelThinkingLevel::High),
        ModelThinkingLevel::Off
    );
}

#[test]
fn models_are_equal_compares_id_and_provider() {
    let a = test_model("p", "m");
    let b = test_model("p", "m");
    let other = test_model("q", "m");
    assert!(models_are_equal(Some(&a), Some(&b)));
    assert!(!models_are_equal(Some(&a), Some(&other)));
    assert!(!models_are_equal(Some(&a), None));
    assert!(!models_are_equal(None, None));
}

#[test]
fn has_api_narrows_dynamically_looked_up_models() {
    let model = test_model("p2", "m3");
    assert!(!has_api(&model, "openai-completions"));
    assert!(has_api(&model, "test-api"));
}

#[test]
fn merge_headers_replaces_case_insensitively() {
    let base: ProviderHeaders = [("X-Test".to_string(), Some("base".to_string()))]
        .into_iter()
        .collect::<BTreeMap<_, _>>();
    let override_headers: ProviderHeaders = [("x-test".to_string(), Some("override".to_string()))]
        .into_iter()
        .collect::<BTreeMap<_, _>>();
    let merged = merge_headers(Some(&base), Some(&override_headers)).expect("merged");
    assert_eq!(merged.len(), 1);
    assert_eq!(merged.get("x-test"), Some(&Some("override".to_string())));
    assert_eq!(merge_headers(None, None), None);
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

#[test]
fn registers_replaces_and_deletes_providers() {
    let models = create_models(None);
    models.set_provider(Arc::new(TestProvider::new("p1")));
    models.set_provider(Arc::new(TestProvider::new("p2")));
    assert_eq!(
        models
            .get_providers()
            .iter()
            .map(|p| p.id().to_string())
            .collect::<Vec<_>>(),
        ["p1", "p2"]
    );

    let replacement =
        Arc::new(TestProvider::new("p1").with_models(vec![test_model("p1", "replaced")]));
    models.set_provider(replacement);
    assert_eq!(models.get_providers().len(), 2);
    assert_eq!(
        models.get_models(Some("p1")).first().map(|m| m.id.clone()),
        Some("replaced".to_string())
    );

    models.delete_provider("p1");
    assert!(models.get_provider("p1").is_none());

    models.clear_providers();
    assert_eq!(models.get_providers().len(), 0);
}

#[test]
fn lists_and_finds_models_per_provider() {
    let models = create_models(None);
    models.set_provider(Arc::new(
        TestProvider::new("p1").with_models(vec![test_model("p1", "m1"), test_model("p1", "m2")]),
    ));
    models.set_provider(Arc::new(
        TestProvider::new("p2").with_models(vec![test_model("p2", "m3")]),
    ));

    assert_eq!(
        models
            .get_models(None)
            .iter()
            .map(|m| m.id.clone())
            .collect::<Vec<_>>(),
        ["m1", "m2", "m3"]
    );
    assert_eq!(
        models
            .get_models(Some("p1"))
            .iter()
            .map(|m| m.id.clone())
            .collect::<Vec<_>>(),
        ["m1", "m2"]
    );
    assert_eq!(models.get_models(Some("nope")).len(), 0);
    assert_eq!(
        models.get_model("p2", "m3").map(|m| m.id),
        Some("m3".to_string())
    );
    assert!(models.get_model("p2", "missing").is_none());
}

/// The network phase runs only for configured providers, so every refresh test seeds a
/// credential (TS: "passes effective API-key credentials ... while skipping unconfigured
/// providers").
#[tokio::test]
async fn refresh_updates_dynamic_providers_and_skips_static_ones() {
    let credentials = configured_credentials(&["dyn", "static"]).await;
    let models = create_models(Some(CreateModelsOptions {
        credentials: Some(credentials as Arc<dyn CredentialStore>),
        ..Default::default()
    }));
    let mut dynamic = TestProvider::new("dyn").with_models(vec![test_model("dyn", "before")]);
    dynamic.dynamic = true;
    dynamic.fetched = vec![test_model("dyn", "after")];
    let refreshes = Arc::clone(&dynamic.refreshes);
    models.set_provider(Arc::new(dynamic));
    models.set_provider(Arc::new(
        TestProvider::new("static").with_models(vec![test_model("static", "s1")]),
    ));

    let result = models.refresh(None).await;
    assert!(!result.aborted);
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(refreshes.load(Ordering::SeqCst), 1);
    assert_eq!(
        models
            .get_models(Some("dyn"))
            .iter()
            .map(|m| m.id.clone())
            .collect::<Vec<_>>(),
        ["after"]
    );
    assert_eq!(
        models
            .get_models(Some("static"))
            .iter()
            .map(|m| m.id.clone())
            .collect::<Vec<_>>(),
        ["s1"]
    );
}

#[tokio::test]
async fn refresh_restricts_work_to_selected_providers() {
    let credentials = configured_credentials(&["a", "b"]).await;
    let models = create_models(Some(CreateModelsOptions {
        credentials: Some(credentials as Arc<dyn CredentialStore>),
        ..Default::default()
    }));
    let mut first = TestProvider::new("a");
    first.dynamic = true;
    first.fetched = vec![test_model("a", "a1")];
    let first_refreshes = Arc::clone(&first.refreshes);
    let mut second = TestProvider::new("b");
    second.dynamic = true;
    second.fetched = vec![test_model("b", "b1")];
    let second_refreshes = Arc::clone(&second.refreshes);
    models.set_provider(Arc::new(first));
    models.set_provider(Arc::new(second));

    models
        .refresh(Some(ModelsRefreshOptions {
            providers: Some(vec!["a".to_string()]),
            ..Default::default()
        }))
        .await;
    assert_eq!(first_refreshes.load(Ordering::SeqCst), 1);
    assert_eq!(second_refreshes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn restores_cached_models_without_network_access() {
    let store = Arc::new(InMemoryModelsStore::new());
    store
        .write(
            "dyn",
            ModelsStoreEntry {
                models: vec![test_model("dyn", "cached")],
                ..Default::default()
            },
            None,
        )
        .await
        .expect("seed");
    let models = create_models(Some(CreateModelsOptions {
        models_store: Some(Arc::clone(&store) as Arc<dyn ModelsStore>),
        credentials: Some(configured_credentials(&["dyn"]).await as Arc<dyn CredentialStore>),
        ..Default::default()
    }));
    let mut dynamic = TestProvider::new("dyn");
    dynamic.dynamic = true;
    dynamic.fetched = vec![test_model("dyn", "network")];
    let refreshes = Arc::clone(&dynamic.refreshes);
    models.set_provider(Arc::new(dynamic));

    models
        .refresh(Some(ModelsRefreshOptions {
            allow_network: Some(false),
            ..Default::default()
        }))
        .await;
    assert_eq!(
        refreshes.load(Ordering::SeqCst),
        0,
        "the network phase must not run"
    );
    assert_eq!(
        models
            .get_models(Some("dyn"))
            .iter()
            .map(|m| m.id.clone())
            .collect::<Vec<_>>(),
        ["cached"]
    );
}

#[tokio::test]
async fn persists_dynamic_catalogs() {
    let store = Arc::new(InMemoryModelsStore::new());
    let models = create_models(Some(CreateModelsOptions {
        models_store: Some(Arc::clone(&store) as Arc<dyn ModelsStore>),
        credentials: Some(configured_credentials(&["dyn"]).await as Arc<dyn CredentialStore>),
        ..Default::default()
    }));
    let mut dynamic = TestProvider::new("dyn");
    dynamic.dynamic = true;
    dynamic.fetched = vec![test_model("dyn", "fetched")];
    models.set_provider(Arc::new(dynamic));

    models.refresh(None).await;
    let entry = store.read("dyn", None).await.expect("read").expect("entry");
    assert_eq!(
        entry
            .models
            .iter()
            .map(|m| m.id.clone())
            .collect::<Vec<_>>(),
        ["fetched"]
    );
    assert!(entry.checked_at.is_some());
}

// ---------------------------------------------------------------------------
// Auth application and streaming
// ---------------------------------------------------------------------------

#[tokio::test]
async fn enumerates_credential_metadata_without_exposing_secrets() {
    let credentials = InMemoryCredentialStore::new();
    credentials
        .modify(
            "api-provider",
            Box::new(|_| {
                Box::pin(async {
                    Ok(Some(Credential::ApiKey(ApiKeyCredential {
                        key: Some("secret".to_string()),
                        env: None,
                    })))
                })
            }),
            None,
        )
        .await
        .expect("seed");
    credentials
        .modify(
            "oauth-provider",
            Box::new(|_| {
                Box::pin(async {
                    Ok(Some(Credential::OAuth(OAuthCredential {
                        access: "access".to_string(),
                        refresh: "refresh".to_string(),
                        expires: now_ms() + 60_000,
                        extra: Default::default(),
                    })))
                })
            }),
            None,
        )
        .await
        .expect("seed");

    let listed = credentials.list(None).await.expect("list");
    assert_eq!(
        listed,
        vec![
            CredentialInfo {
                provider_id: "api-provider".to_string(),
                credential_type: AuthType::ApiKey
            },
            CredentialInfo {
                provider_id: "oauth-provider".to_string(),
                credential_type: AuthType::OAuth
            },
        ]
    );
}

#[tokio::test]
async fn checks_provider_auth_and_filters_available_models() {
    let credentials = Arc::new(InMemoryCredentialStore::new());
    let models = create_models(Some(CreateModelsOptions {
        credentials: Some(Arc::clone(&credentials) as Arc<dyn CredentialStore>),
        ..Default::default()
    }));
    models.set_provider(Arc::new(
        TestProvider::new("p").with_models(vec![test_model("p", "m1")]),
    ));

    // Unconfigured: no auth, no available models.
    assert!(models.check_auth("p", None).await.expect("check").is_none());
    assert!(
        models
            .get_available(None, None)
            .await
            .expect("available")
            .is_empty()
    );

    credentials
        .modify(
            "p",
            Box::new(|_| {
                Box::pin(async {
                    Ok(Some(Credential::ApiKey(ApiKeyCredential {
                        key: Some("k".to_string()),
                        env: None,
                    })))
                })
            }),
            None,
        )
        .await
        .expect("seed");

    let check = models
        .check_auth("p", None)
        .await
        .expect("check")
        .expect("configured");
    assert_eq!(check.check_type, AuthType::ApiKey);
    assert_eq!(
        models
            .get_available(None, None)
            .await
            .expect("available")
            .len(),
        1
    );
}

#[tokio::test]
async fn produces_an_error_stream_for_unknown_providers_instead_of_failing() {
    let models = create_models(None);
    let stream = models.stream_simple(test_model("missing", "m"), Context::default(), None);
    let result = stream.result().await;
    assert_eq!(result.stop_reason, StopReason::Error);
    assert_eq!(
        result.error_message.as_deref(),
        Some("Unknown provider: missing")
    );
}

#[tokio::test]
async fn produces_an_error_stream_when_the_provider_is_not_configured() {
    let models = create_models(None);
    models.set_provider(Arc::new(
        TestProvider::new("p").with_models(vec![test_model("p", "m")]),
    ));
    let stream = models.stream_simple(test_model("p", "m"), Context::default(), None);
    let result = stream.result().await;
    assert_eq!(result.stop_reason, StopReason::Error);
    assert_eq!(
        result.error_message.as_deref(),
        Some("Provider is not configured: p")
    );
}

#[tokio::test]
async fn streams_through_the_provider_once_auth_resolves() {
    let credentials = Arc::new(InMemoryCredentialStore::new());
    credentials
        .modify(
            "p",
            Box::new(|_| {
                Box::pin(async {
                    Ok(Some(Credential::ApiKey(ApiKeyCredential {
                        key: Some("k".to_string()),
                        env: None,
                    })))
                })
            }),
            None,
        )
        .await
        .expect("seed");
    let models = create_models(Some(CreateModelsOptions {
        credentials: Some(Arc::clone(&credentials) as Arc<dyn CredentialStore>),
        ..Default::default()
    }));
    models.set_provider(Arc::new(
        TestProvider::new("p").with_models(vec![test_model("p", "m")]),
    ));

    let result = models
        .complete_simple(test_model("p", "m"), Context::default(), None)
        .await;
    assert_eq!(result.stop_reason, StopReason::Stop);
    assert_eq!(
        result.content,
        vec![AssistantContent::Text(TextContent::new("streamed"))]
    );
}

#[tokio::test]
async fn logout_removes_the_stored_credential() {
    let credentials = Arc::new(InMemoryCredentialStore::new());
    credentials
        .modify(
            "p",
            Box::new(|_| {
                Box::pin(async {
                    Ok(Some(Credential::ApiKey(ApiKeyCredential {
                        key: Some("k".to_string()),
                        env: None,
                    })))
                })
            }),
            None,
        )
        .await
        .expect("seed");
    let models = create_models(Some(CreateModelsOptions {
        credentials: Some(Arc::clone(&credentials) as Arc<dyn CredentialStore>),
        ..Default::default()
    }));
    models.set_provider(Arc::new(TestProvider::new("p")));

    models.logout("p", None).await.expect("logout");
    assert!(credentials.read("p", None).await.expect("read").is_none());
}
