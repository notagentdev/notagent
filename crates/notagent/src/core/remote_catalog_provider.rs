//! Port of `packages/coding-agent/src/core/remote-catalog-provider.ts` (132 LOC).

use std::sync::{Arc, Mutex};

use notagent_ai::auth::resolve::{ModelsError, now_ms};
use notagent_ai::auth::types::{BoxFuture, Credential, ProviderAuth};
use notagent_ai::models::{ModelsPublication, Provider, RefreshModelsContext};
use notagent_ai::models_store::ModelsStoreEntry;
use notagent_ai::types::{
    Context, DeferredCancelOptions, DeferredFetchOptions, DeferredHandle, Model, ProviderHeaders,
    SimpleStreamOptions, StreamOptions,
};
use notagent_ai::utils::event_stream::AssistantMessageEventStream;
use serde_json::Value;

use crate::config::VERSION;
use crate::utils::abort::race_with_abort_signal;
use crate::utils::management_http::{FetchRetryOptions, fetch_with_retry};
use crate::utils::notagent_user_agent::get_pi_user_agent;

const DEFAULT_CATALOG_BASE_URL: &str = "https://notagent.dev";
pub const REMOTE_CATALOG_REFRESH_INTERVAL_MS: i64 = 4 * 60 * 60 * 1000;

/// `mergeModels(baseline, dynamic)`
fn merge_models(baseline: Vec<Model>, dynamic: &[Model]) -> Vec<Model> {
    let mut merged = baseline;
    for model in dynamic {
        match merged.iter().position(|entry| entry.id == model.id) {
            Some(index) => merged[index] = model.clone(),
            None => merged.push(model.clone()),
        }
    }
    merged
}

/// `parseCatalog(providerId, value)`
///
/// Deviation (class 1): TS spreads whatever object it finds, so a malformed entry
/// travels on as an invalid `Model`. Rust must deserialize, so entries that are not
/// valid models are skipped instead of being carried along broken.
fn parse_catalog(provider_id: &str, value: Value) -> Result<Vec<Model>, String> {
    let entries: Vec<Value> = match value {
        Value::Array(entries) => entries,
        Value::Object(object) => match object.get("models") {
            Some(Value::Array(models)) => models.clone(),
            _ => object.values().cloned().collect(),
        },
        _ => {
            return Err(format!(
                "Invalid model catalog for provider \"{provider_id}\""
            ));
        }
    };
    Ok(entries
        .into_iter()
        .filter_map(|entry| {
            let mut object = entry.as_object()?.clone();
            object.get("id")?;
            object.insert("provider".to_owned(), Value::String(provider_id.to_owned()));
            serde_json::from_value::<Model>(Value::Object(object)).ok()
        })
        .collect())
}

/// `remoteModels(entry, localGeneratedAt)`
fn remote_models(entry: Option<&ModelsStoreEntry>, local_generated_at: Option<i64>) -> Vec<Model> {
    let Some(entry) = entry else {
        return Vec::new();
    };
    if let Some(local_generated_at) = local_generated_at
        && entry
            .last_modified
            .is_none_or(|last_modified| last_modified <= local_generated_at)
    {
        return Vec::new();
    }
    entry.models.clone()
}

/// Add a persisted notagent.dev catalog overlay to a static built-in provider.
pub struct RemoteCatalogProvider {
    inner: Arc<dyn Provider>,
    catalog_base_url: String,
    local_generated_at: Option<i64>,
    dynamic_models: Mutex<Vec<Model>>,
}

/// `withRemoteCatalog(provider, catalogBaseUrl?, localGeneratedAt?)`
pub fn with_remote_catalog(
    provider: Arc<dyn Provider>,
    catalog_base_url: Option<&str>,
    local_generated_at: Option<i64>,
) -> Arc<dyn Provider> {
    Arc::new(RemoteCatalogProvider {
        inner: provider,
        catalog_base_url: catalog_base_url
            .unwrap_or(DEFAULT_CATALOG_BASE_URL)
            .to_owned(),
        local_generated_at,
        dynamic_models: Mutex::new(Vec::new()),
    })
}

impl RemoteCatalogProvider {
    fn set_dynamic_models(&self, models: Vec<Model>) {
        *self.dynamic_models.lock().expect("dynamic models") = models;
    }

    async fn refresh<'a>(&'a self, context: RefreshModelsContext<'a>) -> Result<(), String> {
        let stored = context.stored.clone();
        let restored: Vec<Model> = remote_models(stored.as_ref(), self.local_generated_at)
            .into_iter()
            .filter(|model| model.provider == self.inner.id())
            .collect();
        let published = (context.publish)(ModelsPublication {
            persist: None,
            update: Some(Box::new(move || self.set_dynamic_models(restored))),
        })
        .await;
        if !published {
            return Ok(());
        }
        if !context.allow_network || context.signal.is_cancelled() {
            return Ok(());
        }
        if context.force != Some(true)
            && let Some(stored) = &stored
            && let (Some(checked_at), Some(_)) = (stored.checked_at, stored.last_modified)
            && now_ms() - checked_at < REMOTE_CATALOG_REFRESH_INTERVAL_MS
        {
            return Ok(());
        }

