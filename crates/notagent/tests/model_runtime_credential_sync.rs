//! Port of `packages/coding-agent/test/model-runtime-credential-sync.test.ts` (375 LOC).

use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use notagent::core::auth_storage::AuthStorage;
use notagent::core::model_runtime::{
    CreateModelRuntimeOptions, CredentialSynchronizationOperation, ModelRuntime, ModelRuntimeError,
};
use notagent_ai::auth::types::{
    ApiKeyAuth, ApiKeyAuthInput, ApiKeyCredential, AuthCheck, AuthError, AuthEvent,
    AuthInteraction, AuthOperationOptions, AuthPrompt, AuthResult, AuthType, Credential,
    CredentialInfo, CredentialStore, CredentialStoreError, ModelAuth, ModifyFn, ProviderAuth,
    ProviderAuthInteraction,
};
use notagent_ai::models::{ModelsRefreshOptions, Provider, RefreshModelsContext};
use notagent_ai::types::{Context, Modality, Model, ModelCost, SimpleStreamOptions, StreamOptions};
use notagent_ai::utils::event_stream::AssistantMessageEventStream;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

fn model(provider: &str) -> Model {
    Model {
        id: "dynamic".to_owned(),
        name: "Dynamic".to_owned(),
        api: "openai-completions".to_owned(),
        provider: provider.to_owned(),
        base_url: "https://example.test/v1".to_owned(),
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

type LoginHook = Arc<dyn Fn() -> BoxFuture<'static, ApiKeyCredential> + Send + Sync>;
type CheckHook =
    Arc<dyn Fn(Option<ApiKeyCredential>) -> BoxFuture<'static, Option<AuthCheck>> + Send + Sync>;
type RefreshHook = Arc<dyn Fn(bool) -> BoxFuture<'static, Result<(), String>> + Send + Sync>;

#[derive(Default)]
struct TestProviderOptions {
    login: Option<LoginHook>,
    check: Option<CheckHook>,
    refresh_models: Option<RefreshHook>,
}

struct TestApiKeyAuth {
    id: String,
    login: Option<LoginHook>,
    check: Option<CheckHook>,
}

impl ApiKeyAuth for TestApiKeyAuth {
    fn name(&self) -> &str {
        "API key"
    }

    fn login<'a>(
        &'a self,
        _interaction: &'a ProviderAuthInteraction<'a>,
    ) -> Option<BoxFuture<'a, Result<ApiKeyCredential, AuthError>>> {
        let login = self.login.clone();
        let id = self.id.clone();
        Some(Box::pin(async move {
            Ok(match login {
                Some(login) => login().await,
                None => ApiKeyCredential {
                    key: Some(format!("{id}-key")),
                    env: None,
                },
            })
        }))
    }

    fn check<'a>(
        &'a self,
        input: ApiKeyAuthInput<'a>,
    ) -> Option<BoxFuture<'a, Result<Option<AuthCheck>, AuthError>>> {
        let check = self.check.clone();
        let credential = input.credential.clone();
        Some(Box::pin(async move {
            Ok(match check {
                Some(check) => check(credential).await,
                None => credential.map(|_| AuthCheck {
                    source: Some("stored".to_owned()),
                    check_type: AuthType::ApiKey,
                }),
            })
        }))
    }

    fn resolve<'a>(
        &'a self,
        input: ApiKeyAuthInput<'a>,
    ) -> BoxFuture<'a, Result<Option<AuthResult>, AuthError>> {
        let credential = input.credential.clone();
        Box::pin(async move {
            Ok(credential.map(|credential| AuthResult {
                auth: ModelAuth {
                    api_key: credential.key,
                    headers: None,
                    base_url: None,
                },
                env: None,
                source: Some("stored".to_owned()),
            }))
        })
    }
}

struct TestProvider {
    id: String,
    auth: ProviderAuth,
    models: Vec<Model>,
    refresh_models: Option<RefreshHook>,
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
        self.models.clone()
    }

    fn is_dynamic(&self) -> bool {
        self.refresh_models.is_some()
    }

    fn refresh_models<'a>(
        &'a self,
        context: RefreshModelsContext<'a>,
    ) -> Option<BoxFuture<'a, Result<(), String>>> {
        let refresh = self.refresh_models.clone()?;
        let allow_network = context.allow_network;
        Some(Box::pin(async move { refresh(allow_network).await }))
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

