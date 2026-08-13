//! Legacy extension OAuth types.
//!
//! 1:1 port of `packages/ai/src/compat/extension-oauth-types.ts` (45 LOC). The callback
//! surface exists only for coding-agent extensions; the current flows use
//! [`crate::auth::types::AuthInteraction`].

use crate::auth::types::{AuthError, BoxFuture, OAuthCredential};
use tokio_util::sync::CancellationToken;

/// `OAuthPrompt { message, placeholder?, allowEmpty? }`
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OAuthPrompt {
    pub message: String,
    pub placeholder: Option<String>,
    pub allow_empty: Option<bool>,
}

/// `OAuthAuthInfo { url, instructions? }`
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OAuthAuthInfo {
    pub url: String,
    pub instructions: Option<String>,
}

/// `OAuthDeviceCodeInfo { userCode, verificationUri, intervalSeconds?, expiresInSeconds? }`
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OAuthDeviceCodeInfo {
    pub user_code: String,
    pub verification_uri: String,
    pub interval_seconds: Option<u64>,
    pub expires_in_seconds: Option<u64>,
}

/// `OAuthSelectOption { id, label }`
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OAuthSelectOption {
    pub id: String,
    pub label: String,
}

/// `OAuthSelectPrompt { message, options }`
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OAuthSelectPrompt {
    pub message: String,
    pub options: Vec<OAuthSelectOption>,
}

/// `OAuthLoginCallbacks` — the callback surface extensions implement.
pub trait OAuthLoginCallbacks: Send + Sync {
    fn on_auth(&self, info: OAuthAuthInfo);
    fn on_device_code(&self, info: OAuthDeviceCodeInfo);
    fn on_prompt<'a>(&'a self, prompt: OAuthPrompt) -> BoxFuture<'a, Result<String, AuthError>>;
    fn on_progress(&self, _message: String) {}
    /// `onManualCodeInput?()` — `None` means the extension does not offer it.
    fn on_manual_code_input(&self) -> Option<BoxFuture<'_, Result<String, AuthError>>> {
        None
    }
    /// Returns the selected option id, or `None` when the user cancels.
    fn on_select<'a>(
        &'a self,
        prompt: OAuthSelectPrompt,
    ) -> BoxFuture<'a, Result<Option<String>, AuthError>>;
    fn signal(&self) -> Option<CancellationToken> {
        None
    }
}

/// `export type { OAuthCredentials }` — the untagged token shape of an OAuth credential;
/// the canonical [`crate::auth::types::Credential`] adds the `type: "oauth"` tag.
pub type OAuthCredentials = OAuthCredential;
