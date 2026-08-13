//! Port of `packages/coding-agent/src/core/auth-storage.ts`.
//!
//! `CredentialStore` implementation backed by auth.json. Provider auth
//! orchestration belongs to ModelRuntime and notagent-ai Models.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use futures::FutureExt;
use futures::future::Shared;
use notagent_ai::auth::types::{
    AuthOperationOptions, BoxFuture, Credential, CredentialInfo, CredentialStore,
    CredentialStoreError, ModifyFn,
};
use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use crate::config::get_auth_path;
use crate::core::resolve_config_value::{is_command_config_value, resolve_config_value};
use crate::utils::abort::race_with_abort_signal;
use crate::utils::lockfile::{LockError, LockGuard, LockOptions, try_lock};
use crate::utils::paths::{PathError, get_file_revision, normalize_path_default};

/// `Record<string, Credential>` — kept as raw JSON so that entries this build
/// does not understand survive a read/write round-trip, exactly like the TS
/// version, which parses auth.json without validating it.
pub type AuthStorageData = Map<String, Value>;

/// The callback of `withLock`: it receives the current file content and returns
/// the content to write, if any.
///
/// Deviation (class 1): TS returns `{ result, next }` because a JS callback
/// cannot write to its caller's binding. A Rust closure can, so only `next`
/// travels through the backend and the trait stays object-safe.
pub type SyncLockFn<'a> =
    Box<dyn FnOnce(Option<String>) -> Result<Option<String>, CredentialStoreError> + Send + 'a>;

pub type AsyncLockFn<'a> = Box<
    dyn FnOnce(Option<String>) -> BoxFuture<'a, Result<Option<String>, CredentialStoreError>>
        + Send
        + 'a,
>;

fn aborted_error() -> CredentialStoreError {
    CredentialStoreError("The operation was aborted".to_owned())
}

fn signal_of(options: &Option<AuthOperationOptions>) -> Option<&CancellationToken> {
    options.as_ref().and_then(|options| options.signal.as_ref())
}

fn throw_if_aborted(options: &Option<AuthOperationOptions>) -> Result<(), CredentialStoreError> {
    match signal_of(options) {
        Some(signal) if signal.is_cancelled() => Err(aborted_error()),
        _ => Ok(()),
    }
}

pub trait AuthStorageBackend: Send + Sync {
    fn with_lock(&self, apply: SyncLockFn<'_>) -> Result<(), CredentialStoreError>;

    fn with_lock_async<'a>(
        &'a self,
        apply: AsyncLockFn<'a>,
        options: Option<AuthOperationOptions>,
    ) -> BoxFuture<'a, Result<(), CredentialStoreError>>;
}

// =============================================================================
// File backend
// =============================================================================

/// auth.json is written 0600 in a 0700 directory: it holds bearer tokens.
const AUTH_FILE_MODE: u32 = 0o600;
const AUTH_DIR_MODE: u32 = 0o700;

const LOCK_STALE: Duration = Duration::from_secs(30);
const LOCK_MAX_DELAY: Duration = Duration::from_secs(2);

pub struct FileAuthStorageBackend {
    auth_path: PathBuf,
    lock_options: LockOptions,
}

impl std::fmt::Debug for FileAuthStorageBackend {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FileAuthStorageBackend")
            .field("auth_path", &self.auth_path)
            .finish()
    }
}

impl FileAuthStorageBackend {
    pub fn new(auth_path: &str) -> Result<Self, PathError> {
        Ok(Self::at(Path::new(&normalize_path_default(auth_path)?)))
    }

    /// The backend for an already normalized path.
    pub fn at(auth_path: &Path) -> Self {
        Self {
            auth_path: auth_path.to_path_buf(),
            lock_options: LockOptions {
                stale: LOCK_STALE,
                ..LockOptions::default()
            },
        }
    }

    /// The heartbeat interval of the underlying lock; used by the tests.
    pub fn with_lock_update_interval(mut self, update: Duration) -> Self {
        self.lock_options.update = Some(update);
        self
    }

    pub fn auth_path(&self) -> &Path {
        &self.auth_path
    }

