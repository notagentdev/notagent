use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use notagent::core::auth_storage::AuthStorage;
use notagent::core::model_registry::ModelRegistry;
use notagent::core::model_runtime::{
    CreateModelRuntimeOptions, ModelRuntime, ModelsDeferredCancelOptions,
    ModelsDeferredFetchOptions, ModelsRequestTransforms,
};
use notagent_ai::auth::resolve::ModelsError;
use notagent_ai::auth::types::{
    ApiKeyAuth, ApiKeyAuthInput, ApiKeyCredential, AuthCheck, AuthError, AuthPrompt,
    AuthPromptKind, AuthResult, AuthType, CredentialStore, ModelAuth, ProviderAuth,
    ProviderAuthInteraction,
};
use notagent_ai::models::Provider;
use notagent_ai::models_store::InMemoryModelsStore;
use notagent_ai::types::{
    AssistantMessage, AssistantMessageEvent, Context, DeferredCancelOptions, DeferredFetchOptions,
    DeferredHandle, Modality, Model, ModelCost, ProviderHeaders, SimpleStreamOptions, StopReason,
    StreamOptions, Usage,
};
use notagent_ai::utils::event_stream::{
    AssistantMessageEventStream, create_assistant_message_event_stream,
};
use serde_json::json;

fn model(id: &str, provider: &str, base_url: &str) -> Model {
    Model {
        id: id.to_owned(),
        name: id.to_owned(),
        api: "openai-completions".to_owned(),
        provider: provider.to_owned(),
        base_url: base_url.to_owned(),
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

async fn create_runtime(models_path: Option<String>) -> Arc<ModelRuntime> {
    ModelRuntime::create(CreateModelRuntimeOptions {
        credentials: Some(
            Arc::new(AuthStorage::in_memory(serde_json::Map::new())) as Arc<dyn CredentialStore>
        ),
        models_store: Some(Arc::new(InMemoryModelsStore::new())),
        models_path: Some(models_path),
        allow_model_network: Some(false),
        ..CreateModelRuntimeOptions::default()
    })
    .await
    .expect("runtime")
}

// ---------------------------------------------------------------------------
// 1. native provider registration
// ---------------------------------------------------------------------------

struct NativeSetupAuth;

impl ApiKeyAuth for NativeSetupAuth {
    fn name(&self) -> &str {
        "Native setup"
    }

    fn login<'a>(
        &'a self,
        interaction: &'a ProviderAuthInteraction<'a>,
    ) -> Option<BoxFuture<'a, Result<ApiKeyCredential, AuthError>>> {
        Some(Box::pin(async move {
            let key = interaction
                .prompt(AuthPrompt {
                    signal: None,
                    kind: AuthPromptKind::Secret {
                        message: "API key".to_owned(),
                        placeholder: None,
                    },
                })
                .await?;
            Ok(ApiKeyCredential {
                key: Some(key),
                env: None,
            })
        }))
    }

    fn check<'a>(
        &'a self,
        input: ApiKeyAuthInput<'a>,
    ) -> Option<BoxFuture<'a, Result<Option<AuthCheck>, AuthError>>> {
        let credential = input.credential.clone();
        Some(Box::pin(async move {
            Ok(credential
                .and_then(|credential| credential.key)
                .map(|_| AuthCheck {
                    source: Some("stored native key".to_owned()),
                    check_type: AuthType::ApiKey,
                }))
        }))
    }

    fn resolve<'a>(
        &'a self,
        input: ApiKeyAuthInput<'a>,
    ) -> BoxFuture<'a, Result<Option<AuthResult>, AuthError>> {
        let credential = input.credential.clone();
        Box::pin(async move {
            Ok(credential
                .and_then(|credential| credential.key)
                .map(|key| AuthResult {
                    auth: ModelAuth {
                        api_key: Some(key),
                        headers: None,
                        base_url: Some("https://resolved.test/v1".to_owned()),
                    },
                    env: None,
                    source: Some("stored native key".to_owned()),
                }))
        })
    }
}

