//! The server-backed providers must survive the runtime's composition step,
//! which is what decides whether `/login` can offer them at all.

use std::sync::Arc;

use notagent::core::auth_storage::AuthStorage;
use notagent::core::model_runtime::{CreateModelRuntimeOptions, ModelRuntime};
use notagent::core::models_store::InMemoryCodingAgentModelsStore;
use notagent_ai::auth::types::CredentialStore;
use serde_json::json;

async fn create_runtime() -> Arc<ModelRuntime> {
    let credentials: Arc<dyn CredentialStore> = Arc::new(AuthStorage::in_memory(
        json!({}).as_object().cloned().unwrap_or_default(),
    ));
    ModelRuntime::create(CreateModelRuntimeOptions {
        credentials: Some(credentials),
        models_path: Some(None),
        models_store: Some(Arc::new(InMemoryCodingAgentModelsStore::new())),
        allow_model_network: Some(false),
        refresh_on_create: Some(false),
        ..CreateModelRuntimeOptions::default()
    })
    .await
    .expect("model runtime")
}

#[tokio::test]
async fn the_server_backed_providers_reach_the_composed_runtime() {
    let runtime = create_runtime().await;
    let ids: Vec<String> = runtime
        .get_providers()
        .into_iter()
        .map(|provider| provider.id().to_owned())
        .collect();
    for id in ["ollama", "lmstudio", "custom"] {
        assert!(
            ids.contains(&id.to_string()),
            "{id} did not survive composition; ids: {ids:?}"
        );
    }
}

#[tokio::test]
async fn they_offer_the_api_key_login_the_selector_lists() {
    let runtime = create_runtime().await;
    for id in ["ollama", "lmstudio", "custom"] {
        let provider = runtime
            .get_provider(id)
            .unwrap_or_else(|| panic!("{id} is missing"));
        assert!(
            provider.auth().api_key.is_some(),
            "{id} offers no api-key login, so the selector has nothing to list"
        );
    }
}
