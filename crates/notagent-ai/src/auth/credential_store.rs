use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use tokio::sync::Mutex as AsyncMutex;

use crate::auth::types::{
    AuthOperationOptions, BoxFuture, Credential, CredentialInfo, CredentialStore,
    CredentialStoreError, ModifyFn,
};

#[derive(Default)]
struct State {
    credentials: BTreeMap<String, Credential>,
    chains: BTreeMap<String, Arc<AsyncMutex<()>>>,
}

/// `InMemoryCredentialStore`
#[derive(Default, Clone)]
pub struct InMemoryCredentialStore {
    state: Arc<Mutex<State>>,
}

impl InMemoryCredentialStore {
    pub fn new() -> Self {
        InMemoryCredentialStore::default()
    }

    fn chain(&self, provider_id: &str) -> Arc<AsyncMutex<()>> {
        let mut state = self.state.lock().expect("credential store poisoned");
        Arc::clone(state.chains.entry(provider_id.to_string()).or_default())
    }
}

fn aborted(options: &Option<AuthOperationOptions>) -> bool {
    options
        .as_ref()
        .and_then(|options| options.signal.as_ref())
        .is_some_and(|signal| signal.is_cancelled())
}

impl CredentialStore for InMemoryCredentialStore {
    fn read(
        &self,
        provider_id: &str,
        options: Option<AuthOperationOptions>,
    ) -> BoxFuture<'_, Result<Option<Credential>, CredentialStoreError>> {
        let provider_id = provider_id.to_string();
        let state = Arc::clone(&self.state);
        Box::pin(async move {
            if aborted(&options) {
                return Err(CredentialStoreError(
                    "The operation was aborted".to_string(),
                ));
            }
            let state = state.lock().expect("credential store poisoned");
            Ok(state.credentials.get(&provider_id).cloned())
        })
    }

    fn list(
        &self,
        options: Option<AuthOperationOptions>,
    ) -> BoxFuture<'_, Result<Vec<CredentialInfo>, CredentialStoreError>> {
        let state = Arc::clone(&self.state);
        Box::pin(async move {
            if aborted(&options) {
                return Err(CredentialStoreError(
                    "The operation was aborted".to_string(),
                ));
            }
            let state = state.lock().expect("credential store poisoned");
            Ok(state
                .credentials
                .iter()
                .map(|(provider_id, credential)| CredentialInfo {
                    provider_id: provider_id.clone(),
                    credential_type: credential.credential_type(),
                })
                .collect())
        })
    }

    fn modify<'a>(
        &'a self,
        provider_id: &'a str,
        modify: ModifyFn<'a>,
        options: Option<AuthOperationOptions>,
    ) -> BoxFuture<'a, Result<Option<Credential>, CredentialStoreError>> {
        let provider_id = provider_id.to_string();
        let chain = self.chain(&provider_id);
        let state = Arc::clone(&self.state);
        Box::pin(async move {
            let _guard = chain.lock().await;
            if aborted(&options) {
                return Err(CredentialStoreError(
                    "The operation was aborted".to_string(),
                ));
            }
            let current = {
                let state = state.lock().expect("credential store poisoned");
                state.credentials.get(&provider_id).cloned()
            };
            let next = modify(current.clone()).await?;
            if aborted(&options) {
                return Err(CredentialStoreError(
                    "The operation was aborted".to_string(),
                ));
            }
            if let Some(next) = &next {
                let mut state = state.lock().expect("credential store poisoned");
                state.credentials.insert(provider_id.clone(), next.clone());
            }
            Ok(next.or(current))
        })
    }

    fn delete(
        &self,
        provider_id: &str,
        options: Option<AuthOperationOptions>,
    ) -> BoxFuture<'_, Result<(), CredentialStoreError>> {
        let provider_id = provider_id.to_string();
        let chain = self.chain(&provider_id);
        let state = Arc::clone(&self.state);
        Box::pin(async move {
            let _guard = chain.lock().await;
            if aborted(&options) {
                return Err(CredentialStoreError(
                    "The operation was aborted".to_string(),
                ));
            }
            let mut state = state.lock().expect("credential store poisoned");
            state.credentials.remove(&provider_id);
            Ok(())
        })
    }
}
