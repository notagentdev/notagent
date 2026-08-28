use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use notagent_ai::auth::resolve::{AuthProvider, now_ms};
use notagent_ai::auth::types::*;
use notagent_ai::auth::{
    AuthResolutionOverrides, InMemoryCredentialStore, ModelsErrorCode, resolve_provider_auth,
};
use notagent_ai::env_api_keys::{find_env_keys, get_env_api_key};
use notagent_ai::types::ProviderEnv;

fn env(pairs: &[(&str, &str)]) -> ProviderEnv {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect::<BTreeMap<_, _>>()
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------

#[test]
fn does_not_treat_generic_github_tokens_as_copilot_credentials() {
    let scoped = env(&[("GH_TOKEN", "gh-token"), ("GITHUB_TOKEN", "github-token")]);
    assert_eq!(find_env_keys("github-copilot", Some(&scoped)), None);
    assert_eq!(get_env_api_key("github-copilot", Some(&scoped)), None);
}

#[test]
fn resolves_github_copilot_credentials_from_copilot_github_token() {
    let scoped = env(&[
        ("COPILOT_GITHUB_TOKEN", "copilot-token"),
        ("GH_TOKEN", "gh-token"),
        ("GITHUB_TOKEN", "github-token"),
    ]);
    assert_eq!(
        find_env_keys("github-copilot", Some(&scoped)),
        Some(vec!["COPILOT_GITHUB_TOKEN".to_string()])
    );
    assert_eq!(
        get_env_api_key("github-copilot", Some(&scoped)),
        Some("copilot-token".to_string())
    );
}

#[test]
fn resolves_zai_china_coding_plan_credentials() {
    let scoped = env(&[("ZAI_CODING_CN_API_KEY", "zai-coding-cn-token")]);
    assert_eq!(
        find_env_keys("zai-coding-cn", Some(&scoped)),
        Some(vec!["ZAI_CODING_CN_API_KEY".to_string()])
    );
    assert_eq!(
        get_env_api_key("zai-coding-cn", Some(&scoped)),
        Some("zai-coding-cn-token".to_string())
    );
}

#[test]
fn reports_anthropic_auth_token_but_prefers_the_oauth_token_for_api_key_lookup() {
    let scoped = env(&[
        ("ANTHROPIC_AUTH_TOKEN", "auth-token"),
        ("ANTHROPIC_OAUTH_TOKEN", "oauth-token"),
        ("ANTHROPIC_API_KEY", "api-key"),
    ]);
    assert_eq!(
        find_env_keys("anthropic", Some(&scoped)),
        Some(vec![
            "ANTHROPIC_AUTH_TOKEN".to_string(),
            "ANTHROPIC_OAUTH_TOKEN".to_string(),
            "ANTHROPIC_API_KEY".to_string(),
        ])
    );
    assert_eq!(
        get_env_api_key("anthropic", Some(&scoped)),
        Some("oauth-token".to_string())
    );
}

#[test]
fn does_not_return_the_anthropic_auth_token_as_an_api_key() {
    let scoped = env(&[("ANTHROPIC_AUTH_TOKEN", "auth-token")]);
    assert_eq!(
        find_env_keys("anthropic", Some(&scoped)),
        Some(vec!["ANTHROPIC_AUTH_TOKEN".to_string()])
    );
    assert_eq!(get_env_api_key("anthropic", Some(&scoped)), None);
}

#[test]
fn preserves_the_anthropic_oauth_token_as_an_api_key() {
    let scoped = env(&[("ANTHROPIC_OAUTH_TOKEN", "oauth-token")]);
    assert_eq!(
        find_env_keys("anthropic", Some(&scoped)),
        Some(vec!["ANTHROPIC_OAUTH_TOKEN".to_string()])
    );
    assert_eq!(
        get_env_api_key("anthropic", Some(&scoped)),
        Some("oauth-token".to_string())
    );
}

#[test]
fn falls_back_to_the_anthropic_api_key() {
    let scoped = env(&[("ANTHROPIC_API_KEY", "api-key")]);
    assert_eq!(
        get_env_api_key("anthropic", Some(&scoped)),
        Some("api-key".to_string())
    );
}

#[test]
fn reports_bedrock_ambient_credential_chain() {
    for pairs in [
        vec![("AWS_PROFILE", "default")],
        vec![
            ("AWS_ACCESS_KEY_ID", "id"),
            ("AWS_SECRET_ACCESS_KEY", "secret"),
        ],
        vec![("AWS_BEARER_TOKEN_BEDROCK", "token")],
        vec![("AWS_CONTAINER_CREDENTIALS_RELATIVE_URI", "/uri")],
        vec![("AWS_CONTAINER_CREDENTIALS_FULL_URI", "https://uri")],
        vec![("AWS_WEB_IDENTITY_TOKEN_FILE", "/file")],
    ] {
        assert_eq!(
            get_env_api_key("amazon-bedrock", Some(&env(&pairs))),
            Some("<authenticated>".to_string())
        );
    }
    // A lone access key id is not enough.
    assert_eq!(
        get_env_api_key("amazon-bedrock", Some(&env(&[("AWS_ACCESS_KEY_ID", "id")]))),
        None
    );
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------

struct TestAuthContext {
    env: ProviderEnv,
}

impl AuthContext for TestAuthContext {
    fn env(&self, name: &str) -> BoxFuture<'_, Option<String>> {
        let value = self.env.get(name).cloned();
        Box::pin(async move { value })
    }

    fn file_exists(&self, _path: &str) -> BoxFuture<'_, bool> {
        Box::pin(async { false })
    }
}

