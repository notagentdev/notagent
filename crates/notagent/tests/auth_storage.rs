//! Port of `packages/coding-agent/test/auth-storage.test.ts`.
//!
//! Every test holds `global_lock()`: the shared auth read state is process-wide,
//! and vitest runs the TS file's tests sequentially as well.
#![allow(clippy::await_holding_lock)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use notagent::core::auth_storage::{
    AsyncLockFn, AuthStorage, AuthStorageBackend, AuthStorageData, FileAuthStorageBackend,
    InMemoryAuthStorageBackend, ReadOnlyAuthStorage, SyncLockFn, read_stored_credential, testing,
};
use notagent::utils::lockfile::{LockOptions, lock_path_for, try_lock};
use notagent_ai::auth::types::{
    ApiKeyCredential, AuthOperationOptions, AuthType, BoxFuture, Credential, CredentialStore,
    CredentialStoreError, OAuthCredential,
};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

// The shared read state and the process environment are global, so the tests
// that depend on them run one at a time.
fn global_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let directory = tempfile::Builder::new()
            .prefix("notagent-test-auth-storage-")
            .tempdir()
            .expect("temp dir");
        let path = directory.path().to_path_buf();
        let _ = directory.keep();
        Self { path }
    }

    fn auth_json(&self) -> String {
        self.path.join("auth.json").to_string_lossy().into_owned()
    }

    fn join(&self, name: &str) -> String {
        self.path.join(name).to_string_lossy().into_owned()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn write_auth_json(path: &str, data: Value) {
    std::fs::write(path, serde_json::to_string(&data).expect("json")).expect("write auth.json");
}

fn read_auth_json(path: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).expect("read auth.json")).expect("json")
}

fn storage_data(data: Value) -> AuthStorageData {
    match data {
        Value::Object(map) => map,
        other => panic!("expected an object, got {other}"),
    }
}

fn api_key_credential(key: &str) -> Credential {
    Credential::ApiKey(ApiKeyCredential {
        key: Some(key.to_owned()),
        env: None,
    })
}

fn returns<'a>(credential: Option<Credential>) -> notagent_ai::auth::types::ModifyFn<'a> {
    Box::new(move |_current| Box::pin(async move { Ok(credential) }))
}

fn signal(token: &CancellationToken) -> Option<AuthOperationOptions> {
    Some(AuthOperationOptions {
        signal: Some(token.clone()),
    })
}

/// Counts lock acquisitions the way the TS suite spies on `lockfile.lock`.
#[derive(Default)]
struct LockCounters {
    sync: AtomicUsize,
    asynchronous: AtomicUsize,
    fail_next_async: AtomicBool,
}

struct CountingBackend {
    inner: FileAuthStorageBackend,
    counters: Arc<LockCounters>,
}

impl CountingBackend {
    fn new(path: &str, counters: Arc<LockCounters>) -> Self {
        Self {
            inner: FileAuthStorageBackend::at(Path::new(path)),
            counters,
        }
    }
}

impl AuthStorageBackend for CountingBackend {
    fn with_lock(&self, apply: SyncLockFn<'_>) -> Result<(), CredentialStoreError> {
        self.counters.sync.fetch_add(1, Ordering::SeqCst);
        self.inner.with_lock(apply)
    }

    fn with_lock_async<'a>(
        &'a self,
        apply: AsyncLockFn<'a>,
        options: Option<AuthOperationOptions>,
    ) -> BoxFuture<'a, Result<(), CredentialStoreError>> {
        self.counters.asynchronous.fetch_add(1, Ordering::SeqCst);
        if self.counters.fail_next_async.swap(false, Ordering::SeqCst) {
            return Box::pin(async { Err(CredentialStoreError("lock unavailable".to_owned())) });
        }
        self.inner.with_lock_async(apply, options)
    }
}

fn counting_storage(path: &str, counters: &Arc<LockCounters>) -> AuthStorage {
    AuthStorage::from_storage_at(
        Arc::new(CountingBackend::new(path, Arc::clone(counters))),
        path,
    )
    .expect("storage")
}

#[tokio::test]
async fn reads_and_resolves_stored_api_key_credentials() {
    let _guard = global_lock();
    let directory = TempDir::new();
    let path = directory.auth_json();
    // SAFETY: the global lock keeps other tests from reading the environment
    // while it is being changed.
    unsafe { std::env::set_var("TEST_AUTH_STORAGE_KEY", "environment-key") };
    write_auth_json(
        &path,
        json!({ "anthropic": { "type": "api_key", "key": "$TEST_AUTH_STORAGE_KEY" } }),
    );
    let storage = AuthStorage::create(&path).expect("storage");
    let credential = storage.read("anthropic", None).await.expect("read");
    unsafe { std::env::remove_var("TEST_AUTH_STORAGE_KEY") };
    assert_eq!(credential, Some(api_key_credential("environment-key")));
}

