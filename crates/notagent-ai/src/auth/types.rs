use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::types::{ProviderEnv, ProviderHeaders};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// `ModelAuth { apiKey?, headers?, baseUrl? }` — request auth for one model request.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModelAuth {
    pub api_key: Option<String>,
    pub headers: Option<ProviderHeaders>,
    pub base_url: Option<String>,
}

/// `ApiKeyCredential { type: "api_key", key?, env? }`
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ApiKeyCredential {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// Provider-scoped environment values such as Cloudflare account/gateway ids.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<ProviderEnv>,
}

/// `OAuthCredential { type: "oauth", refresh, access, expires, ... }`
/// (for example `accountId`); `extra` keeps them for a lossless round-trip.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OAuthCredential {
    pub refresh: String,
    pub access: String,
    /// Unix timestamp in milliseconds.
    pub expires: i64,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

/// `Credential = ApiKeyCredential | OAuthCredential`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Credential {
    ApiKey(ApiKeyCredential),
    /// `type: "oauth"` — `rename_all` would produce `o_auth`.
    #[serde(rename = "oauth")]
    OAuth(OAuthCredential),
}

impl Credential {
    /// `credential.type`
    pub fn credential_type(&self) -> AuthType {
        match self {
            Credential::ApiKey(_) => AuthType::ApiKey,
            Credential::OAuth(_) => AuthType::OAuth,
        }
    }

    pub fn as_api_key(&self) -> Option<&ApiKeyCredential> {
        match self {
            Credential::ApiKey(credential) => Some(credential),
            Credential::OAuth(_) => None,
        }
    }

    pub fn as_oauth(&self) -> Option<&OAuthCredential> {
        match self {
            Credential::OAuth(credential) => Some(credential),
            Credential::ApiKey(_) => None,
        }
    }
}

/// `CredentialInfo { providerId, type }`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialInfo {
    pub provider_id: String,
    pub credential_type: AuthType,
}

/// `AuthType = "api_key" | "oauth"`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthType {
    ApiKey,
    /// `"oauth"` — `rename_all` would produce `o_auth`.
    #[serde(rename = "oauth")]
    OAuth,
}

/// `AuthOperationOptions { signal? }`
#[derive(Debug, Clone, Default)]
pub struct AuthOperationOptions {
    pub signal: Option<CancellationToken>,
}

/// Storage failure of a [`CredentialStore`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct CredentialStoreError(pub String);

/// The mutation callback of `modify`. It may borrow from the caller's scope, which is
/// what lets OAuth refresh run inside the lock while holding a borrowed `OAuthAuth`.
pub type ModifyFn<'a> = Box<
    dyn FnOnce(
            Option<Credential>,
        ) -> BoxFuture<'a, Result<Option<Credential>, CredentialStoreError>>
        + Send
        + 'a,
>;

/// `CredentialStore` — app-owned credential storage keyed by `Provider.id`.
/// `modify` is the only write path, so every mutation is a serialized
/// read-modify-write; OAuth refresh runs inside it so concurrent requests cannot
/// double-refresh a rotated token.
pub trait CredentialStore: Send + Sync {
    fn read(
        &self,
        provider_id: &str,
        options: Option<AuthOperationOptions>,
    ) -> BoxFuture<'_, Result<Option<Credential>, CredentialStoreError>>;

    fn list(
        &self,
        options: Option<AuthOperationOptions>,
    ) -> BoxFuture<'_, Result<Vec<CredentialInfo>, CredentialStoreError>>;

    fn modify<'a>(
        &'a self,
        provider_id: &'a str,
        modify: ModifyFn<'a>,
        options: Option<AuthOperationOptions>,
    ) -> BoxFuture<'a, Result<Option<Credential>, CredentialStoreError>>;

    fn delete(
        &self,
        provider_id: &str,
        options: Option<AuthOperationOptions>,
    ) -> BoxFuture<'_, Result<(), CredentialStoreError>>;
}

/// `AuthContext` — environment access for auth resolution, injectable for tests.
pub trait AuthContext: Send + Sync {
    fn env(&self, name: &str) -> BoxFuture<'_, Option<String>>;
    /// Whether a file exists; supports a leading `~`.
    fn file_exists(&self, path: &str) -> BoxFuture<'_, bool>;
}

/// `AuthResult { auth, env?, source? }`
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AuthResult {
    pub auth: ModelAuth,
    pub env: Option<ProviderEnv>,
    /// Label for status UI: `"ANTHROPIC_API_KEY"`, `"OAuth"`, `"~/.aws/credentials"`.
    pub source: Option<String>,
}

/// `AuthCheck { source?, type }`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthCheck {
    pub source: Option<String>,
    pub check_type: AuthType,
}

/// `AuthPrompt` — prompt shown to the user during login.
#[derive(Debug, Clone)]
pub struct AuthPrompt {
    pub signal: Option<CancellationToken>,
    pub kind: AuthPromptKind,
}

