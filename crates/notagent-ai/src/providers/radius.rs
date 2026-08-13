//! `radiusProvider(options?)`.
//!
//! 1:1 port of `packages/ai/src/providers/radius.ts` (82 LOC). Radius is the only
//! provider with a purely dynamic catalog: it starts empty, restores the persisted list,
//! imports the catalog cached by the pre-`ModelsStore` implementation and only then goes
//! to the network.

use std::sync::{Arc, Mutex};

use crate::api::streams::PiMessagesApi;
use crate::auth::helpers::EnvApiKeyAuth;
use crate::auth::oauth::radius::{RadiusOAuthOptions, create_radius_oauth};
use crate::auth::types::{BoxFuture, Credential, ProviderAuth};
use crate::models::{Provider, RefreshModelsContext};
use crate::models_store::ModelsStoreEntry;
use crate::providers::radius_config::{
    DEFAULT_RADIUS_GATEWAY, get_radius_models, get_radius_models_from_config,
    load_radius_gateway_config, normalize_radius_gateway_url,
};
use crate::types::{
    Context, DeferredCancelOptions, DeferredFetchOptions, DeferredHandle, Model, ProviderHeaders,
    ProviderStreams, SimpleStreamOptions, StreamOptions,
};
use crate::utils::event_stream::AssistantMessageEventStream;

/// `RadiusProviderOptions { id?, name?, gateway? }`
#[derive(Debug, Clone, Default)]
pub struct RadiusProviderOptions {
    pub id: Option<String>,
    pub name: Option<String>,
    pub gateway: Option<String>,
}

/// The provider object `radiusProvider()` returns; it is hand-built rather than produced
/// by `createProvider`, because its refresh has its own cache-migration step.
pub struct RadiusProvider {
    id: String,
    name: String,
    auth: ProviderAuth,
    gateway: String,
    models: Mutex<Vec<Model>>,
    streams: Arc<dyn ProviderStreams>,
}

/// `radiusProvider(options)`
pub fn radius_provider(options: RadiusProviderOptions) -> Arc<RadiusProvider> {
    let id = options.id.unwrap_or_else(|| "radius".to_string());
    let name = options.name.unwrap_or_else(|| "Radius".to_string());
    let gateway = normalize_radius_gateway_url(
        &options
            .gateway
            .unwrap_or_else(|| DEFAULT_RADIUS_GATEWAY.to_string()),
    );
    Arc::new(RadiusProvider {
        auth: ProviderAuth {
            api_key: Some(Arc::new(EnvApiKeyAuth::new(
                "Radius API key",
                ["RADIUS_API_KEY"],
            ))),
            oauth: Some(create_radius_oauth(RadiusOAuthOptions {
                name: name.clone(),
                gateway: gateway.clone(),
            })),
        },
        // `getRadiusModels(id, undefined)` is always empty; the credential-backed catalog
        // arrives with the first refresh.
        models: Mutex::new(get_radius_models(&id, None)),
        id,
        name,
        gateway,
        streams: Arc::new(PiMessagesApi),
    })
}

impl RadiusProvider {
    fn set_models(&self, models: Vec<Model>) {
        *self.models.lock().expect("radius models poisoned") = models;
    }
}

impl Provider for RadiusProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn headers(&self) -> Option<&ProviderHeaders> {
        None
    }

    fn auth(&self) -> &ProviderAuth {
        &self.auth
    }

    fn get_models(&self) -> Vec<Model> {
        self.models.lock().expect("radius models poisoned").clone()
    }

    fn is_dynamic(&self) -> bool {
        true
    }

    fn refresh_models<'a>(
        &'a self,
        context: RefreshModelsContext<'a>,
    ) -> Option<BoxFuture<'a, Result<(), String>>> {
        Some(Box::pin(async move {
            if let Some(stored) = context.stored.clone() {
                let restored: Vec<Model> = stored
                    .models
                    .into_iter()
                    .filter(|model| model.provider == self.id)
                    .collect();
                if !((context.publish)(crate::models::ModelsPublication {
                    persist: None,
                    update: Some(Box::new(move || self.set_models(restored))),
                })
                .await)
                {
                    return Ok(());
                }
            }

            // Import catalogs cached by the pre-ModelsStore Radius implementation.
            if context.stored.is_none()
                && let Some(Credential::OAuth(credential)) = &context.credential
            {
                let legacy = get_radius_models(&self.id, Some(credential));
                if !legacy.is_empty() {
                    let published = legacy.clone();
                    if !((context.publish)(crate::models::ModelsPublication {
                        persist: Some(Some(ModelsStoreEntry {
                            models: legacy,
                            checked_at: Some(crate::auth::resolve::now_ms()),
                            etag: None,
                            last_modified: None,
                        })),
                        update: Some(Box::new(move || self.set_models(published))),
                    })
                    .await)
                    {
                        return Ok(());
                    }
                }
            }

            if !context.allow_network || context.signal.is_cancelled() {
                return Ok(());
            }
            let api_key = match &context.credential {
                Some(Credential::OAuth(credential)) => Some(credential.access.clone()),
                Some(Credential::ApiKey(credential)) => credential.key.clone(),
                None => None,
            };
            let config =
                load_radius_gateway_config(&self.gateway, api_key.as_deref(), &context.signal)
                    .await?;
            if context.signal.is_cancelled() {
                return Ok(());
            }
            let refreshed = get_radius_models_from_config(&self.id, &config);
            let published = refreshed.clone();
            (context.publish)(crate::models::ModelsPublication {
                persist: Some(Some(ModelsStoreEntry {
                    models: refreshed,
                    checked_at: Some(crate::auth::resolve::now_ms()),
                    etag: None,
                    last_modified: None,
                })),
                update: Some(Box::new(move || self.set_models(published))),
            })
            .await;
            Ok(())
        }))
    }

    fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: Option<StreamOptions>,
    ) -> AssistantMessageEventStream {
        self.streams.stream(model, context, options)
    }

    fn stream_simple(
        &self,
        model: &Model,
        context: &Context,
        options: Option<SimpleStreamOptions>,
    ) -> AssistantMessageEventStream {
        self.streams.stream_simple(model, context, options)
    }

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
    ) -> Option<BoxFuture<'a, Result<(), crate::auth::resolve::ModelsError>>> {
        None
    }
}