#[tokio::test]
async fn resolves_command_backed_api_key_credentials() {
    let _guard = global_lock();
    let directory = TempDir::new();
    let path = directory.auth_json();
    write_auth_json(
        &path,
        json!({ "anthropic": { "type": "api_key", "key": "!printf 'command-key'" } }),
    );
    let storage = AuthStorage::create(&path).expect("storage");
    assert_eq!(
        storage.read("anthropic", None).await.expect("read"),
        Some(api_key_credential("command-key"))
    );
}

#[tokio::test]
async fn returns_oauth_credentials_unchanged() {
    let _guard = global_lock();
    let credential = Credential::OAuth(OAuthCredential {
        access: "access-token".to_owned(),
        refresh: "refresh-token".to_owned(),
        expires: 1_760_000_000_000,
        extra: serde_json::Map::new(),
    });
    // The stored form is the one the TS app writes: `type: "oauth"`.
    let storage = AuthStorage::in_memory(storage_data(json!({
        "anthropic": {
            "type": "oauth",
            "access": "access-token",
            "refresh": "refresh-token",
            "expires": 1_760_000_000_000i64,
        }
    })));
    assert_eq!(
        storage.read("anthropic", None).await.expect("read"),
        Some(credential)
    );
}

#[tokio::test]
async fn credential_scoped_env_takes_precedence_and_remains_inspectable() {
    let _guard = global_lock();
    let directory = TempDir::new();
    let path = directory.auth_json();
    write_auth_json(
        &path,
        json!({
            "anthropic": {
                "type": "api_key",
                "key": "$SCOPED_KEY",
                "env": { "SCOPED_KEY": "scoped-value", "REGION": "test-region" },
            }
        }),
    );
    let storage = AuthStorage::create(&path).expect("storage");
    let credential = storage
        .read("anthropic", None)
        .await
        .expect("read")
        .expect("credential");
    let api_key = credential.as_api_key().expect("api key credential");
    assert_eq!(api_key.key.as_deref(), Some("scoped-value"));
    let env = api_key.env.clone().expect("env");
    assert_eq!(
        env.get("SCOPED_KEY").map(String::as_str),
        Some("scoped-value")
    );
    assert_eq!(env.get("REGION").map(String::as_str), Some("test-region"));
}

