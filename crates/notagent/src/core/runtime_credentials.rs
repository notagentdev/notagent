use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use notagent_ai::auth::types::{
    ApiKeyCredential, AuthOperationOptions, Credential, CredentialInfo, CredentialStore,
    CredentialStoreError, ModifyFn,
};

fn throw_if_aborted(options: Option<&AuthOperationOptions>) -> Result<(), CredentialStoreError> {
    match options.and_then(|options| options.signal.as_ref()) {
        Some(signal) if signal.is_cancelled() => {
            Err(CredentialStoreError("The operation was aborted".to_owned()))
        }
        _ => Ok(()),
    }
}

/// Async credential store overlay for non-persistent runtime API keys.
pub struct RuntimeCredentials {
    store: Arc<dyn CredentialStore>,
    overrides: Mutex<BTreeMap<String, String>>,
}

impl RuntimeCredentials {
    pub fn new(store: Arc<dyn CredentialStore>) -> Self {
        Self {
            store,
            overrides: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn set_runtime_api_key(&self, provider_id: &str, api_key: &str) {
        self.overrides
            .lock()
            .expect("overrides poisoned")
            .insert(provider_id.to_owned(), api_key.to_owned());
    }

    pub fn remove_runtime_api_key(&self, provider_id: &str) {
        self.overrides
            .lock()
            .expect("overrides poisoned")
            .remove(provider_id);
    }

    pub fn has_runtime_api_key(&self, provider_id: &str) -> bool {
        self.overrides
            .lock()
            .expect("overrides poisoned")
            .contains_key(provider_id)
    }

    fn override_for(&self, provider_id: &str) -> Option<String> {
        self.overrides
            .lock()
            .expect("overrides poisoned")
            .get(provider_id)
            .cloned()
    }

    fn override_ids(&self) -> Vec<String> {
        self.overrides
            .lock()
            .expect("overrides poisoned")
            .keys()
            .cloned()
            .collect()
    }
}

impl CredentialStore for RuntimeCredentials {
    fn read(
        &self,
        provider_id: &str,
        options: Option<AuthOperationOptions>,
    ) -> BoxFuture<'_, Result<Option<Credential>, CredentialStoreError>> {
        let provider_id = provider_id.to_owned();
        Box::pin(async move {
            throw_if_aborted(options.as_ref())?;
            match self.override_for(&provider_id) {
                Some(key) => Ok(Some(Credential::ApiKey(ApiKeyCredential {
                    key: Some(key),
                    env: None,
                }))),
                None => self.store.read(&provider_id, options).await,
            }
        })
    }

    fn list(
        &self,
        options: Option<AuthOperationOptions>,
    ) -> BoxFuture<'_, Result<Vec<CredentialInfo>, CredentialStoreError>> {
        Box::pin(async move {
            let stored = self.store.list(options.clone()).await?;
            // Insertion order of the underlying list wins; runtime-only ids are appended.
            let mut entries: Vec<CredentialInfo> = stored;
            throw_if_aborted(options.as_ref())?;
            for provider_id in self.override_ids() {
                let entry = CredentialInfo {
                    provider_id: provider_id.clone(),
                    credential_type: notagent_ai::auth::types::AuthType::ApiKey,
                };
                match entries
                    .iter_mut()
                    .find(|existing| existing.provider_id == provider_id)
                {
                    Some(existing) => *existing = entry,
                    None => entries.push(entry),
                }
            }
            Ok(entries)
        })
    }

    fn modify<'a>(
        &'a self,
        provider_id: &'a str,
        modify: ModifyFn<'a>,
        options: Option<AuthOperationOptions>,
    ) -> BoxFuture<'a, Result<Option<Credential>, CredentialStoreError>> {
        self.store.modify(provider_id, modify, options)
    }

    fn delete(
        &self,
        provider_id: &str,
        options: Option<AuthOperationOptions>,
    ) -> BoxFuture<'_, Result<(), CredentialStoreError>> {
        let provider_id = provider_id.to_owned();
        Box::pin(async move {
            throw_if_aborted(options.as_ref())?;
            self.store.delete(&provider_id, options).await?;
            self.remove_runtime_api_key(&provider_id);
            Ok(())
        })
    }
}
