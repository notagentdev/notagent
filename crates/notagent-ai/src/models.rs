use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;

use crate::api::lazy::lazy_stream;
use crate::auth::context::default_provider_auth_context;
use crate::auth::credential_store::InMemoryCredentialStore;
use crate::auth::resolve::{
    AuthProvider, AuthResolutionOverrides, ModelsError, ModelsErrorCode, resolve_provider_auth,
};
use crate::auth::types::{
    AuthCheck, AuthContext, AuthOperationOptions, AuthType, BoxFuture, Credential, CredentialStore,
    ModelAuth, ProviderAuth,
};
use crate::models_store::{
    InMemoryModelsStore, ModelsStore, ModelsStoreEntry, ModelsStoreOperationOptions,
};
use crate::types::{
    Context, DeferredCancelOptions, DeferredFetchOptions, DeferredHandle, Model, ModelCostRates,
    ModelThinkingLevel, ProviderHeaders, SimpleStreamOptions, StreamOptions, Usage, UsageCost,
};
use crate::utils::event_stream::AssistantMessageEventStream;

/// `ModelsPublication`
/// The update closure may borrow from the provider, so the publication carries a
#[derive(Default)]
pub struct ModelsPublication<'a> {
    /// Provider-selected persisted catalog; `None` leaves storage unchanged,
    /// `Some(None)` deletes it.
    pub persist: Option<Option<ModelsStoreEntry>>,
    /// Synchronous update of provider-private in-memory catalog state.
    pub update: Option<Box<dyn FnOnce() + Send + 'a>>,
}

/// `RefreshModelsContext`
pub struct RefreshModelsContext<'a> {
    /// Effective configured credential; OAuth credentials are refreshed beforehand.
    pub credential: Option<Credential>,
    /// Immutable snapshot captured before this refresh phase.
    pub stored: Option<ModelsStoreEntry>,
    /// Generation-checked publication.
    pub publish: Box<dyn Fn(ModelsPublication<'a>) -> BoxFuture<'a, bool> + Send + Sync + 'a>,
    /// False during offline/cache-only initialization.
    pub allow_network: bool,
    /// Bypass provider freshness checks when network access is allowed.
    pub force: Option<bool>,
    /// Always present, even when the public caller omits its signal.
    pub signal: CancellationToken,
}

/// `ModelsRefreshOptions`
#[derive(Debug, Clone, Default)]
pub struct ModelsRefreshOptions {
    pub allow_network: Option<bool>,
    /// Restrict the refresh to these provider ids.
    pub providers: Option<Vec<String>>,
    pub force: Option<bool>,
    pub signal: Option<CancellationToken>,
}

/// `ModelsRefreshResult`
#[derive(Debug, Default)]
pub struct ModelsRefreshResult {
    pub aborted: bool,
    pub errors: BTreeMap<String, ModelsError>,
}

/// `Provider` — the concrete runtime unit.
pub trait Provider: Send + Sync {
    fn id(&self) -> &str;
    fn name(&self) -> &str;
    fn base_url(&self) -> Option<&str> {
        None
    }
    fn headers(&self) -> Option<&ProviderHeaders> {
        None
    }
    /// At least one of `api_key`/`oauth` is present.
    fn auth(&self) -> &ProviderAuth;

    /// Current known models, synchronous. Must not fail; a failing implementation is
    /// treated as having no models.
    fn get_models(&self) -> Vec<Model>;

    /// `provider.refreshModels !== undefined`, which Rust cannot probe on a trait).
    fn is_dynamic(&self) -> bool {
        false
    }

    /// Dynamic providers only: restore `context.stored` and optionally fetch a newer
    /// list, retaining the previous list on failure.
    fn refresh_models<'a>(
        &'a self,
        _context: RefreshModelsContext<'a>,
    ) -> Option<BoxFuture<'a, Result<(), String>>> {
        None
    }

    /// Optional provider policy for credential-specific availability.
    fn filter_models(&self, models: Vec<Model>, _credential: Option<&Credential>) -> Vec<Model> {
        models
    }

    fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: Option<StreamOptions>,
    ) -> AssistantMessageEventStream;

    fn stream_simple(
        &self,
        model: &Model,
        context: &Context,
        options: Option<SimpleStreamOptions>,
    ) -> AssistantMessageEventStream;

    fn fetch_deferred(
        &self,
        _model: &Model,
        _handle: &DeferredHandle,
        _options: Option<DeferredFetchOptions>,
    ) -> Option<AssistantMessageEventStream> {
        None
    }

    fn cancel_deferred<'a>(
        &'a self,
        _model: &'a Model,
        _handle: &'a DeferredHandle,
        _options: Option<DeferredCancelOptions>,
    ) -> Option<BoxFuture<'a, Result<(), ModelsError>>> {
        None
    }
}

/// `mergeHeaders(base, override)` — case-insensitive replacement, override wins.
pub fn merge_headers(
    base: Option<&ProviderHeaders>,
    override_headers: Option<&ProviderHeaders>,
) -> Option<ProviderHeaders> {
    if base.is_none() && override_headers.is_none() {
        return None;
    }
    let mut merged = base.cloned().unwrap_or_default();
    for (name, value) in override_headers.cloned().unwrap_or_default() {
        let lower_name = name.to_lowercase();
        let existing: Vec<String> = merged
            .keys()
            .filter(|existing| existing.to_lowercase() == lower_name)
            .cloned()
            .collect();
        for existing_name in existing {
            merged.remove(&existing_name);
        }
        merged.insert(name, value);
    }
    Some(merged)
}

/// `CreateModelsOptions`
#[derive(Clone, Default)]
pub struct CreateModelsOptions {
    pub credentials: Option<Arc<dyn CredentialStore>>,
    pub models_store: Option<Arc<dyn ModelsStore>>,
    pub auth_context: Option<Arc<dyn AuthContext>>,
}

#[derive(Default)]
struct RefreshState {
    generations: BTreeMap<String, u64>,
    controllers: BTreeMap<String, CancellationToken>,
}