#[tokio::test]
async fn coalesces_file_reloads_across_concurrent_readers_and_storage_instances() {
    let _guard = global_lock();
    testing::reset_shared_read_state();
    let directory = TempDir::new();
    let path = directory.auth_json();
    write_auth_json(
        &path,
        json!({ "anthropic": { "type": "api_key", "key": "old" } }),
    );

    let counters = Arc::new(LockCounters::default());
    let first = counting_storage(&path, &counters);
    let second = counting_storage(&path, &counters);
    // The second instance joins the shared read state instead of re-reading.
    assert_eq!(counters.sync.load(Ordering::SeqCst), 1);

    write_auth_json(
        &path,
        json!({
            "anthropic": { "type": "api_key", "key": "new" },
            "openai": { "type": "api_key", "key": "openai-key" },
        }),
    );

    let (anthropic, openai, credentials) = tokio::join!(
        first.read("anthropic", signal(&CancellationToken::new())),
        second.read("openai", signal(&CancellationToken::new())),
        first.list(signal(&CancellationToken::new())),
    );
    assert_eq!(anthropic.expect("read"), Some(api_key_credential("new")));
    assert_eq!(
        openai.expect("read"),
        Some(api_key_credential("openai-key"))
    );
    let credentials = credentials.expect("list");
    assert_eq!(
        credentials
            .iter()
            .map(|info| info.provider_id.as_str())
            .collect::<Vec<_>>(),
        vec!["anthropic", "openai"]
    );
    assert!(
        credentials
            .iter()
            .all(|info| info.credential_type == AuthType::ApiKey)
    );
    assert_eq!(counters.asynchronous.load(Ordering::SeqCst), 1);

    assert_eq!(
        second.read("anthropic", None).await.expect("read"),
        Some(api_key_credential("new"))
    );
    assert_eq!(counters.asynchronous.load(Ordering::SeqCst), 1);

    let other_path = directory.join("other-auth.json");
    write_auth_json(
        &other_path,
        json!({ "other": { "type": "api_key", "key": "other-key" } }),
    );
    let other_counters = Arc::new(LockCounters::default());
    let other_first = counting_storage(&other_path, &other_counters);
    let other_second = counting_storage(&other_path, &other_counters);
    other_first.read("other", None).await.expect("read");
    other_second.read("other", None).await.expect("read");
    other_first.list(None).await.expect("list");
    assert_eq!(other_counters.asynchronous.load(Ordering::SeqCst), 0);
    assert_eq!(counters.asynchronous.load(Ordering::SeqCst), 1);

    let third = counting_storage(&path, &counters);
    write_auth_json(
        &path,
        json!({ "anthropic": { "type": "api_key", "key": "newest" } }),
    );
    let (first_reload, third_reload) =
        tokio::join!(first.read("anthropic", None), third.read("anthropic", None));
    assert_eq!(
        first_reload.expect("read"),
        Some(api_key_credential("newest"))
    );
    assert_eq!(
        third_reload.expect("read"),
        Some(api_key_credential("newest"))
    );
    assert_eq!(counters.asynchronous.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn keeps_a_coalesced_reload_alive_while_another_credential_reader_is_waiting() {
    let _guard = global_lock();
    testing::reset_shared_read_state();
    let directory = TempDir::new();
    let path = directory.auth_json();
    write_auth_json(
        &path,
        json!({ "anthropic": { "type": "api_key", "key": "old" } }),
    );
    let storage = AuthStorage::create(&path).expect("storage");
    write_auth_json(
        &path,
        json!({ "anthropic": { "type": "api_key", "key": "new" } }),
    );

    // Hold the file lock so the coalesced reload cannot finish yet.
    let held = try_lock(Path::new(&path), &LockOptions::default()).expect("lock");
    let first_signal = CancellationToken::new();
    let second_signal = CancellationToken::new();
    let mut first = Box::pin(storage.read("anthropic", signal(&first_signal)));
    let mut second = Box::pin(storage.read("anthropic", signal(&second_signal)));
    // Start both readers on the same reload.
    tokio::select! {
        _ = &mut first => panic!("the reload must wait for the lock"),
        _ = &mut second => panic!("the reload must wait for the lock"),
        () = tokio::time::sleep(Duration::from_millis(30)) => {}
    }

    first_signal.cancel();
    let first = first.await.expect_err("the aborted reader fails");
    assert_eq!(first.to_string(), "The operation was aborted");
    held.release();
    assert_eq!(second.await.expect("read"), Some(api_key_credential("new")));
    assert!(
        !lock_path_for(Path::new(&path)).exists(),
        "the reload released the lock"
    );
}

#[tokio::test]
async fn modify_persists_a_credential_while_preserving_unrelated_external_edits() {
    let _guard = global_lock();
    let directory = TempDir::new();
    let path = directory.auth_json();
    write_auth_json(
        &path,
        json!({ "anthropic": { "type": "api_key", "key": "old" } }),
    );
    let storage = AuthStorage::create(&path).expect("storage");
    write_auth_json(
        &path,
        json!({
            "anthropic": { "type": "api_key", "key": "old" },
            "openai": { "type": "api_key", "key": "external" },
        }),
    );

    storage
        .modify("anthropic", returns(Some(api_key_credential("new"))), None)
        .await
        .expect("modify");

    assert_eq!(
        read_auth_json(&path),
        json!({
            "anthropic": { "type": "api_key", "key": "new" },
            "openai": { "type": "api_key", "key": "external" },
        })
    );
}

#[tokio::test]
async fn modify_with_none_leaves_the_current_credential_unchanged() {
    let _guard = global_lock();
    let directory = TempDir::new();
    let path = directory.auth_json();
    write_auth_json(
        &path,
        json!({ "anthropic": { "type": "api_key", "key": "stored" } }),
    );
    let storage = AuthStorage::create(&path).expect("storage");
    assert_eq!(
        storage
            .modify("anthropic", returns(None), None)
            .await
            .expect("modify"),
        Some(api_key_credential("stored"))
    );
    assert_eq!(
        storage.read("anthropic", None).await.expect("read"),
        Some(api_key_credential("stored"))
    );
}

#[tokio::test]
async fn serializes_concurrent_modifications() {
    let _guard = global_lock();
    let directory = TempDir::new();
    let path = directory.auth_json();
    write_auth_json(&path, json!({}));
    let first = AuthStorage::create(&path).expect("storage");
    let second = AuthStorage::create(&path).expect("storage");
    let (left, right) = tokio::join!(
        first.modify(
            "anthropic",
            returns(Some(api_key_credential("anthropic-key"))),
            None
        ),
        second.modify(
            "openai",
            returns(Some(api_key_credential("openai-key"))),
            None
        ),
    );
    left.expect("modify");
    right.expect("modify");
    let stored = read_auth_json(&path);
    assert_eq!(
        stored["anthropic"],
        json!({ "type": "api_key", "key": "anthropic-key" })
    );
    assert_eq!(
        stored["openai"],
        json!({ "type": "api_key", "key": "openai-key" })
    );
    assert_eq!(stored.as_object().expect("object").len(), 2);
}

#[tokio::test]
async fn delete_removes_one_credential_while_preserving_others() {
    let _guard = global_lock();
    let directory = TempDir::new();
    let path = directory.auth_json();
    write_auth_json(
        &path,
        json!({
            "anthropic": { "type": "api_key", "key": "anthropic-key" },
            "openai": { "type": "api_key", "key": "openai-key" },
        }),
    );
    let storage = AuthStorage::create(&path).expect("storage");
    write_auth_json(
        &path,
        json!({
            "anthropic": { "type": "api_key", "key": "anthropic-key" },
            "openai": { "type": "api_key", "key": "openai-key" },
            "google": { "type": "api_key", "key": "external-key" },
        }),
    );
    storage.delete("anthropic", None).await.expect("delete");
    let credentials = storage.list(None).await.expect("list");
    assert_eq!(
        credentials
            .iter()
            .map(|info| info.provider_id.as_str())
            .collect::<Vec<_>>(),
        vec!["openai", "google"]
    );
    assert_eq!(storage.read("anthropic", None).await.expect("read"), None);
    assert_eq!(
        storage.read("openai", None).await.expect("read"),
        Some(api_key_credential("openai-key"))
    );
    assert_eq!(
        storage.read("google", None).await.expect("read"),
        Some(api_key_credential("external-key"))
    );
}

#[tokio::test]
async fn in_memory_storage_implements_the_same_credential_store_behavior() {
    let _guard = global_lock();
    let storage = AuthStorage::in_memory(storage_data(
        json!({ "anthropic": { "type": "api_key", "key": "initial" } }),
    ));
    assert_eq!(
        storage.read("anthropic", None).await.expect("read"),
        Some(api_key_credential("initial"))
    );
    storage
        .modify(
            "anthropic",
            returns(Some(api_key_credential("updated"))),
            None,
        )
        .await
        .expect("modify");
    assert_eq!(
        storage.read("anthropic", None).await.expect("read"),
        Some(api_key_credential("updated"))
    );
    storage.delete("anthropic", None).await.expect("delete");
    assert!(storage.list(None).await.expect("list").is_empty());
}

#[tokio::test]
async fn does_not_write_after_lock_acquisition_failure_and_recovers_on_retry() {
    let _guard = global_lock();
    testing::reset_shared_read_state();
    let directory = TempDir::new();
    let path = directory.auth_json();
    write_auth_json(
        &path,
        json!({ "anthropic": { "type": "api_key", "key": "stored" } }),
    );
    let counters = Arc::new(LockCounters::default());
    let storage = counting_storage(&path, &counters);
    counters.fail_next_async.store(true, Ordering::SeqCst);

    let error = storage
        .modify("openai", returns(Some(api_key_credential("new"))), None)
        .await
        .expect_err("the lock is unavailable");
    assert_eq!(error.to_string(), "lock unavailable");
    assert_eq!(
        read_auth_json(&path),
        json!({ "anthropic": { "type": "api_key", "key": "stored" } })
    );

    storage
        .modify("openai", returns(Some(api_key_credential("new"))), None)
        .await
        .expect("modify");
    assert_eq!(
        read_auth_json(&path),
        json!({
            "anthropic": { "type": "api_key", "key": "stored" },
            "openai": { "type": "api_key", "key": "new" },
        })
    );
}

#[tokio::test]
async fn retries_a_briefly_contended_file_lock() {
    let _guard = global_lock();
    let directory = TempDir::new();
    let path = directory.auth_json();
    write_auth_json(
        &path,
        json!({ "anthropic": { "type": "api_key", "key": "stored" } }),
    );
    let backend = FileAuthStorageBackend::at(Path::new(&path));
    let held = try_lock(Path::new(&path), &LockOptions::default()).expect("lock");
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&calls);

    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(40)).await;
        held.release();
    });
    backend
        .with_lock_async(
            Box::new(move |_current| {
                Box::pin(async move {
                    counted.fetch_add(1, Ordering::SeqCst);
                    Ok(None)
                })
            }),
            None,
        )
        .await
        .expect("acquires the lock after the holder releases it");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn surfaces_a_compromised_file_storage_lock() {
    let _guard = global_lock();
    let directory = TempDir::new();
    let path = directory.auth_json();
    write_auth_json(
        &path,
        json!({ "anthropic": { "type": "api_key", "key": "stored" } }),
    );
    let backend = FileAuthStorageBackend::at(Path::new(&path))
        .with_lock_update_interval(Duration::from_millis(100));
    let lock_path = lock_path_for(Path::new(&path));

    // Losing the lock directory is how a lock is compromised in practice; the TS
    // suite injects the same condition through `onCompromised`.
    let error = backend
        .with_lock_async(
            Box::new(move |_current| {
                Box::pin(async move {
                    std::fs::remove_dir_all(&lock_path).expect("remove the lock");
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    Ok(Some("{}".to_owned()))
                })
            }),
            None,
        )
        .await
        .expect_err("the compromised lock is surfaced");
    assert!(
        error.to_string().contains("Lock file was deleted"),
        "{error}"
    );
    assert_eq!(
        read_auth_json(&path),
        json!({ "anthropic": { "type": "api_key", "key": "stored" } })
    );
}

