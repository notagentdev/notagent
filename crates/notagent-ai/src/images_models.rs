use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use crate::auth::context::default_provider_auth_context;
use crate::auth::credential_store::InMemoryCredentialStore;
use crate::auth::resolve::{
    AuthProvider, AuthResolutionOverrides, ModelsError, ModelsErrorCode, resolve_provider_auth,
};
use crate::auth::types::{AuthContext, AuthResult, CredentialStore, ProviderAuth};
use crate::models::CreateModelsOptions;
use crate::types::{
    AssistantImages, ImagesContext, ImagesModel, ImagesOptions, ImagesStopReason, ProviderEnv,
    ProviderHeaders, ProviderImages,
};

/// `refreshModels?(): Promise<readonly ImagesModel[]>`
pub type FetchImagesModelsFn = Arc<
    dyn Fn() -> Pin<Box<dyn Future<Output = Result<Vec<ImagesModel>, String>> + Send>>
        + Send
        + Sync,
>;

/// The future returned by a dynamic provider's refresh.
pub type RefreshImagesFuture = Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;

/// `ImagesProvider`
pub trait ImagesProvider: Send + Sync {
    fn id(&self) -> &str;
    fn name(&self) -> &str;
    /// At least one of `api_key`/`oauth` is present.
    fn auth(&self) -> &ProviderAuth;
    /// Current known models, synchronous. Must not fail.
    fn get_models(&self) -> Vec<ImagesModel>;
    /// Dynamic providers only: fetch and store the current list.
    fn refresh_models(&self) -> Option<RefreshImagesFuture> {
        None
    }
    fn generate_images(
        &self,
        model: &ImagesModel,
        context: &ImagesContext,
        options: Option<ImagesOptions>,
    ) -> Pin<Box<dyn Future<Output = AssistantImages> + Send>>;
}

/// `CreateImagesProviderOptions`
pub struct CreateImagesProviderOptions {
    pub id: String,
    /// Display name; defaults to `id`.
    pub name: Option<String>,
    pub auth: ProviderAuth,
    /// Initial model list (empty for purely dynamic providers).
    pub models: Vec<ImagesModel>,
    pub refresh_models: Option<FetchImagesModelsFn>,
    pub api: Arc<dyn ProviderImages>,
}

/// A provider built from parts.
pub struct BuiltImagesProvider {
    id: String,
    name: String,
    auth: ProviderAuth,
    models: Arc<Mutex<Vec<ImagesModel>>>,
    refresh_models: Option<FetchImagesModelsFn>,
    api: Arc<dyn ProviderImages>,
}

/// `createImagesProvider(input)`
pub fn create_images_provider(input: CreateImagesProviderOptions) -> Arc<BuiltImagesProvider> {
    Arc::new(BuiltImagesProvider {
        name: input.name.unwrap_or_else(|| input.id.clone()),
        id: input.id,
        auth: input.auth,
        models: Arc::new(Mutex::new(input.models)),
        refresh_models: input.refresh_models,
        api: input.api,
    })
}

impl ImagesProvider for BuiltImagesProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn auth(&self) -> &ProviderAuth {
        &self.auth
    }

    fn get_models(&self) -> Vec<ImagesModel> {
        self.models
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn refresh_models(&self) -> Option<RefreshImagesFuture> {
        let refresh = self.refresh_models.clone()?;
        // port keeps the same observable contract (a failed refresh leaves the stored
        // list untouched and a later call retries) — see `models.rs` for the same shape.
        let models = self.models.clone();
        Some(Box::pin(async move {
            let fetched = refresh().await?;
            *models
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = fetched;
            Ok(())
        }))
    }

    fn generate_images(
        &self,
        model: &ImagesModel,
        context: &ImagesContext,
        options: Option<ImagesOptions>,
    ) -> Pin<Box<dyn Future<Output = AssistantImages> + Send>> {
        self.api.generate_images(model, context, options)
    }
}

/// `ImagesModels`
pub struct ImagesModels {
    providers: Mutex<BTreeMap<String, Arc<dyn ImagesProvider>>>,
    order: Mutex<Vec<String>>,
    credentials: Arc<dyn CredentialStore>,
    auth_context: Arc<dyn AuthContext>,
}