/// `ModelsImpl` — runtime collection of providers plus auth application.
pub struct Models {
    providers: Mutex<Vec<Arc<dyn Provider>>>,
    credentials: Arc<dyn CredentialStore>,
    models_store: Arc<dyn ModelsStore>,
    auth_context: Arc<dyn AuthContext>,
    refresh: Mutex<RefreshState>,
    publication_chains: Mutex<BTreeMap<String, Arc<AsyncMutex<()>>>>,
}

/// `createModels(options?)`
pub fn create_models(options: Option<CreateModelsOptions>) -> Arc<Models> {
    let options = options.unwrap_or_default();
    Arc::new(Models {
        providers: Mutex::new(Vec::new()),
        credentials: options
            .credentials
            .unwrap_or_else(|| Arc::new(InMemoryCredentialStore::new())),
        models_store: options
            .models_store
            .unwrap_or_else(|| Arc::new(InMemoryModelsStore::new())),
        auth_context: options
            .auth_context
            .unwrap_or_else(default_provider_auth_context),
        refresh: Mutex::new(RefreshState::default()),
        publication_chains: Mutex::new(BTreeMap::new()),
    })
}

impl Models {
    // -- MutableModels ------------------------------------------------------

    /// `setProvider(provider)` — upsert by id.
    pub fn set_provider(&self, provider: Arc<dyn Provider>) {
        self.supersede_provider_refresh(provider.id());
        let mut providers = self.providers.lock().expect("providers poisoned");
        match providers
            .iter_mut()
            .find(|existing| existing.id() == provider.id())
        {
            Some(existing) => *existing = provider,
            None => providers.push(provider),
        }
    }

    /// `deleteProvider(id)`
    pub fn delete_provider(&self, id: &str) {
        self.supersede_provider_refresh(id);
        self.providers
            .lock()
            .expect("providers poisoned")
            .retain(|provider| provider.id() != id);
    }

    /// `clearProviders()`
    pub fn clear_providers(&self) {
        let ids: Vec<String> = {
            let providers = self.providers.lock().expect("providers poisoned");
            let refresh = self.refresh.lock().expect("refresh state poisoned");
            providers
                .iter()
                .map(|provider| provider.id().to_string())
                .chain(refresh.controllers.keys().cloned())
                .collect()
        };
        for id in ids {
            self.supersede_provider_refresh(&id);
        }
        self.providers.lock().expect("providers poisoned").clear();
    }

    /// `getProviders()`
    pub fn get_providers(&self) -> Vec<Arc<dyn Provider>> {
        self.providers.lock().expect("providers poisoned").clone()
    }

    /// `getProvider(id)`
    pub fn get_provider(&self, id: &str) -> Option<Arc<dyn Provider>> {
        self.providers
            .lock()
            .expect("providers poisoned")
            .iter()
            .find(|provider| provider.id() == id)
            .cloned()
    }