    fn ensure_parent_dir(&self) -> Result<(), CredentialStoreError> {
        let Some(directory) = self.auth_path.parent() else {
            return Ok(());
        };
        if directory.as_os_str().is_empty() || directory.exists() {
            return Ok(());
        }
        create_dir_all_with_mode(directory).map_err(|error| CredentialStoreError(error.to_string()))
    }

    fn ensure_file_exists(&self) -> Result<(), CredentialStoreError> {
        if self.auth_path.exists() {
            return Ok(());
        }
        write_auth_file(&self.auth_path, "{}")
            .map_err(|error| CredentialStoreError(error.to_string()))
    }

    fn read_current(&self) -> Result<Option<String>, CredentialStoreError> {
        if !self.auth_path.exists() {
            return Ok(None);
        }
        std::fs::read_to_string(&self.auth_path)
            .map(Some)
            .map_err(|error| CredentialStoreError(error.to_string()))
    }

    fn commit(&self, next: Option<String>) -> Result<(), CredentialStoreError> {
        let Some(next) = next else { return Ok(()) };
        write_auth_file(&self.auth_path, &next)
            .map_err(|error| CredentialStoreError(error.to_string()))
    }

    /// `acquireLockSyncWithRetry`: 10 attempts, 20 ms apart.
    fn acquire_lock_sync_with_retry(&self) -> Result<LockGuard, CredentialStoreError> {
        crate::utils::lockfile::lock_with_retry(
            &self.auth_path,
            &self.lock_options,
            10,
            Duration::from_millis(20),
        )
        .map_err(|error| CredentialStoreError(error.to_string()))
    }

    /// `acquireLockAsync`: retry with exponential backoff plus jitter until the
    /// stale window has elapsed, and give the lock back when the caller aborts
    /// in the same tick.
    async fn acquire_lock_async(
        &self,
        options: &Option<AuthOperationOptions>,
        compromised: &Arc<Mutex<Option<LockError>>>,
    ) -> Result<LockGuard, CredentialStoreError> {
        let recorder = Arc::clone(compromised);
        let lock_options = LockOptions {
            on_compromised: Some(Arc::new(move |error| {
                let mut slot = recorder.lock().expect("compromise recorder");
                if slot.is_none() {
                    *slot = Some(error);
                }
            })),
            ..self.lock_options.clone()
        };
        let deadline = std::time::Instant::now() + LOCK_STALE;
        let mut retry = 0u32;
        loop {
            throw_if_aborted(options)?;
            match try_lock(&self.auth_path, &lock_options) {
                Ok(guard) => {
                    if signal_of(options).is_some_and(|signal| signal.is_cancelled()) {
                        drop(guard);
                        return Err(aborted_error());
                    }
                    return Ok(guard);
                }
                Err(LockError::Locked) => {
                    let now = std::time::Instant::now();
                    if now >= deadline {
                        return Err(CredentialStoreError(LockError::Locked.to_string()));
                    }
                    let remaining = deadline - now;
                    let base = Duration::from_millis(10 * 2u64.saturating_pow(retry.min(20)))
                        .min(LOCK_MAX_DELAY / 2);
                    retry += 1;
                    let delay = base.mul_f64(1.0 + jitter()).min(remaining);
                    match signal_of(options) {
                        Some(signal) => {
                            tokio::select! {
                                () = tokio::time::sleep(delay) => {}
                                () = signal.cancelled() => return Err(aborted_error()),
                            }
                        }
                        None => tokio::time::sleep(delay).await,
                    }
                }
                Err(error) => return Err(CredentialStoreError(error.to_string())),
            }
        }
    }
}

/// `Math.random()` stands in for lock jitter; the value only spreads retries.
fn jitter() -> f64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.subsec_nanos())
        .unwrap_or(0);
    f64::from(nanos % 1_000_000) / 1_000_000.0
}

fn create_dir_all_with_mode(directory: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(AUTH_DIR_MODE)
            .create(directory)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(directory)
    }
}

fn write_auth_file(path: &Path, contents: &str) -> std::io::Result<()> {
    std::fs::write(path, contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(AUTH_FILE_MODE))?;
    }
    Ok(())
}