        // Only revalidate when a cached body backs the validator, so a 304 can never
        // leave the overlay empty.
        let validator = stored
            .as_ref()
            .filter(|stored| !stored.models.is_empty())
            .and_then(|stored| stored.etag.clone());
        let url = format!(
            "{}/api/models/providers/{}",
            self.catalog_base_url.trim_end_matches('/'),
            urlencode(self.inner.id())
        );
        let client = reqwest::Client::new();
        let request = || {
            let mut request = client
                .get(&url)
                .header("accept", "application/json")
                .header("User-Agent", get_pi_user_agent(VERSION));
            if let Some(validator) = &validator {
                request = request.header("if-none-match", validator);
            }
            request
        };
        let response = race_with_abort_signal(
            fetch_with_retry(&client, request, FetchRetryOptions::default()),
            Some(&context.signal),
        )
        .await
        .map_err(|_| "The operation was aborted".to_owned())??;
        if context.signal.is_cancelled() {
            return Ok(());
        }
        let checked_at = now_ms();
        let status = response.status().as_u16();
        // Unchanged: dynamicModels already holds the stored overlay, so only the
        // freshness window moves.
        if status == 304
            && let Some(stored) = &stored
        {
            (context.publish)(ModelsPublication {
                persist: Some(Some(ModelsStoreEntry {
                    checked_at: Some(checked_at),
                    ..stored.clone()
                })),
                update: None,
            })
            .await;
            return Ok(());
        }
        if status == 404 || status == 501 {
            (context.publish)(ModelsPublication {
                persist: Some(Some(ModelsStoreEntry {
                    checked_at: Some(checked_at),
                    last_modified: Some(0),
                    etag: None,
                    ..stored.clone().unwrap_or_default()
                })),
                update: None,
            })
            .await;
            return Ok(());
        }
        if !(200..300).contains(&status) {
            // Transient failure: the cached body and its validator stay valid, so keep the
            // etag and let the next refresh revalidate instead of downloading the catalog.
            (context.publish)(ModelsPublication {
                persist: Some(Some(ModelsStoreEntry {
                    checked_at: Some(checked_at),
                    ..stored.clone().unwrap_or_default()
                })),
                update: None,
            })
            .await;
            return Err(format!(
                "Model catalog request failed for {}: {status}",
                self.inner.id()
            ));
        }
        let last_modified = response
            .headers()
            .get("last-modified")
            .and_then(|value| value.to_str().ok())
            .and_then(parse_http_date);
        let etag = response
            .headers()
            .get("etag")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let body: Value = response.json().await.map_err(|error| error.to_string())?;
        let refreshed = parse_catalog(self.inner.id(), body)?;
        if context.signal.is_cancelled() {
            return Ok(());
        }
        let entry = ModelsStoreEntry {
            models: refreshed,
            checked_at: Some(checked_at),
            last_modified: Some(last_modified.unwrap_or(0)),
            etag,
        };
        let published = remote_models(Some(&entry), self.local_generated_at);
        (context.publish)(ModelsPublication {
            persist: Some(Some(entry)),
            update: Some(Box::new(move || self.set_dynamic_models(published))),
        })
        .await;
        Ok(())
    }
}

/// `encodeURIComponent(providerId)` for the path segment.
fn urlencode(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\''
            | b'('
            | b')' => encoded.push(byte as char),
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

/// `Date.parse(header)` for an RFC 7231 `Last-Modified` value, in milliseconds.
fn parse_http_date(value: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc2822(value)
        .ok()
        .map(|value| value.timestamp_millis())
}

impl Provider for RemoteCatalogProvider {
    fn id(&self) -> &str {
        self.inner.id()
    }

    fn name(&self) -> &str {
        self.inner.name()
    }

    fn base_url(&self) -> Option<&str> {
        self.inner.base_url()
    }

    fn headers(&self) -> Option<&ProviderHeaders> {
        self.inner.headers()
    }

    fn auth(&self) -> &ProviderAuth {
        self.inner.auth()
    }

    fn get_models(&self) -> Vec<Model> {
        merge_models(
            self.inner.get_models(),
            &self.dynamic_models.lock().expect("dynamic models"),
        )
    }

    /// The overlay always defines `refreshModels`, even over a static provider.
    fn is_dynamic(&self) -> bool {
        true
    }

    fn refresh_models<'a>(
        &'a self,
        context: RefreshModelsContext<'a>,
    ) -> Option<BoxFuture<'a, Result<(), String>>> {
        Some(Box::pin(self.refresh(context)))
    }

    fn filter_models(&self, models: Vec<Model>, credential: Option<&Credential>) -> Vec<Model> {
        self.inner.filter_models(models, credential)
    }

    fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: Option<StreamOptions>,
    ) -> AssistantMessageEventStream {
        self.inner.stream(model, context, options)
    }

    fn stream_simple(
        &self,
        model: &Model,
        context: &Context,
        options: Option<SimpleStreamOptions>,
    ) -> AssistantMessageEventStream {
        self.inner.stream_simple(model, context, options)
    }

    fn fetch_deferred(
        &self,
        model: &Model,
        handle: &DeferredHandle,
        options: Option<DeferredFetchOptions>,
    ) -> Option<AssistantMessageEventStream> {
        self.inner.fetch_deferred(model, handle, options)
    }

    fn cancel_deferred<'a>(
        &'a self,
        model: &'a Model,
        handle: &'a DeferredHandle,
        options: Option<DeferredCancelOptions>,
    ) -> Option<BoxFuture<'a, Result<(), ModelsError>>> {
        self.inner.cancel_deferred(model, handle, options)
    }
}
