//! Port of `packages/coding-agent/test/model-runtime-auth-options.test.ts` (326 LOC).
//!
//! The three cases that only exercise the extension `registerProvider(name, config)`
//! API (extension API-key method, extension OAuth refresh cancellation, no fabricated
//! API-key method for an OAuth-only extension provider) are excluded with that API
//! (deviation class 2). The two header cases are re-expressed through models.json plus
//! a native provider, which is the path that survives.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use notagent::core::auth_storage::AuthStorage;
use notagent::core::model_runtime::{
    CreateModelRuntimeOptions, ModelRuntime, ModelsRequestTransforms, ModelsSimpleStreamOptions,
};
use notagent_ai::auth::credential_store::InMemoryCredentialStore;
use notagent_ai::auth::types::{
    ApiKeyAuth, ApiKeyAuthInput, AuthError, AuthOperationOptions, AuthResult, AuthType, Credential,
    CredentialInfo, CredentialStore, CredentialStoreError, ModelAuth, ModifyFn, ProviderAuth,
};
use notagent_ai::models::Provider;
use notagent_ai::types::{
    AssistantMessageEvent, Context, ErrorReason, Modality, Model, ModelCost, ProviderHeaders,
    SimpleStreamOptions, StreamOptions,
};
use notagent_ai::utils::event_stream::{
    AssistantMessageEventStream, create_assistant_message_event_stream,
};
use serde_json::json;

fn in_memory_auth(data: serde_json::Value) -> Arc<AuthStorage> {
    Arc::new(AuthStorage::in_memory(
        data.as_object().expect("object").clone(),
    ))
}

async fn runtime(credentials: Arc<dyn CredentialStore>) -> Arc<ModelRuntime> {
    ModelRuntime::create(CreateModelRuntimeOptions {
        credentials: Some(credentials),
        models_path: Some(None),
        allow_model_network: Some(false),
        ..CreateModelRuntimeOptions::default()
    })
    .await
    .expect("runtime")
}

async fn runtime_with_models_json(
    credentials: Arc<dyn CredentialStore>,
    dir: &std::path::Path,
    providers: serde_json::Value,
) -> Arc<ModelRuntime> {
    let path = dir.join("models.json");
    std::fs::write(
        &path,
        serde_json::to_string(&json!({ "providers": providers })).expect("json"),
    )
    .expect("write");
    ModelRuntime::create(CreateModelRuntimeOptions {
        credentials: Some(credentials),
        models_path: Some(Some(path.to_string_lossy().into_owned())),
        allow_model_network: Some(false),
        ..CreateModelRuntimeOptions::default()
    })
    .await
    .expect("runtime")
}

/// `authOptions(runtime, type?)`
fn auth_options(
    runtime: &ModelRuntime,
    auth_type: Option<AuthType>,
) -> Vec<(AuthType, String, String)> {
    runtime
        .get_providers()
        .into_iter()
        .flat_map(|provider| {
            let mut options = Vec::new();
            if auth_type != Some(AuthType::ApiKey)
                && let Some(oauth) = provider.auth().oauth.clone()
            {
                options.push((
                    AuthType::OAuth,
                    provider.id().to_owned(),
                    oauth.name().to_owned(),
                ));
            }
            if auth_type != Some(AuthType::OAuth)
                && let Some(api_key) = provider.auth().api_key.clone()
            {
                options.push((
                    AuthType::ApiKey,
                    provider.id().to_owned(),
                    api_key.name().to_owned(),
                ));
            }
            options
        })
        .collect()
}

fn provider_name(runtime: &ModelRuntime, id: &str) -> Option<String> {
    runtime
        .get_provider(id)
        .map(|provider| provider.name().to_owned())
}