impl AuthStorageBackend for FileAuthStorageBackend {
    fn with_lock(&self, apply: SyncLockFn<'_>) -> Result<(), CredentialStoreError> {
        self.ensure_parent_dir()?;
        self.ensure_file_exists()?;
        let guard = self.acquire_lock_sync_with_retry()?;
        let result = (|| {
            let current = self.read_current()?;
            self.commit(apply(current)?)
        })();
        guard.release();
        result
    }

    fn with_lock_async<'a>(
        &'a self,
        apply: AsyncLockFn<'a>,
        options: Option<AuthOperationOptions>,
    ) -> BoxFuture<'a, Result<(), CredentialStoreError>> {
        Box::pin(async move {
            throw_if_aborted(&options)?;
            self.ensure_parent_dir()?;
            self.ensure_file_exists()?;

            let compromised: Arc<Mutex<Option<LockError>>> = Arc::new(Mutex::new(None));
            let throw_if_compromised = || -> Result<(), CredentialStoreError> {
                match compromised.lock().expect("compromise recorder").clone() {
                    Some(error) => Err(CredentialStoreError(error.to_string())),
                    None => Ok(()),
                }
            };

            let guard = self.acquire_lock_async(&options, &compromised).await?;
            let result = async {
                throw_if_compromised()?;
                throw_if_aborted(&options)?;
                let current = self.read_current()?;
                let next = apply(current).await?;
                throw_if_compromised()?;
                throw_if_aborted(&options)?;
                self.commit(next)?;
                throw_if_compromised()
            }
            .await;
            guard.release();
            result
        })
    }
}

// =============================================================================
// In-memory backend
// =============================================================================

#[derive(Debug, Default)]
pub struct InMemoryAuthStorageBackend {
    value: Mutex<Option<String>>,
    /// The TS promise chain: mutations run one at a time, in arrival order.
    chain: tokio::sync::Mutex<()>,
}

impl InMemoryAuthStorageBackend {
    pub fn new() -> Self {
        Self::default()
    }
}

impl AuthStorageBackend for InMemoryAuthStorageBackend {
    fn with_lock(&self, apply: SyncLockFn<'_>) -> Result<(), CredentialStoreError> {
        let current = self.value.lock().expect("auth storage value").clone();
        if let Some(next) = apply(current)? {
            *self.value.lock().expect("auth storage value") = Some(next);
        }
        Ok(())
    }

    fn with_lock_async<'a>(
        &'a self,
        apply: AsyncLockFn<'a>,
        options: Option<AuthOperationOptions>,
    ) -> BoxFuture<'a, Result<(), CredentialStoreError>> {
        Box::pin(async move {
            let operation = async {
                let _turn = self.chain.lock().await;
                throw_if_aborted(&options)?;
                let current = self.value.lock().expect("auth storage value").clone();
                let next = apply(current).await?;
                throw_if_aborted(&options)?;
                if let Some(next) = next {
                    *self.value.lock().expect("auth storage value") = Some(next);
                }
                Ok(())
            };
            // Deviation (class 1): TS keeps an abandoned mutation running because a
            // JS promise outlives its awaiter. A Rust future is cancelled when it is
            // dropped, so an aborted mutation stops where it is — it still cannot
            // commit, and later mutations still run in order.
            match race_with_abort_signal(operation, signal_of(&options)).await {
                Ok(result) => result,
                Err(_) => Err(aborted_error()),
            }
        })
    }
}

// =============================================================================
// AuthStorage
// =============================================================================

type ReloadResult = Result<AuthStorageData, CredentialStoreError>;

#[derive(Clone)]
struct AuthFileReload {
    id: u64,
    controller: CancellationToken,
    promise: Shared<BoxFuture<'static, ReloadResult>>,
    readers: usize,
}

#[derive(Default)]
struct AuthFileReadState {
    data: AuthStorageData,
    revision: Option<String>,
    reload: Option<AuthFileReload>,
}

struct SharedReadState {
    auth_path: String,
    read_state: Arc<Mutex<AuthFileReadState>>,
}