struct StaticKeyAuth;

impl ApiKeyAuth for StaticKeyAuth {
    fn name(&self) -> &str {
        "Native key"
    }

    fn resolve<'a>(
        &'a self,
        _input: ApiKeyAuthInput<'a>,
    ) -> BoxFuture<'a, Result<Option<AuthResult>, AuthError>> {
        Box::pin(async {
            Ok(Some(AuthResult {
                auth: ModelAuth {
                    api_key: Some("key".to_owned()),
                    headers: None,
                    base_url: None,
                },
                env: None,
                source: Some("native".to_owned()),
            }))
        })
    }
}

struct NativeProvider {
    id: String,
    name: String,
    auth: ProviderAuth,
    models: Vec<Model>,
}

impl Provider for NativeProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn auth(&self) -> &ProviderAuth {
        &self.auth
    }

    fn get_models(&self) -> Vec<Model> {
        self.models.clone()
    }

    fn stream(
        &self,
        _model: &Model,
        _context: &Context,
        _options: Option<StreamOptions>,
    ) -> AssistantMessageEventStream {
        panic!("unused")
    }

    fn stream_simple(
        &self,
        _model: &Model,
        _context: &Context,
        _options: Option<SimpleStreamOptions>,
    ) -> AssistantMessageEventStream {
        panic!("unused")
    }
}

struct SilentPrompt(String);

impl notagent_ai::auth::types::AuthInteraction for SilentPrompt {
    fn signal(&self) -> Option<tokio_util::sync::CancellationToken> {
        None
    }

    fn prompt(&self, _prompt: AuthPrompt) -> BoxFuture<'_, Result<String, AuthError>> {
        let answer = self.0.clone();
        Box::pin(async move { Ok(answer) })
    }

    fn notify(&self, _event: notagent_ai::auth::types::AuthEvent) {}
}

#[tokio::test]
async fn registers_native_providers_with_their_auth_implementation() {
    let runtime = create_runtime(None).await;
    let provider: Arc<dyn Provider> = Arc::new(NativeProvider {
        id: "extension-native".to_owned(),
        name: "Extension Native".to_owned(),
        auth: ProviderAuth {
            api_key: Some(Arc::new(NativeSetupAuth)),
            oauth: None,
        },
        models: vec![model(
            "native",
            "extension-native",
            "https://fallback.test/v1",
        )],
    });

    runtime
        .register_native_provider(Arc::clone(&provider))
        .expect("register");
    let registry = ModelRegistry::new(Arc::clone(&runtime));
    assert!(Arc::ptr_eq(
        &registry.get_provider("extension-native").expect("provider"),
        &provider
    ));
    assert!(Arc::ptr_eq(
        &registry
            .get_registered_native_provider("extension-native")
            .expect("native"),
        &provider
    ));
    assert!(
        registry
            .get_registered_provider_ids()
            .contains(&"extension-native".to_owned())
    );
    assert!(registry.find("extension-native", "native").is_some());

    runtime
        .login(
            "extension-native",
            AuthType::ApiKey,
            &SilentPrompt("secret".to_owned()),
        )
        .await
        .expect("login");
    let auth = registry
        .get_provider_auth("extension-native")
        .await
        .expect("auth")
        .expect("resolution");
    assert_eq!(auth.auth.api_key.as_deref(), Some("secret"));
    assert_eq!(
        auth.auth.base_url.as_deref(),
        Some("https://resolved.test/v1")
    );

    registry.unregister_provider("extension-native");
    assert!(registry.get_provider("extension-native").is_none());
}

// ---------------------------------------------------------------------------
// 2. deferred methods through overlays
// ---------------------------------------------------------------------------

/// The fields the suite asserts on: api key, wait/timeout and headers.
type CapturedOptions = Option<(Option<String>, Option<u64>, Option<ProviderHeaders>)>;