    /// `getModels(provider?)`
    pub fn get_models(&self, provider: Option<&str>) -> Vec<Model> {
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

    /// `getModel(provider, id)`
    pub fn get_model(&self, provider: &str, id: &str) -> Option<Model> {
        self.get_models(Some(provider))
            .into_iter()
            .find(|model| model.id == id)
    }

    // -- refresh ------------------------------------------------------------

    fn supersede_provider_refresh(&self, provider_id: &str) -> u64 {
        let mut refresh = self.refresh.lock().expect("refresh state poisoned");
        let generation = refresh.generations.get(provider_id).copied().unwrap_or(0) + 1;
        refresh
            .generations
            .insert(provider_id.to_string(), generation);
        if let Some(previous) = refresh.controllers.remove(provider_id) {
            previous.cancel();
        }
        generation
    }

    fn begin_provider_refresh(&self, provider_id: &str) -> (u64, CancellationToken) {
        let generation = self.supersede_provider_refresh(provider_id);
        let controller = CancellationToken::new();
        self.refresh
            .lock()
            .expect("refresh state poisoned")
            .controllers
            .insert(provider_id.to_string(), controller.clone());
        (generation, controller)
    }

    fn current_generation(&self, provider_id: &str) -> u64 {
        self.refresh
            .lock()
            .expect("refresh state poisoned")
            .generations
            .get(provider_id)
            .copied()
            .unwrap_or(0)
    }

    fn publication_chain(&self, provider_id: &str) -> Arc<AsyncMutex<()>> {
        let mut chains = self
            .publication_chains
            .lock()
            .expect("publication chains poisoned");
        Arc::clone(chains.entry(provider_id.to_string()).or_default())
    }

    /// `publishProviderModels(providerId, generation, signal, publication)`
    async fn publish_provider_models(
        &self,
        provider_id: &str,
        generation: u64,
        signal: &CancellationToken,
        publication: ModelsPublication<'_>,
    ) -> bool {
        let chain = self.publication_chain(provider_id);
        let _guard = chain.lock().await;
        if signal.is_cancelled() || self.current_generation(provider_id) != generation {
            return false;
        }

        match publication.persist {
            Some(None) => {
                let _ = self
                    .models_store
                    .delete(
                        provider_id,
                        Some(ModelsStoreOperationOptions {
                            signal: Some(signal.clone()),
                        }),
                    )
                    .await;
            }
            Some(Some(entry)) => {
                let _ = self
                    .models_store
                    .write(
                        provider_id,
                        entry,
                        Some(ModelsStoreOperationOptions {
                            signal: Some(signal.clone()),
                        }),
                    )
                    .await;
            }
            None => {}
        }

        if signal.is_cancelled() || self.current_generation(provider_id) != generation {
            return false;
        }
        if let Some(update) = publication.update {
            update();
        }
        true
    }

    /// `refresh(options?)`
    pub async fn refresh(&self, options: Option<ModelsRefreshOptions>) -> ModelsRefreshResult {
        let options = options.unwrap_or_default();
        let allow_network = options.allow_network.unwrap_or(true);
        let caller_signal = options.signal.clone().unwrap_or_default();
        let mut errors = BTreeMap::new();
        if caller_signal.is_cancelled() {
            return ModelsRefreshResult {
                aborted: true,
                errors,
            };
        }

        let refreshable: Vec<Arc<dyn Provider>> = self
            .get_providers()
            .into_iter()
            .filter(|provider| {
                let selected = options
                    .providers
                    .as_ref()
                    .is_none_or(|selected| selected.iter().any(|id| id == provider.id()));
                selected && provider.is_dynamic()
            })
            .collect();

        // `Promise.all(refreshable.map(...))`: the providers refresh concurrently, and
        // each operation is raced against its own signal so that an implementation
        // ignoring the signal cannot hold the caller.
        let operations = refreshable.into_iter().map(|provider| {
            let caller_signal = caller_signal.clone();
            let force = options.force;
            async move {
                let (generation, controller) = self.begin_provider_refresh(provider.id());
                let signal = child_of(&[&caller_signal, &controller]);
                let outcome = crate::utils::abort::race_with_abort_signal(
                    self.refresh_provider(
                        provider.as_ref(),
                        generation,
                        &signal,
                        allow_network,
                        force,
                    ),
                    &signal,
                )
                .await;
                let error = match outcome {
                    Ok(Err(error)) if !signal.is_cancelled() => Some(error),
                    _ => None,
                };
                {
                    let mut refresh = self.refresh.lock().expect("refresh state poisoned");
                    if refresh
                        .controllers
                        .get(provider.id())
                        .is_some_and(|current| current.is_cancelled() == controller.is_cancelled())
                    {
                        refresh.controllers.remove(provider.id());
                    }
                }
                (provider.id().to_string(), error)
            }
        });
        if let Ok(results) = crate::utils::abort::race_with_abort_signal(
            futures::future::join_all(operations),
            &caller_signal,
        )
        .await
        {
            for (provider_id, error) in results {
                if let Some(error) = error {
                    errors.insert(provider_id, error);
                }
            }
        }

        ModelsRefreshResult {
            aborted: caller_signal.is_cancelled(),
            errors,
        }
    }

    async fn refresh_provider(
        &self,
        provider: &dyn Provider,
        generation: u64,
        signal: &CancellationToken,
        allow_network: bool,
        force: Option<bool>,
    ) -> Result<(), ModelsError> {
        let stored_credential = self.read_credential(provider.id(), signal).await;

        // Restore cached provider state before auth resolution or network access.
        self.run_provider_refresh_phase(
            provider,
            stored_credential.as_ref().ok().and_then(Clone::clone),
            false,
            None,
            generation,
            signal,
        )
        .await?;
        let stored_credential = stored_credential?;
        if !allow_network || signal.is_cancelled() {
            return Ok(());
        }

        let Some(credential) = self
            .resolve_refresh_credential(provider, stored_credential, signal)
            .await?
        else {
            return Ok(());
        };
        self.run_provider_refresh_phase(provider, Some(credential), true, force, generation, signal)
            .await
    }

    /// `runProviderRefreshPhase(...)`
    async fn run_provider_refresh_phase(
        &self,
        provider: &dyn Provider,
        credential: Option<Credential>,
        allow_network: bool,
        force: Option<bool>,
        generation: u64,
        signal: &CancellationToken,
    ) -> Result<(), ModelsError> {
        let stored = self
            .models_store
            .read(
                provider.id(),
                Some(ModelsStoreOperationOptions {
                    signal: Some(signal.clone()),
                }),
            )
            .await
            .map_err(|error| {
                ModelsError::with_cause(
                    ModelsErrorCode::ModelSource,
                    format!("Model store read failed for {}", provider.id()),
                    &error,
                )
            })?;

        // The closure is used only while this call awaits `refresh_models`, so the
        // publication borrow and the returned future share that scope.
        fn publisher<'a>(
            models: &'a Models,
            provider_id: &'a str,
            generation: u64,
            signal: &'a CancellationToken,
        ) -> impl Fn(ModelsPublication<'a>) -> BoxFuture<'a, bool> + Send + Sync + use<'a> {
            move |publication| {
                Box::pin(models.publish_provider_models(
                    provider_id,
                    generation,
                    signal,
                    publication,
                ))
            }
        }
        let publish = Box::new(publisher(self, provider.id(), generation, signal));

        let context = RefreshModelsContext {
            credential,
            stored,
            publish,
            allow_network,
            force: if allow_network { force } else { None },
            signal: signal.clone(),
        };

        match provider.refresh_models(context) {
            Some(future) => future.await.map_err(|error| {
                ModelsError::with_cause(
                    ModelsErrorCode::ModelSource,
                    format!("Model refresh failed for {}", provider.id()),
                    &error,
                )
            }),
            None => Ok(()),
        }
    }

    /// `resolveRefreshCredential(provider, stored, signal)`
    async fn resolve_refresh_credential(
        &self,
        provider: &dyn Provider,
        stored: Option<Credential>,
        signal: &CancellationToken,
    ) -> Result<Option<Credential>, ModelsError> {
        if let Some(Credential::OAuth(stored)) = &stored {
            let Some(oauth) = &provider.auth().oauth else {
                return Ok(None);
            };
            if crate::auth::resolve::now_ms() < stored.expires {
                return Ok(Some(Credential::OAuth(stored.clone())));
            }
            if signal.is_cancelled() {
                return Ok(None);
            }
            let oauth = Arc::clone(oauth);
            let refresh_signal = signal.clone();
            let post = self
                .credentials
                .modify(
                    provider.id(),
                    Box::new(move |current| {
                        Box::pin(async move {
                            let Some(Credential::OAuth(current)) = current else {
                                return Ok(None);
                            };
                            if crate::auth::resolve::now_ms() < current.expires {
                                return Ok(None);
                            }
                            match oauth.refresh(current, refresh_signal).await {
                                Ok(credential) => Ok(Some(Credential::OAuth(credential))),
                                Err(error) => {
                                    Err(crate::auth::types::CredentialStoreError(error.to_string()))
                                }
                            }
                        })
                    }),
                    Some(AuthOperationOptions {
                        signal: Some(signal.clone()),
                    }),
                )
                .await
                .map_err(|error| {
                    ModelsError::with_cause(
                        ModelsErrorCode::Auth,
                        format!("Credential store modify failed for {}", provider.id()),
                        &error,
                    )
                })?;
            return Ok(match post {
                Some(Credential::OAuth(credential)) => Some(Credential::OAuth(credential)),
                _ => None,
            });
        }

        let Some(api_key) = &provider.auth().api_key else {
            return Ok(None);
        };
        let credential = match stored {
            Some(Credential::ApiKey(credential)) => Some(credential),
            _ => None,
        };
        let result = api_key
            .resolve(crate::auth::types::ApiKeyAuthInput {
                ctx: self.auth_context.as_ref(),
                credential,
                signal: signal.clone(),
            })
            .await
            .map_err(|error| {
                ModelsError::with_cause(
                    ModelsErrorCode::Auth,
                    format!("API key auth failed for provider {}", provider.id()),
                    &error,
                )
            })?;
        Ok(result.map(|result| {
            Credential::ApiKey(crate::auth::types::ApiKeyCredential {
                key: result.auth.api_key,
                env: result.env,
            })
        }))
    }