fn provider(id: &str, options: TestProviderOptions) -> Arc<dyn Provider> {
    Arc::new(TestProvider {
        id: id.to_owned(),
        auth: ProviderAuth {
            api_key: Some(Arc::new(TestApiKeyAuth {
                id: id.to_owned(),
                login: options.login,
                check: options.check,
            })),
            oauth: None,
        },
        models: vec![model(id)],
        refresh_models: options.refresh_models,
    })
}

struct SilentInteraction {
    signal: Option<CancellationToken>,
}

impl AuthInteraction for SilentInteraction {
    fn signal(&self) -> Option<CancellationToken> {
        self.signal.clone()
    }

    fn prompt(&self, _prompt: AuthPrompt) -> BoxFuture<'_, Result<String, AuthError>> {
        Box::pin(async { Ok("unused".to_owned()) })
    }

    fn notify(&self, _event: AuthEvent) {}
}

async fn runtime_with_credentials(credentials: Arc<dyn CredentialStore>) -> Arc<ModelRuntime> {
    ModelRuntime::create(CreateModelRuntimeOptions {
        credentials: Some(credentials),
        models_path: Some(None),
        allow_model_network: Some(false),
        ..CreateModelRuntimeOptions::default()
    })
    .await
    .expect("runtime")
}

async fn runtime_with_provider(
    registered: Arc<dyn Provider>,
    credentials: Arc<dyn CredentialStore>,
) -> Arc<ModelRuntime> {
    let runtime = runtime_with_credentials(credentials).await;
    let id = registered.id().to_owned();
    runtime
        .register_native_provider(registered)
        .expect("register");
    runtime
        .refresh(ModelsRefreshOptions {
            allow_network: Some(false),
            providers: Some(vec![id]),
            ..ModelsRefreshOptions::default()
        })
        .await;
    runtime
}

fn in_memory() -> Arc<AuthStorage> {
    Arc::new(AuthStorage::in_memory(serde_json::Map::new()))
}

fn api_key(key: &str) -> Credential {
    Credential::ApiKey(ApiKeyCredential {
        key: Some(key.to_owned()),
        env: None,
    })
}

#[tokio::test]
async fn publishes_locally_consistent_availability_before_login_and_logout_resolve() {
    let credentials = in_memory();
    let runtime = runtime_with_provider(
        provider("dynamic", TestProviderOptions::default()),
        Arc::clone(&credentials) as Arc<dyn CredentialStore>,
    )
    .await;

    runtime
        .login(
            "dynamic",
            AuthType::ApiKey,
            &SilentInteraction { signal: None },
        )
        .await
        .expect("login");
    assert!(runtime.has_configured_auth("dynamic"));
    assert!(
        runtime
            .get_available_snapshot()
            .iter()
            .any(|entry| entry.id == "dynamic")
    );
    assert_eq!(
        credentials.read("dynamic", None).await.expect("read"),
        Some(api_key("dynamic-key"))
    );

    runtime.logout("dynamic", None).await.expect("logout");
    assert!(!runtime.has_configured_auth("dynamic"));
    assert!(
        !runtime
            .get_available_snapshot()
            .iter()
            .any(|entry| entry.provider == "dynamic")
    );
    assert_eq!(credentials.read("dynamic", None).await.expect("read"), None);
}

#[tokio::test]
async fn orders_same_provider_credential_operations_through_local_synchronization() {
    let login_started = Arc::new(Notify::new());
    let blocked_login = Arc::new(Notify::new());
    let started = Arc::clone(&login_started);
    let blocked = Arc::clone(&blocked_login);
    let credentials = in_memory();
    let runtime = runtime_with_provider(
        provider(
            "ordered",
            TestProviderOptions {
                login: Some(Arc::new(move || {
                    let started = Arc::clone(&started);
                    let blocked = Arc::clone(&blocked);
                    Box::pin(async move {
                        started.notify_one();
                        blocked.notified().await;
                        ApiKeyCredential {
                            key: Some("ordered-key".to_owned()),
                            env: None,
                        }
                    })
                })),
                ..TestProviderOptions::default()
            },
        ),
        Arc::clone(&credentials) as Arc<dyn CredentialStore>,
    )
    .await;

    let login_runtime = Arc::clone(&runtime);
    let wait_started = login_started.notified();
    let login = tokio::spawn(async move {
        login_runtime
            .login(
                "ordered",
                AuthType::ApiKey,
                &SilentInteraction { signal: None },
            )
            .await
            .map(|_| ())
    });
    wait_started.await;
    let logout_runtime = Arc::clone(&runtime);
    let logout = tokio::spawn(async move { logout_runtime.logout("ordered", None).await });
    tokio::task::yield_now().await;
    assert_eq!(credentials.read("ordered", None).await.expect("read"), None);

    blocked_login.notify_waiters();
    let _ = login.await.expect("join");
    let _ = logout.await.expect("join");
    assert_eq!(credentials.read("ordered", None).await.expect("read"), None);
    assert!(!runtime.has_configured_auth("ordered"));
}