#[tokio::test]
async fn accepts_a_notagent_ai_credential_store() {
    let credentials = Arc::new(InMemoryCredentialStore::new());
    credentials
        .modify(
            "anthropic",
            Box::new(|_current| {
                Box::pin(async {
                    Ok(Some(Credential::ApiKey(
                        notagent_ai::auth::types::ApiKeyCredential {
                            key: Some("stored-key".to_owned()),
                            env: None,
                        },
                    )))
                })
            }),
            None,
        )
        .await
        .expect("modify");
    let runtime = runtime(credentials as Arc<dyn CredentialStore>).await;

    assert_eq!(
        runtime
            .get_auth_for_provider("anthropic", None)
            .await
            .expect("auth")
            .and_then(|resolution| resolution.auth.api_key)
            .as_deref(),
        Some("stored-key")
    );
}

/// A store that records every read and can be switched to failing.
struct RecordingStore {
    base: InMemoryCredentialStore,
    reads: Mutex<Vec<String>>,
    fail: Mutex<bool>,
}

impl CredentialStore for RecordingStore {
    fn read(
        &self,
        provider_id: &str,
        options: Option<AuthOperationOptions>,
    ) -> BoxFuture<'_, Result<Option<Credential>, CredentialStoreError>> {
        self.reads
            .lock()
            .expect("reads")
            .push(provider_id.to_owned());
        let fail = *self.fail.lock().expect("fail");
        let provider_id = provider_id.to_owned();
        Box::pin(async move {
            if fail {
                return Err(CredentialStoreError(format!(
                    "read failed for {provider_id}"
                )));
            }
            self.base.read(&provider_id, options).await
        })
    }

    fn list(
        &self,
        options: Option<AuthOperationOptions>,
    ) -> BoxFuture<'_, Result<Vec<CredentialInfo>, CredentialStoreError>> {
        self.base.list(options)
    }

    fn modify<'a>(
        &'a self,
        provider_id: &'a str,
        modify: ModifyFn<'a>,
        options: Option<AuthOperationOptions>,
    ) -> BoxFuture<'a, Result<Option<Credential>, CredentialStoreError>> {
        self.base.modify(provider_id, modify, options)
    }

    fn delete(
        &self,
        provider_id: &str,
        options: Option<AuthOperationOptions>,
    ) -> BoxFuture<'_, Result<(), CredentialStoreError>> {
        self.base.delete(provider_id, options)
    }
}

#[tokio::test]
async fn scopes_provider_availability_reads_and_records_refresh_failures() {
    let credentials = Arc::new(RecordingStore {
        base: InMemoryCredentialStore::new(),
        reads: Mutex::new(Vec::new()),
        fail: Mutex::new(false),
    });
    let runtime = runtime(Arc::clone(&credentials) as Arc<dyn CredentialStore>).await;

    credentials.reads.lock().expect("reads").clear();
    runtime
        .get_available(Some("anthropic"), None)
        .await
        .expect("available");
    let reads: std::collections::BTreeSet<String> = credentials
        .reads
        .lock()
        .expect("reads")
        .iter()
        .cloned()
        .collect();
    assert_eq!(
        reads,
        std::collections::BTreeSet::from(["anthropic".to_owned()])
    );

    *credentials.fail.lock().expect("fail") = true;
    let error = runtime
        .get_available(Some("anthropic"), None)
        .await
        .expect_err("read failure");
    assert!(
        error
            .message
            .contains("Credential store read failed for anthropic"),
        "unexpected message: {}",
        error.message
    );
    assert!(
        runtime
            .get_error()
            .expect("error")
            .contains("Availability refresh: Credential store read failed for anthropic")
    );

    *credentials.fail.lock().expect("fail") = false;
    runtime.get_available(None, None).await.expect("available");
    assert_eq!(runtime.get_error(), None);
}