/// `envApiKeyAuth`-like stub whose resolutions are observable.
struct StubApiKeyAuth {
    env_var: String,
    calls: Arc<AtomicUsize>,
}

impl ApiKeyAuth for StubApiKeyAuth {
    fn name(&self) -> &str {
        "stub api key"
    }

    fn resolve<'a>(
        &'a self,
        input: ApiKeyAuthInput<'a>,
    ) -> BoxFuture<'a, Result<Option<AuthResult>, AuthError>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if let Some(key) = input
                .credential
                .as_ref()
                .and_then(|credential| credential.key.clone())
            {
                return Ok(Some(AuthResult {
                    auth: ModelAuth {
                        api_key: Some(key),
                        ..Default::default()
                    },
                    env: input.credential.and_then(|credential| credential.env),
                    source: Some("stored credential".to_string()),
                }));
            }
            match input.ctx.env(&self.env_var).await {
                Some(value) => Ok(Some(AuthResult {
                    auth: ModelAuth {
                        api_key: Some(value),
                        ..Default::default()
                    },
                    env: None,
                    source: Some(self.env_var.clone()),
                })),
                None => Ok(None),
            }
        })
    }
}

struct StubOAuth {
    refreshes: Arc<AtomicUsize>,
    fail: bool,
    delay_ms: u64,
}

impl OAuthAuth for StubOAuth {
    fn name(&self) -> &str {
        "stub oauth"
    }

    fn login<'a>(
        &'a self,
        _interaction: &'a ProviderAuthInteraction<'a>,
    ) -> BoxFuture<'a, Result<OAuthCredential, AuthError>> {
        Box::pin(async { Err(AuthError("not implemented".to_string())) })
    }

    fn refresh<'a>(
        &'a self,
        credential: OAuthCredential,
        _signal: tokio_util::sync::CancellationToken,
    ) -> BoxFuture<'a, Result<OAuthCredential, AuthError>> {
        Box::pin(async move {
            if self.delay_ms > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(self.delay_ms)).await;
            }
            self.refreshes.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                return Err(AuthError("invalid_grant".to_string()));
            }
            Ok(OAuthCredential {
                access: "refreshed-access".to_string(),
                refresh: "refreshed-refresh".to_string(),
                expires: now_ms() + 3_600_000,
                extra: credential.extra,
            })
        })
    }

    fn to_auth<'a>(
        &'a self,
        credential: OAuthCredential,
    ) -> BoxFuture<'a, Result<ModelAuth, AuthError>> {
        Box::pin(async move {
            Ok(ModelAuth {
                api_key: Some(credential.access),
                ..Default::default()
            })
        })
    }
}

