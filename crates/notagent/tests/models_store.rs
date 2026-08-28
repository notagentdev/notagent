use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use notagent::core::auth_storage::{
    AsyncLockFn, AuthStorageBackend, FileAuthStorageBackend, SyncLockFn,
};
use notagent::core::models_store::FileModelsStore;
use notagent_ai::auth::types::{AuthOperationOptions, BoxFuture, CredentialStoreError};
use notagent_ai::models_store::{ModelsStore, ModelsStoreEntry, ModelsStoreOperationOptions};
use notagent_ai::types::{Modality, Model, ModelCost};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

fn model(provider: &str, id: &str) -> Model {
    Model {
        id: id.to_owned(),
        name: id.to_owned(),
        api: "openai-completions".to_owned(),
        provider: provider.to_owned(),
        base_url: "https://example.test/v1".to_owned(),
        reasoning: false,
        thinking_level_map: None,
        input: vec![Modality::Text],
        cost: ModelCost::default(),
        context_window: 1000,
        max_tokens: 100,
        sampling_params: None,
        headers: None,
        compat: None,
    }
}

fn entry(provider: &str, id: &str, checked_at: Option<i64>) -> ModelsStoreEntry {
    ModelsStoreEntry {
        models: vec![model(provider, id)],
        last_modified: None,
        checked_at,
        etag: None,
    }
}

fn ids(entry: &ModelsStoreEntry) -> Vec<String> {
    entry.models.iter().map(|model| model.id.clone()).collect()
}

/// Counts every lock acquisition and can hold them until released.
struct CountingBackend {
    inner: FileAuthStorageBackend,
    locks: Arc<AtomicUsize>,
    gate: Option<Arc<Notify>>,
}

impl AuthStorageBackend for CountingBackend {
    fn with_lock(&self, apply: SyncLockFn<'_>) -> Result<(), CredentialStoreError> {
        self.locks.fetch_add(1, Ordering::SeqCst);
        self.inner.with_lock(apply)
    }

    fn with_lock_async<'a>(
        &'a self,
        apply: AsyncLockFn<'a>,
        options: Option<AuthOperationOptions>,
    ) -> BoxFuture<'a, Result<(), CredentialStoreError>> {
        self.locks.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            if let Some(gate) = &self.gate {
                gate.notified().await;
            }
            self.inner.with_lock_async(apply, options).await
        })
    }
}

fn counting_store(
    path: &std::path::Path,
    locks: Arc<AtomicUsize>,
    gate: Option<Arc<Notify>>,
) -> FileModelsStore {
    FileModelsStore::with_backend(
        Arc::new(CountingBackend {
            inner: FileAuthStorageBackend::at(path),
            locks,
            gate,
        }),
        &path.to_string_lossy(),
    )
    .expect("store")
}

async fn persists_provider_catalogs_without_replacing_unrelated_providers(dir: &std::path::Path) {
    let path = dir.join("models-store.json");
    let store = FileModelsStore::new(&path.to_string_lossy()).expect("store");

    store
        .write("one", entry("one", "m1", Some(100)), None)
        .await
        .expect("write one");
    store
        .write("two", entry("two", "m2", Some(200)), None)
        .await
        .expect("write two");

    let reloaded = FileModelsStore::new(&path.to_string_lossy()).expect("store");
    let one = reloaded
        .read("one", None)
        .await
        .expect("read")
        .expect("one");
    assert_eq!(ids(&one), vec!["m1"]);
    assert_eq!(one.checked_at, Some(100));
    let two = reloaded
        .read("two", None)
        .await
        .expect("read")
        .expect("two");
    assert_eq!(ids(&two), vec!["m2"]);

    reloaded.delete("one", None).await.expect("delete");
    assert!(reloaded.read("one", None).await.expect("read").is_none());
    let two = reloaded
        .read("two", None)
        .await
        .expect("read")
        .expect("two");
    assert_eq!(ids(&two), vec!["m2"]);
}

