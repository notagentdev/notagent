use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use notagent_ai::auth::resolve::{AuthResolutionOverrides, ModelsError, ModelsErrorCode};
use notagent_ai::auth::types::{
    AuthCheck, AuthInteraction, AuthOperationOptions, AuthResult, AuthType, Credential,
    CredentialInfo, CredentialStore, ModelAuth,
};
use notagent_ai::lazy_stream;
use notagent_ai::models::{
    CreateModelsOptions, Models, ModelsRefreshOptions, ModelsRefreshResult, Provider,
    create_models, merge_headers,
};
use notagent_ai::models_store::ModelsStore;
use notagent_ai::providers::all as builtin_provider_catalog;
use notagent_ai::providers::mtplx;
use notagent_ai::providers::openai_compatible;
use notagent_ai::providers::radius::{RadiusProviderOptions, radius_provider};
use notagent_ai::types::{
    AssistantMessage, Context, DeferredCancelOptions, DeferredFetchOptions, DeferredHandle, Model,
    ProviderEnv, ProviderHeaders, SimpleStreamOptions, StreamOptions,
};
use notagent_ai::utils::event_stream::AssistantMessageEventStream;
use tokio_util::sync::CancellationToken;

use crate::config::get_agent_dir;
use crate::core::auth_storage::AuthStorage;
use crate::core::model_config::ModelConfig;
use crate::core::models_store::{FileModelsStore, InMemoryCodingAgentModelsStore};
use crate::core::provider_composer::{
    AuthStatus, AuthStatusSource, CompatibilityRequestConfig, compose_model_provider,
    configured_request_auth_status, resolve_compatibility_request_config,
    resolve_configured_model_headers,
};
use crate::core::remote_catalog_provider::with_remote_catalog;
use crate::core::runtime_credentials::RuntimeCredentials;
use crate::utils::abort::operation_signal;

#[derive(Default, Clone)]
struct ModelRuntimeSnapshot {
    all: Vec<Model>,
    available: Vec<Model>,
    configured_providers: BTreeSet<String>,
    stored_providers: BTreeSet<String>,
    auth: BTreeMap<String, AuthCheck>,
}

/// `CreateModelRuntimeOptions`
#[derive(Default)]
pub struct CreateModelRuntimeOptions {
    /// Credential storage. Defaults to the file at `auth_path`.
    pub credentials: Option<Arc<dyn CredentialStore>>,
    pub auth_path: Option<String>,
    pub models_path: Option<Option<String>>,
    pub models_store: Option<Arc<dyn ModelsStore>>,
    pub models_store_path: Option<String>,
    /// Allow `create()` to refresh model catalogs over the network. Defaults to false.
    pub allow_model_network: Option<bool>,
    /// Timeout for the create-time network model refresh.
    pub model_refresh_timeout_ms: Option<u64>,
    pub catalog_base_url: Option<String>,
    /// Optional caller cancellation for initial cache restoration and availability checks.
    pub signal: Option<CancellationToken>,
    /// Skip initial catalog and availability refresh. Static models remain available.
    pub refresh_on_create: Option<bool>,
}

/// `ModelRuntimeAuthOverrides extends AuthOperationOptions`
#[derive(Default, Clone)]
pub struct ModelRuntimeAuthOverrides {
    pub api_key: Option<String>,
    pub env: Option<ProviderEnv>,
    /// Require this much remaining OAuth-token validity; defaults to five minutes.
    pub min_oauth_validity_ms: Option<i64>,
    pub signal: Option<CancellationToken>,
}

impl ModelRuntimeAuthOverrides {
    fn as_resolution(&self) -> AuthResolutionOverrides {
        AuthResolutionOverrides {
            api_key: self.api_key.clone(),
            env: self.env.clone(),
            min_oauth_validity_ms: self.min_oauth_validity_ms,
            signal: self.signal.clone(),
        }
    }
}

/// `CredentialSynchronizationOperation`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialSynchronizationOperation {
    Login,
    Logout,
    SetRuntimeApiKey,
    RemoveRuntimeApiKey,
}

impl std::fmt::Display for CredentialSynchronizationOperation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            CredentialSynchronizationOperation::Login => "login",
            CredentialSynchronizationOperation::Logout => "logout",
            CredentialSynchronizationOperation::SetRuntimeApiKey => "setRuntimeApiKey",
            CredentialSynchronizationOperation::RemoveRuntimeApiKey => "removeRuntimeApiKey",
        };
        formatter.write_str(name)
    }
}

/// Credentials changed successfully, but the local model/auth snapshot could not be
/// synchronized.
#[derive(Debug, Clone)]
pub struct CredentialSynchronizationError {
    pub provider_id: String,
    pub operation: CredentialSynchronizationOperation,
    pub credential: Option<Credential>,
    pub cause: String,
}

impl std::fmt::Display for CredentialSynchronizationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "Credential {} committed for {}, but local synchronization failed",
            self.operation, self.provider_id
        )
    }
}

impl std::error::Error for CredentialSynchronizationError {}

/// The error of every `ModelRuntime` operation that can also fail while syncing.
#[derive(Debug, Clone)]
pub enum ModelRuntimeError {
    Models(ModelsError),
    Synchronization(Box<CredentialSynchronizationError>),
}