/// `createImagesModels(options)`
pub fn create_images_models(options: Option<CreateModelsOptions>) -> Arc<ImagesModels> {
    let options = options.unwrap_or_default();
    Arc::new(ImagesModels {
        providers: Mutex::new(BTreeMap::new()),
        order: Mutex::new(Vec::new()),
        credentials: options
            .credentials
            .unwrap_or_else(|| Arc::new(InMemoryCredentialStore::default())),
        auth_context: options
            .auth_context
            .unwrap_or_else(default_provider_auth_context),
    })
}

impl ImagesModels {
    /// `setProvider(provider)` — upsert by id.
    pub fn set_provider(&self, provider: Arc<dyn ImagesProvider>) {
        let id = provider.id().to_string();
        let mut providers = self
            .providers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if providers.insert(id.clone(), provider).is_none() {
            self.order
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(id);
        }
    }

    pub fn delete_provider(&self, id: &str) {
        self.providers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(id);
        self.order
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|entry| entry != id);
    }

    pub fn clear_providers(&self) {
        self.providers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        self.order
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    /// `getProviders()` — insertion order, like the JS `Map`.
    pub fn get_providers(&self) -> Vec<Arc<dyn ImagesProvider>> {
        let providers = self
            .providers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.order
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter_map(|id| providers.get(id).cloned())
            .collect()
    }

    pub fn get_provider(&self, id: &str) -> Option<Arc<dyn ImagesProvider>> {
        self.providers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(id)
            .cloned()
    }

    /// `getModels(provider?)`
    pub fn get_models(&self, provider: Option<&str>) -> Vec<ImagesModel> {
        match provider {
            Some(provider) => self
                .get_provider(provider)
                .map(|provider| provider.get_models())
                .unwrap_or_default(),
            None => self
                .get_providers()
                .iter()
                .flat_map(|provider| provider.get_models())
                .collect(),
        }
    }

    pub fn get_model(&self, provider: &str, id: &str) -> Option<ImagesModel> {
        self.get_models(Some(provider))
            .into_iter()
            .find(|model| model.id == id)
    }

    /// `refresh(provider?)`
    pub async fn refresh(&self, provider: Option<&str>) -> Result<(), ModelsError> {
        if let Some(provider) = provider {
            let Some(entry) = self.get_provider(provider) else {
                return Ok(());
            };
            let Some(refresh) = entry.refresh_models() else {
                return Ok(());
            };
            return refresh.await.map_err(|error| {
                ModelsError::with_cause(
                    ModelsErrorCode::ModelSource,
                    format!("Model refresh failed for {provider}"),
                    &error,
                )
            });
        }

        // Cannot fail: every provider's refresh is awaited best-effort.
        for entry in self.get_providers() {
            if let Some(refresh) = entry.refresh_models() {
                let _ = refresh.await;
            }
        }
        Ok(())
    }

    /// `getAuth(providerId | model, overrides?)`
    pub async fn get_auth(
        &self,
        provider_id: &str,
        overrides: Option<&AuthResolutionOverrides>,
    ) -> Result<Option<AuthResult>, ModelsError> {
        let Some(provider) = self.get_provider(provider_id) else {
            return Ok(None);
        };
        resolve_provider_auth(
            AuthProvider {
                id: provider.id(),
                auth: provider.auth(),
            },
            self.credentials.as_ref(),
            self.auth_context.clone(),
            overrides,
        )
        .await
    }

    /// `generateImages(model, context, options)` — never fails; errors come back as an
    /// `AssistantImages` with `stopReason: "error"`.
    pub async fn generate_images(
        &self,
        model: &ImagesModel,
        context: &ImagesContext,
        options: Option<ImagesOptions>,
    ) -> AssistantImages {
        let timestamp = crate::auth::resolve::now_ms();
        match self
            .generate_images_inner(model, context, options, timestamp)
            .await
        {
            Ok(images) => images,
            Err(message) => AssistantImages {
                api: model.api.clone(),
                provider: model.provider.clone(),
                model: model.id.clone(),
                output: Vec::new(),
                response_id: None,
                usage: None,
                stop_reason: ImagesStopReason::Error,
                error_message: Some(message),
                timestamp,
            },
        }
    }

    async fn generate_images_inner(
        &self,
        model: &ImagesModel,
        context: &ImagesContext,
        options: Option<ImagesOptions>,
        _timestamp: i64,
    ) -> Result<AssistantImages, String> {
        let Some(provider) = self.get_provider(&model.provider) else {
            return Err(format!("Unknown provider: {}", model.provider));
        };

        let resolution = self
            .get_auth(
                &model.provider,
                Some(&AuthResolutionOverrides {
                    api_key: options
                        .as_ref()
                        .and_then(|options| options.base.api_key.clone()),
                    env: options
                        .as_ref()
                        .and_then(|options| options.base.env.clone()),
                    signal: options
                        .as_ref()
                        .and_then(|options| options.base.signal.clone()),
                    ..AuthResolutionOverrides::default()
                }),
            )
            .await
            .map_err(|error| error.to_string())?;

        let Some(resolution) = resolution else {
            return Ok(provider.generate_images(model, context, options).await);
        };

        let request_model = match &resolution.auth.base_url {
            Some(base_url) => ImagesModel {
                base_url: base_url.clone(),
                ..model.clone()
            },
            None => model.clone(),
        };

        // Explicit request options win per field; headers and env merge per key.
        let mut request_options = options.unwrap_or_default();
        request_options.base.api_key = request_options
            .base
            .api_key
            .or_else(|| resolution.auth.api_key.clone());
        request_options.base.headers = merge_headers(
            resolution.auth.headers.as_ref(),
            request_options.base.headers,
        );
        request_options.base.env = merge_env(resolution.env.as_ref(), request_options.base.env);

        Ok(provider
            .generate_images(&request_model, context, Some(request_options))
            .await)
    }
}