async fn coalesces_file_reloads_across_concurrent_readers(dir: &std::path::Path) {
    let path = dir.join("models-store.json");
    std::fs::write(
        &path,
        serde_json::to_string(&serde_json::json!({
            "one": { "models": [model("one", "old")] },
            "two": { "models": [model("two", "m2")] },
        }))
        .expect("json"),
    )
    .expect("seed");

    let locks = Arc::new(AtomicUsize::new(0));
    let first = counting_store(&path, Arc::clone(&locks), None);
    let second = counting_store(&path, Arc::clone(&locks), None);
    let signal = || {
        Some(ModelsStoreOperationOptions {
            signal: Some(CancellationToken::new()),
        })
    };

    let (one, two, missing) = tokio::join!(
        first.read("one", signal()),
        second.read("two", signal()),
        first.read("missing", signal()),
    );
    assert_eq!(ids(&one.expect("read").expect("one")), vec!["old"]);
    assert_eq!(ids(&two.expect("read").expect("two")), vec!["m2"]);
    assert!(missing.expect("read").is_none());
    assert_eq!(locks.load(Ordering::SeqCst), 1);

    let one = second.read("one", None).await.expect("read").expect("one");
    assert_eq!(ids(&one), vec!["old"]);
    assert_eq!(locks.load(Ordering::SeqCst), 1);

    let other_path = dir.join("other-models-store.json");
    std::fs::write(&other_path, "{}").expect("seed other");
    let other_locks = Arc::new(AtomicUsize::new(0));
    let other = counting_store(&other_path, Arc::clone(&other_locks), None);
    assert!(other.read("one", None).await.expect("read").is_none());
    assert!(other.read("one", None).await.expect("read").is_none());

    let third = counting_store(&path, Arc::clone(&locks), None);
    std::fs::write(
        &path,
        serde_json::to_string(&serde_json::json!({
            "one": { "models": [model("one", "newest-model")] },
        }))
        .expect("json"),
    )
    .expect("rewrite");
    let (first_reload, third_reload) =
        tokio::join!(first.read("one", None), third.read("one", None));
    assert_eq!(
        ids(&first_reload.expect("read").expect("one")),
        vec!["newest-model"]
    );
    assert_eq!(
        ids(&third_reload.expect("read").expect("one")),
        vec!["newest-model"]
    );
    assert_eq!(locks.load(Ordering::SeqCst), 2);
}

async fn keeps_a_coalesced_reload_alive_while_another_reader_is_still_waiting(
    dir: &std::path::Path,
) {
    let path = dir.join("models-store.json");
    std::fs::write(
        &path,
        serde_json::to_string(&serde_json::json!({
            "one": { "models": [model("one", "stored")] },
        }))
        .expect("json"),
    )
    .expect("seed");

    let locks = Arc::new(AtomicUsize::new(0));
    let gate = Arc::new(Notify::new());
    let store = Arc::new(counting_store(
        &path,
        Arc::clone(&locks),
        Some(Arc::clone(&gate)),
    ));
    let first_signal = CancellationToken::new();
    let second_signal = CancellationToken::new();

    let first_store = Arc::clone(&store);
    let first_token = first_signal.clone();
    let first = tokio::spawn(async move {
        first_store
            .read(
                "one",
                Some(ModelsStoreOperationOptions {
                    signal: Some(first_token),
                }),
            )
            .await
    });
    let second_store = Arc::clone(&store);
    let second_token = second_signal.clone();
    let second = tokio::spawn(async move {
        second_store
            .read(
                "one",
                Some(ModelsStoreOperationOptions {
                    signal: Some(second_token),
                }),
            )
            .await
    });
    while locks.load(Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;

    first_signal.cancel();
    let first = first.await.expect("join").expect_err("aborted");
    assert_eq!(first.0, "The operation was aborted");
    gate.notify_waiters();
    let second = second.await.expect("join").expect("read").expect("one");
    assert_eq!(ids(&second), vec!["stored"]);
    assert_eq!(locks.load(Ordering::SeqCst), 1);
}

async fn cancels_a_catalog_write_waiting_for_a_held_file_lock(dir: &std::path::Path) {
    let path = dir.join("models-store.json");
    std::fs::write(
        &path,
        serde_json::to_string(&serde_json::json!({
            "one": { "models": [model("one", "existing")] },
        }))
        .expect("json"),
    )
    .expect("seed");
    let store = FileModelsStore::new(&path.to_string_lossy()).expect("store");
    let guard = notagent::utils::lockfile::try_lock(&path, &Default::default()).expect("lock");

    let signal = CancellationToken::new();
    let pending_signal = signal.clone();
    let pending = tokio::spawn(async move {
        store
            .write(
                "two",
                entry("two", "cancelled", None),
                Some(ModelsStoreOperationOptions {
                    signal: Some(pending_signal),
                }),
            )
            .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    signal.cancel();
    let error = pending.await.expect("join").expect_err("aborted");
    assert_eq!(error.0, "The operation was aborted");
    drop(guard);
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;

    let stored: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
    assert!(stored.get("one").is_some());
    assert!(stored.get("two").is_none());
}

#[tokio::test]
async fn file_models_store() {
    let dir = tempfile::tempdir().expect("tempdir");
    persists_provider_catalogs_without_replacing_unrelated_providers(dir.path()).await;
    coalesces_file_reloads_across_concurrent_readers(dir.path()).await;
    keeps_a_coalesced_reload_alive_while_another_reader_is_still_waiting(dir.path()).await;
    cancels_a_catalog_write_waiting_for_a_held_file_lock(dir.path()).await;
}