/// One option of a `select` prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthPromptOption {
    pub id: String,
    pub label: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthPromptKind {
    Text {
        message: String,
        placeholder: Option<String>,
    },
    Secret {
        message: String,
        placeholder: Option<String>,
    },
    Select {
        message: String,
        options: Vec<AuthPromptOption>,
    },
    ManualCode {
        message: String,
        placeholder: Option<String>,
    },
}

/// `AuthInfoLink { url, label? }`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthInfoLink {
    pub url: String,
    pub label: Option<String>,
}

/// `AuthEvent`
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthEvent {
    Info {
        message: String,
        links: Vec<AuthInfoLink>,
    },
    AuthUrl {
        url: String,
        instructions: Option<String>,
    },
    DeviceCode {
        user_code: String,
        verification_uri: String,
        interval_seconds: Option<u64>,
        expires_in_seconds: Option<u64>,
    },
    Progress {
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct AuthError(pub String);

/// `AuthInteraction` — login callbacks serving api-key and OAuth flows.
pub trait AuthInteraction: Send + Sync {
    fn signal(&self) -> Option<CancellationToken>;
    /// Returns the entered/selected string; `select` returns the option id.
    fn prompt(&self, prompt: AuthPrompt) -> BoxFuture<'_, Result<String, AuthError>>;
    fn notify(&self, event: AuthEvent);
}

/// `ProviderAuthInteraction = AuthInteraction & { signal: AbortSignal }`
pub struct ProviderAuthInteraction<'a> {
    pub interaction: &'a dyn AuthInteraction,
    pub signal: CancellationToken,
}

impl ProviderAuthInteraction<'_> {
    pub fn prompt(&self, prompt: AuthPrompt) -> BoxFuture<'_, Result<String, AuthError>> {
        self.interaction.prompt(prompt)
    }

    pub fn notify(&self, event: AuthEvent) {
        self.interaction.notify(event);
    }
}

/// Input of [`ApiKeyAuth::resolve`] and [`ApiKeyAuth::check`].
pub struct ApiKeyAuthInput<'a> {
    pub ctx: &'a dyn AuthContext,
    pub credential: Option<ApiKeyCredential>,
    pub signal: CancellationToken,
}

/// `ApiKeyAuth` — stored key plus ambient sources (env vars, AWS profiles, ADC files).
pub trait ApiKeyAuth: Send + Sync {
    /// Display name, e.g. "Anthropic API key".
    fn name(&self) -> &str;

    /// Interactive setup; `None` means ambient-only.
    fn login<'a>(
        &'a self,
        _interaction: &'a ProviderAuthInteraction<'a>,
    ) -> Option<BoxFuture<'a, Result<ApiKeyCredential, AuthError>>> {
        None
    }

    /// Side-effect-free availability check; `None` means "check by resolving".
    fn check<'a>(
        &'a self,
        _input: ApiKeyAuthInput<'a>,
    ) -> Option<BoxFuture<'a, Result<Option<AuthCheck>, AuthError>>> {
        None
    }

    /// Resolve auth from the stored credential and/or ambient sources.
    fn resolve<'a>(
        &'a self,
        input: ApiKeyAuthInput<'a>,
    ) -> BoxFuture<'a, Result<Option<AuthResult>, AuthError>>;
}

/// `OAuthAuth` — the refresh/toAuth split lets `Models` own the locked refresh.
pub trait OAuthAuth: Send + Sync {
    /// Display name, e.g. "Anthropic (Claude Pro/Max)".
    fn name(&self) -> &str;

    /// Whether access through this method is backed by a provider subscription.
    fn is_subscription(&self) -> bool {
        false
    }

    /// Selector label for the OAuth login option.
    fn login_label(&self) -> Option<&str> {
        None
    }

    fn login<'a>(
        &'a self,
        interaction: &'a ProviderAuthInteraction<'a>,
    ) -> BoxFuture<'a, Result<OAuthCredential, AuthError>>;

    /// Exchange the refresh token; runs under the store lock.
    fn refresh<'a>(
        &'a self,
        credential: OAuthCredential,
        signal: CancellationToken,
    ) -> BoxFuture<'a, Result<OAuthCredential, AuthError>>;

    /// Side-effect-free derivation of request auth from a valid credential.
    fn to_auth<'a>(
        &'a self,
        credential: OAuthCredential,
    ) -> BoxFuture<'a, Result<ModelAuth, AuthError>>;
}

/// `ProviderAuth { apiKey?, oauth? }` — at least one must be present.
#[derive(Clone, Default)]
pub struct ProviderAuth {
    pub api_key: Option<Arc<dyn ApiKeyAuth>>,
    pub oauth: Option<Arc<dyn OAuthAuth>>,
}

impl std::fmt::Debug for ProviderAuth {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProviderAuth")
            .field("api_key", &self.api_key.as_ref().map(|auth| auth.name()))
            .field("oauth", &self.oauth.as_ref().map(|auth| auth.name()))
            .finish()
    }
}

/// Helper for building a `ProviderEnv` from pairs.
pub fn provider_env(pairs: impl IntoIterator<Item = (String, String)>) -> ProviderEnv {
    pairs.into_iter().collect::<BTreeMap<_, _>>()
}