#[tokio::test]
async fn projects_provider_owned_methods_names_and_status() {
    let runtime = runtime(in_memory_auth(json!({})) as Arc<dyn CredentialStore>).await;
    let options = auth_options(&runtime, None);

    assert!(options.contains(&(
        AuthType::ApiKey,
        "amazon-bedrock".to_owned(),
        "AWS credentials or bearer token".to_owned()
    )));
    assert_eq!(
        provider_name(&runtime, "amazon-bedrock").as_deref(),
        Some("Amazon Bedrock")
    );
    assert!(options.contains(&(
        AuthType::ApiKey,
        "google-vertex".to_owned(),
        "Google Cloud credentials".to_owned()
    )));
    assert_eq!(
        provider_name(&runtime, "google-vertex").as_deref(),
        Some("Google Vertex AI")
    );
    assert!(
        options
            .iter()
            .any(|(kind, id, _)| *kind == AuthType::OAuth && id == "anthropic")
    );
    assert_eq!(
        provider_name(&runtime, "anthropic").as_deref(),
        Some("Anthropic")
    );
    for id in ["cloudflare-ai-gateway", "cloudflare-workers-ai"] {
        assert!(
            options
                .iter()
                .any(|(kind, provider, _)| *kind == AuthType::ApiKey && provider == id)
        );
    }
    assert_eq!(
        provider_name(&runtime, "cloudflare-ai-gateway").as_deref(),
        Some("Cloudflare AI Gateway")
    );
    assert_eq!(
        provider_name(&runtime, "cloudflare-workers-ai").as_deref(),
        Some("Cloudflare Workers AI")
    );

    assert!(
        auth_options(&runtime, Some(AuthType::ApiKey))
            .iter()
            .all(|(kind, _, _)| *kind == AuthType::ApiKey)
    );
    assert!(
        auth_options(&runtime, Some(AuthType::OAuth))
            .iter()
            .all(|(kind, _, _)| *kind == AuthType::OAuth)
    );
    assert!(
        !options
            .iter()
            .any(|(kind, id, _)| id == "openai-codex" && *kind == AuthType::ApiKey)
    );
}

#[tokio::test]
async fn attaches_the_providers_active_auth_status_to_every_method_option() {
    let runtime = runtime(in_memory_auth(json!({
        "anthropic": {
            "type": "oauth",
            "access": "access",
            "refresh": "refresh",
            "expires": 4_102_444_800_000_i64,
        }
    })) as Arc<dyn CredentialStore>)
    .await;

    let options: Vec<_> = auth_options(&runtime, None)
        .into_iter()
        .filter(|(_, id, _)| id == "anthropic")
        .collect();
    assert_eq!(options.len(), 2);
    assert_eq!(
        runtime
            .check_auth("anthropic", None)
            .await
            .expect("check")
            .map(|check| check.check_type),
        Some(AuthType::OAuth)
    );
}

#[tokio::test]
async fn distinguishes_subscription_oauth_from_generic_oauth_sign_in() {
    let runtime = runtime(in_memory_auth(json!({
        "anthropic": {
            "type": "oauth",
            "access": "anthropic-access",
            "refresh": "anthropic-refresh",
            "expires": 4_102_444_800_000_i64,
        },
        "openrouter": {
            "type": "oauth",
            "access": "openrouter-key",
            "refresh": "",
            "expires": 9_007_199_254_740_991_i64,
        },
        "radius": {
            "type": "oauth",
            "access": "radius-access",
            "refresh": "radius-refresh",
            "expires": 4_102_444_800_000_i64,
        },
    })) as Arc<dyn CredentialStore>)
    .await;

    assert!(runtime.is_using_oauth("anthropic"));
    assert!(runtime.is_using_subscription("anthropic"));
    assert!(runtime.is_using_oauth("openrouter"));
    assert!(!runtime.is_using_subscription("openrouter"));
    assert!(runtime.is_using_oauth("radius"));
    assert!(!runtime.is_using_subscription("radius"));
}

