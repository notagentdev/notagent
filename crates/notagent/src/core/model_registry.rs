use std::sync::Arc;

use notagent_ai::auth::resolve::ModelsError;
use notagent_ai::auth::types::AuthResult;
use notagent_ai::models::{ModelsRefreshOptions, ModelsRefreshResult, Provider};
use notagent_ai::types::{AssistantMessage, Context, Model, ProviderEnv, ProviderHeaders};

use crate::core::model_runtime::{ModelRuntime, ModelsApiStreamOptions};
use crate::core::provider_composer::AuthStatus;

/// `ResolvedRequestAuth`
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedRequestAuth {
    Ok {
        api_key: Option<String>,
        headers: Option<ProviderHeaders>,
        base_url: Option<String>,
        env: Option<ProviderEnv>,
    },
    Err(String),
}

pub use crate::core::provider_composer::clear_api_key_cache;

pub struct ModelRegistry {
    runtime: Arc<ModelRuntime>,
}

impl ModelRegistry {
    pub fn new(runtime: Arc<ModelRuntime>) -> Self {
        ModelRegistry { runtime }
    }

    /// Reload models.json asynchronously. Await before making synchronous registry reads.
    pub async fn refresh(&self, options: Option<ModelsRefreshOptions>) -> ModelsRefreshResult {
        self.runtime.refresh(options.unwrap_or_default()).await
    }

    pub fn get_error(&self) -> Option<String> {
        self.runtime.get_error()
    }

    pub fn get_all(&self) -> Vec<Model> {
        self.runtime.get_models(None)
    }

    pub fn get_available(&self) -> Vec<Model> {
        self.runtime.get_available_snapshot()
    }

    pub fn find(&self, provider: &str, model_id: &str) -> Option<Model> {
        self.runtime.get_model(provider, model_id)
    }

    pub fn has_configured_auth(&self, model: &Model) -> bool {
        self.runtime.has_configured_auth(&model.provider)
    }

    /// `getApiKeyAndHeaders(model)`
    pub async fn get_api_key_and_headers(&self, model: &Model) -> ResolvedRequestAuth {
        match self.runtime.get_auth_for_model(model, None).await {
            Ok(Some(resolution)) => ResolvedRequestAuth::Ok {
                api_key: resolution.auth.api_key,
                headers: resolution.auth.headers,
                base_url: resolution.auth.base_url,
                env: resolution.env,
            },
            Ok(None) => match self.runtime.get_compatibility_request_config(model) {
                Ok(compatibility) => {
                    if compatibility.auth_header {
                        ResolvedRequestAuth::Err(format!(
                            "No API key found for \"{}\"",
                            model.provider
                        ))
                    } else {
                        ResolvedRequestAuth::Ok {
                            api_key: None,
                            headers: compatibility.headers,
                            base_url: None,
                            env: None,
                        }
                    }
                }
                Err(error) => ResolvedRequestAuth::Err(error),
            },
            Err(error) => ResolvedRequestAuth::Err(map_auth_error(&error, &model.provider)),
        }
    }

    pub fn get_provider_auth_status(&self, provider: &str) -> AuthStatus {
        self.runtime.get_provider_auth_status(provider)
    }

    pub fn get_provider(&self, provider: &str) -> Option<Arc<dyn Provider>> {
        self.runtime.get_provider(provider)
    }

    pub async fn complete(
        &self,
        model: &Model,
        context: &Context,
        options: Option<ModelsApiStreamOptions>,
    ) -> AssistantMessage {
        self.runtime.complete(model, context, options).await
    }

    pub fn get_provider_display_name(&self, provider: &str) -> String {
        self.runtime
            .get_provider(provider)
            .map_or_else(|| provider.to_owned(), |entry| entry.name().to_owned())
    }

    pub async fn get_provider_auth(
        &self,
        provider: &str,
    ) -> Result<Option<AuthResult>, ModelsError> {
        self.runtime.get_auth_for_provider(provider, None).await
    }

    pub async fn get_api_key_for_provider(&self, provider: &str) -> Option<String> {
        self.runtime
            .get_auth_for_provider(provider, None)
            .await
            .ok()
            .flatten()
            .and_then(|resolution| resolution.auth.api_key)
    }

    pub fn is_using_oauth(&self, model: &Model) -> bool {
        self.runtime.is_using_oauth(&model.provider)
    }

    /// `registerProvider(provider)` — the native overload.
    pub fn register_provider(&self, provider: Arc<dyn Provider>) -> Result<(), String> {
        self.runtime.register_native_provider(provider)
    }

    pub fn unregister_provider(&self, provider_name: &str) {
        self.runtime.unregister_provider(provider_name);
    }

    pub fn get_registered_native_provider(&self, provider_name: &str) -> Option<Arc<dyn Provider>> {
        self.runtime.get_registered_native_provider(provider_name)
    }

    pub fn get_registered_provider_ids(&self) -> Vec<String> {
        self.runtime.get_registered_provider_ids()
    }
}

/// (`"<context>: <cause>"`), so the marker is matched at the end (class 1).
fn map_auth_error(error: &ModelsError, provider: &str) -> String {
    const AUTH_HEADER_MARKER: &str = "authHeader requires a resolved API key";
    if error.message == AUTH_HEADER_MARKER
        || error.message.ends_with(&format!(": {AUTH_HEADER_MARKER}"))
    {
        return format!("No API key found for \"{provider}\"");
    }
    error.message.clone()
}