    async fn read_credential(
        &self,
        provider_id: &str,
        signal: &CancellationToken,
    ) -> Result<Option<Credential>, ModelsError> {
        self.credentials
            .read(
                provider_id,
                Some(AuthOperationOptions {
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

    // -- auth ---------------------------------------------------------------

    /// `checkProviderAuth(provider, credential, signal)`
    async fn check_provider_auth(
        &self,
        provider: &dyn Provider,
        credential: Option<&Credential>,
        signal: &CancellationToken,
    ) -> Result<Option<AuthCheck>, ModelsError> {
        if let Some(Credential::OAuth(_)) = credential {
            return Ok(provider.auth().oauth.as_ref().map(|_| AuthCheck {
                source: Some("OAuth".to_string()),
                check_type: AuthType::OAuth,
            }));
        }
        let Some(api_key) = &provider.auth().api_key else {
            return Ok(None);
        };
        if let Some(check) = api_key.check(crate::auth::types::ApiKeyAuthInput {
            ctx: self.auth_context.as_ref(),
            credential: credential.and_then(Credential::as_api_key).cloned(),
            signal: signal.clone(),
        }) {
            return check.await.map_err(|error| {
                ModelsError::with_cause(
                    ModelsErrorCode::Auth,
                    format!("API key auth check failed for provider {}", provider.id()),
                    &error,
                )
            });
        }

        let resolution = resolve_provider_auth(
            AuthProvider {
                id: provider.id(),
                auth: provider.auth(),
            },
            self.credentials.as_ref(),
            Arc::clone(&self.auth_context),
            Some(&AuthResolutionOverrides {
                signal: Some(signal.clone()),
                ..Default::default()
            }),
        )
        .await?;
        Ok(resolution.map(|resolution| AuthCheck {
            source: resolution.source,
            check_type: AuthType::ApiKey,
        }))
    }

    /// `checkAuth(providerId, options?)`
    pub async fn check_auth(
        &self,
        provider_id: &str,
        options: Option<AuthOperationOptions>,
    ) -> Result<Option<AuthCheck>, ModelsError> {
        let signal = options
            .and_then(|options| options.signal)
            .unwrap_or_default();
        let check = async {
            let Some(provider) = self.get_provider(provider_id) else {
                return Ok(None);
            };
            let credential = self.read_credential(provider_id, &signal).await?;
            self.check_provider_auth(provider.as_ref(), credential.as_ref(), &signal)
                .await
        };
        // `raceWithAbortSignal(check, signal)`: a provider check that ignores the
        // signal must not keep the caller waiting.
        crate::utils::abort::race_with_abort_signal(check, &signal)
            .await
            .unwrap_or_else(|_| {
                Err(ModelsError::new(
                    ModelsErrorCode::Auth,
                    "The operation was aborted",
                ))
            })
    }

    /// `getAvailable(providerId?, options?)`
    pub async fn get_available(
        &self,
        provider_id: Option<&str>,
        options: Option<AuthOperationOptions>,
    ) -> Result<Vec<Model>, ModelsError> {
        let signal = options
            .and_then(|options| options.signal)
            .unwrap_or_default();
        let collect = async {
            let providers = match provider_id {
                Some(provider_id) => self
                    .get_provider(provider_id)
                    .into_iter()
                    .collect::<Vec<_>>(),
                None => self.get_providers(),
            };

            let mut available = Vec::new();
            for provider in providers {
                let credential = self.read_credential(provider.id(), &signal).await?;
                let auth = self
                    .check_provider_auth(provider.as_ref(), credential.as_ref(), &signal)
                    .await?;
                if auth.is_none() {
                    continue;
                }
                available
                    .extend(provider.filter_models(provider.get_models(), credential.as_ref()));
            }
            Ok(available)
        };
        // `raceWithAbortSignal(available, signal)`.
        crate::utils::abort::race_with_abort_signal(collect, &signal)
            .await
            .unwrap_or_else(|_| {
                Err(ModelsError::new(
                    ModelsErrorCode::Auth,
                    "The operation was aborted",
                ))
            })
    }

    /// `getAuth(providerId | model, overrides?)`
    pub async fn get_auth_for_provider(
        &self,
        provider_id: &str,
        overrides: Option<&AuthResolutionOverrides>,
    ) -> Result<Option<crate::auth::types::AuthResult>, ModelsError> {
        let Some(provider) = self.get_provider(provider_id) else {
            return Ok(None);
        };
        resolve_provider_auth(
            AuthProvider {
                id: provider.id(),
                auth: provider.auth(),
            },
            self.credentials.as_ref(),
            Arc::clone(&self.auth_context),
            overrides,
        )
        .await
    }

    /// `getAuth(model, overrides?)` — provider auth plus static model headers.
    pub async fn get_auth_for_model(
        &self,
        model: &Model,
        overrides: Option<&AuthResolutionOverrides>,
    ) -> Result<Option<crate::auth::types::AuthResult>, ModelsError> {
        let result = self
            .get_auth_for_provider(&model.provider, overrides)
            .await?;
        let Some(result) = result else {
            return Ok(None);
        };
        let Some(model_headers) = &model.headers else {
            return Ok(Some(result));
        };
        let model_headers: ProviderHeaders = model_headers
            .iter()
            .map(|(name, value)| (name.clone(), Some(value.clone())))
            .collect();
        Ok(Some(crate::auth::types::AuthResult {
            auth: ModelAuth {
                headers: merge_headers(result.auth.headers.as_ref(), Some(&model_headers)),
                ..result.auth
            },
            ..result
        }))
    }

    /// `logout(providerId, options?)`
    pub async fn logout(
        &self,
        provider_id: &str,
        options: Option<AuthOperationOptions>,
    ) -> Result<(), ModelsError> {
        let signal = options
            .and_then(|options| options.signal)
            .unwrap_or_default();
        if signal.is_cancelled() {
            return Err(ModelsError::new(
                ModelsErrorCode::Auth,
                "The operation was aborted",
            ));
        }
        self.credentials
            .delete(
                provider_id,
                Some(AuthOperationOptions {
                    signal: Some(signal),
                }),
            )
            .await
            .map_err(|error| {
                ModelsError::with_cause(
                    ModelsErrorCode::Auth,
                    format!("Credential store delete failed for {provider_id}"),
                    &error,
                )
            })
    }

    // -- requests -----------------------------------------------------------

    fn require_provider(&self, model: &Model) -> Result<Arc<dyn Provider>, ModelsError> {
        self.get_provider(&model.provider).ok_or_else(|| {
            ModelsError::new(
                ModelsErrorCode::Provider,
                format!("Unknown provider: {}", model.provider),
            )
        })
    }

    /// `applyAuth(model, options)` — resolved auth merged into model and request options.
    async fn apply_auth(
        &self,
        model: &Model,
        api_key: Option<String>,
        env: Option<crate::types::ProviderEnv>,
        headers: Option<ProviderHeaders>,
        signal: Option<CancellationToken>,
    ) -> Result<(Model, ModelAuth, Option<crate::types::ProviderEnv>), ModelsError> {
        self.require_provider(model)?;
        let resolution = self
            .get_auth_for_model(
                model,
                Some(&AuthResolutionOverrides {
                    api_key: api_key.clone(),
                    env: env.clone(),
                    min_oauth_validity_ms: None,
                    signal,
                }),
            )
            .await?
            .ok_or_else(|| {
                ModelsError::new(
                    ModelsErrorCode::Auth,
                    format!("Provider is not configured: {}", model.provider),
                )
            })?;

        // Explicit request options win per field.
        let resolved_api_key = api_key.or(resolution.auth.api_key.clone());
        let merged_headers = merge_headers(resolution.auth.headers.as_ref(), headers.as_ref());
        let merged_env = match (&resolution.env, &env) {
            (None, None) => None,
            (resolved, requested) => {
                let mut merged = resolved.clone().unwrap_or_default();
                merged.extend(requested.clone().unwrap_or_default());
                Some(merged)
            }
        };
        let request_model = match &resolution.auth.base_url {
            Some(base_url) => Model {
                base_url: base_url.clone(),
                ..model.clone()
            },
            None => model.clone(),
        };
        Ok((
            request_model,
            ModelAuth {
                api_key: resolved_api_key,
                headers: merged_headers,
                base_url: None,
            },
            merged_env,
        ))
    }

    /// `streamSimple(model, context, options?)`
    pub fn stream_simple(
        self: &Arc<Self>,
        model: Model,
        context: Context,
        options: Option<SimpleStreamOptions>,
    ) -> AssistantMessageEventStream {
        let models = Arc::clone(self);
        let setup_model = model.clone();
        lazy_stream(model.clone(), move || async move {
            let provider = models
                .require_provider(&model)
                .map_err(|error| error.to_string())?;
            let options = options.unwrap_or_default();
            let (request_model, auth, env) = models
                .apply_auth(
                    &model,
                    options.base.base.api_key.clone(),
                    options.base.base.env.clone(),
                    options.base.base.headers.clone(),
                    options.base.base.signal.clone(),
                )
                .await
                .map_err(|error| error.to_string())?;
            let mut request_options = options;
            request_options.base.base.api_key = auth.api_key;
            request_options.base.base.headers = auth.headers;
            request_options.base.base.env = env;
            let _ = setup_model;
            Ok(provider.stream_simple(&request_model, &context, Some(request_options)))
        })
    }

    /// `stream(model, context, options?)`
    pub fn stream(
        self: &Arc<Self>,
        model: Model,
        context: Context,
        options: Option<StreamOptions>,
    ) -> AssistantMessageEventStream {
        let models = Arc::clone(self);
        lazy_stream(model.clone(), move || async move {
            let provider = models
                .require_provider(&model)
                .map_err(|error| error.to_string())?;
            let options = options.unwrap_or_default();
            let (request_model, auth, env) = models
                .apply_auth(
                    &model,
                    options.base.api_key.clone(),
                    options.base.env.clone(),
                    options.base.headers.clone(),
                    options.base.signal.clone(),
                )
                .await
                .map_err(|error| error.to_string())?;
            let mut request_options = options;
            request_options.base.api_key = auth.api_key;
            request_options.base.headers = auth.headers;
            request_options.base.env = env;
            Ok(provider.stream(&request_model, &context, Some(request_options)))
        })
    }

    /// `complete(model, context, options?)`
    pub async fn complete(
        self: &Arc<Self>,
        model: Model,
        context: Context,
        options: Option<StreamOptions>,
    ) -> crate::types::AssistantMessage {
        self.stream(model, context, options).result().await
    }

    /// `completeSimple(model, context, options?)`
    pub async fn complete_simple(
        self: &Arc<Self>,
        model: Model,
        context: Context,
        options: Option<SimpleStreamOptions>,
    ) -> crate::types::AssistantMessage {
        self.stream_simple(model, context, options).result().await
    }

    /// `fetchDeferred(model, handle, options?)`
    /// provider without deferred support surfaces as an error message rather than a
    /// rejection.
    pub async fn fetch_deferred(
        self: &Arc<Self>,
        model: Model,
        handle: DeferredHandle,
        options: Option<DeferredFetchOptions>,
    ) -> crate::types::AssistantMessage {
        let models = Arc::clone(self);
        lazy_stream(model.clone(), move || async move {
            let provider = models
                .require_provider(&model)
                .map_err(|error| error.to_string())?;
            let options = options.unwrap_or_default();
            let (request_model, auth, env) = models
                .apply_auth(
                    &model,
                    options.base.api_key.clone(),
                    options.base.env.clone(),
                    options.base.headers.clone(),
                    options.base.signal.clone(),
                )
                .await
                .map_err(|error| error.to_string())?;
            let mut request_options = options;
            request_options.base.api_key = auth.api_key;
            request_options.base.headers = auth.headers;
            request_options.base.env = env;
            provider
                .fetch_deferred(&request_model, &handle, Some(request_options))
                .ok_or_else(|| {
                    ModelsError::new(
                        ModelsErrorCode::Provider,
                        format!(
                            "Provider {} does not support deferred responses",
                            request_model.provider
                        ),
                    )
                    .to_string()
                })
        })
        .result()
        .await
    }

    /// `cancelDeferred(model, handle, options?)` — unlike `fetchDeferred` this one does
    pub async fn cancel_deferred(
        self: &Arc<Self>,
        model: Model,
        handle: DeferredHandle,
        options: Option<DeferredCancelOptions>,
    ) -> Result<(), ModelsError> {
        let provider = self.require_provider(&model)?;
        let options = options.unwrap_or_default();
        let (request_model, auth, env) = self
            .apply_auth(
                &model,
                options.api_key.clone(),
                options.env.clone(),
                options.headers.clone(),
                options.signal.clone(),
            )
            .await?;
        let mut request_options = options;
        request_options.api_key = auth.api_key;
        request_options.headers = auth.headers;
        request_options.env = env;
        match provider.cancel_deferred(&request_model, &handle, Some(request_options)) {
            Some(future) => future.await,
            None => Err(ModelsError::new(
                ModelsErrorCode::Provider,
                format!(
                    "Provider {} does not support deferred responses",
                    request_model.provider
                ),
            )),
        }
    }
}

/// Combines several cancellation tokens like `AbortSignal.any([...])`.
fn child_of(signals: &[&CancellationToken]) -> CancellationToken {
    let combined = CancellationToken::new();
    for signal in signals {
        if signal.is_cancelled() {
            combined.cancel();
            return combined;
        }
    }
    let target = combined.clone();
    let sources: Vec<CancellationToken> = signals.iter().map(|signal| (*signal).clone()).collect();
    tokio::spawn(async move {
        let mut futures: Vec<_> = sources
            .iter()
            .map(|signal| Box::pin(signal.cancelled()))
            .collect();
        futures::future::select_all(&mut futures).await;
        target.cancel();
    });
    combined
}

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

/// `hasApi(model, api)`
pub fn has_api(model: &Model, api: &str) -> bool {
    model.api == api
}

/// `calculateCost(model, usage)` — the highest matching input tier applies.
pub fn calculate_cost(model: &Model, usage: &mut Usage) -> UsageCost {
    let input_tokens = usage.input + usage.cache_read + usage.cache_write;
    let mut rates = ModelCostRates {
        input: model.cost.input,
        output: model.cost.output,
        cache_read: model.cost.cache_read,
        cache_write: model.cost.cache_write,
    };
    let mut matched_threshold: i128 = -1;
    for tier in model.cost.tiers.iter().flatten() {
        if input_tokens > tier.input_tokens_above
            && (tier.input_tokens_above as i128) > matched_threshold
        {
            rates = ModelCostRates {
                input: tier.input,
                output: tier.output,
                cache_read: tier.cache_read,
                cache_write: tier.cache_write,
            };
            matched_threshold = tier.input_tokens_above as i128;
        }
    }

    // Anthropic charges 2x base input for 1h cache writes.
    let long_write = usage.cache_write1h.unwrap_or(0);
    let short_write = usage.cache_write - long_write;
    usage.cost.input = (rates.input / 1_000_000.0) * usage.input as f64;
    usage.cost.output = (rates.output / 1_000_000.0) * usage.output as f64;
    usage.cost.cache_read = (rates.cache_read / 1_000_000.0) * usage.cache_read as f64;
    usage.cost.cache_write = (rates.cache_write * short_write as f64
        + rates.input * 2.0 * long_write as f64)
        / 1_000_000.0;
    usage.cost.total =
        usage.cost.input + usage.cost.output + usage.cost.cache_read + usage.cost.cache_write;
    usage.cost
}

const EXTENDED_THINKING_LEVELS: [ModelThinkingLevel; 7] = [
    ModelThinkingLevel::Off,
    ModelThinkingLevel::Minimal,
    ModelThinkingLevel::Low,
    ModelThinkingLevel::Medium,
    ModelThinkingLevel::High,
    ModelThinkingLevel::Xhigh,
    ModelThinkingLevel::Max,
];

/// `getSupportedThinkingLevels(model)`
pub fn get_supported_thinking_levels(model: &Model) -> Vec<ModelThinkingLevel> {
    if !model.reasoning {
        return vec![ModelThinkingLevel::Off];
    }
    EXTENDED_THINKING_LEVELS
        .into_iter()
        .filter(|level| {
            let mapped = model
                .thinking_level_map
                .as_ref()
                .and_then(|map| map.get(level));
            match mapped {
                // `null` marks a level as unsupported.
                Some(None) => false,
                mapped => {
                    if matches!(level, ModelThinkingLevel::Xhigh | ModelThinkingLevel::Max) {
                        mapped.is_some()
                    } else {
                        true
                    }
                }
            }
        })
        .collect()
}

/// `clampThinkingLevel(model, level)`
pub fn clamp_thinking_level(model: &Model, level: ModelThinkingLevel) -> ModelThinkingLevel {
    let available = get_supported_thinking_levels(model);
    if available.contains(&level) {
        return level;
    }
    let Some(requested_index) = EXTENDED_THINKING_LEVELS
        .iter()
        .position(|candidate| *candidate == level)
    else {
        return available
            .first()
            .copied()
            .unwrap_or(ModelThinkingLevel::Off);
    };
    for candidate in EXTENDED_THINKING_LEVELS.iter().skip(requested_index) {
        if available.contains(candidate) {
            return *candidate;
        }
    }
    for candidate in EXTENDED_THINKING_LEVELS.iter().take(requested_index).rev() {
        if available.contains(candidate) {
            return *candidate;
        }
    }
    available
        .first()
        .copied()
        .unwrap_or(ModelThinkingLevel::Off)
}

/// `modelsAreEqual(a, b)` — false when either side is missing.
pub fn models_are_equal(a: Option<&Model>, b: Option<&Model>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.id == b.id && a.provider == b.provider,
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// createProvider
// ---------------------------------------------------------------------------

/// The API implementation(s) of a provider: one for all models, or a map keyed by
/// `model.api` for mixed-API providers.
pub enum ProviderApis {
    Single(Arc<dyn crate::types::ProviderStreams>),
    ByApi(BTreeMap<String, Arc<dyn crate::types::ProviderStreams>>),
}

impl ProviderApis {
    fn for_model(&self, model: &Model) -> Option<Arc<dyn crate::types::ProviderStreams>> {
        match self {
            ProviderApis::Single(streams) => Some(Arc::clone(streams)),
            ProviderApis::ByApi(by_api) => by_api.get(&model.api).cloned(),
        }
    }

    fn all(&self) -> Vec<Arc<dyn crate::types::ProviderStreams>> {
        match self {
            ProviderApis::Single(streams) => vec![Arc::clone(streams)],
            ProviderApis::ByApi(by_api) => by_api.values().cloned().collect(),
        }
    }
}

/// `fetchModels(context)` of [`CreateProviderOptions`].
pub type FetchModelsFn = Arc<
    dyn for<'a> Fn(&RefreshModelsContext<'a>) -> BoxFuture<'a, Result<Vec<Model>, String>>
        + Send
        + Sync,
>;

/// `filterModels(models, credential)`
pub type FilterModelsFn = Arc<dyn Fn(Vec<Model>, Option<&Credential>) -> Vec<Model> + Send + Sync>;

/// `CreateProviderOptions`
pub struct CreateProviderOptions {
    pub id: String,
    /// Display name; defaults to `id`.
    pub name: Option<String>,
    pub base_url: Option<String>,
    pub headers: Option<ProviderHeaders>,
    pub auth: ProviderAuth,
    /// Static baseline model list (empty for purely dynamic providers).
    pub models: Vec<Model>,
    pub fetch_models: Option<FetchModelsFn>,
    pub filter_models: Option<FilterModelsFn>,
    pub api: ProviderApis,
}

/// Provider built by [`create_provider`].
pub struct BuiltProvider {
    id: String,
    name: String,
    base_url: Option<String>,
    headers: Option<ProviderHeaders>,
    auth: ProviderAuth,
    baseline_models: Vec<Model>,
    dynamic_models: Mutex<Vec<Model>>,
    fetch_models: Option<FetchModelsFn>,
    filter_models: Option<FilterModelsFn>,
    api: ProviderApis,
}

/// `createProvider(input)` — builds a provider from parts.
pub fn create_provider(input: CreateProviderOptions) -> Arc<BuiltProvider> {
    Arc::new(BuiltProvider {
        name: input.name.unwrap_or_else(|| input.id.clone()),
        id: input.id,
        base_url: input.base_url,
        headers: input.headers,
        auth: input.auth,
        baseline_models: input.models,
        dynamic_models: Mutex::new(Vec::new()),
        fetch_models: input.fetch_models,
        filter_models: input.filter_models,
        api: input.api,
    })
}

impl BuiltProvider {
    /// `currentModels()` — baseline with the dynamic overlay merged in by model id.
    fn current_models(&self) -> Vec<Model> {
        let mut merged = self.baseline_models.clone();
        for model in self
            .dynamic_models
            .lock()
            .expect("dynamic models poisoned")
            .iter()
        {
            match merged.iter().position(|entry| entry.id == model.id) {
                Some(index) => merged[index] = model.clone(),
                None => merged.push(model.clone()),
            }
        }
        merged
    }

    /// `dispatch(model, run)` — a model whose api has no entry produces a stream error.
    fn dispatch(
        &self,
        model: &Model,
        run: impl FnOnce(Arc<dyn crate::types::ProviderStreams>) -> AssistantMessageEventStream,
    ) -> AssistantMessageEventStream {
        match self.api.for_model(model) {
            Some(streams) => run(streams),
            None => {
                let message = format!(
                    "Provider {} has no API implementation for \"{}\"",
                    self.id, model.api
                );
                lazy_stream(model.clone(), move || async move { Err(message) })
            }
        }
    }
}

impl Provider for BuiltProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn base_url(&self) -> Option<&str> {
        self.base_url.as_deref()
    }

    fn headers(&self) -> Option<&ProviderHeaders> {
        self.headers.as_ref()
    }

    fn auth(&self) -> &ProviderAuth {
        &self.auth
    }

    fn get_models(&self) -> Vec<Model> {
        self.current_models()
    }

    fn is_dynamic(&self) -> bool {
        self.fetch_models.is_some()
    }

    fn refresh_models<'a>(
        &'a self,
        context: RefreshModelsContext<'a>,
    ) -> Option<BoxFuture<'a, Result<(), String>>> {
        let fetch_models = Arc::clone(self.fetch_models.as_ref()?);
        Some(Box::pin(async move {
            if let Some(stored) = &context.stored {
                let restored: Vec<Model> = stored
                    .models
                    .iter()
                    .filter(|model| model.provider == self.id)
                    .cloned()
                    .collect();
                let published = (context.publish)(ModelsPublication {
                    persist: None,
                    update: Some(Box::new({
                        let restored = restored.clone();
                        move || {
                            *self.dynamic_models.lock().expect("dynamic models poisoned") =
                                restored;
                        }
                    })),
                })
                .await;
                if !published {
                    return Ok(());
                }
            }
            if !context.allow_network || context.signal.is_cancelled() {
                return Ok(());
            }
            let refreshed = fetch_models(&context).await?;
            if context.signal.is_cancelled() {
                return Ok(());
            }
            (context.publish)(ModelsPublication {
                persist: Some(Some(ModelsStoreEntry {
                    models: refreshed.clone(),
                    checked_at: Some(crate::auth::resolve::now_ms()),
                    ..Default::default()
                })),
                update: Some(Box::new(move || {
                    *self.dynamic_models.lock().expect("dynamic models poisoned") = refreshed;
                })),
            })
            .await;
            Ok(())
        }))
    }

    fn filter_models(&self, models: Vec<Model>, credential: Option<&Credential>) -> Vec<Model> {
        match &self.filter_models {
            Some(filter) => filter(models, credential),
            None => models,
        }
    }

    fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: Option<StreamOptions>,
    ) -> AssistantMessageEventStream {
        self.dispatch(model, |streams| streams.stream(model, context, options))
    }