/// The first file-backed store publishes its read state so that further stores
/// for the same path coalesce their reloads instead of locking the file again.
static SHARED_AUTH_FILE_READ_STATE: LazyLock<Mutex<Option<SharedReadState>>> =
    LazyLock::new(|| Mutex::new(None));

static NEXT_RELOAD_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Credential storage backed by a JSON file.
pub struct AuthStorage {
    storage: Arc<dyn AuthStorageBackend>,
    auth_path: Option<String>,
    read_state: Arc<Mutex<AuthFileReadState>>,
}

impl std::fmt::Debug for AuthStorage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AuthStorage")
            .field("auth_path", &self.auth_path)
            .finish()
    }
}

impl AuthStorage {
    fn new(storage: Arc<dyn AuthStorageBackend>, auth_path: Option<String>) -> Self {
        let read_state = {
            let mut shared = SHARED_AUTH_FILE_READ_STATE
                .lock()
                .expect("shared auth read state");
            match (&auth_path, shared.as_ref()) {
                (Some(path), Some(existing)) if &existing.auth_path == path => {
                    Arc::clone(&existing.read_state)
                }
                (Some(path), None) => {
                    let read_state = Arc::new(Mutex::new(AuthFileReadState::default()));
                    *shared = Some(SharedReadState {
                        auth_path: path.clone(),
                        read_state: Arc::clone(&read_state),
                    });
                    read_state
                }
                _ => Arc::new(Mutex::new(AuthFileReadState::default())),
            }
        };
        let storage = Self {
            storage,
            auth_path,
            read_state,
        };
        if let Some(path) = &storage.auth_path {
            let revision = get_file_revision(path);
            let known = storage
                .read_state
                .lock()
                .expect("auth read state")
                .revision
                .clone();
            if revision.is_some() && revision == known {
                return storage;
            }
        }
        storage.reload();
        storage
    }

    /// `AuthStorage.create(authPath)`.
    pub fn create(auth_path: &str) -> Result<Self, PathError> {
        let normalized = normalize_path_default(auth_path)?;
        let backend = FileAuthStorageBackend::at(Path::new(&normalized));
        Ok(Self::new(Arc::new(backend), Some(normalized)))
    }

    /// `AuthStorage.create()` — the auth.json of the agent directory.
    pub fn create_default() -> Result<Self, PathError> {
        Self::create(&get_auth_path().to_string_lossy())
    }

    pub fn from_storage(storage: Arc<dyn AuthStorageBackend>) -> Self {
        Self::new(storage, None)
    }

    /// `new AuthStorage(storage, authPath)` — a custom backend that still takes
    /// part in the shared read state of `authPath`.
    pub fn from_storage_at(
        storage: Arc<dyn AuthStorageBackend>,
        auth_path: &str,
    ) -> Result<Self, PathError> {
        Ok(Self::new(storage, Some(normalize_path_default(auth_path)?)))
    }

    pub fn in_memory(data: AuthStorageData) -> Self {
        let storage = InMemoryAuthStorageBackend::new();
        let serialized = serialize_storage_data(&data);
        storage
            .with_lock(Box::new(move |_current| Ok(Some(serialized))))
            .expect("in-memory storage never fails");
        Self::from_storage(Arc::new(storage))
    }

    /// Reload credentials from storage.
    pub fn reload(&self) {
        let captured: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let content = Arc::clone(&captured);
        let outcome = self.storage.with_lock(Box::new(move |current| {
            *content.lock().expect("reload content") = current;
            Ok(None)
        }));
        if outcome.is_err() {
            // Preserve the last valid in-memory snapshot.
            return;
        }
        let revision = self.auth_path.as_deref().and_then(get_file_revision);
        let content = captured.lock().expect("reload content").clone();
        match parse_storage_data(content.as_deref()) {
            Ok(data) => self.update_read_state(data, revision),
            Err(_) => {
                // Preserve the last valid in-memory snapshot.
            }
        }
    }

