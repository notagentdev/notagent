//! Cloudflare api-key auth for Workers AI and the AI Gateway.
//!
//! 1:1 port of `packages/ai/src/providers/cloudflare-auth.ts` (103 LOC). Both flows
//! merge per field: a stored credential value wins, ambient env fills the rest, so a
//! credential carrying only the API key still picks up the account and gateway id.

use crate::auth::types::{
    ApiKeyAuth, ApiKeyAuthInput, ApiKeyCredential, AuthContext, AuthError, AuthPrompt,
    AuthPromptKind, AuthResult, BoxFuture, ModelAuth, ProviderAuthInteraction,
};
use crate::types::{ProviderEnv, ProviderHeaders};
use tokio_util::sync::CancellationToken;

const CLOUDFLARE_API_KEY: &str = "CLOUDFLARE_API_KEY";
const CLOUDFLARE_ACCOUNT_ID: &str = "CLOUDFLARE_ACCOUNT_ID";
const CLOUDFLARE_GATEWAY_ID: &str = "CLOUDFLARE_GATEWAY_ID";

/// `CloudflareAuthKind`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CloudflareAuthKind {
    WorkersAi,
    AiGateway,
}

/// `resolveValue(name, ctx, credential, signal)`
async fn resolve_value(
    name: &str,
    ctx: &dyn AuthContext,
    credential: Option<&ApiKeyCredential>,
    signal: &CancellationToken,
) -> Result<Option<String>, AuthError> {
    let from_credential = credential.and_then(|credential| {
        if name == CLOUDFLARE_API_KEY {
            credential.key.clone()
        } else {
            credential
                .env
                .as_ref()
                .and_then(|env| env.get(name).cloned())
        }
    });
    if let Some(value) = from_credential {
        return Ok(Some(value));
    }
    if signal.is_cancelled() {
        return Err(AuthError("The operation was aborted".to_string()));
    }
    let value = ctx.env(name).await;
    if signal.is_cancelled() {
        return Err(AuthError("The operation was aborted".to_string()));
    }
    Ok(value)
}

/// Result of `resolveCloudflareEnv`.
struct ResolvedCloudflareEnv {
    api_key: String,
    env: ProviderEnv,
    source: String,
}

/// `resolveCloudflareEnv(kind, ctx, credential, signal)`
async fn resolve_cloudflare_env(
    kind: CloudflareAuthKind,
    ctx: &dyn AuthContext,
    credential: Option<&ApiKeyCredential>,
    signal: &CancellationToken,
) -> Result<Option<ResolvedCloudflareEnv>, AuthError> {
    let api_key = resolve_value(CLOUDFLARE_API_KEY, ctx, credential, signal).await?;
    let account_id = resolve_value(CLOUDFLARE_ACCOUNT_ID, ctx, credential, signal).await?;
    let gateway_id = match kind {
        CloudflareAuthKind::AiGateway => {
            resolve_value(CLOUDFLARE_GATEWAY_ID, ctx, credential, signal).await?
        }
        CloudflareAuthKind::WorkersAi => None,
    };

    // Empty strings are falsy in TS, so they count as missing here too.
    let api_key = api_key.filter(|value| !value.is_empty());
    let account_id = account_id.filter(|value| !value.is_empty());
    let gateway_id = gateway_id.filter(|value| !value.is_empty());
    let (Some(api_key), Some(account_id)) = (api_key, account_id) else {
        return Ok(None);
    };
    if kind == CloudflareAuthKind::AiGateway && gateway_id.is_none() {
        return Ok(None);
    }

    let mut env = ProviderEnv::new();
    env.insert(CLOUDFLARE_ACCOUNT_ID.to_string(), account_id);
    if let Some(gateway_id) = gateway_id {
        env.insert(CLOUDFLARE_GATEWAY_ID.to_string(), gateway_id);
    }
    Ok(Some(ResolvedCloudflareEnv {
        api_key,
        env,
        source: if credential.is_some() {
            "stored credential".to_string()
        } else {
            CLOUDFLARE_API_KEY.to_string()
        },
    }))
}

async fn prompt_text(
    interaction: &ProviderAuthInteraction<'_>,
    message: &str,
) -> Result<String, AuthError> {
    interaction
        .prompt(AuthPrompt {
            signal: None,
            kind: AuthPromptKind::Text {
                message: message.to_string(),
                placeholder: None,
            },
        })
        .await
}