    fn stream_simple(
        &self,
        model: &Model,
        context: &Context,
        options: Option<SimpleStreamOptions>,
    ) -> AssistantMessageEventStream {
        self.dispatch(model, |streams| {
            streams.stream_simple(model, context, options)
        })
    }

    fn fetch_deferred(
        &self,
        model: &Model,
        handle: &DeferredHandle,
        options: Option<DeferredFetchOptions>,
    ) -> Option<AssistantMessageEventStream> {
        // per-api lookup then happens inside `lazyStream`, so a provider that supports
        // deferred responses for a *different* api reports that in the stream.
        if !self
            .api
            .all()
            .iter()
            .any(|streams| streams.supports_fetch_deferred())
        {
            return None;
        }
        let implementation = self
            .api
            .for_model(model)
            .filter(|streams| streams.supports_fetch_deferred());
        let unsupported = format!(
            "Provider {} does not support deferred responses for \"{}\"",
            self.id, model.api
        );
        let model = model.clone();
        let handle = handle.clone();
        Some(lazy_stream(model.clone(), move || async move {
            let implementation = implementation.ok_or_else(|| unsupported.clone())?;
            implementation
                .fetch_deferred(&model, &handle, options)
                .ok_or(unsupported)
        }))
    }

    fn cancel_deferred<'a>(
        &'a self,
        model: &'a Model,
        handle: &'a DeferredHandle,
        options: Option<DeferredCancelOptions>,
    ) -> Option<BoxFuture<'a, Result<(), ModelsError>>> {
        if !self
            .api
            .all()
            .iter()
            .any(|streams| streams.supports_cancel_deferred())
        {
            return None;
        }
        Some(Box::pin(async move {
            let implementation = self
                .api
                .for_model(model)
                .filter(|streams| streams.supports_cancel_deferred())
                .ok_or_else(|| {
                    ModelsError::new(
                        ModelsErrorCode::Provider,
                        format!(
                            "Provider {} cannot cancel deferred responses for \"{}\"",
                            self.id, model.api
                        ),
                    )
                })?;
            match implementation.cancel_deferred(model, handle, options) {
                Some(future) => {
                    future.await;
                    Ok(())
                }
                None => Err(ModelsError::new(
                    ModelsErrorCode::Provider,
                    format!(
                        "Provider {} cannot cancel deferred responses for \"{}\"",
                        self.id, model.api
                    ),
                )),
            }
        }))
    }
}