#[tokio::test]
async fn pre_aborted_file_operations_do_not_create_the_backing_file_or_run_the_mutation() {
    let _guard = global_lock();
    let directory = TempDir::new();
    let path = directory.auth_json();
    let backend = FileAuthStorageBackend::at(Path::new(&path));
    let token = CancellationToken::new();
    token.cancel();
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&calls);

    let error = backend
        .with_lock_async(
            Box::new(move |_current| {
                Box::pin(async move {
                    counted.fetch_add(1, Ordering::SeqCst);
                    Ok(Some("{}".to_owned()))
                })
            }),
            signal(&token),
        )
        .await
        .expect_err("aborted");
    assert_eq!(error.to_string(), "The operation was aborted");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(!Path::new(&path).exists());
}

#[tokio::test]
async fn aborts_while_waiting_for_a_held_file_lock_without_running_the_mutation_later() {
    let _guard = global_lock();
    let directory = TempDir::new();
    let path = directory.auth_json();
    write_auth_json(
        &path,
        json!({ "anthropic": { "type": "api_key", "key": "stored" } }),
    );
    let held = try_lock(Path::new(&path), &LockOptions::default()).expect("lock");
    let backend = FileAuthStorageBackend::at(Path::new(&path));
    let token = CancellationToken::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&calls);

    let mut pending = Box::pin(backend.with_lock_async(
        Box::new(move |_current| {
            Box::pin(async move {
                counted.fetch_add(1, Ordering::SeqCst);
                Ok(Some("{}".to_owned()))
            })
        }),
        signal(&token),
    ));
    tokio::select! {
        _ = &mut pending => panic!("the lock is held"),
        () = tokio::time::sleep(Duration::from_millis(10)) => {}
    }
    token.cancel();
    let error = pending.await.expect_err("aborted");
    assert_eq!(error.to_string(), "The operation was aborted");
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    held.release();
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        read_auth_json(&path),
        json!({ "anthropic": { "type": "api_key", "key": "stored" } })
    );
    assert!(
        !lock_path_for(Path::new(&path)).exists(),
        "the abandoned attempt holds no lock"
    );
}