#[tokio::test]
async fn allows_different_providers_to_run_credential_flows_concurrently() {
    let first_started = Arc::new(Notify::new());
    let second_started = Arc::new(Notify::new());
    let blocked = Arc::new(Notify::new());
    let runtime = runtime_with_credentials(in_memory() as Arc<dyn CredentialStore>).await;
    for (id, started) in [("one", &first_started), ("two", &second_started)] {
        let started = Arc::clone(started);
        let blocked = Arc::clone(&blocked);
        let key = id.to_owned();
        runtime
            .register_native_provider(provider(
                id,
                TestProviderOptions {
                    login: Some(Arc::new(move || {
                        let started = Arc::clone(&started);
                        let blocked = Arc::clone(&blocked);
                        let key = key.clone();
                        Box::pin(async move {
                            started.notify_one();
                            blocked.notified().await;
                            ApiKeyCredential {
                                key: Some(key),
                                env: None,
                            }
                        })
                    })),
                    ..TestProviderOptions::default()
                },
            ))
            .expect("register");
    }
    runtime
        .refresh(ModelsRefreshOptions {
            allow_network: Some(false),
            providers: Some(vec!["one".to_owned(), "two".to_owned()]),
            ..ModelsRefreshOptions::default()
        })
        .await;

    let wait_first = first_started.notified();
    let wait_second = second_started.notified();
    let one_runtime = Arc::clone(&runtime);
    let one = tokio::spawn(async move {
        one_runtime
            .login("one", AuthType::ApiKey, &SilentInteraction { signal: None })
            .await
            .map(|_| ())
    });
    let two_runtime = Arc::clone(&runtime);
    let two = tokio::spawn(async move {
        two_runtime
            .login("two", AuthType::ApiKey, &SilentInteraction { signal: None })
            .await
            .map(|_| ())
    });
    wait_first.await;
    wait_second.await;
    blocked.notify_waiters();
    one.await.expect("join").expect("one");
    two.await.expect("join").expect("two");
}

#[tokio::test]
async fn does_not_wait_for_unrelated_provider_availability() {
    let stall = Arc::new(Mutex::new(false));
    let runtime = runtime_with_credentials(in_memory() as Arc<dyn CredentialStore>).await;
    runtime
        .register_native_provider(provider("target", TestProviderOptions::default()))
        .expect("register");
    let stall_check = Arc::clone(&stall);
    runtime
        .register_native_provider(provider(
            "unrelated",
            TestProviderOptions {
                check: Some(Arc::new(move |_credential| {
                    let stall = Arc::clone(&stall_check);
                    Box::pin(async move {
                        if *stall.lock().expect("stall") {
                            std::future::pending::<()>().await;
                        }
                        None
                    })
                })),
                ..TestProviderOptions::default()
            },
        ))
        .expect("register");
    runtime
        .refresh(ModelsRefreshOptions {
            allow_network: Some(false),
            providers: Some(vec!["target".to_owned(), "unrelated".to_owned()]),
            ..ModelsRefreshOptions::default()
        })
        .await;
    *stall.lock().expect("stall") = true;

    runtime
        .login(
            "target",
            AuthType::ApiKey,
            &SilentInteraction { signal: None },
        )
        .await
        .expect("login");
    assert!(runtime.has_configured_auth("target"));
    let result = runtime
        .refresh(ModelsRefreshOptions {
            allow_network: Some(false),
            providers: Some(vec!["target".to_owned()]),
            ..ModelsRefreshOptions::default()
        })
        .await;
    assert!(!result.aborted);
}