async fn prompt_secret(
    interaction: &ProviderAuthInteraction<'_>,
    message: &str,
) -> Result<String, AuthError> {
    interaction
        .prompt(AuthPrompt {
            signal: None,
            kind: AuthPromptKind::Secret {
                message: message.to_string(),
                placeholder: None,
            },
        })
        .await
}

/// `cloudflareWorkersAIAuth()`
pub struct CloudflareWorkersAiAuth;

impl ApiKeyAuth for CloudflareWorkersAiAuth {
    fn name(&self) -> &str {
        "Cloudflare API key"
    }

    fn login<'a>(
        &'a self,
        interaction: &'a ProviderAuthInteraction<'a>,
    ) -> Option<BoxFuture<'a, Result<ApiKeyCredential, AuthError>>> {
        Some(Box::pin(async move {
            let key = prompt_secret(interaction, "Enter Cloudflare API key").await?;
            let account_id = prompt_text(interaction, "Enter Cloudflare account ID").await?;
            let mut env = ProviderEnv::new();
            env.insert(CLOUDFLARE_ACCOUNT_ID.to_string(), account_id);
            Ok(ApiKeyCredential {
                key: Some(key),
                env: Some(env),
            })
        }))
    }

    fn resolve<'a>(
        &'a self,
        input: ApiKeyAuthInput<'a>,
    ) -> BoxFuture<'a, Result<Option<AuthResult>, AuthError>> {
        Box::pin(async move {
            let Some(resolved) = resolve_cloudflare_env(
                CloudflareAuthKind::WorkersAi,
                input.ctx,
                input.credential.as_ref(),
                &input.signal,
            )
            .await?
            else {
                return Ok(None);
            };
            Ok(Some(AuthResult {
                auth: ModelAuth {
                    api_key: Some(resolved.api_key),
                    ..Default::default()
                },
                env: Some(resolved.env),
                source: Some(resolved.source),
            }))
        })
    }
}

/// `cloudflareAIGatewayAuth()`
pub struct CloudflareAiGatewayAuth;

impl ApiKeyAuth for CloudflareAiGatewayAuth {
    fn name(&self) -> &str {
        "Cloudflare API key"
    }

    fn login<'a>(
        &'a self,
        interaction: &'a ProviderAuthInteraction<'a>,
    ) -> Option<BoxFuture<'a, Result<ApiKeyCredential, AuthError>>> {
        Some(Box::pin(async move {
            let key = prompt_secret(interaction, "Enter Cloudflare API key").await?;
            let account_id = prompt_text(interaction, "Enter Cloudflare account ID").await?;
            let gateway_id = prompt_text(interaction, "Enter Cloudflare AI Gateway ID").await?;
            let mut env = ProviderEnv::new();
            env.insert(CLOUDFLARE_ACCOUNT_ID.to_string(), account_id);
            env.insert(CLOUDFLARE_GATEWAY_ID.to_string(), gateway_id);
            Ok(ApiKeyCredential {
                key: Some(key),
                env: Some(env),
            })
        }))
    }

    fn resolve<'a>(
        &'a self,
        input: ApiKeyAuthInput<'a>,
    ) -> BoxFuture<'a, Result<Option<AuthResult>, AuthError>> {
        Box::pin(async move {
            let Some(resolved) = resolve_cloudflare_env(
                CloudflareAuthKind::AiGateway,
                input.ctx,
                input.credential.as_ref(),
                &input.signal,
            )
            .await?
            else {
                return Ok(None);
            };
            // The gateway authenticates through its own header; the provider defaults
            // for `Authorization`/`x-api-key` are suppressed with `null`.
            let mut headers = ProviderHeaders::new();
            headers.insert(
                "cf-aig-authorization".to_string(),
                Some(format!("Bearer {}", resolved.api_key)),
            );
            headers.insert("Authorization".to_string(), None);
            headers.insert("x-api-key".to_string(), None);
            Ok(Some(AuthResult {
                auth: ModelAuth {
                    headers: Some(headers),
                    ..Default::default()
                },
                env: Some(resolved.env),
                source: Some(resolved.source),
            }))
        })
    }
}
