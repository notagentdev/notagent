use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::auth::types::{
    ApiKeyAuth, ApiKeyAuthInput, ApiKeyCredential, AuthContext, AuthResult, BoxFuture, Credential,
    CredentialStore, OAuthAuth, OAuthCredential, ProviderAuth,
};
use crate::types::ProviderEnv;

/// `ModelsErrorCode`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelsErrorCode {
    ModelSource,
    ModelValidation,
    Provider,
    Stream,
    Auth,
    OAuth,
}

impl ModelsErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            ModelsErrorCode::ModelSource => "model_source",
            ModelsErrorCode::ModelValidation => "model_validation",
            ModelsErrorCode::Provider => "provider",
            ModelsErrorCode::Stream => "stream",
            ModelsErrorCode::Auth => "auth",
            ModelsErrorCode::OAuth => "oauth",
        }
    }
}

/// `ModelsError` — callers surface `error.message` only, so the cause is folded in.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct ModelsError {
    pub code: ModelsErrorCode,
    pub message: String,
}

impl ModelsError {
    pub fn new(code: ModelsErrorCode, message: impl Into<String>) -> Self {
        ModelsError {
            code,
            message: message.into(),
        }
    }

    /// `withCauseDetail(message, cause)`
    pub fn with_cause(
        code: ModelsErrorCode,
        message: impl Into<String>,
        cause: &dyn std::fmt::Display,
    ) -> Self {
        let message = message.into();
        let detail = cause.to_string().trim().to_string();
        if detail.is_empty() || message.contains(&detail) {
            return ModelsError { code, message };
        }
        ModelsError {
            code,
            message: format!("{message}: {detail}"),
        }
    }
}

/// `AuthResolutionOverrides`
#[derive(Clone, Default)]
pub struct AuthResolutionOverrides {
    pub api_key: Option<String>,
    pub env: Option<ProviderEnv>,
    /// Required remaining OAuth-token validity; defaults to five minutes.
    pub min_oauth_validity_ms: Option<i64>,
    pub signal: Option<CancellationToken>,
}

const DEFAULT_OAUTH_MINIMUM_VALIDITY_MS: i64 = 5 * 60 * 1000;
const DEFAULT_OAUTH_REFRESH_TIMEOUT_MS: u64 = 15_000;

/// Provider identity as far as auth resolution is concerned.
pub struct AuthProvider<'a> {
    pub id: &'a str,
    pub auth: &'a ProviderAuth,
}

/// Wraps an [`AuthContext`] so overridden env values take precedence.
struct EnvOverlayAuthContext {
    base: Arc<dyn AuthContext>,
    env: ProviderEnv,
}

impl AuthContext for EnvOverlayAuthContext {
    fn env(&self, name: &str) -> BoxFuture<'_, Option<String>> {
        let overridden = self
            .env
            .get(name)
            .filter(|value| !value.is_empty())
            .cloned();
        let name = name.to_string();
        Box::pin(async move {
            match overridden {
                Some(value) => Some(value),
                None => self.base.env(&name).await,
            }
        })
    }

    fn file_exists(&self, path: &str) -> BoxFuture<'_, bool> {
        self.base.file_exists(path)
    }
}

/// `resolveProviderAuth(provider, credentials, authContext, overrides?)`
pub async fn resolve_provider_auth(
    provider: AuthProvider<'_>,
    credentials: &dyn CredentialStore,
    auth_context: Arc<dyn AuthContext>,
    overrides: Option<&AuthResolutionOverrides>,
) -> Result<Option<AuthResult>, ModelsError> {
    let signal = overrides
        .and_then(|overrides| overrides.signal.clone())
        .unwrap_or_default();
    if signal.is_cancelled() {
        return Err(ModelsError::new(
            ModelsErrorCode::Auth,
            "The operation was aborted",
        ));
    }

    let request_auth_context: Arc<dyn AuthContext> =
        match overrides.and_then(|overrides| overrides.env.clone()) {
            Some(env) => Arc::new(EnvOverlayAuthContext {
                base: Arc::clone(&auth_context),
                env,
            }),
            None => Arc::clone(&auth_context),
        };

    if let Some(api_key_override) = overrides.and_then(|overrides| overrides.api_key.clone())
        && let Some(api_key) = &provider.auth.api_key
    {
        let credential = ApiKeyCredential {
            key: Some(api_key_override),
            env: overrides.and_then(|overrides| overrides.env.clone()),
        };
        return resolve_api_key(
            request_auth_context.as_ref(),
            api_key.as_ref(),
            provider.id,
            Some(credential),
            signal,
        )
        .await;
    }

    let stored = read_credential(credentials, provider.id, &signal).await?;
    if let Some(stored) = stored {
        match stored {
            Credential::OAuth(stored) => {
                let Some(oauth) = &provider.auth.oauth else {
                    return Ok(None);
                };
                return resolve_stored_oauth(
                    credentials,
                    provider.id,
                    oauth.as_ref(),
                    stored,
                    signal,
                    overrides.and_then(|overrides| overrides.min_oauth_validity_ms),
                )
                .await;
            }
            Credential::ApiKey(stored) => {
                let Some(api_key) = &provider.auth.api_key else {
                    return Ok(None);
                };
                let credential = match overrides.and_then(|overrides| overrides.env.clone()) {
                    Some(env) => {
                        let mut merged = stored.env.clone().unwrap_or_default();
                        merged.extend(env);
                        ApiKeyCredential {
                            env: Some(merged),
                            ..stored
                        }
                    }
                    None => stored,
                };
                return resolve_api_key(
                    request_auth_context.as_ref(),
                    api_key.as_ref(),
                    provider.id,
                    Some(credential),
                    signal,
                )
                .await;
            }
        }
    }

    // Ambient sources (env vars, AWS profiles, ADC files).
    match &provider.auth.api_key {
        Some(api_key) => {
            resolve_api_key(
                request_auth_context.as_ref(),
                api_key.as_ref(),
                provider.id,
                None,
                signal,
            )
            .await
        }
        None => Ok(None),
    }
}

