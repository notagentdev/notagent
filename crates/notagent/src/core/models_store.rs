use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use futures::FutureExt;
use futures::future::{BoxFuture, Shared};
use notagent_ai::auth::types::AuthOperationOptions;
use notagent_ai::models_store::{
    ModelsStore, ModelsStoreEntry, ModelsStoreError, ModelsStoreOperationOptions,
};
use tokio_util::sync::CancellationToken;

use crate::core::auth_storage::{AuthStorageBackend, FileAuthStorageBackend};
use crate::utils::abort::race_with_abort_signal;
use crate::utils::paths::{PathError, get_file_revision, normalize_path_default};

/// `InMemoryCodingAgentModelsStore`
/// duplicating the implementation.
pub type InMemoryCodingAgentModelsStore = notagent_ai::models_store::InMemoryModelsStore;

type StoredModels = BTreeMap<String, ModelsStoreEntry>;
type ReloadResult = Result<StoredModels, ModelsStoreError>;

#[derive(Clone)]
struct ModelsFileReload {
    id: u64,
    controller: CancellationToken,
    promise: Shared<BoxFuture<'static, ReloadResult>>,
    readers: usize,
}

#[derive(Default)]
struct ModelsFileReadState {
    data: StoredModels,
    revision: Option<String>,
    reload: Option<ModelsFileReload>,
}

struct SharedReadState {
    path: String,
    read_state: Arc<Mutex<ModelsFileReadState>>,
}

/// Optimize the common path without retaining an unbounded set of custom paths.
static SHARED_MODELS_FILE_READ_STATE: LazyLock<Mutex<Option<SharedReadState>>> =
    LazyLock::new(|| Mutex::new(None));

static NEXT_RELOAD_ID: AtomicU64 = AtomicU64::new(1);

fn aborted() -> ModelsStoreError {
    ModelsStoreError("The operation was aborted".to_owned())
}

fn signal_of(options: &Option<ModelsStoreOperationOptions>) -> Option<&CancellationToken> {
    options.as_ref().and_then(|options| options.signal.as_ref())
}

fn throw_if_aborted(options: &Option<ModelsStoreOperationOptions>) -> Result<(), ModelsStoreError> {
    match signal_of(options) {
        Some(signal) if signal.is_cancelled() => Err(aborted()),
        _ => Ok(()),
    }
}

fn as_auth_options(options: &Option<ModelsStoreOperationOptions>) -> Option<AuthOperationOptions> {
    options.as_ref().map(|options| AuthOperationOptions {
        signal: options.signal.clone(),
    })
}

fn parse(content: Option<&str>) -> StoredModels {
    // failure, which the caller surfaces as a store error.
    match content {
        Some(content) if !content.is_empty() => {
            serde_json::from_str(content).unwrap_or_else(|_| StoredModels::new())
        }
        _ => StoredModels::new(),
    }
}

/// Locked JSON-backed storage for dynamically refreshed provider catalogs.
pub struct FileModelsStore {
    storage: Arc<dyn AuthStorageBackend>,
    path: String,
    read_state: Arc<Mutex<ModelsFileReadState>>,
}

impl std::fmt::Debug for FileModelsStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FileModelsStore")
            .field("path", &self.path)
            .finish()
    }
}

impl FileModelsStore {
    pub fn new(path: &str) -> Result<Self, PathError> {
        let path = normalize_path_default(path)?;
        let storage: Arc<dyn AuthStorageBackend> =
            Arc::new(FileAuthStorageBackend::at(Path::new(&path)));
        let read_state = {
            let mut shared = SHARED_MODELS_FILE_READ_STATE
                .lock()
                .expect("shared models read state");
            match shared.as_ref() {
                Some(existing) if existing.path == path => Arc::clone(&existing.read_state),
                Some(_) => Arc::new(Mutex::new(ModelsFileReadState::default())),
                None => {
                    let read_state = Arc::new(Mutex::new(ModelsFileReadState::default()));
                    *shared = Some(SharedReadState {
                        path: path.clone(),
                        read_state: Arc::clone(&read_state),
                    });
                    read_state
                }
            }
        };
        Ok(Self {
            storage,
            path,
            read_state,
        })
    }

    /// Test seam for `vi.spyOn(lockfile, "lock")`: the same store over a wrapped
    /// backend, so a suite can observe or gate the file lock.
    pub fn with_backend(
        storage: Arc<dyn AuthStorageBackend>,
        path: &str,
    ) -> Result<Self, PathError> {
        let mut store = Self::new(path)?;
        store.storage = storage;
        Ok(store)
    }

    pub fn at_default_path() -> Result<Self, PathError> {
        Self::new(
            &crate::config::get_agent_dir()
                .join("models-store.json")
                .to_string_lossy(),
        )
    }

    fn update_read_state(&self, data: StoredModels, revision: Option<String>) {
        let mut state = self.read_state.lock().expect("models read state");
        state.data = data;
        state.revision = revision;
    }