    fn update_read_state(&self, data: AuthStorageData, revision: Option<String>) {
        let mut state = self.read_state.lock().expect("auth read state");
        state.data = data;
        state.revision = revision;
    }

    fn current_data(&self) -> AuthStorageData {
        self.read_state
            .lock()
            .expect("auth read state")
            .data
            .clone()
    }

    /// `reloadFromStorageAsync` as an owned future, so several readers can share it.
    fn reload_from_storage_async(
        &self,
        options: Option<AuthOperationOptions>,
    ) -> BoxFuture<'static, ReloadResult> {
        let storage = Arc::clone(&self.storage);
        let auth_path = self.auth_path.clone();
        let read_state = Arc::clone(&self.read_state);
        Box::pin(async move {
            let captured: Arc<Mutex<AuthStorageData>> =
                Arc::new(Mutex::new(AuthStorageData::new()));
            let result = Arc::clone(&captured);
            storage
                .with_lock_async(
                    Box::new(move |content| {
                        Box::pin(async move {
                            let current = parse_storage_data(content.as_deref())?;
                            let revision = auth_path.as_deref().and_then(get_file_revision);
                            {
                                let mut state = read_state.lock().expect("auth read state");
                                state.data = current.clone();
                                state.revision = revision;
                            }
                            *result.lock().expect("reload result") = current;
                            Ok(None)
                        })
                    }),
                    options,
                )
                .await?;
            let data = captured.lock().expect("reload result").clone();
            Ok(data)
        })
    }

    async fn read_latest_data(
        &self,
        options: &Option<AuthOperationOptions>,
    ) -> Result<AuthStorageData, CredentialStoreError> {
        throw_if_aborted(options)?;
        let Some(auth_path) = self.auth_path.clone() else {
            let reload = self.reload_from_storage_async(options.clone());
            return match reload.await {
                Ok(data) => Ok(data),
                Err(error) if signal_of(options).is_some() => Err(error),
                Err(_) => Ok(self.current_data()),
            };
        };

        let revision = get_file_revision(&auth_path);
        {
            let state = self.read_state.lock().expect("auth read state");
            if revision.is_some() && revision == state.revision {
                return Ok(state.data.clone());
            }
        }

        let reload = self.join_or_start_reload();
        let result = race_with_abort_signal(reload.promise.clone(), signal_of(options)).await;
        self.leave_reload(reload.id);

        match result {
            Ok(Ok(data)) => Ok(data),
            Ok(Err(error)) if signal_of(options).is_some() => Err(error),
            Ok(Err(_)) => Ok(self.current_data()),
            Err(_) => Err(aborted_error()),
        }
    }

    fn join_or_start_reload(&self) -> AuthFileReload {
        let mut state = self.read_state.lock().expect("auth read state");
        if state.reload.is_none() {
            let controller = CancellationToken::new();
            let future = self.reload_from_storage_async(Some(AuthOperationOptions {
                signal: Some(controller.clone()),
            }));
            // The reload runs on its own task so that a reader leaving does not
            // cancel it while other readers still wait for the same result.
            let handle = tokio::spawn(future);
            let promise: BoxFuture<'static, ReloadResult> = Box::pin(async move {
                match handle.await {
                    Ok(result) => result,
                    Err(error) => Err(CredentialStoreError(error.to_string())),
                }
            });
            state.reload = Some(AuthFileReload {
                id: NEXT_RELOAD_ID.fetch_add(1, std::sync::atomic::Ordering::SeqCst),
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
        let mut state = self.read_state.lock().expect("auth read state");
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

    /// List credential metadata without resolving configured key values.
    async fn list_impl(
        &self,
        options: Option<AuthOperationOptions>,
    ) -> Result<Vec<CredentialInfo>, CredentialStoreError> {
        let data = self.read_latest_data(&options).await?;
        throw_if_aborted(&options)?;
        data.iter()
            .map(|(provider_id, value)| credential_info(provider_id, value))
            .collect()
    }

    async fn read_impl(
        &self,
        provider: &str,
        options: Option<AuthOperationOptions>,
    ) -> Result<Option<Credential>, CredentialStoreError> {
        let data = self.read_latest_data(&options).await?;
        throw_if_aborted(&options)?;
        let Some(value) = data.get(provider) else {
            return Ok(None);
        };
        let credential = credential_from_value(provider, value)?;
        Ok(Some(resolve_credential(credential)))
    }

    async fn modify_impl<'a>(
        &'a self,
        provider: &'a str,
        modify: ModifyFn<'a>,
        options: Option<AuthOperationOptions>,
    ) -> Result<Option<Credential>, CredentialStoreError> {
        let latest_data: Arc<Mutex<AuthStorageData>> = Arc::new(Mutex::new(self.current_data()));
        let revision: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let result: Arc<Mutex<Option<Credential>>> = Arc::new(Mutex::new(None));

        let auth_path = self.auth_path.clone();
        let captured_data = Arc::clone(&latest_data);
        let captured_revision = Arc::clone(&revision);
        let captured_result = Arc::clone(&result);
        self.storage
            .with_lock_async(
                Box::new(move |content| {
                    Box::pin(async move {
                        let mut current_data = parse_storage_data(content.as_deref())?;
                        let current = match current_data.get(provider) {
                            Some(value) => Some(credential_from_value(provider, value)?),
                            None => None,
                        };
                        let next = modify(current.clone()).await?;
                        let Some(next) = next else {
                            *captured_data.lock().expect("latest data") = current_data;
                            *captured_revision.lock().expect("revision") =
                                auth_path.as_deref().and_then(get_file_revision);
                            *captured_result.lock().expect("modify result") = current;
                            return Ok(None);
                        };
                        current_data.insert(provider.to_owned(), credential_to_value(&next)?);
                        *captured_data.lock().expect("latest data") = current_data.clone();
                        *captured_result.lock().expect("modify result") = Some(next);
                        Ok(Some(serialize_storage_data(&current_data)))
                    })
                }),
                options,
            )
            .await?;

        let data = latest_data.lock().expect("latest data").clone();
        let revision = revision.lock().expect("revision").clone();
        self.update_read_state(data, revision);
        let result = result.lock().expect("modify result").clone();
        Ok(result)
    }

    async fn delete_impl(
        &self,
        provider: &str,
        options: Option<AuthOperationOptions>,
    ) -> Result<(), CredentialStoreError> {
        let latest_data: Arc<Mutex<AuthStorageData>> = Arc::new(Mutex::new(self.current_data()));
        let captured_data = Arc::clone(&latest_data);
        let provider = provider.to_owned();
        self.storage
            .with_lock_async(
                Box::new(move |content| {
                    Box::pin(async move {
                        let mut current_data = parse_storage_data(content.as_deref())?;
                        // `shift_remove` keeps the order of the remaining providers,
                        // which is what `delete obj[key]` does in JS.
                        current_data.shift_remove(&provider);
                        *captured_data.lock().expect("latest data") = current_data.clone();
                        Ok(Some(serialize_storage_data(&current_data)))
                    })
                }),
                options,
            )
            .await?;
        let data = latest_data.lock().expect("latest data").clone();
        self.update_read_state(data, None);
        Ok(())
    }
}