#[tokio::test]
async fn reports_cancellation_during_provider_scoped_availability() {
    let block = Arc::new(Mutex::new(false));
    let started = Arc::new(Notify::new());
    let block_check = Arc::clone(&block);
    let started_check = Arc::clone(&started);
    let runtime = runtime_with_provider(
        provider(
            "cancelled-availability",
            TestProviderOptions {
                check: Some(Arc::new(move |credential| {
                    let block = Arc::clone(&block_check);
                    let started = Arc::clone(&started_check);
                    Box::pin(async move {
                        if *block.lock().expect("block") {
                            started.notify_one();
                            std::future::pending::<()>().await;
                        }
                        credential.map(|_| AuthCheck {
                            source: Some("stored".to_owned()),
                            check_type: AuthType::ApiKey,
                        })
                    })
                })),
                ..TestProviderOptions::default()
            },
        ),
        in_memory() as Arc<dyn CredentialStore>,
    )
    .await;
    runtime
        .set_runtime_api_key("cancelled-availability", "key", None)
        .await
        .expect("runtime key");
    *block.lock().expect("block") = true;

    let signal = CancellationToken::new();
    let wait_started = started.notified();
    let refresh_runtime = Arc::clone(&runtime);
    let refresh_signal = signal.clone();
    let refresh = tokio::spawn(async move {
        refresh_runtime
            .refresh(ModelsRefreshOptions {
                allow_network: Some(false),
                providers: Some(vec!["cancelled-availability".to_owned()]),
                signal: Some(refresh_signal),
                ..ModelsRefreshOptions::default()
            })
            .await
    });
    wait_started.await;
    signal.cancel();

    let result = refresh.await.expect("join");
    assert!(result.aborted);
}

#[tokio::test]
async fn does_not_run_network_refresh_inside_the_credential_operation_chain() {
    let network_calls = Arc::new(Mutex::new(0_usize));
    let counter = Arc::clone(&network_calls);
    let runtime = runtime_with_provider(
        provider(
            "local-only",
            TestProviderOptions {
                refresh_models: Some(Arc::new(move |allow_network| {
                    let counter = Arc::clone(&counter);
                    Box::pin(async move {
                        if allow_network {
                            *counter.lock().expect("counter") += 1;
                            std::future::pending::<()>().await;
                        }
                        Ok(())
                    })
                })),
                ..TestProviderOptions::default()
            },
        ),
        in_memory() as Arc<dyn CredentialStore>,
    )
    .await;

    runtime
        .login(
            "local-only",
            AuthType::ApiKey,
            &SilentInteraction { signal: None },
        )
        .await
        .expect("login");
    assert_eq!(*network_calls.lock().expect("counter"), 0);
    assert!(runtime.has_configured_auth("local-only"));
}

#[tokio::test]
async fn keeps_provider_scoped_refreshes_from_superseding_unrelated_providers() {
    let started = Arc::new(Notify::new());
    let blocked = Arc::new(Notify::new());
    let captured_signal: Arc<Mutex<Option<CancellationToken>>> = Arc::new(Mutex::new(None));
    let runtime = runtime_with_credentials(in_memory() as Arc<dyn CredentialStore>).await;

    let started_hook = Arc::clone(&started);
    let blocked_hook = Arc::clone(&blocked);
    let observed = Arc::clone(&captured_signal);
    // The refresh signal is only reachable inside `refreshModels`, so the observation
    // happens there instead of through a captured `AbortSignal`.
    runtime
        .register_native_provider(Arc::new(SignalObservingProvider {
            id: "one".to_owned(),
            auth: ProviderAuth {
                api_key: Some(Arc::new(TestApiKeyAuth {
                    id: "one".to_owned(),
                    login: None,
                    check: None,
                })),
                oauth: None,
            },
            models: vec![model("one")],
            started: started_hook,
            blocked: blocked_hook,
            observed,
        }))
        .expect("register");
    runtime
        .register_native_provider(provider("two", TestProviderOptions::default()))
        .expect("register");
    runtime
        .refresh(ModelsRefreshOptions {
            allow_network: Some(false),
            providers: Some(vec!["one".to_owned(), "two".to_owned()]),
            ..ModelsRefreshOptions::default()
        })
        .await;
    runtime
        .set_runtime_api_key("one", "one-key", None)
        .await
        .expect("one key");
    runtime
        .set_runtime_api_key("two", "two-key", None)
        .await
        .expect("two key");

    let wait_started = started.notified();
    let first_runtime = Arc::clone(&runtime);
    let first = tokio::spawn(async move {
        first_runtime
            .refresh(ModelsRefreshOptions {
                allow_network: Some(true),
                providers: Some(vec!["one".to_owned()]),
                ..ModelsRefreshOptions::default()
            })
            .await
    });
    wait_started.await;
    runtime
        .refresh(ModelsRefreshOptions {
            allow_network: Some(true),
            providers: Some(vec!["two".to_owned()]),
            ..ModelsRefreshOptions::default()
        })
        .await;
    assert_eq!(
        captured_signal
            .lock()
            .expect("captured")
            .as_ref()
            .map(CancellationToken::is_cancelled),
        Some(false)
    );

    blocked.notify_waiters();
    let _ = first.await.expect("join");
}