/// `{ ...auth.headers, ...options?.headers }`
fn merge_headers(
    auth_headers: Option<&ProviderHeaders>,
    option_headers: Option<ProviderHeaders>,
) -> Option<ProviderHeaders> {
    match (auth_headers, option_headers) {
        (None, None) => None,
        (Some(auth_headers), None) => Some(auth_headers.clone()),
        (None, Some(option_headers)) => Some(option_headers),
        (Some(auth_headers), Some(option_headers)) => {
            let mut merged = auth_headers.clone();
            merged.extend(option_headers);
            Some(merged)
        }
    }
}

/// `{ ...(resolution.env ?? {}), ...(options?.env ?? {}) }`
fn merge_env(
    auth_env: Option<&ProviderEnv>,
    option_env: Option<ProviderEnv>,
) -> Option<ProviderEnv> {
    match (auth_env, option_env) {
        (None, None) => None,
        (Some(auth_env), None) => Some(auth_env.clone()),
        (None, Some(option_env)) => Some(option_env),
        (Some(auth_env), Some(option_env)) => {
            let mut merged = auth_env.clone();
            merged.extend(option_env);
            Some(merged)
        }
    }
}

// ---------------------------------------------------------------------------
// Built-in image providers
// ---------------------------------------------------------------------------

pub fn openrouter_images_provider() -> Arc<BuiltImagesProvider> {
    create_images_provider(CreateImagesProviderOptions {
        id: "openrouter".to_string(),
        name: Some("OpenRouter".to_string()),
        auth: ProviderAuth {
            api_key: Some(Arc::new(crate::auth::helpers::EnvApiKeyAuth::new(
                "OpenRouter API key",
                ["OPENROUTER_API_KEY"],
            ))),
            oauth: Some(crate::auth::oauth::openrouter::open_router_oauth()),
        },
        models: crate::images::built_in_image_models("openrouter"),
        refresh_models: None,
        api: Arc::new(crate::api::openrouter_images::OpenRouterImages),
    })
}

/// `builtinImagesProviders()`
pub fn builtin_images_providers() -> Vec<Arc<dyn ImagesProvider>> {
    vec![openrouter_images_provider()]
}

/// `builtinImagesModels(options)`
pub fn builtin_images_models(options: Option<CreateModelsOptions>) -> Arc<ImagesModels> {
    let models = create_images_models(options);
    for provider in builtin_images_providers() {
        models.set_provider(provider);
    }
    models
}