    fn reload_from_storage(
        &self,
        options: Option<ModelsStoreOperationOptions>,
    ) -> BoxFuture<'static, ReloadResult> {
        let storage = Arc::clone(&self.storage);
        let path = self.path.clone();
        let read_state = Arc::clone(&self.read_state);
        Box::pin(async move {
            let captured: Arc<Mutex<StoredModels>> = Arc::new(Mutex::new(StoredModels::new()));
            let result = Arc::clone(&captured);
            storage
                .with_lock_async(
                    Box::new(move |content| {
                        Box::pin(async move {
                            let data = parse(content.as_deref());
                            let revision = get_file_revision(&path);
                            {
                                let mut state = read_state.lock().expect("models read state");
                                state.data = data.clone();
                                state.revision = revision;
                            }
                            *result.lock().expect("reload result") = data;
                            Ok(None)
                        })
                    }),
                    as_auth_options(&options),
                )
                .await
                .map_err(|error| ModelsStoreError(error.0))?;
            let data = captured.lock().expect("reload result").clone();
            Ok(data)
        })
    }

    async fn read_latest(
        &self,
        options: &Option<ModelsStoreOperationOptions>,
    ) -> Result<StoredModels, ModelsStoreError> {
        throw_if_aborted(options)?;
        let revision = get_file_revision(&self.path);
        {
            let state = self.read_state.lock().expect("models read state");
            if revision.is_some() && revision == state.revision {
                return Ok(state.data.clone());
            }
        }

        let reload = self.join_or_start_reload();
        let result = race_with_abort_signal(reload.promise.clone(), signal_of(options)).await;
        self.leave_reload(reload.id);
        match result {
            Ok(result) => result,
            Err(_) => Err(aborted()),
        }
    }

    fn join_or_start_reload(&self) -> ModelsFileReload {
        let mut state = self.read_state.lock().expect("models read state");
        if state.reload.is_none() {
            let controller = CancellationToken::new();
            let future = self.reload_from_storage(Some(ModelsStoreOperationOptions {
                signal: Some(controller.clone()),
            }));
            // The reload runs on its own task so that a leaving reader cannot cancel
            // it while other readers still wait for the same result.
            let handle = tokio::spawn(future);
            let promise: BoxFuture<'static, ReloadResult> = Box::pin(async move {
                match handle.await {
                    Ok(result) => result,
                    Err(error) => Err(ModelsStoreError(error.to_string())),
                }
            });
            state.reload = Some(ModelsFileReload {
                id: NEXT_RELOAD_ID.fetch_add(1, Ordering::SeqCst),
                controller,
                promise: promise.shared(),
                readers: 0,
            });
        }
        let reload = state.reload.as_mut().expect("reload");
        reload.readers += 1;
        reload.clone()
    }

    fn leave_reload(&self, id: u64) {
        let mut state = self.read_state.lock().expect("models read state");
        let Some(reload) = state.reload.as_mut().filter(|reload| reload.id == id) else {
            return;
        };
        reload.readers -= 1;
        if reload.readers == 0 {
            let controller = reload.controller.clone();
            state.reload = None;
            controller.cancel();
        }
    }

    async fn mutate(
        &self,
        options: Option<ModelsStoreOperationOptions>,
        change: impl FnOnce(&mut StoredModels) + Send + 'static,
    ) -> Result<(), ModelsStoreError> {
        let latest: Arc<Mutex<Option<StoredModels>>> = Arc::new(Mutex::new(None));
        let captured = Arc::clone(&latest);
        self.storage
            .with_lock_async(
                Box::new(move |content| {
                    Box::pin(async move {
                        let mut current = parse(content.as_deref());
                        change(&mut current);
                        let next = serde_json::to_string_pretty(&current)
                            .expect("models store serializes");
                        *captured.lock().expect("latest models") = Some(current);
                        Ok(Some(next))
                    })
                }),
                as_auth_options(&options),
            )
            .await
            .map_err(|error| ModelsStoreError(error.0))?;
        let latest = latest.lock().expect("latest models").clone();
        if let Some(latest) = latest {
            self.update_read_state(latest, None);
        }
        Ok(())
    }
}

impl ModelsStore for FileModelsStore {
    fn read(
        &self,
        provider_id: &str,
        options: Option<ModelsStoreOperationOptions>,
    ) -> BoxFuture<'_, Result<Option<ModelsStoreEntry>, ModelsStoreError>> {
        let provider_id = provider_id.to_owned();
        Box::pin(async move {
            let entry = self.read_latest(&options).await?.remove(&provider_id);
            throw_if_aborted(&options)?;
            Ok(entry)
        })
    }

    fn write(
        &self,
        provider_id: &str,
        entry: ModelsStoreEntry,
        options: Option<ModelsStoreOperationOptions>,
    ) -> BoxFuture<'_, Result<(), ModelsStoreError>> {
        let provider_id = provider_id.to_owned();
        Box::pin(async move {
            self.mutate(options, move |current| {
                current.insert(provider_id, entry);
            })
            .await
        })
    }

    fn delete(
        &self,
        provider_id: &str,
        options: Option<ModelsStoreOperationOptions>,
    ) -> BoxFuture<'_, Result<(), ModelsStoreError>> {
        let provider_id = provider_id.to_owned();
        Box::pin(async move {
            self.mutate(options, move |current| {
                current.remove(&provider_id);
            })
            .await
        })
    }
}