#[derive(Default)]
struct DeferredCapture {
    fetched_base_url: Mutex<Option<String>>,
    fetched_options: Mutex<CapturedOptions>,
    cancelled_id: Mutex<Option<String>>,
    cancelled_options: Mutex<CapturedOptions>,
}

struct DeferredProvider {
    id: String,
    auth: ProviderAuth,
    models: Vec<Model>,
    capture: Arc<DeferredCapture>,
}

impl Provider for DeferredProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        "Extension Native Deferred"
    }

    fn auth(&self) -> &ProviderAuth {
        &self.auth
    }

    fn get_models(&self) -> Vec<Model> {
        self.models.clone()
    }

    fn stream(
        &self,
        _model: &Model,
        _context: &Context,
        _options: Option<StreamOptions>,
    ) -> AssistantMessageEventStream {
        panic!("unused")
    }

    fn stream_simple(
        &self,
        _model: &Model,
        _context: &Context,
        _options: Option<SimpleStreamOptions>,
    ) -> AssistantMessageEventStream {
        panic!("unused")
    }

    fn fetch_deferred(
        &self,
        model: &Model,
        _handle: &DeferredHandle,
        options: Option<DeferredFetchOptions>,
    ) -> Option<AssistantMessageEventStream> {
        *self.capture.fetched_base_url.lock().expect("base url") = Some(model.base_url.clone());
        *self.capture.fetched_options.lock().expect("options") = options.map(|options| {
            (
                options.base.api_key.clone(),
                options.wait,
                options.base.headers.clone(),
            )
        });
        let message = AssistantMessage {
            content: vec![],
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
        };
        let stream = create_assistant_message_event_stream();
        stream.push(AssistantMessageEvent::Start {
            partial: message.clone(),
        });
        stream.push(AssistantMessageEvent::Done {
            reason: notagent_ai::types::DoneReason::Stop,
            message: message.clone(),
        });
        stream.end(Some(message));
        Some(stream)
    }

    fn cancel_deferred<'a>(
        &'a self,
        _model: &'a Model,
        handle: &'a DeferredHandle,
        options: Option<DeferredCancelOptions>,
    ) -> Option<BoxFuture<'a, Result<(), ModelsError>>> {
        *self.capture.cancelled_id.lock().expect("id") = Some(handle.id.clone());
        *self.capture.cancelled_options.lock().expect("options") = options.map(|options| {
            (
                options.api_key.clone(),
                options.timeout_ms,
                options.headers.clone(),
            )
        });
        Some(Box::pin(async { Ok(()) }))
    }
}