#[tokio::test]
async fn holds_the_file_lock_until_a_cancelled_active_callback_settles_without_committing_it() {
    let _guard = global_lock();
    let directory = TempDir::new();
    let path = directory.auth_json();
    write_auth_json(
        &path,
        json!({ "anthropic": { "type": "api_key", "key": "stored" } }),
    );
    let backend = Arc::new(FileAuthStorageBackend::at(Path::new(&path)));
    let token = CancellationToken::new();
    let started = Arc::new(tokio::sync::Notify::new());
    let blocked = Arc::new(tokio::sync::Notify::new());

    let pending = tokio::spawn({
        let backend = Arc::clone(&backend);
        let token = token.clone();
        let started = Arc::clone(&started);
        let blocked = Arc::clone(&blocked);
        async move {
            backend
                .with_lock_async(
                    Box::new(move |_current| {
                        Box::pin(async move {
                            started.notify_one();
                            blocked.notified().await;
                            Ok(Some(
                                json!({ "openai": { "type": "api_key", "key": "cancelled" } })
                                    .to_string(),
                            ))
                        })
                    }),
                    signal(&token),
                )
                .await
        }
    });

    started.notified().await;
    token.cancel();
    let competing_calls = Arc::new(AtomicUsize::new(0));
    let competing = tokio::spawn({
        let backend = Arc::clone(&backend);
        let counted = Arc::clone(&competing_calls);
        async move {
            backend
                .with_lock_async(
                    Box::new(move |_current| {
                        Box::pin(async move {
                            counted.fetch_add(1, Ordering::SeqCst);
                            Ok(Some(
                                json!({ "google": { "type": "api_key", "key": "committed" } })
                                    .to_string(),
                            ))
                        })
                    }),
                    None,
                )
                .await
        }
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(
        competing_calls.load(Ordering::SeqCst),
        0,
        "the cancelled callback still holds the lock"
    );

    blocked.notify_one();
    let error = pending.await.expect("join").expect_err("aborted");
    assert_eq!(error.to_string(), "The operation was aborted");
    competing
        .await
        .expect("join")
        .expect("the competing mutation commits");
    assert_eq!(competing_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        read_auth_json(&path),
        json!({ "google": { "type": "api_key", "key": "committed" } })
    );
}

#[tokio::test]
async fn cancels_a_signalled_credential_read_waiting_for_a_held_file_lock() {
    let _guard = global_lock();
    testing::reset_shared_read_state();
    let directory = TempDir::new();
    let path = directory.auth_json();
    write_auth_json(
        &path,
        json!({ "anthropic": { "type": "api_key", "key": "old" } }),
    );
    let counters = Arc::new(LockCounters::default());
    let storage = counting_storage(&path, &counters);
    write_auth_json(
        &path,
        json!({ "anthropic": { "type": "api_key", "key": "new-value" } }),
    );
    let held = try_lock(Path::new(&path), &LockOptions::default()).expect("lock");
    let token = CancellationToken::new();

    let mut pending = Box::pin(storage.read("anthropic", signal(&token)));
    tokio::select! {
        _ = &mut pending => panic!("the lock is held"),
        () = tokio::time::sleep(Duration::from_millis(10)) => {}
    }
    token.cancel();
    let error = pending.await.expect_err("aborted");
    assert_eq!(error.to_string(), "The operation was aborted");

    held.release();
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(counters.asynchronous.load(Ordering::SeqCst), 1);
    assert_eq!(
        storage.read("anthropic", None).await.expect("read"),
        Some(api_key_credential("new-value"))
    );
}

#[tokio::test]
async fn serializes_in_memory_mutations_across_providers() {
    let _guard = global_lock();
    let storage = AuthStorage::in_memory(AuthStorageData::new());
    let started = Arc::new(tokio::sync::Notify::new());
    let blocked = Arc::new(tokio::sync::Notify::new());
    let first_started = Arc::clone(&started);
    let first_blocked = Arc::clone(&blocked);

    let mut first = Box::pin(storage.modify(
        "anthropic",
        Box::new(move |_current| {
            Box::pin(async move {
                first_started.notify_one();
                first_blocked.notified().await;
                Ok(Some(api_key_credential("anthropic-key")))
            })
        }),
        None,
    ));
    tokio::select! {
        _ = &mut first => panic!("the first mutation is blocked"),
        () = started.notified() => {}
    }

    let second_calls = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&second_calls);
    let mut second = Box::pin(storage.modify(
        "openai",
        Box::new(move |_current| {
            Box::pin(async move {
                counted.fetch_add(1, Ordering::SeqCst);
                Ok(Some(api_key_credential("openai-key")))
            })
        }),
        None,
    ));
    tokio::select! {
        _ = &mut first => panic!("the first mutation is blocked"),
        _ = &mut second => panic!("the second mutation must wait"),
        () = tokio::time::sleep(Duration::from_millis(20)) => {}
    }
    assert_eq!(second_calls.load(Ordering::SeqCst), 0);

    blocked.notify_one();
    let (left, right) = tokio::join!(first, second);
    left.expect("first");
    right.expect("second");
    assert_eq!(
        storage.read("anthropic", None).await.expect("read"),
        Some(api_key_credential("anthropic-key"))
    );
    assert_eq!(
        storage.read("openai", None).await.expect("read"),
        Some(api_key_credential("openai-key"))
    );
}

#[tokio::test]
async fn cancels_a_queued_in_memory_mutation_without_running_it_later() {
    let _guard = global_lock();
    let storage = AuthStorage::in_memory(AuthStorageData::new());
    let started = Arc::new(tokio::sync::Notify::new());
    let blocked = Arc::new(tokio::sync::Notify::new());
    let first_started = Arc::clone(&started);
    let first_blocked = Arc::clone(&blocked);

    let mut first = Box::pin(storage.modify(
        "anthropic",
        Box::new(move |_current| {
            Box::pin(async move {
                first_started.notify_one();
                first_blocked.notified().await;
                Ok(Some(api_key_credential("anthropic-key")))
            })
        }),
        None,
    ));
    tokio::select! {
        _ = &mut first => panic!("the first mutation is blocked"),
        () = started.notified() => {}
    }

    let token = CancellationToken::new();
    let second_calls = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&second_calls);
    let second = storage.modify(
        "openai",
        Box::new(move |_current| {
            Box::pin(async move {
                counted.fetch_add(1, Ordering::SeqCst);
                Ok(Some(api_key_credential("openai-key")))
            })
        }),
        signal(&token),
    );
    token.cancel();
    let error = second.await.expect_err("aborted");
    assert_eq!(error.to_string(), "The operation was aborted");
    assert_eq!(second_calls.load(Ordering::SeqCst), 0);

    blocked.notify_one();
    first.await.expect("first");
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(second_calls.load(Ordering::SeqCst), 0);
    assert_eq!(storage.read("openai", None).await.expect("read"), None);
}