fn oauth_credential(expires_in_ms: i64) -> Credential {
    Credential::OAuth(OAuthCredential {
        access: "stored-access".to_string(),
        refresh: "stored-refresh".to_string(),
        expires: now_ms() + expires_in_ms,
        extra: Default::default(),
    })
}

async fn store_with(credential: Credential) -> InMemoryCredentialStore {
    let store = InMemoryCredentialStore::new();
    store
        .modify(
            "p",
            Box::new(move |_| Box::pin(async move { Ok(Some(credential)) })),
            None,
        )
        .await
        .expect("seed credential");
    store
}

#[tokio::test]
async fn a_stored_api_key_credential_suppresses_the_env_fallback() {
    let calls = Arc::new(AtomicUsize::new(0));
    let auth = ProviderAuth {
        api_key: Some(Arc::new(StubApiKeyAuth {
            env_var: "STUB_KEY".to_string(),
            calls: Arc::clone(&calls),
        })),
        oauth: None,
    };
    let store = store_with(Credential::ApiKey(ApiKeyCredential {
        key: Some("stored-key".to_string()),
        env: None,
    }))
    .await;
    let context: Arc<dyn AuthContext> = Arc::new(TestAuthContext {
        env: env(&[("STUB_KEY", "env-key")]),
    });

    let result = resolve_provider_auth(
        AuthProvider {
            id: "p",
            auth: &auth,
        },
        &store,
        context,
        None,
    )
    .await
    .expect("resolution")
    .expect("configured");
    assert_eq!(result.auth.api_key, Some("stored-key".to_string()));
    assert_eq!(result.source, Some("stored credential".to_string()));
}

#[tokio::test]
async fn ambient_env_is_used_when_nothing_is_stored() {
    let calls = Arc::new(AtomicUsize::new(0));
    let auth = ProviderAuth {
        api_key: Some(Arc::new(StubApiKeyAuth {
            env_var: "STUB_KEY".to_string(),
            calls: Arc::clone(&calls),
        })),
        oauth: None,
    };
    let store = InMemoryCredentialStore::new();
    let context: Arc<dyn AuthContext> = Arc::new(TestAuthContext {
        env: env(&[("STUB_KEY", "env-key")]),
    });

    let result = resolve_provider_auth(
        AuthProvider {
            id: "p",
            auth: &auth,
        },
        &store,
        context,
        None,
    )
    .await
    .expect("resolution")
    .expect("configured");
    assert_eq!(result.auth.api_key, Some("env-key".to_string()));
    assert_eq!(result.source, Some("STUB_KEY".to_string()));
}