#[tokio::test]
async fn preserves_native_deferred_methods_through_provider_overlays() {
    let dir = tempfile::tempdir().expect("tempdir");
    let models_path = dir.path().join("models.json");
    std::fs::write(
        &models_path,
        serde_json::to_string(&json!({
            "providers": {
                "extension-native-deferred": { "baseUrl": "https://overlay.test/v1" },
            }
        }))
        .expect("json"),
    )
    .expect("write");
    let runtime = create_runtime(Some(models_path.to_string_lossy().into_owned())).await;
    let capture = Arc::new(DeferredCapture::default());
    runtime
        .register_native_provider(Arc::new(DeferredProvider {
            id: "extension-native-deferred".to_owned(),
            auth: ProviderAuth {
                api_key: Some(Arc::new(StaticKeyAuth)),
                oauth: None,
            },
            models: vec![model(
                "native-deferred",
                "extension-native-deferred",
                "https://native.test/v1",
            )],
            capture: Arc::clone(&capture),
        }))
        .expect("register");
    let composed = runtime
        .get_model("extension-native-deferred", "native-deferred")
        .expect("model");

    let mut fetch = ModelsDeferredFetchOptions {
        transforms: ModelsRequestTransforms {
            transform_headers: Some(Arc::new(|headers: ProviderHeaders| {
                let mut next = headers;
                next.insert("X-Transformed".to_owned(), Some("fetch".to_owned()));
                Box::pin(async move { next })
            })),
        },
        ..ModelsDeferredFetchOptions::default()
    };
    fetch.options.wait = Some(25);
    fetch.options.base.headers = Some(ProviderHeaders::from([(
        "X-Fetch".to_owned(),
        Some("fetch".to_owned()),
    )]));
    runtime
        .fetch_deferred(
            &composed,
            &DeferredHandle {
                provider: "extension-native-deferred".to_owned(),
                model_id: "native-deferred".to_owned(),
                api: "openai-completions".to_owned(),
                id: "fetch-id".to_owned(),
                expires_at: None,
                poll_after_ms: None,
                data: None,
            },
            Some(fetch),
        )
        .await
        .expect("fetch");

    let mut cancel = ModelsDeferredCancelOptions {
        transforms: ModelsRequestTransforms {
            transform_headers: Some(Arc::new(|headers: ProviderHeaders| {
                let mut next = headers;
                next.insert("X-Transformed".to_owned(), Some("cancel".to_owned()));
                Box::pin(async move { next })
            })),
        },
        ..ModelsDeferredCancelOptions::default()
    };
    cancel.options.timeout_ms = Some(100);
    runtime
        .cancel_deferred(
            &composed,
            &DeferredHandle {
                provider: "extension-native-deferred".to_owned(),
                model_id: "native-deferred".to_owned(),
                api: "openai-completions".to_owned(),
                id: "cancel-id".to_owned(),
                expires_at: None,
                poll_after_ms: None,
                data: None,
            },
            Some(cancel),
        )
        .await
        .expect("cancel");

    assert_eq!(
        capture
            .fetched_base_url
            .lock()
            .expect("base url")
            .as_deref(),
        Some("https://overlay.test/v1")
    );
    assert_eq!(
        *capture.fetched_options.lock().expect("options"),
        Some((
            Some("key".to_owned()),
            Some(25),
            Some(ProviderHeaders::from([
                ("X-Fetch".to_owned(), Some("fetch".to_owned())),
                ("X-Transformed".to_owned(), Some("fetch".to_owned())),
            ]))
        ))
    );
    assert_eq!(
        capture.cancelled_id.lock().expect("id").as_deref(),
        Some("cancel-id")
    );
    assert_eq!(
        *capture.cancelled_options.lock().expect("options"),
        Some((
            Some("key".to_owned()),
            Some(100),
            Some(ProviderHeaders::from([(
                "X-Transformed".to_owned(),
                Some("cancel".to_owned())
            )]))
        ))
    );
}

// ---------------------------------------------------------------------------
// 3. models.json overrides above native providers
// ---------------------------------------------------------------------------

#[tokio::test]
async fn applies_models_json_overrides_above_native_providers() {
    let dir = tempfile::tempdir().expect("tempdir");
    let models_path = dir.path().join("models.json");
    std::fs::write(
        &models_path,
        serde_json::to_string(&json!({
            "providers": {
                "extension-native": { "modelOverrides": { "native": { "contextWindow": 4242 } } },
            }
        }))
        .expect("json"),
    )
    .expect("write");
    let runtime = create_runtime(Some(models_path.to_string_lossy().into_owned())).await;
    runtime
        .register_native_provider(Arc::new(NativeProvider {
            id: "extension-native".to_owned(),
            name: "Extension Native".to_owned(),
            auth: ProviderAuth {
                api_key: Some(Arc::new(StaticKeyAuth)),
                oauth: None,
            },
            models: vec![model(
                "native",
                "extension-native",
                "https://native.test/v1",
            )],
        }))
        .expect("register");

    assert_eq!(
        runtime
            .get_model("extension-native", "native")
            .expect("model")
            .context_window,
        4242
    );
}