/// Publishes its own refresh signal so the suite can inspect it while blocked.
struct SignalObservingProvider {
    id: String,
    auth: ProviderAuth,
    models: Vec<Model>,
    started: Arc<Notify>,
    blocked: Arc<Notify>,
    observed: Arc<Mutex<Option<CancellationToken>>>,
}

impl Provider for SignalObservingProvider {
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

    fn is_dynamic(&self) -> bool {
        true
    }

    fn refresh_models<'a>(
        &'a self,
        context: RefreshModelsContext<'a>,
    ) -> Option<BoxFuture<'a, Result<(), String>>> {
        Some(Box::pin(async move {
            if !context.allow_network {
                return Ok(());
            }
            *self.observed.lock().expect("observed") = Some(context.signal.clone());
            self.started.notify_one();
            self.blocked.notified().await;
            Ok(())
        }))
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

/// A store whose `modify` commits and then blocks before returning.
struct DelayedCommitStore {
    stored: Mutex<Option<Credential>>,
    committed: Arc<Notify>,
    finish: Arc<Notify>,
}

impl CredentialStore for DelayedCommitStore {
    fn read(
        &self,
        _provider_id: &str,
        _options: Option<AuthOperationOptions>,
    ) -> BoxFuture<'_, Result<Option<Credential>, CredentialStoreError>> {
        let stored = self.stored.lock().expect("stored").clone();
        Box::pin(async move { Ok(stored) })
    }

    fn list(
        &self,
        _options: Option<AuthOperationOptions>,
    ) -> BoxFuture<'_, Result<Vec<CredentialInfo>, CredentialStoreError>> {
        let stored = self.stored.lock().expect("stored").clone();
        Box::pin(async move {
            Ok(stored
                .map(|credential| {
                    vec![CredentialInfo {
                        provider_id: "delayed-commit".to_owned(),
                        credential_type: credential.credential_type(),
                    }]
                })
                .unwrap_or_default())
        })
    }

    fn modify<'a>(
        &'a self,
        _provider_id: &'a str,
        modify: ModifyFn<'a>,
        _options: Option<AuthOperationOptions>,
    ) -> BoxFuture<'a, Result<Option<Credential>, CredentialStoreError>> {
        Box::pin(async move {
            let current = self.stored.lock().expect("stored").clone();
            let next = modify(current).await?;
            if let Some(next) = next {
                *self.stored.lock().expect("stored") = Some(next);
            }
            self.committed.notify_one();
            self.finish.notified().await;
            Ok(self.stored.lock().expect("stored").clone())
        })
    }

    fn delete(
        &self,
        _provider_id: &str,
        _options: Option<AuthOperationOptions>,
    ) -> BoxFuture<'_, Result<(), CredentialStoreError>> {
        Box::pin(async move {
            *self.stored.lock().expect("stored") = None;
            Ok(())
        })
    }
}

#[tokio::test]
async fn waits_for_a_committed_credential_mutation_to_settle_before_reporting_cancellation() {
    let committed = Arc::new(Notify::new());
    let finish = Arc::new(Notify::new());
    let store = Arc::new(DelayedCommitStore {
        stored: Mutex::new(None),
        committed: Arc::clone(&committed),
        finish: Arc::clone(&finish),
    });
    let runtime = runtime_with_provider(
        provider("delayed-commit", TestProviderOptions::default()),
        Arc::clone(&store) as Arc<dyn CredentialStore>,
    )
    .await;

    let signal = CancellationToken::new();
    let settled = Arc::new(Mutex::new(false));
    let login_runtime = Arc::clone(&runtime);
    let login_signal = signal.clone();
    let login_settled = Arc::clone(&settled);
    let wait_committed = committed.notified();
    let login = tokio::spawn(async move {
        let outcome = login_runtime
            .login(
                "delayed-commit",
                AuthType::ApiKey,
                &SilentInteraction {
                    signal: Some(login_signal),
                },
            )
            .await;
        *login_settled.lock().expect("settled") = true;
        outcome
    });
    wait_committed.await;
    signal.cancel();
    tokio::task::yield_now().await;
    assert!(!*settled.lock().expect("settled"));

    finish.notify_waiters();
    let error = login
        .await
        .expect("join")
        .expect_err("synchronization error");
    match error {
        ModelRuntimeError::Synchronization(error) => {
            assert_eq!(error.provider_id, "delayed-commit");
            assert_eq!(error.operation, CredentialSynchronizationOperation::Login);
            assert_eq!(error.credential, Some(api_key("delayed-commit-key")));
        }
        other => panic!("unexpected error: {other}"),
    }
    assert_eq!(
        *store.stored.lock().expect("stored"),
        Some(api_key("delayed-commit-key"))
    );
}