/// OAuth resolution with double-checked locking.
async fn resolve_stored_oauth(
    credentials: &dyn CredentialStore,
    provider_id: &str,
    oauth: &dyn OAuthAuth,
    stored: OAuthCredential,
    signal: CancellationToken,
    min_oauth_validity_ms: Option<i64>,
) -> Result<Option<AuthResult>, ModelsError> {
    let minimum_validity_ms =
        DEFAULT_OAUTH_MINIMUM_VALIDITY_MS.max(min_oauth_validity_ms.unwrap_or(0));
    let expires_soon =
        |credential: &OAuthCredential| now_ms() + minimum_validity_ms >= credential.expires;
    let mut credential = stored;

    if expires_soon(&credential) {
        // The optimistic check said expired; the authoritative check runs under the lock.
        let refresh_error: Arc<std::sync::Mutex<Option<ModelsError>>> =
            Arc::new(std::sync::Mutex::new(None));
        let refresh_error_sink = Arc::clone(&refresh_error);
        let refresh_signal = signal.clone();
        let refresh_provider_id = provider_id.to_string();

        let post = credentials
            .modify(
                provider_id,
                Box::new(move |current| {
                    Box::pin(async move {
                        let Some(Credential::OAuth(current)) = current else {
                            return Ok(None);
                        }; // logged out meanwhile
                        if now_ms() + minimum_validity_ms < current.expires {
                            return Ok(None); // another process/request refreshed
                        }
                        match refresh_oauth_credential(
                            oauth,
                            current,
                            refresh_signal,
                            &refresh_provider_id,
                        )
                        .await
                        {
                            Ok(credential) => Ok(Some(Credential::OAuth(credential))),
                            Err(error) => {
                                *refresh_error_sink
                                    .lock()
                                    .expect("refresh error slot poisoned") = Some(error);
                                // Abort the mutation; the stored credential is preserved for retry.
                                Err(crate::auth::types::CredentialStoreError(
                                    "refresh failed".to_string(),
                                ))
                            }
                        }
                    })
                }),
                Some(crate::auth::types::AuthOperationOptions {
                    signal: Some(signal.clone()),
                }),
            )
            .await;

        if let Some(error) = refresh_error
            .lock()
            .expect("refresh error slot poisoned")
            .take()
        {
            return Err(error);
        }
        let post = post.map_err(|error| {
            ModelsError::with_cause(
                ModelsErrorCode::Auth,
                format!("Credential store modify failed for {provider_id}"),
                &error,
            )
        })?;
        let Some(Credential::OAuth(post)) = post else {
            return Ok(None);
        }; // logged out meanwhile
        credential = post;
        // The five-minute window triggers a refresh but imposes no provider contract;
        // explicit callers do require their requested minimum afterwards.
        if min_oauth_validity_ms.is_some() && expires_soon(&credential) {
            return Err(ModelsError::new(
                ModelsErrorCode::OAuth,
                format!("OAuth refresh returned a token that expires too soon for {provider_id}"),
            ));
        }
    }

    match oauth.to_auth(credential).await {
        Ok(auth) => Ok(Some(AuthResult {
            auth,
            env: None,
            source: Some("OAuth".to_string()),
        })),
        Err(error) => Err(ModelsError::with_cause(
            ModelsErrorCode::OAuth,
            format!("OAuth auth derivation failed for {provider_id}"),
            &error,
        )),
    }
}

/// `oauth.refresh(current, AbortSignal.any([signal, AbortSignal.timeout(15_000)]))`
async fn refresh_oauth_credential(
    oauth: &dyn OAuthAuth,
    credential: OAuthCredential,
    signal: CancellationToken,
    provider_id: &str,
) -> Result<OAuthCredential, ModelsError> {
    let refresh_signal = signal.child_token();
    let timeout_signal = refresh_signal.clone();
    let timeout = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(DEFAULT_OAUTH_REFRESH_TIMEOUT_MS)).await;
        timeout_signal.cancel();
    });
    let result = oauth.refresh(credential, refresh_signal).await;
    timeout.abort();
    result.map_err(|error| {
        ModelsError::with_cause(
            ModelsErrorCode::OAuth,
            format!("OAuth refresh failed for {provider_id}"),
            &error,
        )
    })
}

async fn resolve_api_key(
    auth_context: &dyn AuthContext,
    api_key: &dyn ApiKeyAuth,
    provider_id: &str,
    credential: Option<ApiKeyCredential>,
    signal: CancellationToken,
) -> Result<Option<AuthResult>, ModelsError> {
    api_key
        .resolve(ApiKeyAuthInput {
            ctx: auth_context,
            credential,
            signal,
        })
        .await
        .map_err(|error| {
            ModelsError::with_cause(
                ModelsErrorCode::Auth,
                format!("API key auth failed for provider {provider_id}"),
                &error,
            )
        })
}

async fn read_credential(
    credentials: &dyn CredentialStore,
    provider_id: &str,
    signal: &CancellationToken,
) -> Result<Option<Credential>, ModelsError> {
    credentials
        .read(
            provider_id,
            Some(crate::auth::types::AuthOperationOptions {
                signal: Some(signal.clone()),
            }),
        )
        .await
        .map_err(|error| {
            ModelsError::with_cause(
                ModelsErrorCode::Auth,
                format!("Credential store read failed for {provider_id}"),
                &error,
            )
        })
}

/// `Date.now()`
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time before the Unix epoch")
        .as_millis() as i64
}