#[tokio::test]
async fn preserves_the_stored_credential_after_cancelling_an_active_refresh_mutation() {
    let _guard = global_lock();
    let previous = Credential::OAuth(OAuthCredential {
        access: "expired".to_owned(),
        refresh: "refresh-token".to_owned(),
        expires: 0,
        extra: serde_json::Map::new(),
    });
    let storage = AuthStorage::in_memory(storage_data(json!({ "oauth": previous })));
    let token = CancellationToken::new();
    let started = Arc::new(tokio::sync::Notify::new());
    let blocked = Arc::new(tokio::sync::Notify::new());
    let mutation_started = Arc::clone(&started);
    let mutation_blocked = Arc::clone(&blocked);

    let mut pending = Box::pin(storage.modify(
        "oauth",
        Box::new(move |_current| {
            Box::pin(async move {
                mutation_started.notify_one();
                mutation_blocked.notified().await;
                Ok(Some(api_key_credential("refreshed")))
            })
        }),
        signal(&token),
    ));
    tokio::select! {
        _ = &mut pending => panic!("the mutation is blocked"),
        () = started.notified() => {}
    }
    token.cancel();
    let error = pending.await.expect_err("aborted");
    assert_eq!(error.to_string(), "The operation was aborted");

    // Deviation (class 1): TS keeps the abandoned mutation running, so the
    // competing one waits for `finish`. A dropped Rust future stops where it is,
    // which frees the queue earlier — the outcome is the same either way.
    let competing_calls = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&competing_calls);
    storage
        .modify(
            "other",
            Box::new(move |_current| {
                Box::pin(async move {
                    counted.fetch_add(1, Ordering::SeqCst);
                    Ok(Some(api_key_credential("other")))
                })
            }),
            None,
        )
        .await
        .expect("competing modify");
    blocked.notify_one();
    assert_eq!(competing_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        storage.read("oauth", None).await.expect("read"),
        Some(previous)
    );
}