#[tokio::test]
async fn reports_a_typed_error_when_cancellation_interrupts_post_commit_synchronization() {
    let block = Arc::new(Mutex::new(false));
    let started = Arc::new(Notify::new());
    let block_hook = Arc::clone(&block);
    let started_hook = Arc::clone(&started);
    let credentials = in_memory();
    let runtime = runtime_with_provider(
        provider(
            "cancelled-sync",
            TestProviderOptions {
                refresh_models: Some(Arc::new(move |allow_network| {
                    let block = Arc::clone(&block_hook);
                    let started = Arc::clone(&started_hook);
                    Box::pin(async move {
                        if !allow_network && *block.lock().expect("block") {
                            started.notify_one();
                            std::future::pending::<()>().await;
                        }
                        Ok(())
                    })
                })),
                ..TestProviderOptions::default()
            },
        ),
        Arc::clone(&credentials) as Arc<dyn CredentialStore>,
    )
    .await;
    *block.lock().expect("block") = true;

    let signal = CancellationToken::new();
    let wait_started = started.notified();
    let login_runtime = Arc::clone(&runtime);
    let login_signal = signal.clone();
    let login = tokio::spawn(async move {
        login_runtime
            .login(
                "cancelled-sync",
                AuthType::ApiKey,
                &SilentInteraction {
                    signal: Some(login_signal),
                },
            )
            .await
    });
    wait_started.await;
    signal.cancel();

    let error = login
        .await
        .expect("join")
        .expect_err("synchronization error");
    match error {
        ModelRuntimeError::Synchronization(error) => {
            assert_eq!(error.provider_id, "cancelled-sync");
            assert_eq!(error.operation, CredentialSynchronizationOperation::Login);
            assert_eq!(error.credential, Some(api_key("cancelled-sync-key")));
        }
        other => panic!("unexpected error: {other}"),
    }
    assert_eq!(
        credentials
            .read("cancelled-sync", None)
            .await
            .expect("read"),
        Some(api_key("cancelled-sync-key"))
    );
}

#[tokio::test]
async fn reports_committed_credentials_when_local_synchronization_fails() {
    let fail = Arc::new(Mutex::new(false));
    let fail_hook = Arc::clone(&fail);
    let credentials = in_memory();
    let runtime = runtime_with_provider(
        provider(
            "broken-sync",
            TestProviderOptions {
                refresh_models: Some(Arc::new(move |allow_network| {
                    let fail = Arc::clone(&fail_hook);
                    Box::pin(async move {
                        if !allow_network && *fail.lock().expect("fail") {
                            return Err("cache restore failed".to_owned());
                        }
                        Ok(())
                    })
                })),
                ..TestProviderOptions::default()
            },
        ),
        Arc::clone(&credentials) as Arc<dyn CredentialStore>,
    )
    .await;
    *fail.lock().expect("fail") = true;

    let error = runtime
        .login(
            "broken-sync",
            AuthType::ApiKey,
            &SilentInteraction { signal: None },
        )
        .await
        .expect_err("synchronization error");
    match error {
        ModelRuntimeError::Synchronization(error) => {
            assert_eq!(error.provider_id, "broken-sync");
            assert_eq!(error.operation, CredentialSynchronizationOperation::Login);
            assert_eq!(error.credential, Some(api_key("broken-sync-key")));
        }
        other => panic!("unexpected error: {other}"),
    }
    assert_eq!(
        credentials.read("broken-sync", None).await.expect("read"),
        Some(api_key("broken-sync-key"))
    );
}
