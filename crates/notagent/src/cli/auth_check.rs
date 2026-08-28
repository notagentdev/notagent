use std::sync::Arc;

use notagent_ai::auth::types::{AuthOperationOptions, AuthType, CredentialStore};

use crate::cli::args::Args;
use crate::cli::auth_command::{
    AuthCommandError, AuthCommandKind, get_auth_credential, validate_auth_command_args,
};
use crate::core::model_resolver::resolve_cli_model;
use crate::core::model_runtime::{
    CreateModelRuntimeOptions, ModelRuntime, ModelRuntimeAuthOverrides,
};
use crate::core::models_store::InMemoryCodingAgentModelsStore;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthCheckStatus {
    Ready,
    NotReady,
    Invalid,
}

impl AuthCheckStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            AuthCheckStatus::Ready => "ready",
            AuthCheckStatus::NotReady => "not_ready",
            AuthCheckStatus::Invalid => "invalid",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthCheckReason {
    ProviderNotFound,
    CredentialsNotConfigured,
    CredentialNotAvailable,
    InvalidState,
}

impl AuthCheckReason {
    pub fn as_str(self) -> &'static str {
        match self {
            AuthCheckReason::ProviderNotFound => "provider_not_found",
            AuthCheckReason::CredentialsNotConfigured => "credentials_not_configured",
            AuthCheckReason::CredentialNotAvailable => "credential_not_available",
            AuthCheckReason::InvalidState => "invalid_state",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthCheckResult {
    pub status: AuthCheckStatus,
    pub provider: String,
    pub reason: Option<AuthCheckReason>,
    pub auth_type: Option<AuthType>,
}

/// The JSON form of `--json`, with the optional fields omitted like the
pub fn auth_check_result_to_json(
    result: &AuthCheckResult,
    credential: Option<&str>,
) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    map.insert(
        "status".to_owned(),
        serde_json::Value::from(result.status.as_str()),
    );
    map.insert(
        "provider".to_owned(),
        serde_json::Value::from(result.provider.clone()),
    );
    if let Some(reason) = result.reason {
        map.insert(
            "reason".to_owned(),
            serde_json::Value::from(reason.as_str()),
        );
    }
    if let Some(auth_type) = result.auth_type {
        map.insert(
            "authType".to_owned(),
            serde_json::Value::from(match auth_type {
                AuthType::ApiKey => "api_key",
                AuthType::OAuth => "oauth",
            }),
        );
    }
    if let Some(credential) = credential {
        map.insert(
            "credentials".to_owned(),
            serde_json::Value::from(credential),
        );
    }
    serde_json::Value::Object(map)
}

/// Resolves the provider the check is about and reports its auth state.
pub async fn check_provider_auth(
    args: &Args,
    model_runtime: &ModelRuntime,
    refresh: bool,
) -> Result<AuthCheckResult, AuthCommandError> {
    let target = validate_auth_command_args(args, AuthCommandKind::Check)?;
    let mut provider = target.provider.clone();
    if let Some(cli_model) = target.model.as_deref() {
        let resolved = resolve_cli_model(
            target.provider.as_deref(),
            Some(cli_model),
            None,
            model_runtime,
        );
        match (resolved.error, resolved.model) {
            (None, Some(model)) => provider = Some(model.provider.clone()),
            (error, _) => {
                return Err(AuthCommandError(error.unwrap_or_else(|| {
                    format!("Unable to resolve model \"{cli_model}\"")
                })));
            }
        }
    }
    let Some(provider) = provider else {
        return Err(AuthCommandError(
            "Unable to resolve an auth provider".into(),
        ));
    };

    if model_runtime.get_error().is_some() {
        return Ok(AuthCheckResult {
            status: AuthCheckStatus::Invalid,
            provider,
            reason: Some(AuthCheckReason::InvalidState),
            auth_type: None,
        });
    }
    if model_runtime.get_provider(&provider).is_none() {
        return Ok(AuthCheckResult {
            status: AuthCheckStatus::NotReady,
            provider,
            reason: Some(AuthCheckReason::ProviderNotFound),
            auth_type: None,
        });
    }

    let invalid = AuthCheckResult {
        status: AuthCheckStatus::Invalid,
        provider: provider.clone(),
        reason: Some(AuthCheckReason::InvalidState),
        auth_type: None,
    };
    let not_configured = AuthCheckResult {
        status: AuthCheckStatus::NotReady,
        provider: provider.clone(),
        reason: Some(AuthCheckReason::CredentialsNotConfigured),
        auth_type: None,
    };

    let auth = match model_runtime.check_auth(&provider, None).await {
        Ok(auth) => auth,
        Err(_) => return Ok(invalid),
    };
    let Some(auth) = auth else {
        return Ok(not_configured);
    };
    if refresh {
        match model_runtime.get_auth_for_provider(&provider, None).await {
            Ok(None) => return Ok(not_configured),
            Ok(Some(_)) => {}
            Err(_) => return Ok(invalid),
        }
    }
    Ok(AuthCheckResult {
        status: AuthCheckStatus::Ready,
        provider,
        reason: None,
        auth_type: Some(auth.check_type),
    })
}

/// The credential of a provider, refreshing an OAuth token unless told not to.
pub async fn get_provider_credential(
    provider_id: &str,
    model_runtime: &ModelRuntime,
    credentials: &dyn CredentialStore,
    refresh: bool,
) -> Option<String> {
    let credential = credentials
        .read(provider_id, None::<AuthOperationOptions>)
        .await
        .ok()
        .flatten();
    if !refresh && let Some(notagent_ai::auth::types::Credential::OAuth(oauth)) = &credential {
        return Some(oauth.access.clone());
    }
    let auth = model_runtime
        .get_auth_for_provider(provider_id, None)
        .await
        .ok()
        .flatten();
    get_auth_credential(auth.as_ref())
}

/// The runtime the auth commands use: no catalog storage, no network.
pub async fn create_auth_check_model_runtime(
    credentials: Arc<dyn CredentialStore>,
) -> Result<Arc<ModelRuntime>, String> {
    ModelRuntime::create(CreateModelRuntimeOptions {
        credentials: Some(credentials),
        models_store: Some(Arc::new(InMemoryCodingAgentModelsStore::new())),
        allow_model_network: Some(false),
        refresh_on_create: Some(false),
        ..CreateModelRuntimeOptions::default()
    })
    .await
}

/// Keeps the auth override type in the public surface of this module, matching
pub type AuthOverrides = ModelRuntimeAuthOverrides;