#[tokio::test]
async fn resolves_configured_auth_from_request_scoped_environment_overrides() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = runtime_with_models_json(
        in_memory_auth(json!({})) as Arc<dyn CredentialStore>,
        dir.path(),
        json!({
            "request-env-provider": {
                "baseUrl": "https://example.test/v1",
                "apiKey": "$REQUEST_SCOPED_API_KEY",
                "headers": { "x-request-value": "$REQUEST_SCOPED_HEADER" },
                "api": "openai-completions",
                "models": [test_model_json("request-env-model")],
            },
        }),
    )
    .await;

    let auth = runtime
        .get_auth_for_provider(
            "request-env-provider",
            Some(&notagent::core::model_runtime::ModelRuntimeAuthOverrides {
                env: Some(BTreeMap::from([
                    (
                        "REQUEST_SCOPED_API_KEY".to_owned(),
                        "request-key".to_owned(),
                    ),
                    (
                        "REQUEST_SCOPED_HEADER".to_owned(),
                        "request-header".to_owned(),
                    ),
                ])),
                ..Default::default()
            }),
        )
        .await
        .expect("auth")
        .expect("resolution");

    assert_eq!(auth.auth.api_key.as_deref(), Some("request-key"));
    assert_eq!(
        auth.auth.headers,
        Some(ProviderHeaders::from([(
            "x-request-value".to_owned(),
            Some("request-header".to_owned())
        )]))
    );
}

fn test_model_json(id: &str) -> serde_json::Value {
    json!({
        "id": id,
        "name": id,
        "reasoning": false,
        "input": ["text"],
        "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
        "contextWindow": 10000,
        "maxTokens": 1000,
    })
}

/// A native provider whose `streamSimple` records the fully assembled headers.
struct CapturingProvider {
    id: String,
    auth: ProviderAuth,
    models: Vec<Model>,
    captured: Arc<Mutex<Option<ProviderHeaders>>>,
}

struct AmbientApiKeyAuth;

impl ApiKeyAuth for AmbientApiKeyAuth {
    fn name(&self) -> &str {
        "API key"
    }

    fn resolve<'a>(
        &'a self,
        input: ApiKeyAuthInput<'a>,
    ) -> BoxFuture<'a, Result<Option<AuthResult>, AuthError>> {
        let credential = input.credential.clone();
        Box::pin(async move {
            Ok(Some(AuthResult {
                auth: ModelAuth {
                    api_key: credential.and_then(|credential| credential.key),
                    headers: None,
                    base_url: None,
                },
                env: None,
                source: Some("ambient".to_owned()),
            }))
        })
    }
}

impl Provider for CapturingProvider {
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
        self.models.clone()
    }

    fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: Option<StreamOptions>,
    ) -> AssistantMessageEventStream {
        self.stream_simple(
            model,
            context,
            options.map(|options| SimpleStreamOptions {
                base: options,
                ..SimpleStreamOptions::default()
            }),
        )
    }

    fn stream_simple(
        &self,
        model: &Model,
        _context: &Context,
        options: Option<SimpleStreamOptions>,
    ) -> AssistantMessageEventStream {
        *self.captured.lock().expect("captured") =
            options.and_then(|options| options.base.base.headers.clone());
        let stream = create_assistant_message_event_stream();
        let message = notagent_ai::create_setup_error_message(model, &"captured", 0);
        stream.push(AssistantMessageEvent::Error {
            reason: ErrorReason::Error,
            error: message.clone(),
        });
        stream.end(Some(message));
        stream
    }
}

fn capturing_provider(
    id: &str,
    model_id: &str,
    captured: Arc<Mutex<Option<ProviderHeaders>>>,
) -> Arc<dyn Provider> {
    Arc::new(CapturingProvider {
        id: id.to_owned(),
        auth: ProviderAuth {
            api_key: Some(Arc::new(AmbientApiKeyAuth)),
            oauth: None,
        },
        models: vec![Model {
            id: model_id.to_owned(),
            name: model_id.to_owned(),
            api: "openai-completions".to_owned(),
            provider: id.to_owned(),
            base_url: "https://example.test/v1".to_owned(),
            reasoning: false,
            thinking_level_map: None,
            input: vec![Modality::Text],
            cost: ModelCost::default(),
            context_window: 10_000,
            max_tokens: 1_000,
            sampling_params: None,
            headers: None,
            compat: None,
        }],
        captured,
    })
}