#[tokio::test]
async fn does_not_overwrite_malformed_auth_files() {
    let _guard = global_lock();
    let directory = TempDir::new();
    let path = directory.auth_json();
    write_auth_json(
        &path,
        json!({ "anthropic": { "type": "api_key", "key": "stored" } }),
    );
    let storage = AuthStorage::create(&path).expect("storage");
    std::fs::write(&path, "{invalid-json").expect("write");
    storage
        .modify("openai", returns(Some(api_key_credential("new"))), None)
        .await
        .expect_err("the malformed file is not parseable");
    assert_eq!(
        std::fs::read_to_string(&path).expect("read"),
        "{invalid-json"
    );
}

#[tokio::test]
async fn read_only_storage_validates_and_never_runs_commands() {
    let _guard = global_lock();
    let directory = TempDir::new();
    let path = directory.auth_json();
    write_auth_json(
        &path,
        json!({
            "anthropic": { "type": "api_key", "key": "!printf 'never-run'" },
            "openai": { "type": "api_key", "key": "$READ_ONLY_MISSING_VAR" },
        }),
    );
    let storage = ReadOnlyAuthStorage::new(&path).expect("storage");
    assert_eq!(
        storage.read("anthropic", None).await.expect("read"),
        Some(api_key_credential("!printf 'never-run'"))
    );
    let openai = storage
        .read("openai", None)
        .await
        .expect("read")
        .expect("credential");
    assert_eq!(openai.as_api_key().expect("api key").key, None);
    assert_eq!(storage.list(None).await.expect("list").len(), 2);
    storage
        .modify("anthropic", returns(None), None)
        .await
        .expect_err("read-only");
    storage
        .delete("anthropic", None)
        .await
        .expect_err("read-only");

    write_auth_json(
        &path,
        json!({ "anthropic": { "type": "api_key", "key": 42 } }),
    );
    let invalid = ReadOnlyAuthStorage::new(&path).expect("storage");
    let error = invalid.read("anthropic", None).await.expect_err("invalid");
    assert_eq!(
        error.to_string(),
        "Invalid auth.json credential for provider \"anthropic\""
    );

    std::fs::write(&path, "[]").expect("write");
    let invalid = ReadOnlyAuthStorage::new(&path).expect("storage");
    let error = invalid.read("anthropic", None).await.expect_err("invalid");
    assert_eq!(error.to_string(), "Invalid auth.json: expected an object");

    let missing = ReadOnlyAuthStorage::new(&directory.join("missing.json")).expect("storage");
    assert_eq!(missing.read("anthropic", None).await.expect("read"), None);
}

#[tokio::test]
async fn reads_a_stored_credential_without_a_store() {
    let _guard = global_lock();
    let directory = TempDir::new();
    let path = directory.auth_json();
    write_auth_json(
        &path,
        json!({ "anthropic": { "type": "api_key", "key": "$NOT_RESOLVED" } }),
    );
    assert_eq!(
        read_stored_credential("anthropic", &path),
        Some(api_key_credential("$NOT_RESOLVED"))
    );
    assert_eq!(read_stored_credential("openai", &path), None);
    assert_eq!(
        read_stored_credential("anthropic", &directory.join("missing.json")),
        None
    );
}

#[tokio::test]
async fn in_memory_backend_serializes_and_stores_content() {
    let _guard = global_lock();
    let backend = InMemoryAuthStorageBackend::new();
    backend
        .with_lock(Box::new(|current| {
            assert_eq!(current, None);
            Ok(Some("{\"a\":1}".to_owned()))
        }))
        .expect("with_lock");
    backend
        .with_lock(Box::new(|current| {
            assert_eq!(current.as_deref(), Some("{\"a\":1}"));
            Ok(None)
        }))
        .expect("with_lock");
}

// ---------------------------------------------------------------------------
// AuthStorage behind Models: a refresh runs inside `modify`, under the lock.
// ---------------------------------------------------------------------------

/// Credential store that fails its first `modify` and delegates afterwards.
struct FailingOnceStore {
    base: AuthStorage,
    fail_next_modify: AtomicBool,
}