impl CredentialStore for AuthStorage {
    fn read(
        &self,
        provider_id: &str,
        options: Option<AuthOperationOptions>,
    ) -> BoxFuture<'_, Result<Option<Credential>, CredentialStoreError>> {
        let provider_id = provider_id.to_owned();
        Box::pin(async move { self.read_impl(&provider_id, options).await })
    }

    fn list(
        &self,
        options: Option<AuthOperationOptions>,
    ) -> BoxFuture<'_, Result<Vec<CredentialInfo>, CredentialStoreError>> {
        Box::pin(self.list_impl(options))
    }

    fn modify<'a>(
        &'a self,
        provider_id: &'a str,
        modify: ModifyFn<'a>,
        options: Option<AuthOperationOptions>,
    ) -> BoxFuture<'a, Result<Option<Credential>, CredentialStoreError>> {
        Box::pin(self.modify_impl(provider_id, modify, options))
    }

    fn delete(
        &self,
        provider_id: &str,
        options: Option<AuthOperationOptions>,
    ) -> BoxFuture<'_, Result<(), CredentialStoreError>> {
        let provider_id = provider_id.to_owned();
        Box::pin(async move { self.delete_impl(&provider_id, options).await })
    }
}

// =============================================================================
// Read-only storage
// =============================================================================

