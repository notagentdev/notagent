use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::auth::types::BoxFuture;
use crate::types::Model;

/// `ModelsStoreEntry`
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelsStoreEntry {
    pub models: Vec<Model>,
    /// Unix timestamp from the remote catalog's `Last-Modified` header.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<i64>,
    /// Unix timestamp of the last completed remote check.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked_at: Option<i64>,
    /// Opaque validator from the remote `ETag`, stored verbatim (quotes included).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
}

/// `ModelsStoreOperationOptions`
#[derive(Debug, Clone, Default)]
pub struct ModelsStoreOperationOptions {
    pub signal: Option<tokio_util::sync::CancellationToken>,
}

/// Storage failure of a [`ModelsStore`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ModelsStoreError(pub String);

/// `ModelsStore`
pub trait ModelsStore: Send + Sync {
    fn read(
        &self,
        provider_id: &str,
        options: Option<ModelsStoreOperationOptions>,
    ) -> BoxFuture<'_, Result<Option<ModelsStoreEntry>, ModelsStoreError>>;

    fn write(
        &self,
        provider_id: &str,
        entry: ModelsStoreEntry,
        options: Option<ModelsStoreOperationOptions>,
    ) -> BoxFuture<'_, Result<(), ModelsStoreError>>;

    fn delete(
        &self,
        provider_id: &str,
        options: Option<ModelsStoreOperationOptions>,
    ) -> BoxFuture<'_, Result<(), ModelsStoreError>>;
}

fn aborted(options: &Option<ModelsStoreOperationOptions>) -> bool {
    options
        .as_ref()
        .and_then(|options| options.signal.as_ref())
        .is_some_and(tokio_util::sync::CancellationToken::is_cancelled)
}

#[derive(Default, Clone)]
pub struct InMemoryModelsStore {
    entries: Arc<Mutex<BTreeMap<String, ModelsStoreEntry>>>,
}

impl InMemoryModelsStore {
    pub fn new() -> Self {
        InMemoryModelsStore::default()
    }
}

impl ModelsStore for InMemoryModelsStore {
    fn read(
        &self,
        provider_id: &str,
        options: Option<ModelsStoreOperationOptions>,
    ) -> BoxFuture<'_, Result<Option<ModelsStoreEntry>, ModelsStoreError>> {
        let provider_id = provider_id.to_string();
        let entries = Arc::clone(&self.entries);
        Box::pin(async move {
            if aborted(&options) {
                return Err(ModelsStoreError("The operation was aborted".to_string()));
            }
            Ok(entries
                .lock()
                .expect("models store poisoned")
                .get(&provider_id)
                .cloned())
        })
    }

    fn write(
        &self,
        provider_id: &str,
        entry: ModelsStoreEntry,
        options: Option<ModelsStoreOperationOptions>,
    ) -> BoxFuture<'_, Result<(), ModelsStoreError>> {
        let provider_id = provider_id.to_string();
        let entries = Arc::clone(&self.entries);
        Box::pin(async move {
            if aborted(&options) {
                return Err(ModelsStoreError("The operation was aborted".to_string()));
            }
            entries
                .lock()
                .expect("models store poisoned")
                .insert(provider_id, entry);
            Ok(())
        })
    }

    fn delete(
        &self,
        provider_id: &str,
        options: Option<ModelsStoreOperationOptions>,
    ) -> BoxFuture<'_, Result<(), ModelsStoreError>> {
        let provider_id = provider_id.to_string();
        let entries = Arc::clone(&self.entries);
        Box::pin(async move {
            if aborted(&options) {
                return Err(ModelsStoreError("The operation was aborted".to_string()));
            }
            entries
                .lock()
                .expect("models store poisoned")
                .remove(&provider_id);
            Ok(())
        })
    }
}