#[tokio::test]
async fn an_explicit_authorization_header_overrides_auth_header_case_insensitively() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = runtime_with_models_json(
        in_memory_auth(json!({})) as Arc<dyn CredentialStore>,
        dir.path(),
        json!({
            "auth-header-provider": {
                "baseUrl": "https://example.test/v1",
                "apiKey": "generated-key",
                "authHeader": true,
                "api": "openai-completions",
                "models": [test_model_json("auth-header-model")],
            },
        }),
    )
    .await;
    let captured: Arc<Mutex<Option<ProviderHeaders>>> = Arc::new(Mutex::new(None));
    runtime
        .register_native_provider(capturing_provider(
            "auth-header-provider",
            "auth-header-model",
            Arc::clone(&captured),
        ))
        .expect("register");
    let model = runtime
        .get_model("auth-header-provider", "auth-header-model")
        .expect("model");

    let mut options = ModelsSimpleStreamOptions::default();
    options.options.base.base.headers = Some(ProviderHeaders::from([(
        "authorization".to_owned(),
        Some("Explicit token".to_owned()),
    )]));
    runtime
        .complete_simple(&model, &Context::default(), Some(options))
        .await;

    assert_eq!(
        captured.lock().expect("captured").clone(),
        Some(ProviderHeaders::from([(
            "authorization".to_owned(),
            Some("Explicit token".to_owned())
        )]))
    );
}

#[tokio::test]
async fn transforms_fully_assembled_headers_once_without_forwarding_the_transform() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut model_json = test_model_json("header-model");
    model_json["headers"] = json!({ "x-model": "model" });
    let runtime = runtime_with_models_json(
        in_memory_auth(json!({})) as Arc<dyn CredentialStore>,
        dir.path(),
        json!({
            "header-provider": {
                "baseUrl": "https://example.test/v1",
                "apiKey": "generated-key",
                "authHeader": true,
                "headers": { "x-provider": "provider" },
                "api": "openai-completions",
                "models": [model_json],
            },
        }),
    )
    .await;
    let captured: Arc<Mutex<Option<ProviderHeaders>>> = Arc::new(Mutex::new(None));
    runtime
        .register_native_provider(capturing_provider(
            "header-provider",
            "header-model",
            Arc::clone(&captured),
        ))
        .expect("register");
    let model = runtime
        .get_model("header-provider", "header-model")
        .expect("model");

    let transforms = Arc::new(Mutex::new(0_usize));
    let seen: Arc<Mutex<Option<ProviderHeaders>>> = Arc::new(Mutex::new(None));
    let transform_count = Arc::clone(&transforms);
    let transform_seen = Arc::clone(&seen);
    let mut options = ModelsSimpleStreamOptions {
        transforms: ModelsRequestTransforms {
            transform_headers: Some(Arc::new(move |headers: ProviderHeaders| {
                *transform_count.lock().expect("count") += 1;
                *transform_seen.lock().expect("seen") = Some(headers.clone());
                let mut next = headers;
                next.insert("x-transformed".to_owned(), Some("yes".to_owned()));
                Box::pin(async move { next })
            })),
        },
        ..ModelsSimpleStreamOptions::default()
    };
    options.options.base.base.headers = Some(ProviderHeaders::from([(
        "x-explicit".to_owned(),
        Some("explicit".to_owned()),
    )]));
    runtime
        .complete_simple(&model, &Context::default(), Some(options))
        .await;

    assert_eq!(*transforms.lock().expect("count"), 1);
    let expected_input = ProviderHeaders::from([
        (
            "Authorization".to_owned(),
            Some("Bearer generated-key".to_owned()),
        ),
        ("x-provider".to_owned(), Some("provider".to_owned())),
        ("x-model".to_owned(), Some("model".to_owned())),
        ("x-explicit".to_owned(), Some("explicit".to_owned())),
    ]);
    assert_eq!(
        seen.lock().expect("seen").clone(),
        Some(expected_input.clone())
    );
    let mut expected_output = expected_input;
    expected_output.insert("x-transformed".to_owned(), Some("yes".to_owned()));
    assert_eq!(
        captured.lock().expect("captured").clone(),
        Some(expected_output)
    );
}
