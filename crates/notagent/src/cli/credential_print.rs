use notagent_ai::auth::types::{AuthOperationOptions, AuthType};
use notagent_ai::types::Model;
use tokio_util::sync::CancellationToken;

use crate::cli::args::Args;
use crate::cli::auth_command::{
    AuthCommandError, AuthCommandKind, get_auth_credential, validate_auth_command_args,
};
use crate::core::model_resolver::resolve_cli_model;
use crate::core::model_runtime::{ModelRuntime, ModelRuntimeAuthOverrides};

const DEFAULT_BEARER_TOKEN_MIN_EXPIRY_MS: i64 = 30 * 60_000;

struct ProviderTarget {
    id: String,
    model: Option<Model>,
}

/// Resolves the single credential to print, or says why it cannot.
pub async fn resolve_credential_for_print(
    args: &Args,
    model_runtime: &ModelRuntime,
    kind: AuthCommandKind,
    min_expiry_ms: Option<u64>,
    signal: Option<CancellationToken>,
) -> Result<String, AuthCommandError> {
    let target = validate_auth_command_args(args, kind)?;
    let cli_provider = target.provider.as_deref();
    let cli_model = target.model.as_deref();

    let credential_types: Vec<(String, AuthType)> = model_runtime
        .list_credentials(Some(AuthOperationOptions {
            signal: signal.clone(),
        }))
        .await
        .map_err(|error| AuthCommandError(error.message.clone()))?
        .into_iter()
        .map(|credential| (credential.provider_id, credential.credential_type))
        .collect();
    let credential_type = |provider_id: &str| -> Option<AuthType> {
        credential_types
            .iter()
            .find(|(id, _)| id == provider_id)
            .map(|(_, credential_type)| *credential_type)
    };

    let mut providers: Vec<ProviderTarget> = Vec::new();
    if let Some(cli_provider) = cli_provider {
        let Some(provider) = model_runtime.get_provider(cli_provider) else {
            return Err(AuthCommandError(format!(
                "Unknown provider \"{cli_provider}\". Use --list-models to see available providers."
            )));
        };
        let provider_id = provider.id().to_owned();
        match cli_model {
            Some(cli_model) => {
                let resolved =
                    resolve_cli_model(Some(&provider_id), Some(cli_model), None, model_runtime);
                match (resolved.error, resolved.model) {
                    (None, Some(model)) => providers.push(ProviderTarget {
                        id: provider_id,
                        model: Some(model),
                    }),
                    (error, _) => {
                        return Err(AuthCommandError(error.unwrap_or_else(|| {
                            "Unable to resolve the requested provider/model".to_owned()
                        })));
                    }
                }
            }
            None => providers.push(ProviderTarget {
                id: provider_id,
                model: None,
            }),
        }
    } else {
        for provider in model_runtime.get_providers() {
            let provider_id = provider.id().to_owned();
            if credential_type(&provider_id).is_none() {
                continue;
            }
            // `cliModel` is guaranteed here: validation refused the case where
            // neither provider nor model was given.
            let resolved = resolve_cli_model(Some(&provider_id), cli_model, None, model_runtime);
            let uses_custom_id = resolved
                .warning
                .as_deref()
                .is_some_and(|warning| warning.contains("Using custom model id"));
            if let (Some(model), None, false) = (resolved.model, resolved.error, uses_custom_id) {
                providers.push(ProviderTarget {
                    id: provider_id,
                    model: Some(model),
                });
            }
        }
        if providers.is_empty() {
            return Err(AuthCommandError(format!(
                "Model \"{}\" not found. Use --list-models to see available models.",
                cli_model.unwrap_or_default()
            )));
        }
    }

    let mut credentials: Vec<(String, String)> = Vec::new();
    for provider in &providers {
        let credential_type = credential_type(&provider.id);
        if kind == AuthCommandKind::ApiKey && credential_type == Some(AuthType::OAuth) {
            continue;
        }
        if kind == AuthCommandKind::BearerToken && credential_type != Some(AuthType::OAuth) {
            continue;
        }
        let overrides = ModelRuntimeAuthOverrides {
            min_oauth_validity_ms: (kind == AuthCommandKind::BearerToken).then(|| {
                min_expiry_ms
                    .map(|value| value as i64)
                    .unwrap_or(DEFAULT_BEARER_TOKEN_MIN_EXPIRY_MS)
            }),
            signal: signal.clone(),
            ..ModelRuntimeAuthOverrides::default()
        };
        let auth = match &provider.model {
            Some(model) => {
                model_runtime
                    .get_auth_for_model(model, Some(&overrides))
                    .await
            }
            None => {
                model_runtime
                    .get_auth_for_provider(&provider.id, Some(&overrides))
                    .await
            }
        }
        .map_err(|error| AuthCommandError(error.message.clone()))?;
        if let Some(value) = get_auth_credential(auth.as_ref()) {
            credentials.push((provider.id.clone(), value));
        }
    }

    if credentials.len() == 1 {
        return Ok(credentials.remove(0).1);
    }
    if credentials.is_empty() {
        let provider_id = providers.first().map(|provider| provider.id.clone());
        let credential_type = provider_id.as_deref().and_then(credential_type);
        if cli_provider.is_some()
            && kind == AuthCommandKind::ApiKey
            && credential_type == Some(AuthType::OAuth)
        {
            return Err(AuthCommandError(format!(
                "Provider \"{}\" is configured with OAuth, not an API key",
                provider_id.unwrap_or_default()
            )));
        }
        if cli_provider.is_some()
            && kind == AuthCommandKind::BearerToken
            && credential_type != Some(AuthType::OAuth)
        {
            return Err(AuthCommandError(format!(
                "Provider \"{}\" is not configured with an OAuth bearer token",
                provider_id.unwrap_or_default()
            )));
        }
        return Err(AuthCommandError(format!(
            "No usable {} is configured",
            if kind == AuthCommandKind::ApiKey {
                "API key"
            } else {
                "OAuth bearer token"
            }
        )));
    }
    Err(AuthCommandError(format!(
        "Multiple configured providers matched ({}). Specify --provider.",
        credentials
            .iter()
            .map(|(provider_id, _)| provider_id.clone())
            .collect::<Vec<_>>()
            .join(", ")
    )))
}