/// Credential storage that reads auth.json, validates it strictly and never
/// executes a configured command.
pub struct ReadOnlyAuthStorage {
    auth_path: String,
    data: Mutex<Option<AuthStorageData>>,
}

impl std::fmt::Debug for ReadOnlyAuthStorage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ReadOnlyAuthStorage")
            .field("auth_path", &self.auth_path)
            .finish()
    }
}

impl ReadOnlyAuthStorage {
    pub fn new(auth_path: &str) -> Result<Self, PathError> {
        Ok(Self {
            auth_path: normalize_path_default(auth_path)?,
            data: Mutex::new(None),
        })
    }

    pub fn create_default() -> Result<Self, PathError> {
        Self::new(&get_auth_path().to_string_lossy())
    }

    fn load(&self) -> Result<AuthStorageData, CredentialStoreError> {
        if let Some(data) = self.data.lock().expect("read-only auth data").clone() {
            return Ok(data);
        }

        let contents = match std::fs::read_to_string(&self.auth_path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let data = AuthStorageData::new();
                *self.data.lock().expect("read-only auth data") = Some(data.clone());
                return Ok(data);
            }
            Err(error) => {
                return Err(CredentialStoreError(format!(
                    "Failed to read auth.json: {error}"
                )));
            }
        };
        let parsed: Value = serde_json::from_str(&contents)
            .map_err(|error| CredentialStoreError(format!("Failed to read auth.json: {error}")))?;
        let Value::Object(parsed) = parsed else {
            return Err(CredentialStoreError(
                "Invalid auth.json: expected an object".to_owned(),
            ));
        };

        for (provider_id, credential) in &parsed {
            if !is_valid_stored_credential(credential) {
                return Err(CredentialStoreError(format!(
                    "Invalid auth.json credential for provider \"{provider_id}\""
                )));
            }
        }

        *self.data.lock().expect("read-only auth data") = Some(parsed.clone());
        Ok(parsed)
    }
}

fn is_valid_stored_credential(credential: &Value) -> bool {
    let Value::Object(value) = credential else {
        return false;
    };
    match value.get("type").and_then(Value::as_str) {
        Some("api_key") => {
            let valid_key = match value.get("key") {
                None | Some(Value::Null) => value.get("key").is_none(),
                Some(key) => key.is_string(),
            };
            let valid_env = match value.get("env") {
                None => true,
                Some(Value::Object(env)) => env.values().all(Value::is_string),
                Some(_) => false,
            };
            valid_key && valid_env
        }
        Some("oauth") => {
            value.get("access").is_some_and(Value::is_string)
                && value.get("refresh").is_some_and(Value::is_string)
                && value
                    .get("expires")
                    .and_then(Value::as_f64)
                    .is_some_and(f64::is_finite)
        }
        _ => false,
    }
}

