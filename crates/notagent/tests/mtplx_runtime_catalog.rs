//! The MTPLX provider sources its model list from the server it talks to, so it
//! must keep its own refresh instead of the shared notagent.dev catalog overlay.
//!
//! The overlay replaces `refresh_models` outright rather than chaining to the
//! provider it wraps, so wrapping MTPLX would leave it permanently without
//! models — and silently, because an empty list looks the same as a server that
//! is not running.

use std::sync::{Arc, Mutex};

use notagent::core::auth_storage::AuthStorage;
use notagent::core::model_runtime::{CreateModelRuntimeOptions, ModelRuntime};
use notagent_ai::auth::types::CredentialStore;
use notagent_ai::models_store::InMemoryModelsStore;
use notagent_ai::providers::mtplx::LOCAL_PROVIDER_ID;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Records the request line of everything asked of it and answers 404, which
/// the overlay treats as "this provider has no remote catalog" rather than as a
/// failure.
async fn recording_catalog() -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap_or_else(|error| panic!("binding a loopback port failed: {error}"));
    let address = listener
        .local_addr()
        .unwrap_or_else(|error| panic!("reading the bound port failed: {error}"));
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&seen);
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let recorded = Arc::clone(&recorded);
            tokio::spawn(async move {
                let mut buffer = vec![0_u8; 4096];
                let Ok(read) = socket.read(&mut buffer).await else {
                    return;
                };
                if let Some(line) = String::from_utf8_lossy(&buffer[..read]).lines().next()
                    && let Ok(mut recorded) = recorded.lock()
                {
                    recorded.push(line.to_owned());
                }
                let _ = socket
                    .write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                    .await;
                let _ = socket.flush().await;
            });
        }
    });
    (format!("http://{address}"), seen)
}

#[tokio::test]
async fn the_shared_catalog_is_never_asked_about_the_mtplx_provider() {
    let (catalog_base_url, seen) = recording_catalog().await;
    // A network refresh only runs for a provider whose credential resolves, so
    // both entries are needed: MTPLX to make it eligible at all, and a second
    // provider to prove the catalog is reachable and asked in this run.
    let credentials = json!({
        LOCAL_PROVIDER_ID: { "type": "api_key", "key": "mtplx-local" },
        "openai": { "type": "api_key", "key": "test-key" },
    });
    let runtime = ModelRuntime::create(CreateModelRuntimeOptions {
        credentials: Some(Arc::new(AuthStorage::in_memory(
            credentials.as_object().cloned().unwrap_or_default(),
        )) as Arc<dyn CredentialStore>),
        models_store: Some(Arc::new(InMemoryModelsStore::new())),
        models_path: Some(None),
        catalog_base_url: Some(catalog_base_url),
        allow_model_network: Some(true),
        ..CreateModelRuntimeOptions::default()
    })
    .await
    .unwrap_or_else(|error| panic!("building the runtime failed: {error}"));

    let requests = seen
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    assert!(
        !requests.is_empty(),
        "no provider asked the catalog at all, so this test would pass for the wrong reason"
    );
    let asked_about_mtplx = requests
        .iter()
        .any(|line| line.contains(&format!("/providers/{LOCAL_PROVIDER_ID}")));
    assert!(
        !asked_about_mtplx,
        "the catalog overlay was applied to MTPLX, which would replace its own refresh: {requests:?}"
    );
    drop(runtime);
}

/// Builds a runtime holding a stored credential for MTPLX, which is what
/// `/login mtplx` leaves behind.
async fn activated_runtime(catalog_base_url: String) -> Arc<ModelRuntime> {
    let credentials = json!({ LOCAL_PROVIDER_ID: { "type": "api_key", "key": "mtplx-local" } });
    ModelRuntime::create(CreateModelRuntimeOptions {
        credentials: Some(Arc::new(AuthStorage::in_memory(
            credentials.as_object().cloned().unwrap_or_default(),
        )) as Arc<dyn CredentialStore>),
        models_store: Some(Arc::new(InMemoryModelsStore::new())),
        models_path: Some(None),
        catalog_base_url: Some(catalog_base_url),
        allow_model_network: Some(true),
        ..CreateModelRuntimeOptions::default()
    })
    .await
    .unwrap_or_else(|error| panic!("building the runtime failed: {error}"))
}

#[tokio::test]
async fn an_activated_instance_without_a_server_offers_nothing_and_says_nothing() {
    // Activation alone must not put a model in the picker: the server decides
    // what is on offer, and it is not running here. This must stay quiet rather
    // than surface as an availability error, because never having started MTPLX
    // is the ordinary case.
    //
    // The premise is machine state, so it is checked rather than assumed: with
    // a real server on the local instance's port, the picker rightly offers its
    // model and this case does not apply.
    if std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], 8000)),
        std::time::Duration::from_millis(300),
    )
    .is_ok()
    {
        println!("skipped: an MTPLX server is running on 127.0.0.1:8000");
        return;
    }
    let (catalog_base_url, _) = recording_catalog().await;
    let runtime = activated_runtime(catalog_base_url).await;
    let offered: Vec<String> = runtime
        .get_available_snapshot()
        .into_iter()
        .filter(|model| model.provider == LOCAL_PROVIDER_ID)
        .map(|model| model.id)
        .collect();
    assert!(offered.is_empty(), "unexpected models offered: {offered:?}");
    let error = runtime.get_error().unwrap_or_default();
    assert!(
        !error.contains(LOCAL_PROVIDER_ID),
        "a server that is not running must not read as a fault: {error}"
    );
}

/// Ignored by default: needs `mtplx serve` on port 8000, the address the
/// built-in local instance points at. Run with `-- --ignored`.
#[tokio::test]
#[ignore = "requires a running MTPLX server on 127.0.0.1:8000"]
async fn a_running_server_puts_its_model_in_the_picker() {
    let (catalog_base_url, _) = recording_catalog().await;
    let runtime = activated_runtime(catalog_base_url).await;
    let offered: Vec<String> = runtime
        .get_available_snapshot()
        .into_iter()
        .filter(|model| model.provider == LOCAL_PROVIDER_ID)
        .map(|model| model.id)
        .collect();
    assert_eq!(
        offered.len(),
        1,
        "a server serves exactly one chat model, got: {offered:?}"
    );
    println!("picker offers: {}", offered[0]);
}
