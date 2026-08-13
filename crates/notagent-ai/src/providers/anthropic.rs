//! `anthropicProvider()`.
//!
//! 1:1 port of `packages/ai/src/providers/anthropic.ts` (59 LOC). Anthropic resolves
//! non-standard: an `ANTHROPIC_AUTH_TOKEN` becomes a bearer header, and only then do the
//! OAuth token and the API key env vars apply.

use std::sync::Arc;

use crate::api::streams::AnthropicMessagesApi;
use crate::auth::types::{
    ApiKeyAuth, ApiKeyAuthInput, ApiKeyCredential, AuthError, AuthPrompt, AuthPromptKind,
    AuthResult, BoxFuture, ModelAuth, ProviderAuth, ProviderAuthInteraction,
};
use crate::env_api_keys::{
    ANTHROPIC_API_KEY_ENV, ANTHROPIC_AUTH_TOKEN_ENV, ANTHROPIC_OAUTH_TOKEN_ENV,
};
use crate::model_catalog::get_builtin_models;
use crate::models::{BuiltProvider, CreateProviderOptions, ProviderApis, create_provider};
use crate::types::ProviderHeaders;

/// `anthropicApiKeyAuth()`
pub struct AnthropicApiKeyAuth;

impl ApiKeyAuth for AnthropicApiKeyAuth {
    fn name(&self) -> &str {
        "Anthropic API key"
    }

    fn login<'a>(
        &'a self,
        interaction: &'a ProviderAuthInteraction<'a>,
    ) -> Option<BoxFuture<'a, Result<ApiKeyCredential, AuthError>>> {
        Some(Box::pin(async move {
            if interaction.signal.is_cancelled() {
                return Err(AuthError("The operation was aborted".to_string()));
            }
            let key = interaction
                .prompt(AuthPrompt {
                    signal: None,
                    kind: AuthPromptKind::Secret {
                        message: "Enter Anthropic API key".to_string(),
                        placeholder: None,
                    },
                })
                .await?;
            if interaction.signal.is_cancelled() {
                return Err(AuthError("The operation was aborted".to_string()));
            }
            Ok(ApiKeyCredential {
                key: Some(key),
                env: None,
            })
        }))
    }

    fn resolve<'a>(
        &'a self,
        input: ApiKeyAuthInput<'a>,
    ) -> BoxFuture<'a, Result<Option<AuthResult>, AuthError>> {
        Box::pin(async move {
            if input.signal.is_cancelled() {
                return Err(AuthError("The operation was aborted".to_string()));
            }
            if let Some(credential) = &input.credential
                && let Some(key) = credential.key.as_ref().filter(|key| !key.is_empty())
            {
                return Ok(Some(AuthResult {
                    auth: ModelAuth {
                        api_key: Some(key.clone()),
                        ..Default::default()
                    },
                    env: credential.env.clone(),
                    source: Some("stored credential".to_string()),
                }));
            }

            let auth_token = input.ctx.env(ANTHROPIC_AUTH_TOKEN_ENV).await;
            if input.signal.is_cancelled() {
                return Err(AuthError("The operation was aborted".to_string()));
            }
            if let Some(auth_token) = auth_token.filter(|token| !token.is_empty()) {
                let mut headers = ProviderHeaders::new();
                headers.insert(
                    "Authorization".to_string(),
                    Some(format!("Bearer {auth_token}")),
                );
                return Ok(Some(AuthResult {
                    auth: ModelAuth {
                        headers: Some(headers),
                        ..Default::default()
                    },
                    env: None,
                    source: Some(ANTHROPIC_AUTH_TOKEN_ENV.to_string()),
                }));
            }

            for env_var in [ANTHROPIC_OAUTH_TOKEN_ENV, ANTHROPIC_API_KEY_ENV] {
                let api_key = input.ctx.env(env_var).await;
                if input.signal.is_cancelled() {
                    return Err(AuthError("The operation was aborted".to_string()));
                }
                if let Some(api_key) = api_key.filter(|key| !key.is_empty()) {
                    return Ok(Some(AuthResult {
                        auth: ModelAuth {
                            api_key: Some(api_key),
                            ..Default::default()
                        },
                        env: None,
                        source: Some(env_var.to_string()),
                    }));
                }
            }
            Ok(None)
        })
    }
}

/// `anthropicProvider()`
pub fn anthropic_provider() -> Arc<BuiltProvider> {
    create_provider(CreateProviderOptions {
        id: "anthropic".to_string(),
        name: Some("Anthropic".to_string()),
        base_url: Some("https://api.anthropic.com".to_string()),
        headers: None,
        auth: ProviderAuth {
            api_key: Some(Arc::new(AnthropicApiKeyAuth)),
            oauth: Some(crate::auth::oauth::anthropic::anthropic_oauth()),
        },
        models: get_builtin_models("anthropic"),
        fetch_models: None,
        filter_models: None,
        api: ProviderApis::Single(Arc::new(AnthropicMessagesApi)),
    })
}