impl CredentialStore for FailingOnceStore {
    fn read(
        &self,
        provider_id: &str,
        options: Option<AuthOperationOptions>,
    ) -> BoxFuture<'_, Result<Option<Credential>, CredentialStoreError>> {
        self.base.read(provider_id, options)
    }

    fn list(
        &self,
        options: Option<AuthOperationOptions>,
    ) -> BoxFuture<'_, Result<Vec<notagent_ai::auth::types::CredentialInfo>, CredentialStoreError>>
    {
        self.base.list(options)
    }

    fn modify<'a>(
        &'a self,
        provider_id: &'a str,
        modify: notagent_ai::auth::types::ModifyFn<'a>,
        options: Option<AuthOperationOptions>,
    ) -> BoxFuture<'a, Result<Option<Credential>, CredentialStoreError>> {
        if self.fail_next_modify.swap(false, Ordering::SeqCst) {
            return Box::pin(async {
                Err(CredentialStoreError(
                    "credential store unavailable".to_owned(),
                ))
            });
        }
        self.base.modify(provider_id, modify, options)
    }

    fn delete(
        &self,
        provider_id: &str,
        options: Option<AuthOperationOptions>,
    ) -> BoxFuture<'_, Result<(), CredentialStoreError>> {
        self.base.delete(provider_id, options)
    }
}

struct RefreshingOAuth;

impl notagent_ai::auth::types::OAuthAuth for RefreshingOAuth {
    fn name(&self) -> &str {
        "OAuth"
    }

    fn login<'a>(
        &'a self,
        _interaction: &'a notagent_ai::auth::types::ProviderAuthInteraction<'a>,
    ) -> BoxFuture<'a, Result<OAuthCredential, notagent_ai::auth::types::AuthError>> {
        Box::pin(async { Err(notagent_ai::auth::types::AuthError("not used".to_owned())) })
    }

    fn refresh<'a>(
        &'a self,
        credential: OAuthCredential,
        _signal: CancellationToken,
    ) -> BoxFuture<'a, Result<OAuthCredential, notagent_ai::auth::types::AuthError>> {
        Box::pin(async move {
            Ok(OAuthCredential {
                access: "refreshed-access".to_owned(),
                expires: notagent_ai::auth::resolve::now_ms() + 60_000,
                ..credential
            })
        })
    }

    fn to_auth<'a>(
        &'a self,
        credential: OAuthCredential,
    ) -> BoxFuture<
        'a,
        Result<notagent_ai::auth::types::ModelAuth, notagent_ai::auth::types::AuthError>,
    > {
        Box::pin(async move {
            Ok(notagent_ai::auth::types::ModelAuth {
                api_key: Some(credential.access),
                ..notagent_ai::auth::types::ModelAuth::default()
            })
        })
    }
}

struct OAuthProvider {
    id: String,
    auth: notagent_ai::auth::types::ProviderAuth,
}

impl notagent_ai::models::Provider for OAuthProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        "OAuth Provider"
    }

    fn auth(&self) -> &notagent_ai::auth::types::ProviderAuth {
        &self.auth
    }

    fn get_models(&self) -> Vec<notagent_ai::types::Model> {
        Vec::new()
    }

    fn stream(
        &self,
        _model: &notagent_ai::types::Model,
        _context: &notagent_ai::types::Context,
        _options: Option<notagent_ai::types::StreamOptions>,
    ) -> notagent_ai::utils::event_stream::AssistantMessageEventStream {
        panic!("not used")
    }

    fn stream_simple(
        &self,
        _model: &notagent_ai::types::Model,
        _context: &notagent_ai::types::Context,
        _options: Option<notagent_ai::types::SimpleStreamOptions>,
    ) -> notagent_ai::utils::event_stream::AssistantMessageEventStream {
        panic!("not used")
    }
}

#[tokio::test]
async fn translates_a_credential_store_refresh_failure_and_allows_a_later_retry() {
    let _guard = global_lock();
    let provider_id = "oauth-provider";
    let base = AuthStorage::in_memory(storage_data(json!({
        provider_id: {
            "type": "oauth",
            "access": "expired-access",
            "refresh": "refresh-token",
            "expires": 0,
        }
    })));
    let credentials = Arc::new(FailingOnceStore {
        base,
        fail_next_modify: AtomicBool::new(true),
    });
    let models =
        notagent_ai::models::create_models(Some(notagent_ai::models::CreateModelsOptions {
            credentials: Some(credentials),
            ..Default::default()
        }));
    models.set_provider(Arc::new(OAuthProvider {
        id: provider_id.to_owned(),
        auth: notagent_ai::auth::types::ProviderAuth {
            api_key: None,
            oauth: Some(Arc::new(RefreshingOAuth)),
        },
    }));

    let error = models
        .get_auth_for_provider(provider_id, None)
        .await
        .expect_err("refresh fails");
    assert_eq!(error.code.as_str(), "auth");

    let result = models
        .get_auth_for_provider(provider_id, None)
        .await
        .expect("the retry refreshes")
        .expect("auth");
    assert_eq!(result.auth.api_key.as_deref(), Some("refreshed-access"));
}