impl CredentialStore for ReadOnlyAuthStorage {
    fn read(
        &self,
        provider_id: &str,
        options: Option<AuthOperationOptions>,
    ) -> BoxFuture<'_, Result<Option<Credential>, CredentialStoreError>> {
        let provider_id = provider_id.to_owned();
        Box::pin(async move {
            throw_if_aborted(&options)?;
            let data = self.load()?;
            throw_if_aborted(&options)?;
            let Some(value) = data.get(&provider_id) else {
                return Ok(None);
            };
            let credential = credential_from_value(&provider_id, value)?;
            let Credential::ApiKey(api_key) = &credential else {
                return Ok(Some(credential));
            };
            let Some(key) = api_key.key.as_deref().filter(|key| !key.is_empty()) else {
                return Ok(Some(credential));
            };
            if is_command_config_value(key) {
                return Ok(Some(credential));
            }
            Ok(Some(resolve_credential(credential)))
        })
    }

    fn list(
        &self,
        options: Option<AuthOperationOptions>,
    ) -> BoxFuture<'_, Result<Vec<CredentialInfo>, CredentialStoreError>> {
        Box::pin(async move {
            throw_if_aborted(&options)?;
            let data = self.load()?;
            let credentials: Result<Vec<CredentialInfo>, CredentialStoreError> = data
                .iter()
                .map(|(provider_id, value)| credential_info(provider_id, value))
                .collect();
            throw_if_aborted(&options)?;
            credentials
        })
    }

    fn modify<'a>(
        &'a self,
        _provider_id: &'a str,
        _modify: ModifyFn<'a>,
        _options: Option<AuthOperationOptions>,
    ) -> BoxFuture<'a, Result<Option<Credential>, CredentialStoreError>> {
        Box::pin(async {
            Err(CredentialStoreError(
                "Read-only credential storage cannot modify auth.json".to_owned(),
            ))
        })
    }

    fn delete(
        &self,
        _provider_id: &str,
        _options: Option<AuthOperationOptions>,
    ) -> BoxFuture<'_, Result<(), CredentialStoreError>> {
        Box::pin(async {
            Err(CredentialStoreError(
                "Read-only credential storage cannot modify auth.json".to_owned(),
            ))
        })
    }
}

/// One-off synchronous read of a stored credential from an auth.json file,
/// without instantiating a store or resolving configured key values.
pub fn read_stored_credential(provider_id: &str, auth_path: &str) -> Option<Credential> {
    let normalized = normalize_path_default(auth_path).ok()?;
    let contents = std::fs::read_to_string(normalized).ok()?;
    let data: AuthStorageData = serde_json::from_str(&contents).ok()?;
    serde_json::from_value(data.get(provider_id)?.clone()).ok()
}

// =============================================================================
// Helpers
// =============================================================================

fn parse_storage_data(content: Option<&str>) -> Result<AuthStorageData, CredentialStoreError> {
    let Some(content) = content.filter(|content| !content.is_empty()) else {
        return Ok(AuthStorageData::new());
    };
    serde_json::from_str(content).map_err(|error| CredentialStoreError(error.to_string()))
}

fn serialize_storage_data(data: &AuthStorageData) -> String {
    serde_json::to_string_pretty(data).expect("credential data is serializable")
}

fn credential_from_value(
    provider_id: &str,
    value: &Value,
) -> Result<Credential, CredentialStoreError> {
    serde_json::from_value(value.clone()).map_err(|error| {
        CredentialStoreError(format!(
            "Invalid auth.json credential for provider \"{provider_id}\": {error}"
        ))
    })
}

fn credential_to_value(credential: &Credential) -> Result<Value, CredentialStoreError> {
    serde_json::to_value(credential).map_err(|error| CredentialStoreError(error.to_string()))
}

fn credential_info(
    provider_id: &str,
    value: &Value,
) -> Result<CredentialInfo, CredentialStoreError> {
    let credential = credential_from_value(provider_id, value)?;
    Ok(CredentialInfo {
        provider_id: provider_id.to_owned(),
        credential_type: credential.credential_type(),
    })
}

/// `{ ...credential, key: resolveConfigValue(credential.key, credential.env) }`
fn resolve_credential(credential: Credential) -> Credential {
    let Credential::ApiKey(mut api_key) = credential else {
        return credential;
    };
    let Some(key) = api_key.key.as_deref() else {
        return Credential::ApiKey(api_key);
    };
    let env: Option<BTreeMap<String, String>> = api_key.env.clone();
    api_key.key = resolve_config_value(key, env.as_ref());
    Credential::ApiKey(api_key)
}

/// Test hooks for the process-global read state.
pub mod testing {
    use super::SHARED_AUTH_FILE_READ_STATE;

    /// Forget which auth.json owns the shared read state.
    ///
    /// The TS suite gets this for free because vitest re-imports the module for
    /// every test file; a Rust test binary shares one process.
    pub fn reset_shared_read_state() {
        *SHARED_AUTH_FILE_READ_STATE
            .lock()
            .expect("shared auth read state") = None;
    }
}