impl Models {
    /// `login(providerId, type, interaction)` — runs the flow and persists the credential.
    pub async fn login(
        &self,
        provider_id: &str,
        auth_type: AuthType,
        interaction: &dyn crate::auth::types::AuthInteraction,
    ) -> Result<Credential, ModelsError> {
        let signal = interaction.signal().unwrap_or_default();
        if signal.is_cancelled() {
            return Err(ModelsError::new(
                ModelsErrorCode::Auth,
                "The operation was aborted",
            ));
        }
        let provider = self.get_provider(provider_id).ok_or_else(|| {
            ModelsError::new(
                ModelsErrorCode::Provider,
                format!("Unknown provider: {provider_id}"),
            )
        })?;

        let provider_interaction = crate::auth::types::ProviderAuthInteraction {
            interaction,
            signal: signal.clone(),
        };
        let credential = match auth_type {
            AuthType::OAuth => {
                let oauth = provider.auth().oauth.as_ref().ok_or_else(|| {
                    ModelsError::new(
                        ModelsErrorCode::Auth,
                        format!("{} does not support oauth login", provider.name()),
                    )
                })?;
                Credential::OAuth(oauth.login(&provider_interaction).await.map_err(|error| {
                    ModelsError::with_cause(ModelsErrorCode::Auth, "Login failed", &error)
                })?)
            }
            AuthType::ApiKey => {
                let api_key = provider.auth().api_key.as_ref().ok_or_else(|| {
                    ModelsError::new(
                        ModelsErrorCode::Auth,
                        format!("{} does not support api_key login", provider.name()),
                    )
                })?;
                let login = api_key.login(&provider_interaction).ok_or_else(|| {
                    ModelsError::new(
                        ModelsErrorCode::Auth,
                        format!("{} does not support api_key login", provider.name()),
                    )
                })?;
                Credential::ApiKey(login.await.map_err(|error| {
                    ModelsError::with_cause(ModelsErrorCode::Auth, "Login failed", &error)
                })?)
            }
        };

        // The store write is not abandoned once it has started, so an abort arriving
        // during persistence cannot lose a credential the user just obtained.
        let persisted = credential.clone();
        self.credentials
            .modify(
                provider_id,
                Box::new(move |_current| Box::pin(async move { Ok(Some(persisted)) })),
                Some(AuthOperationOptions { signal: None }),
            )
            .await
            .map_err(|error| {
                ModelsError::with_cause(
                    ModelsErrorCode::Auth,
                    format!("Credential store modify failed for {provider_id}"),
                    &error,
                )
            })?;
        Ok(credential)
    }
}