impl std::fmt::Display for ModelRuntimeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ModelRuntimeError::Models(error) => formatter.write_str(&error.message),
            ModelRuntimeError::Synchronization(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for ModelRuntimeError {}

impl From<ModelsError> for ModelRuntimeError {
    fn from(error: ModelsError) -> Self {
        ModelRuntimeError::Models(error)
    }
}

fn aborted_error() -> ModelsError {
    ModelsError::new(ModelsErrorCode::Auth, "The operation was aborted")
}

#[derive(Default)]
struct AvailabilityState {
    refresh_seq: u64,
    error_seq: u64,
    provider_seq: BTreeMap<String, u64>,
    error: Option<String>,
}

/// Providers whose plan covers the tokens although they authenticate with an
/// API key, so the auth kind alone cannot identify them. Both Z.AI providers
/// use the Coding Plan endpoint.
const SUBSCRIPTION_API_KEY_PROVIDERS: [&str; 4] =
    ["kimi-coding", "cline-pass", "zai", "zai-coding-cn"];

/// Whether `provider_id` is one of the plans that authenticate with an API
/// key. Exposed so that anything mirroring the runtime — a test double, a
/// different front end — answers the question the same way.
#[must_use]
pub fn is_subscription_api_key_provider(provider_id: &str) -> bool {
    SUBSCRIPTION_API_KEY_PROVIDERS.contains(&provider_id)
}

/// Configured notagent-ai Models collection used by coding-agent and SDK consumers.
pub struct ModelRuntime {
    models: Arc<Models>,
    credentials: Arc<RuntimeCredentials>,
    default_builtins: Vec<(String, Arc<dyn Provider>)>,
    builtins: Mutex<BTreeMap<String, Arc<dyn Provider>>>,
    native_extension_providers: Mutex<BTreeMap<String, Arc<dyn Provider>>>,
    composition_errors: Mutex<BTreeMap<String, String>>,
    models_path: Option<String>,
    model_network_enabled: bool,
    config: Mutex<Arc<ModelConfig>>,
    snapshot: Mutex<ModelRuntimeSnapshot>,
    availability: Mutex<AvailabilityState>,
    credential_operations: Mutex<BTreeMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl ModelRuntime {
    fn new(
        credentials: Arc<RuntimeCredentials>,
        config: ModelConfig,
        models_path: Option<String>,
        models_store: Arc<dyn ModelsStore>,
        providers: Vec<Arc<dyn Provider>>,
        model_network_enabled: bool,
    ) -> Arc<Self> {
        let default_builtins: Vec<(String, Arc<dyn Provider>)> = providers
            .into_iter()
            .map(|provider| (provider.id().to_owned(), provider))
            .collect();
        let builtins: BTreeMap<String, Arc<dyn Provider>> =
            default_builtins.iter().cloned().collect();
        let runtime = Arc::new(ModelRuntime {
            models: create_models(Some(CreateModelsOptions {
                credentials: Some(Arc::clone(&credentials) as Arc<dyn CredentialStore>),
                models_store: Some(models_store),
                auth_context: None,
            })),
            credentials,
            default_builtins,
            builtins: Mutex::new(builtins),
            native_extension_providers: Mutex::new(BTreeMap::new()),
            composition_errors: Mutex::new(BTreeMap::new()),
            models_path,
            model_network_enabled,
            config: Mutex::new(Arc::new(config)),
            snapshot: Mutex::new(ModelRuntimeSnapshot::default()),
            availability: Mutex::new(AvailabilityState::default()),
            credential_operations: Mutex::new(BTreeMap::new()),
        });
        runtime.rebuild_providers();
        runtime
    }

    /// `ModelRuntime.create(options)`
    pub async fn create(options: CreateModelRuntimeOptions) -> Result<Arc<Self>, String> {
        let store: Arc<dyn CredentialStore> = match options.credentials {
            Some(credentials) => credentials,
            None => Arc::new(
                match &options.auth_path {
                    Some(auth_path) => AuthStorage::create(auth_path),
                    None => AuthStorage::create_default(),
                }
                .map_err(|error| error.to_string())?,
            ),
        };
        let credentials = Arc::new(RuntimeCredentials::new(store));
        let models_path: Option<String> = match options.models_path {
            Some(None) => None,
            Some(Some(path)) => Some(path),
            None => Some(
                get_agent_dir()
                    .join("models.json")
                    .to_string_lossy()
                    .into_owned(),
            ),
        };
        let config = ModelConfig::load(models_path.as_deref()).await;
        let models_store: Arc<dyn ModelsStore> = match options.models_store {
            Some(models_store) => models_store,
            None => match &models_path {
                Some(models_path) => {
                    let path = options.models_store_path.clone().unwrap_or_else(|| {
                        std::path::Path::new(models_path)
                            .parent()
                            .unwrap_or_else(|| std::path::Path::new("."))
                            .join("models-store.json")
                            .to_string_lossy()
                            .into_owned()
                    });
                    Arc::new(FileModelsStore::new(&path).map_err(|error| error.to_string())?)
                }
                None => Arc::new(InMemoryCodingAgentModelsStore::new()),
            },
        };
        let builtin_model_data_generated_at =
            builtin_provider_catalog::get_builtin_model_data_generated_at();
        let providers: Vec<Arc<dyn Provider>> = builtin_provider_catalog::builtin_providers()
            .into_iter()
            .map(|provider| {
                // The overlay replaces `refresh_models` outright rather than
                // chaining to the provider it wraps, so a provider that sources
                // its own catalog would never run its refresh and would stay
                // empty. Those keep their own list instead of the shared one.
                if provider.id() == "radius"
                    || provider.id() == mtplx::LOCAL_PROVIDER_ID
                    || openai_compatible::OPENAI_COMPATIBLE_PROVIDER_IDS.contains(&provider.id())
                {
                    provider
                } else {
                    with_remote_catalog(
                        provider,
                        options.catalog_base_url.as_deref(),
                        builtin_model_data_generated_at,
                    )
                }
            })
            .collect();
        let runtime = ModelRuntime::new(
            credentials,
            config,
            models_path,
            models_store,
            providers,
            std::env::var_os("NOTAGENT_OFFLINE").is_none(),
        );
        runtime.configure_radius_providers();
        runtime.rebuild_providers();
        let refresh_from_network =
            runtime.model_network_enabled && options.allow_model_network == Some(true);
        let controller = (refresh_from_network && options.model_refresh_timeout_ms.is_some())
            .then(CancellationToken::new);
        let timeout = controller.as_ref().map(|controller| {
            let controller = controller.clone();
            let delay = options.model_refresh_timeout_ms.unwrap_or_default();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                controller.cancel();
            })
        });
        let signal = match (&controller, &options.signal) {
            (Some(controller), Some(caller)) => {
                let combined = CancellationToken::new();
                let target = combined.clone();
                let controller = controller.clone();
                let caller = caller.clone();
                tokio::spawn(async move {
                    tokio::select! {
                        () = controller.cancelled() => target.cancel(),
                        () = caller.cancelled() => target.cancel(),
                    }
                });
                Some(combined)
            }
            (Some(controller), None) => Some(controller.clone()),
            (None, signal) => signal.clone(),
        };
        if options.refresh_on_create != Some(false) {
            runtime
                .refresh(ModelsRefreshOptions {
                    allow_network: Some(refresh_from_network),
                    signal,
                    ..ModelsRefreshOptions::default()
                })
                .await;
        }
        if let Some(timeout) = timeout {
            timeout.abort();
        }
        Ok(runtime)
    }

    fn config(&self) -> Arc<ModelConfig> {
        Arc::clone(&self.config.lock().expect("model config"))
    }

    /// `configureRadiusProviders()`
    fn configure_radius_providers(&self) {
        let config = self.config();
        let mut builtins = self.builtins.lock().expect("builtins");
        builtins.clear();
        for (provider_id, provider) in &self.default_builtins {
            builtins.insert(provider_id.clone(), Arc::clone(provider));
        }
        for provider_id in config.get_provider_ids() {
            let Some(provider_config) = config.get_provider(provider_id) else {
                continue;
            };
            if provider_config.oauth.as_deref() != Some("radius") {
                continue;
            }
            let Some(base_url) = &provider_config.base_url else {
                continue;
            };
            let gateway = base_url
                .trim_end_matches('/')
                .strip_suffix("/v1")
                .unwrap_or_else(|| base_url.trim_end_matches("/v1/").trim_end_matches("/v1"))
                .to_owned();
            builtins.insert(
                provider_id.clone(),
                radius_provider(RadiusProviderOptions {
                    id: Some(provider_id.clone()),
                    name: Some(
                        provider_config
                            .name
                            .clone()
                            .unwrap_or_else(|| provider_id.clone()),
                    ),
                    gateway: Some(gateway),
                }),
            );
        }
    }

    /// `providerIds()`
    fn provider_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = Vec::new();
        let mut push = |id: &String| {
            if !ids.contains(id) {
                ids.push(id.clone());
            }
        };
        for id in self.builtins.lock().expect("builtins").keys() {
            push(id);
        }
        for id in self
            .native_extension_providers
            .lock()
            .expect("native providers")
            .keys()
        {
            push(id);
        }
        for id in self.config().get_provider_ids() {
            push(id);
        }
        ids
    }

    /// `recomposeProvider(providerId)`
    fn recompose_provider(&self, provider_id: &str) {
        let base = self
            .native_extension_providers
            .lock()
            .expect("native providers")
            .get(provider_id)
            .cloned()
            .or_else(|| {
                self.builtins
                    .lock()
                    .expect("builtins")
                    .get(provider_id)
                    .cloned()
            });
        let config = self.config();
        let has_config = config.get_provider(provider_id).is_some();
        if base.is_none() && !has_config {
            self.models.delete_provider(provider_id);
            self.composition_errors
                .lock()
                .expect("composition errors")
                .remove(provider_id);
            return;
        }
        if let Some(base) = &base
            && !has_config
        {
            // No overlays: use the builtin untouched so its auth/login/stream behavior is exact.
            self.models.set_provider(Arc::clone(base));
            self.composition_errors
                .lock()
                .expect("composition errors")
                .remove(provider_id);
            return;
        }
        match compose_model_provider(provider_id, base.clone(), &config) {
            Ok(provider) => {
                self.models.set_provider(provider);
                self.composition_errors
                    .lock()
                    .expect("composition errors")
                    .remove(provider_id);
            }
            Err(error) => {
                self.composition_errors
                    .lock()
                    .expect("composition errors")
                    .insert(provider_id.to_owned(), error);
                match base {
                    Some(base) => self.models.set_provider(base),
                    None => self.models.delete_provider(provider_id),
                }
            }
        }
    }

    /// `rebuildProviders()`
    fn rebuild_providers(&self) {
        self.models.clear_providers();
        self.composition_errors
            .lock()
            .expect("composition errors")
            .clear();
        for provider_id in self.provider_ids() {
            self.recompose_provider(&provider_id);
        }
        self.update_model_snapshot();
    }

    /// `updateModelSnapshot()`
    fn update_model_snapshot(&self) {
        let all = self.models.get_models(None);
        let mut snapshot = self.snapshot.lock().expect("snapshot");
        snapshot.available = all
            .iter()
            .filter(|model| snapshot.configured_providers.contains(&model.provider))
            .cloned()
            .collect();
        snapshot.all = all;
    }

    async fn run_availability_refresh(
        &self,
        seq: u64,
        error_seq: u64,
        signal: CancellationToken,
    ) -> Result<(), ModelsError> {
        let providers = self.models.get_providers();
        let options = Some(AuthOperationOptions {
            signal: Some(signal.clone()),
        });
        let available = self.models.get_available(None, options.clone()).await?;
        let mut checks: Vec<(String, Option<AuthCheck>)> = Vec::new();
        for provider in providers {
            let check = self
                .models
                .check_auth(provider.id(), options.clone())
                .await?;
            checks.push((provider.id().to_owned(), check));
        }
        let credentials = self
            .credentials
            .list(options)
            .await
            .map_err(|error| ModelsError::new(ModelsErrorCode::Auth, error.0))?;
        if seq != self.availability.lock().expect("availability").refresh_seq {
            return Ok(());
        }
        let mut auth = BTreeMap::new();
        let mut configured_providers = BTreeSet::new();
        for (provider_id, check) in checks {
            if let Some(check) = check {
                configured_providers.insert(provider_id.clone());
                auth.insert(provider_id, check);
            }
        }
        *self.snapshot.lock().expect("snapshot") = ModelRuntimeSnapshot {
            all: self.models.get_models(None),
            available,
            configured_providers,
            stored_providers: credentials
                .into_iter()
                .map(|entry| entry.provider_id)
                .collect(),
            auth,
        };
        let mut availability = self.availability.lock().expect("availability");
        if error_seq == availability.error_seq {
            availability.error = None;
        }
        Ok(())
    }

    /// `queueAvailabilityRefresh(signal?)`
    async fn queue_availability_refresh(
        &self,
        signal: Option<CancellationToken>,
    ) -> Result<(), ModelsError> {
        let (seq, error_seq) = {
            let mut availability = self.availability.lock().expect("availability");
            availability.refresh_seq += 1;
            let provider_ids: Vec<String> = availability.provider_seq.keys().cloned().collect();
            for provider_id in provider_ids {
                let entry = availability
                    .provider_seq
                    .get_mut(&provider_id)
                    .expect("provider seq");
                *entry += 1;
            }
            availability.error_seq += 1;
            (availability.refresh_seq, availability.error_seq)
        };
        let effective_signal = operation_signal(signal);
        let result = self
            .run_availability_refresh(seq, error_seq, effective_signal.clone())
            .await;
        if let Err(error) = &result {
            let mut availability = self.availability.lock().expect("availability");
            if error_seq == availability.error_seq && !effective_signal.is_cancelled() {
                availability.error = Some(error.message.clone());
            }
        }
        result
    }

    /// `refreshProviderAvailability(providerId, signal)`
    async fn refresh_provider_availability(
        &self,
        provider_id: &str,
        signal: &CancellationToken,
    ) -> Result<(), ModelsError> {
        let (provider_seq, error_seq) = {
            let mut availability = self.availability.lock().expect("availability");
            // Invalidate any full availability pass that started before this credential change.
            availability.refresh_seq += 1;
            let provider_seq = availability
                .provider_seq
                .get(provider_id)
                .copied()
                .unwrap_or(0)
                + 1;
            availability
                .provider_seq
                .insert(provider_id.to_owned(), provider_seq);
            availability.error_seq += 1;
            (provider_seq, availability.error_seq)
        };
        let result = self
            .refresh_provider_availability_inner(provider_id, signal, provider_seq, error_seq)
            .await;
        if let Err(error) = &result {
            let mut availability = self.availability.lock().expect("availability");
            if availability.provider_seq.get(provider_id) == Some(&provider_seq)
                && error_seq == availability.error_seq
                && !signal.is_cancelled()
            {
                availability.error = Some(error.message.clone());
            }
        }
        result
    }

    async fn refresh_provider_availability_inner(
        &self,
        provider_id: &str,
        signal: &CancellationToken,
        provider_seq: u64,
        error_seq: u64,
    ) -> Result<(), ModelsError> {
        let options = Some(AuthOperationOptions {
            signal: Some(signal.clone()),
        });
        let available = self
            .models
            .get_available(Some(provider_id), options.clone())
            .await?;
        let auth = self.models.check_auth(provider_id, options.clone()).await?;
        let credential = self
            .credentials
            .read(provider_id, options)
            .await
            .map_err(|error| ModelsError::new(ModelsErrorCode::Auth, error.0))?;
        if signal.is_cancelled() {
            return Err(aborted_error());
        }
        if self
            .availability
            .lock()
            .expect("availability")
            .provider_seq
            .get(provider_id)
            != Some(&provider_seq)
        {
            return Ok(());
        }
        {
            let mut snapshot = self.snapshot.lock().expect("snapshot");
            let mut configured_providers = snapshot.configured_providers.clone();
            let mut stored_providers = snapshot.stored_providers.clone();
            let mut auth_by_provider = snapshot.auth.clone();
            match auth {
                Some(auth) => {
                    configured_providers.insert(provider_id.to_owned());
                    auth_by_provider.insert(provider_id.to_owned(), auth);
                }
                None => {
                    configured_providers.remove(provider_id);
                    auth_by_provider.remove(provider_id);
                }
            }
            if credential.is_some() {
                stored_providers.insert(provider_id.to_owned());
            } else {
                stored_providers.remove(provider_id);
            }
            let all = self.models.get_models(None);
            let mut available_by_id: BTreeMap<(String, String), Model> = snapshot
                .available
                .iter()
                .filter(|model| model.provider != provider_id)
                .map(|model| ((model.provider.clone(), model.id.clone()), model.clone()))
                .collect();
            for model in available {
                available_by_id.insert((model.provider.clone(), model.id.clone()), model);
            }
            snapshot.available = all
                .iter()
                .filter_map(|model| {
                    available_by_id
                        .get(&(model.provider.clone(), model.id.clone()))
                        .cloned()
                })
                .collect();
            snapshot.all = all;
            snapshot.configured_providers = configured_providers;
            snapshot.stored_providers = stored_providers;
            snapshot.auth = auth_by_provider;
        }
        let mut availability = self.availability.lock().expect("availability");
        if error_seq == availability.error_seq {
            availability.error = None;
        }
        Ok(())
    }

    // -- Models facade ------------------------------------------------------

    pub fn get_providers(&self) -> Vec<Arc<dyn Provider>> {
        self.models.get_providers()
    }

    pub fn get_provider(&self, provider_id: &str) -> Option<Arc<dyn Provider>> {
        self.models.get_provider(provider_id)
    }

    pub fn get_models(&self, provider_id: Option<&str>) -> Vec<Model> {
        self.models.get_models(provider_id)
    }

    pub fn get_model(&self, provider_id: &str, model_id: &str) -> Option<Model> {
        self.models.get_model(provider_id, model_id)
    }

    pub async fn check_auth(
        &self,
        provider_id: &str,
        options: Option<AuthOperationOptions>,
    ) -> Result<Option<AuthCheck>, ModelsError> {
        self.models.check_auth(provider_id, options).await
    }

    /// `getAvailable(providerId?, options?)`
    pub async fn get_available(
        &self,
        provider_id: Option<&str>,
        options: Option<AuthOperationOptions>,
    ) -> Result<Vec<Model>, ModelsError> {
        if let Some(provider_id) = provider_id {
            let error_seq = {
                let mut availability = self.availability.lock().expect("availability");
                availability.error_seq += 1;
                availability.error_seq
            };
            let signal = options.as_ref().and_then(|options| options.signal.clone());
            return match self.models.get_available(Some(provider_id), options).await {
                Ok(available) => {
                    let mut availability = self.availability.lock().expect("availability");
                    if error_seq == availability.error_seq {
                        availability.error = None;
                    }
                    Ok(available)
                }
                Err(error) => {
                    let mut availability = self.availability.lock().expect("availability");
                    if error_seq == availability.error_seq
                        && !signal.is_some_and(|signal| signal.is_cancelled())
                    {
                        availability.error = Some(error.message.clone());
                    }
                    Err(error)
                }
            };
        }
        self.queue_availability_refresh(options.and_then(|options| options.signal))
            .await?;
        Ok(self.get_available_snapshot())
    }

    pub fn get_available_snapshot(&self) -> Vec<Model> {
        self.snapshot.lock().expect("snapshot").available.clone()
    }

    /// `getError()`
    pub fn get_error(&self) -> Option<String> {
        let mut errors: Vec<String> = Vec::new();
        if let Some(config_error) = self.config().get_error() {
            errors.push(config_error.to_owned());
        }
        for (provider_id, error) in self
            .composition_errors
            .lock()
            .expect("composition errors")
            .iter()
        {
            errors.push(format!("Provider \"{provider_id}\": {error}"));
        }
        if let Some(error) = &self.availability.lock().expect("availability").error {
            errors.push(format!("Availability refresh: {error}"));
        }
        (!errors.is_empty()).then(|| errors.join("\n\n"))
    }

    pub fn get_registered_provider_ids(&self) -> Vec<String> {
        self.native_extension_providers
            .lock()
            .expect("native providers")
            .keys()
            .cloned()
            .collect()
    }

    pub fn get_registered_native_provider(&self, provider_id: &str) -> Option<Arc<dyn Provider>> {
        self.native_extension_providers
            .lock()
            .expect("native providers")
            .get(provider_id)
            .cloned()
    }

    /// Compatibility fallback for `ModelRegistry` when provider auth is unconfigured.
    pub fn get_compatibility_request_config(
        &self,
        model: &Model,
    ) -> Result<CompatibilityRequestConfig, String> {
        resolve_compatibility_request_config(model, self.config().get_provider(&model.provider))
    }

    pub fn is_using_oauth(&self, provider_id: &str) -> bool {
        self.snapshot
            .lock()
            .expect("snapshot")
            .auth
            .get(provider_id)
            .is_some_and(|check| check.check_type == AuthType::OAuth)
    }

    /// Whether the tokens spent on `provider_id` are covered by a plan rather
    /// than billed per token. Callers use it to keep a price off the screen
    /// that nobody owes.
    pub fn is_using_subscription(&self, provider_id: &str) -> bool {
        if is_subscription_api_key_provider(provider_id) {
            return true;
        }
        self.is_using_oauth(provider_id)
            && self
                .models
                .get_provider(provider_id)
                .and_then(|provider| provider.auth().oauth.clone())
                .is_some_and(|oauth| oauth.is_subscription())
    }

    pub fn has_configured_auth(&self, provider_id: &str) -> bool {
        self.snapshot
            .lock()
            .expect("snapshot")
            .configured_providers
            .contains(provider_id)
    }

    /// `getAuth(providerId, overrides?)`
    pub async fn get_auth_for_provider(
        &self,
        provider_id: &str,
        overrides: Option<&ModelRuntimeAuthOverrides>,
    ) -> Result<Option<AuthResult>, ModelsError> {
        let overrides = overrides.cloned().unwrap_or_default().as_resolution();
        self.models
            .get_auth_for_provider(provider_id, Some(&overrides))
            .await
    }

    /// `getAuth(model, overrides?)` — provider auth plus configured model headers.
    pub async fn get_auth_for_model(
        &self,
        model: &Model,
        overrides: Option<&ModelRuntimeAuthOverrides>,
    ) -> Result<Option<AuthResult>, ModelsError> {
        let overrides = overrides.cloned().unwrap_or_default();
        let resolution = self
            .models
            .get_auth_for_model(model, Some(&overrides.as_resolution()))
            .await?;
        let Some(resolution) = resolution else {
            return Ok(None);
        };
        let mut env: BTreeMap<String, String> = resolution.env.clone().unwrap_or_default();
        for (name, value) in overrides.env.clone().unwrap_or_default() {
            env.insert(name, value);
        }
        let configured_headers = resolve_configured_model_headers(
            model,
            self.config().get_provider(&model.provider),
            Some(&env),
        )
        .map_err(|error| ModelsError::new(ModelsErrorCode::Auth, error))?;
        let configured_headers: Option<ProviderHeaders> = configured_headers.map(|headers| {
            headers
                .into_iter()
                .map(|(name, value)| (name, Some(value)))
                .collect()
        });
        Ok(Some(AuthResult {
            auth: ModelAuth {
                headers: merge_headers(
                    resolution.auth.headers.as_ref(),
                    configured_headers.as_ref(),
                ),
                ..resolution.auth
            },
            ..resolution
        }))
    }

    fn credential_lock(&self, provider_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut operations = self.credential_operations.lock().expect("credential ops");
        Arc::clone(
            operations
                .entry(provider_id.to_owned())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))),
        )
    }

    /// `synchronizeCredentialState(providerId, operation, credential, signal)`
    async fn synchronize_credential_state(
        &self,
        provider_id: &str,
        operation: CredentialSynchronizationOperation,
        credential: Option<Credential>,
        signal: &CancellationToken,
    ) -> Result<(), ModelRuntimeError> {
        let failure = |cause: String| {
            ModelRuntimeError::Synchronization(Box::new(CredentialSynchronizationError {
                provider_id: provider_id.to_owned(),
                operation,
                credential: credential.clone(),
                cause,
            }))
        };
        if signal.is_cancelled() {
            return Err(failure("The operation was aborted".to_owned()));
        }
        self.recompose_provider(provider_id);
        if let Some(error) = self
            .composition_errors
            .lock()
            .expect("composition errors")
            .get(provider_id)
            .cloned()
        {
            return Err(failure(error));
        }
        let result = self
            .models
            .refresh(Some(ModelsRefreshOptions {
                allow_network: Some(false),
                providers: Some(vec![provider_id.to_owned()]),
                force: None,
                signal: Some(signal.clone()),
            }))
            .await;
        if result.aborted && signal.is_cancelled() {
            return Err(failure("The operation was aborted".to_owned()));
        }
        if let Some(error) = result.errors.get(provider_id) {
            return Err(failure(error.message.clone()));
        }
        self.update_model_snapshot();
        self.refresh_provider_availability(provider_id, signal)
            .await
            .map_err(|error| failure(error.message))
    }

    /// `setRuntimeApiKey(providerId, apiKey, options?)`
    pub async fn set_runtime_api_key(
        &self,
        provider_id: &str,
        api_key: &str,
        options: Option<AuthOperationOptions>,
    ) -> Result<(), ModelRuntimeError> {
        let signal = operation_signal(options.and_then(|options| options.signal));
        let lock = self.credential_lock(provider_id);
        let _guard = lock.lock().await;
        if signal.is_cancelled() {
            return Err(ModelRuntimeError::Models(aborted_error()));
        }
        self.credentials.set_runtime_api_key(provider_id, api_key);
        self.synchronize_credential_state(
            provider_id,
            CredentialSynchronizationOperation::SetRuntimeApiKey,
            Some(Credential::ApiKey(
                notagent_ai::auth::types::ApiKeyCredential {
                    key: Some(api_key.to_owned()),
                    env: None,
                },
            )),
            &signal,
        )
        .await
    }

    /// `removeRuntimeApiKey(providerId, options?)`
    pub async fn remove_runtime_api_key(
        &self,
        provider_id: &str,
        options: Option<AuthOperationOptions>,
    ) -> Result<(), ModelRuntimeError> {
        let signal = operation_signal(options.and_then(|options| options.signal));
        let lock = self.credential_lock(provider_id);
        let _guard = lock.lock().await;
        if signal.is_cancelled() {
            return Err(ModelRuntimeError::Models(aborted_error()));
        }
        self.credentials.remove_runtime_api_key(provider_id);
        self.synchronize_credential_state(
            provider_id,
            CredentialSynchronizationOperation::RemoveRuntimeApiKey,
            None,
            &signal,
        )
        .await
    }

    pub async fn list_credentials(
        &self,
        options: Option<AuthOperationOptions>,
    ) -> Result<Vec<CredentialInfo>, ModelsError> {
        self.credentials
            .list(options)
            .await
            .map_err(|error| ModelsError::new(ModelsErrorCode::Auth, error.0))
    }

    /// `getProviderAuthStatus(providerId)`
    pub fn get_provider_auth_status(&self, provider_id: &str) -> AuthStatus {
        if self.credentials.has_runtime_api_key(provider_id) {
            return AuthStatus::configured(AuthStatusSource::Runtime);
        }
        if self
            .snapshot
            .lock()
            .expect("snapshot")
            .stored_providers
            .contains(provider_id)
        {
            return AuthStatus::configured(AuthStatusSource::Stored);
        }
        if let Some(configured) =
            configured_request_auth_status(self.config().get_provider(provider_id))
        {
            return configured;
        }
        match self
            .snapshot
            .lock()
            .expect("snapshot")
            .auth
            .get(provider_id)
        {
            Some(check) => AuthStatus {
                configured: true,
                source: Some(AuthStatusSource::Environment),
                label: check.source.clone(),
            },
            None => AuthStatus::unconfigured(),
        }
    }

    /// `prepareRequest(model, options)`
    async fn prepare_request(
        &self,
        model: &Model,
        base: &notagent_ai::types::ProviderRequestOptions<Model>,
        transforms: &ModelsRequestTransforms,
    ) -> Result<PreparedRequest, ModelsError> {
        let provider = self.models.get_provider(&model.provider).ok_or_else(|| {
            ModelsError::new(
                ModelsErrorCode::Provider,
                format!("Unknown provider: {}", model.provider),
            )
        })?;
        let resolution = self
            .get_auth_for_model(
                model,
                Some(&ModelRuntimeAuthOverrides {
                    api_key: base.api_key.clone(),
                    env: base.env.clone(),
                    min_oauth_validity_ms: None,
                    signal: base.signal.clone(),
                }),
            )
            .await?
            .ok_or_else(|| {
                ModelsError::new(
                    ModelsErrorCode::Auth,
                    format!("Provider is not configured: {}", model.provider),
                )
            })?;
        let mut headers = merge_headers(resolution.auth.headers.as_ref(), base.headers.as_ref());
        if let Some(transform) = &transforms.transform_headers {
            headers = Some(transform(headers.unwrap_or_default()).await);
        }
        let env = match (&resolution.env, &base.env) {
            (None, None) => None,
            _ => {
                let mut env: ProviderEnv = resolution.env.clone().unwrap_or_default();
                for (name, value) in base.env.clone().unwrap_or_default() {
                    env.insert(name, value);
                }
                Some(env)
            }
        };
        let mut model = model.clone();
        if let Some(base_url) = &resolution.auth.base_url {
            model.base_url = base_url.clone();
        }
        Ok(PreparedRequest {
            provider,
            model,
            api_key: base.api_key.clone().or(resolution.auth.api_key),
            headers,
            env,
        })
    }

    /// `stream(model, context, options?)`
    pub fn stream(
        self: &Arc<Self>,
        model: &Model,
        context: &Context,
        options: Option<ModelsApiStreamOptions>,
    ) -> AssistantMessageEventStream {
        let runtime = Arc::clone(self);
        let model = model.clone();
        let context = context.clone();
        let setup_model = model.clone();
        lazy_stream(setup_model, move || async move {
            let ModelsApiStreamOptions {
                mut options,
                transforms,
            } = options.unwrap_or_default();
            let prepared = runtime
                .prepare_request(&model, &options.base, &transforms)
                .await
                .map_err(|error| error.message)?;
            prepared.apply(&mut options.base);
            Ok(prepared
                .provider
                .stream(&prepared.model, &context, Some(options)))
        })
    }

    pub async fn complete(
        self: &Arc<Self>,
        model: &Model,
        context: &Context,
        options: Option<ModelsApiStreamOptions>,
    ) -> AssistantMessage {
        self.stream(model, context, options).result().await
    }

    /// `streamSimple(model, context, options?)`
    pub fn stream_simple(
        self: &Arc<Self>,
        model: &Model,
        context: &Context,
        options: Option<ModelsSimpleStreamOptions>,
    ) -> AssistantMessageEventStream {
        let runtime = Arc::clone(self);
        let model = model.clone();
        let context = context.clone();
        let setup_model = model.clone();
        lazy_stream(setup_model, move || async move {
            let ModelsSimpleStreamOptions {
                mut options,
                transforms,
            } = options.unwrap_or_default();
            let prepared = runtime
                .prepare_request(&model, &options.base.base, &transforms)
                .await
                .map_err(|error| error.message)?;
            prepared.apply(&mut options.base.base);
            Ok(prepared
                .provider
                .stream_simple(&prepared.model, &context, Some(options)))
        })
    }

    pub async fn complete_simple(
        self: &Arc<Self>,
        model: &Model,
        context: &Context,
        options: Option<ModelsSimpleStreamOptions>,
    ) -> AssistantMessage {
        self.stream_simple(model, context, options).result().await
    }

    /// `fetchDeferred(model, handle, options?)`
    pub async fn fetch_deferred(
        self: &Arc<Self>,
        model: &Model,
        handle: &DeferredHandle,
        options: Option<ModelsDeferredFetchOptions>,
    ) -> Result<AssistantMessage, ModelsError> {
        let ModelsDeferredFetchOptions {
            mut options,
            transforms,
        } = options.unwrap_or_default();
        let prepared = self
            .prepare_request(model, &options.base, &transforms)
            .await?;
        prepared.apply(&mut options.base);
        let stream = prepared
            .provider
            .fetch_deferred(&prepared.model, handle, Some(options))
            .ok_or_else(|| {
                ModelsError::new(
                    ModelsErrorCode::Provider,
                    format!(
                        "Provider {} does not support deferred responses",
                        model.provider
                    ),
                )
            })?;
        Ok(stream.result().await)
    }

    /// `cancelDeferred(model, handle, options?)`
    pub async fn cancel_deferred(
        self: &Arc<Self>,
        model: &Model,
        handle: &DeferredHandle,
        options: Option<ModelsDeferredCancelOptions>,
    ) -> Result<(), ModelsError> {
        let ModelsDeferredCancelOptions {
            mut options,
            transforms,
        } = options.unwrap_or_default();
        let prepared = self.prepare_request(model, &options, &transforms).await?;
        prepared.apply(&mut options);
        let provider = Arc::clone(&prepared.provider);
        let model = prepared.model.clone();
        let cancel = provider
            .cancel_deferred(&model, handle, Some(options))
            .ok_or_else(|| {
                ModelsError::new(
                    ModelsErrorCode::Provider,
                    format!(
                        "Provider {} does not support deferred responses",
                        model.provider
                    ),
                )
            })?;
        cancel.await
    }

    /// `login(providerId, type, interaction)`
    pub async fn login(
        &self,
        provider_id: &str,
        auth_type: AuthType,
        interaction: &dyn AuthInteraction,
    ) -> Result<Credential, ModelRuntimeError> {
        let signal = operation_signal(interaction.signal());
        let lock = self.credential_lock(provider_id);
        let _guard = lock.lock().await;
        if signal.is_cancelled() {
            return Err(ModelRuntimeError::Models(aborted_error()));
        }
        let credential = self
            .models
            .login(provider_id, auth_type, interaction)
            .await?;
        self.synchronize_credential_state(
            provider_id,
            CredentialSynchronizationOperation::Login,
            Some(credential.clone()),
            &signal,
        )
        .await?;
        Ok(credential)
    }

    /// `logout(providerId, options?)`
    pub async fn logout(
        &self,
        provider_id: &str,
        options: Option<AuthOperationOptions>,
    ) -> Result<(), ModelRuntimeError> {
        let signal = operation_signal(options.and_then(|options| options.signal));
        let lock = self.credential_lock(provider_id);
        let _guard = lock.lock().await;
        if signal.is_cancelled() {
            return Err(ModelRuntimeError::Models(aborted_error()));
        }
        self.models
            .logout(
                provider_id,
                Some(AuthOperationOptions {
                    signal: Some(signal.clone()),
                }),
            )
            .await?;
        self.synchronize_credential_state(
            provider_id,
            CredentialSynchronizationOperation::Logout,
            None,
            &signal,
        )
        .await
    }

    /// `refresh(options?)`
    pub async fn refresh(&self, options: ModelsRefreshOptions) -> ModelsRefreshResult {
        *self.config.lock().expect("model config") =
            Arc::new(ModelConfig::load(self.models_path.as_deref()).await);
        self.configure_radius_providers();
        match &options.providers {
            Some(providers) => {
                let mut seen: Vec<String> = Vec::new();
                for provider_id in providers {
                    if seen.contains(provider_id) {
                        continue;
                    }
                    seen.push(provider_id.clone());
                    self.recompose_provider(provider_id);
                }
                self.update_model_snapshot();
            }
            None => self.rebuild_providers(),
        }
        let refresh_options = ModelsRefreshOptions {
            allow_network: Some(options.allow_network.unwrap_or(self.model_network_enabled)),
            ..options.clone()
        };
        let result = self.models.refresh(Some(refresh_options)).await;
        let mut errors = result.errors;
        self.update_model_snapshot();
        match &options.providers {
            Some(providers) => {
                let mut seen: Vec<String> = Vec::new();
                for provider_id in providers {
                    if seen.contains(provider_id) {
                        continue;
                    }
                    seen.push(provider_id.clone());
                    let signal = operation_signal(options.signal.clone());
                    if let Err(error) = self
                        .refresh_provider_availability(provider_id, &signal)
                        .await
                        && !options
                            .signal
                            .as_ref()
                            .is_some_and(CancellationToken::is_cancelled)
                    {
                        errors.insert(provider_id.clone(), error);
                    }
                }
            }
            None => {
                // Availability errors are recorded by the latest pass; refreshed models remain usable.
                let _ = self
                    .queue_availability_refresh(options.signal.clone())
                    .await;
            }
        }
        ModelsRefreshResult {
            aborted: result.aborted
                || options
                    .signal
                    .as_ref()
                    .is_some_and(CancellationToken::is_cancelled),
            errors,
        }
    }

    /// `registerNativeProvider(provider)`
    pub fn register_native_provider(
        self: &Arc<Self>,
        provider: Arc<dyn Provider>,
    ) -> Result<(), String> {
        if provider.id().trim().is_empty() {
            return Err("Provider id must not be empty.".to_owned());
        }
        let provider_id = provider.id().to_owned();
        self.native_extension_providers
            .lock()
            .expect("native providers")
            .insert(provider_id.clone(), provider);
        self.recompose_provider(&provider_id);
        self.update_model_snapshot();
        self.spawn_offline_refresh();
        Ok(())
    }

    /// `unregisterProvider(providerId)`
    pub fn unregister_provider(self: &Arc<Self>, provider_id: &str) {
        self.native_extension_providers
            .lock()
            .expect("native providers")
            .remove(provider_id);
        self.recompose_provider(provider_id);
        self.update_model_snapshot();
        self.spawn_offline_refresh();
    }

    fn spawn_offline_refresh(self: &Arc<Self>) {
        let runtime = Arc::clone(self);
        tokio::spawn(async move {
            runtime
                .refresh(ModelsRefreshOptions {
                    allow_network: Some(false),
                    ..ModelsRefreshOptions::default()
                })
                .await;
        });
    }
}