#[tokio::test]
async fn a_stored_oauth_credential_without_oauth_support_is_not_configured() {
    let calls = Arc::new(AtomicUsize::new(0));
    let auth = ProviderAuth {
        api_key: Some(Arc::new(StubApiKeyAuth {
            env_var: "STUB_KEY".to_string(),
            calls: Arc::clone(&calls),
        })),
        oauth: None,
    };
    let store = store_with(oauth_credential(3_600_000)).await;
    let context: Arc<dyn AuthContext> = Arc::new(TestAuthContext {
        env: env(&[("STUB_KEY", "env-key")]),
    });

    // No silent env fallback for a credential type without a matching handler.
    let result = resolve_provider_auth(
        AuthProvider {
            id: "p",
            auth: &auth,
        },
        &store,
        context,
        None,
    )
    .await
    .expect("resolution");
    assert!(result.is_none());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_valid_oauth_credential_is_used_without_refreshing() {
    let refreshes = Arc::new(AtomicUsize::new(0));
    let auth = ProviderAuth {
        api_key: None,
        oauth: Some(Arc::new(StubOAuth {
            refreshes: Arc::clone(&refreshes),
            fail: false,
            delay_ms: 0,
        })),
    };
    let store = store_with(oauth_credential(3_600_000)).await;
    let context: Arc<dyn AuthContext> = Arc::new(TestAuthContext {
        env: ProviderEnv::new(),
    });

    let result = resolve_provider_auth(
        AuthProvider {
            id: "p",
            auth: &auth,
        },
        &store,
        context,
        None,
    )
    .await
    .expect("resolution")
    .expect("configured");
    assert_eq!(result.auth.api_key, Some("stored-access".to_string()));
    assert_eq!(result.source, Some("OAuth".to_string()));
    assert_eq!(refreshes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn an_expiring_oauth_credential_is_refreshed_and_persisted_before_release() {
    let refreshes = Arc::new(AtomicUsize::new(0));
    let auth = ProviderAuth {
        api_key: None,
        oauth: Some(Arc::new(StubOAuth {
            refreshes: Arc::clone(&refreshes),
            fail: false,
            delay_ms: 0,
        })),
    };
    // Less than the five-minute minimum validity remaining.
    let store = store_with(oauth_credential(60_000)).await;
    let context: Arc<dyn AuthContext> = Arc::new(TestAuthContext {
        env: ProviderEnv::new(),
    });

    let result = resolve_provider_auth(
        AuthProvider {
            id: "p",
            auth: &auth,
        },
        &store,
        Arc::clone(&context),
        None,
    )
    .await
    .expect("resolution")
    .expect("configured");
    assert_eq!(result.auth.api_key, Some("refreshed-access".to_string()));
    assert_eq!(refreshes.load(Ordering::SeqCst), 1);

    // The rotated credential was persisted before the lock was released.
    let stored = store.read("p", None).await.expect("read").expect("stored");
    assert_eq!(
        stored
            .as_oauth()
            .map(|credential| credential.access.as_str()),
        Some("refreshed-access")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_resolutions_refresh_an_expired_token_only_once() {
    let refreshes = Arc::new(AtomicUsize::new(0));
    let oauth: Arc<dyn OAuthAuth> = Arc::new(StubOAuth {
        refreshes: Arc::clone(&refreshes),
        fail: false,
        delay_ms: 25,
    });
    let auth = ProviderAuth {
        api_key: None,
        oauth: Some(Arc::clone(&oauth)),
    };
    let store = Arc::new(store_with(oauth_credential(60_000)).await);
    let context: Arc<dyn AuthContext> = Arc::new(TestAuthContext {
        env: ProviderEnv::new(),
    });

    // Double-checked locking: the second resolution re-checks expiry under the lock and
    // reuses the credential the first one rotated.
    let mut results = Vec::new();
    for _ in 0..4 {
        let auth = auth.clone();
        let store = Arc::clone(&store);
        let context = Arc::clone(&context);
        results.push(tokio::spawn(async move {
            resolve_provider_auth(
                AuthProvider {
                    id: "p",
                    auth: &auth,
                },
                store.as_ref(),
                context,
                None,
            )
            .await
        }));
    }
    for handle in results {
        let result = handle
            .await
            .expect("task")
            .expect("resolution")
            .expect("configured");
        assert_eq!(result.auth.api_key, Some("refreshed-access".to_string()));
    }
    assert_eq!(
        refreshes.load(Ordering::SeqCst),
        1,
        "the token must be refreshed exactly once"
    );
}

#[tokio::test]
async fn a_failed_refresh_preserves_the_stored_credential_and_reports_an_oauth_error() {
    let refreshes = Arc::new(AtomicUsize::new(0));
    let auth = ProviderAuth {
        api_key: None,
        oauth: Some(Arc::new(StubOAuth {
            refreshes: Arc::clone(&refreshes),
            fail: true,
            delay_ms: 0,
        })),
    };
    let store = store_with(oauth_credential(60_000)).await;
    let context: Arc<dyn AuthContext> = Arc::new(TestAuthContext {
        env: ProviderEnv::new(),
    });

    let error = resolve_provider_auth(
        AuthProvider {
            id: "p",
            auth: &auth,
        },
        &store,
        context,
        None,
    )
    .await
    .expect_err("must fail");
    assert_eq!(error.code, ModelsErrorCode::OAuth);
    assert!(
        error.to_string().contains("OAuth refresh failed for p"),
        "{error}"
    );
    assert!(error.to_string().contains("invalid_grant"), "{error}");

    // The stored credential survives so a later retry (or re-login) can fix it.
    let stored = store.read("p", None).await.expect("read").expect("stored");
    assert_eq!(
        stored
            .as_oauth()
            .map(|credential| credential.access.as_str()),
        Some("stored-access")
    );
}

#[tokio::test]
async fn an_api_key_override_bypasses_the_stored_credential() {
    let calls = Arc::new(AtomicUsize::new(0));
    let auth = ProviderAuth {
        api_key: Some(Arc::new(StubApiKeyAuth {
            env_var: "STUB_KEY".to_string(),
            calls: Arc::clone(&calls),
        })),
        oauth: None,
    };
    let store = store_with(Credential::ApiKey(ApiKeyCredential {
        key: Some("stored-key".to_string()),
        env: None,
    }))
    .await;
    let context: Arc<dyn AuthContext> = Arc::new(TestAuthContext {
        env: ProviderEnv::new(),
    });

    let overrides = AuthResolutionOverrides {
        api_key: Some("override-key".to_string()),
        ..Default::default()
    };
    let result = resolve_provider_auth(
        AuthProvider {
            id: "p",
            auth: &auth,
        },
        &store,
        context,
        Some(&overrides),
    )
    .await
    .expect("resolution")
    .expect("configured");
    assert_eq!(result.auth.api_key, Some("override-key".to_string()));
}

#[tokio::test]
async fn overridden_env_values_take_precedence_in_resolution() {
    let calls = Arc::new(AtomicUsize::new(0));
    let auth = ProviderAuth {
        api_key: Some(Arc::new(StubApiKeyAuth {
            env_var: "STUB_KEY".to_string(),
            calls: Arc::clone(&calls),
        })),
        oauth: None,
    };
    let store = InMemoryCredentialStore::new();
    let context: Arc<dyn AuthContext> = Arc::new(TestAuthContext {
        env: env(&[("STUB_KEY", "ambient")]),
    });

    let overrides = AuthResolutionOverrides {
        env: Some(env(&[("STUB_KEY", "scoped")])),
        ..Default::default()
    };
    let result = resolve_provider_auth(
        AuthProvider {
            id: "p",
            auth: &auth,
        },
        &store,
        context,
        Some(&overrides),
    )
    .await
    .expect("resolution")
    .expect("configured");
    assert_eq!(result.auth.api_key, Some("scoped".to_string()));
}

#[tokio::test]
async fn the_credential_store_serializes_modifications_per_provider() {
    let store = Arc::new(InMemoryCredentialStore::new());
    let observed = Arc::new(std::sync::Mutex::new(Vec::<Option<String>>::new()));

    let mut handles = Vec::new();
    for index in 0..5u32 {
        let store = Arc::clone(&store);
        let observed = Arc::clone(&observed);
        handles.push(tokio::spawn(async move {
            store
                .modify(
                    "p",
                    Box::new(move |current| {
                        Box::pin(async move {
                            observed
                                .lock()
                                .expect("observed poisoned")
                                .push(current.and_then(|credential| {
                                    credential
                                        .as_api_key()
                                        .and_then(|credential| credential.key.clone())
                                }));
                            tokio::task::yield_now().await;
                            Ok(Some(Credential::ApiKey(ApiKeyCredential {
                                key: Some(format!("key-{index}")),
                                env: None,
                            })))
                        })
                    }),
                    None,
                )
                .await
        }));
    }
    for handle in handles {
        handle.await.expect("task").expect("modify");
    }

    // Every callback saw the value written by the previous one: no interleaving.
    let observed = observed.lock().expect("observed poisoned").clone();
    assert_eq!(observed.len(), 5);
    assert_eq!(observed[0], None);
    for entry in observed.iter().skip(1) {
        assert!(
            entry.as_ref().is_some_and(|key| key.starts_with("key-")),
            "{entry:?}"
        );
    }
}
