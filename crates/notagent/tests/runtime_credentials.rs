use std::sync::{Arc, Mutex};

use notagent::core::auth_storage::AuthStorage;
use notagent::core::runtime_credentials::RuntimeCredentials;
use notagent_ai::auth::types::{
    ApiKeyCredential, AuthOperationOptions, AuthType, BoxFuture, Credential, CredentialInfo,
    CredentialStore, CredentialStoreError, ModifyFn,
};
use serde_json::json;
use tokio_util::sync::CancellationToken;

fn api_key(key: &str) -> Credential {
    Credential::ApiKey(ApiKeyCredential {
        key: Some(key.to_owned()),
        env: None,
    })
}

#[tokio::test]
async fn runtime_overrides_mask_stored_credentials_without_persisting() {
    let storage = Arc::new(AuthStorage::in_memory(
        json!({ "anthropic": { "type": "api_key", "key": "stored-key" } })
            .as_object()
            .expect("object")
            .clone(),
    ));
    let credentials = RuntimeCredentials::new(Arc::clone(&storage) as Arc<dyn CredentialStore>);

    credentials.set_runtime_api_key("anthropic", "runtime-key");
    assert_eq!(
        credentials.read("anthropic", None).await.expect("read"),
        Some(api_key("runtime-key"))
    );
    assert_eq!(
        storage.read("anthropic", None).await.expect("read"),
        Some(api_key("stored-key"))
    );

    credentials.remove_runtime_api_key("anthropic");
    assert_eq!(
        credentials.read("anthropic", None).await.expect("read"),
        Some(api_key("stored-key"))
    );
}

#[tokio::test]
async fn enumeration_merges_overrides_without_exposing_keys() {
    let storage = Arc::new(AuthStorage::in_memory(
        json!({
            "anthropic": {
                "type": "oauth",
                "access": "access",
                "refresh": "refresh",
                "expires": 4_102_444_800_000_i64,
            }
        })
        .as_object()
        .expect("object")
        .clone(),
    ));
    let credentials = RuntimeCredentials::new(storage as Arc<dyn CredentialStore>);
    credentials.set_runtime_api_key("anthropic", "runtime-key");
    credentials.set_runtime_api_key("openai", "other-runtime-key");

    assert_eq!(
        credentials.list(None).await.expect("list"),
        vec![
            CredentialInfo {
                provider_id: "anthropic".to_owned(),
                credential_type: AuthType::ApiKey,
            },
            CredentialInfo {
                provider_id: "openai".to_owned(),
                credential_type: AuthType::ApiKey,
            },
        ]
    );
}

/// Records the signal of every forwarded call.
struct RecordingStore {
    received: Mutex<Vec<bool>>,
    fail_delete_once: Mutex<bool>,
}

impl RecordingStore {
    fn new() -> Self {
        RecordingStore {
            received: Mutex::new(Vec::new()),
            fail_delete_once: Mutex::new(false),
        }
    }

    fn record(&self, options: &Option<AuthOperationOptions>) {
        self.received.lock().expect("received").push(
            options
                .as_ref()
                .is_some_and(|options| options.signal.is_some()),
        );
    }
}

impl CredentialStore for RecordingStore {
    fn read(
        &self,
        _provider_id: &str,
        options: Option<AuthOperationOptions>,
    ) -> BoxFuture<'_, Result<Option<Credential>, CredentialStoreError>> {
        self.record(&options);
        Box::pin(async { Ok(None) })
    }

    fn list(
        &self,
        options: Option<AuthOperationOptions>,
    ) -> BoxFuture<'_, Result<Vec<CredentialInfo>, CredentialStoreError>> {
        self.record(&options);
        Box::pin(async { Ok(Vec::new()) })
    }

    fn modify<'a>(
        &'a self,
        _provider_id: &'a str,
        _modify: ModifyFn<'a>,
        options: Option<AuthOperationOptions>,
    ) -> BoxFuture<'a, Result<Option<Credential>, CredentialStoreError>> {
        self.record(&options);
        Box::pin(async { Ok(None) })
    }

    fn delete(
        &self,
        _provider_id: &str,
        options: Option<AuthOperationOptions>,
    ) -> BoxFuture<'_, Result<(), CredentialStoreError>> {
        self.record(&options);
        let fail = {
            let mut fail_delete_once = self.fail_delete_once.lock().expect("fail flag");
            let fail = *fail_delete_once;
            *fail_delete_once = false;
            fail
        };
        Box::pin(async move {
            if fail {
                Err(CredentialStoreError("cancelled".to_owned()))
            } else {
                Ok(())
            }
        })
    }
}

#[tokio::test]
async fn forwards_operation_signals_to_the_persistent_store() {
    let store = Arc::new(RecordingStore::new());
    let credentials = RuntimeCredentials::new(Arc::clone(&store) as Arc<dyn CredentialStore>);
    let signal = CancellationToken::new();
    let options = || {
        Some(AuthOperationOptions {
            signal: Some(signal.clone()),
        })
    };

    credentials
        .read("anthropic", options())
        .await
        .expect("read");
    credentials.list(options()).await.expect("list");
    credentials
        .modify(
            "anthropic",
            Box::new(|_current| Box::pin(async { Ok(None) })),
            options(),
        )
        .await
        .expect("modify");
    credentials
        .delete("anthropic", options())
        .await
        .expect("delete");

    assert_eq!(
        *store.received.lock().expect("received"),
        vec![true, true, true, true]
    );
}

#[tokio::test]
async fn keeps_a_runtime_override_when_persistent_deletion_is_cancelled() {
    let store = Arc::new(RecordingStore::new());
    *store.fail_delete_once.lock().expect("fail flag") = true;
    let credentials = RuntimeCredentials::new(Arc::clone(&store) as Arc<dyn CredentialStore>);
    credentials.set_runtime_api_key("anthropic", "runtime-key");

    let error = credentials
        .delete(
            "anthropic",
            Some(AuthOperationOptions {
                signal: Some(CancellationToken::new()),
            }),
        )
        .await
        .expect_err("cancelled");
    assert_eq!(error.0, "cancelled");
    assert_eq!(store.received.lock().expect("received").len(), 1);
    assert_eq!(
        credentials.read("anthropic", None).await.expect("read"),
        Some(api_key("runtime-key"))
    );
}

#[tokio::test]
async fn delete_clears_both_the_override_and_the_persisted_credential() {
    let storage = Arc::new(AuthStorage::in_memory(
        json!({ "anthropic": { "type": "api_key", "key": "stored-key" } })
            .as_object()
            .expect("object")
            .clone(),
    ));
    let credentials = RuntimeCredentials::new(storage as Arc<dyn CredentialStore>);
    credentials.set_runtime_api_key("anthropic", "runtime-key");

    credentials.delete("anthropic", None).await.expect("delete");

    assert_eq!(
        credentials.read("anthropic", None).await.expect("read"),
        None
    );
    assert!(credentials.list(None).await.expect("list").is_empty());
}