/// `transformHeaders` of `ModelsRequestTransforms`.
pub type TransformHeaders = Arc<
    dyn Fn(ProviderHeaders) -> futures::future::BoxFuture<'static, ProviderHeaders> + Send + Sync,
>;

/// `ModelsRequestTransforms`
#[derive(Clone, Default)]
pub struct ModelsRequestTransforms {
    /// Transform fully assembled model/auth/request headers before provider dispatch.
    pub transform_headers: Option<TransformHeaders>,
}

/// `ModelsApiStreamOptions<TApi> = ApiStreamOptions<TApi> & ModelsRequestTransforms`
/// no structural intersection, so it becomes a field next to the provider options.
#[derive(Clone, Default)]
pub struct ModelsApiStreamOptions {
    pub options: StreamOptions,
    pub transforms: ModelsRequestTransforms,
}

/// `ModelsSimpleStreamOptions = SimpleStreamOptions & ModelsRequestTransforms`
#[derive(Clone, Default)]
pub struct ModelsSimpleStreamOptions {
    pub options: SimpleStreamOptions,
    pub transforms: ModelsRequestTransforms,
}

/// `ModelsDeferredFetchOptions = DeferredFetchOptions & ModelsRequestTransforms`
#[derive(Clone, Default)]
pub struct ModelsDeferredFetchOptions {
    pub options: DeferredFetchOptions,
    pub transforms: ModelsRequestTransforms,
}

/// `ModelsDeferredCancelOptions = DeferredCancelOptions & ModelsRequestTransforms`
#[derive(Clone, Default)]
pub struct ModelsDeferredCancelOptions {
    pub options: DeferredCancelOptions,
    pub transforms: ModelsRequestTransforms,
}

struct PreparedRequest {
    provider: Arc<dyn Provider>,
    model: Model,
    api_key: Option<String>,
    headers: Option<ProviderHeaders>,
    env: Option<ProviderEnv>,
}

impl PreparedRequest {
    fn apply(&self, base: &mut notagent_ai::types::ProviderRequestOptions<Model>) {
        base.api_key = self.api_key.clone();
        base.headers = self.headers.clone();
        base.env = self.env.clone();
    }
}
