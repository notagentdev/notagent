use std::sync::Arc;

use notagent_ai::auth::credential_store::InMemoryCredentialStore;
use notagent_ai::auth::types::{Credential, CredentialStore, OAuthCredential};
use notagent_ai::models::{CreateModelsOptions, ModelsRefreshOptions, Provider, create_models};
use notagent_ai::models_store::{InMemoryModelsStore, ModelsStore, ModelsStoreEntry};
use notagent_ai::providers::radius::{RadiusProviderOptions, radius_provider};
use notagent_ai::providers::radius_config::{
    get_radius_models, normalize_radius_gateway_url, sanitize_radius_gateway_config,
};
use serde_json::{Map, Value, json};

fn gateway_model(id: &str) -> Value {
    json!({
        "id": id,
        "name": id,
        "reasoning": true,
        "input": ["text"],
        "cost": { "input": 1, "output": 2, "cacheRead": 0, "cacheWrite": 0 },
        "contextWindow": 128000,
        "maxTokens": 4096,
    })
}

fn oauth_credential(config: Value) -> Credential {
    Credential::OAuth(OAuthCredential {
        refresh: "refresh".to_string(),
        access: "access".to_string(),
        expires: i64::MAX,
        extra: Map::from_iter([("gatewayConfig".to_string(), config)]),
    })
}

#[test]
fn the_gateway_url_gets_a_scheme_and_loses_trailing_slashes() {
    assert_eq!(
        normalize_radius_gateway_url("radius.example.com"),
        "https://radius.example.com"
    );
    assert_eq!(
        normalize_radius_gateway_url("https://radius.example.com///"),
        "https://radius.example.com"
    );
    assert_eq!(
        normalize_radius_gateway_url("http://radius.example.com"),
        "http://radius.example.com"
    );
    // A non-http scheme is not a scheme for this check.
    assert_eq!(
        normalize_radius_gateway_url("ftp://radius.example.com"),
        "https://ftp://radius.example.com"
    );
}

#[test]
fn the_sanitizer_drops_malformed_configs_and_models() {
    assert!(sanitize_radius_gateway_config(&json!([])).is_none());
    assert!(sanitize_radius_gateway_config(&json!({ "models": [] })).is_none());
    assert!(sanitize_radius_gateway_config(&json!({ "baseUrl": "https://gw.test" })).is_none());

    let config = sanitize_radius_gateway_config(&json!({
        "baseUrl": "https://gw.test",
        "models": [
            gateway_model("keep"),
            { "id": "no-name", "reasoning": true, "input": [], "cost": {}, "contextWindow": 1, "maxTokens": 1 },
            { "id": "bad-cost", "name": "bad", "reasoning": true, "input": [], "cost": [], "contextWindow": 1, "maxTokens": 1 },
            "not-an-object",
        ],
    }))
    .expect("config");
    assert_eq!(config.base_url, "https://gw.test");
    assert_eq!(config.models.len(), 1);
    assert_eq!(config.models[0]["id"], json!("keep"));
}

#[test]
fn credential_catalogs_become_models_of_the_configured_provider() {
    let credential = oauth_credential(json!({
        "baseUrl": "https://gw.test",
        "models": [gateway_model("radius-1")],
    }));
    let Credential::OAuth(credential) = &credential else {
        unreachable!()
    };
    let models = get_radius_models("radius-eu", Some(credential));
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id, "radius-1");
    assert_eq!(models[0].provider, "radius-eu");
    assert_eq!(models[0].api, "pi-messages");
    assert_eq!(models[0].base_url, "https://gw.test");
    assert_eq!(models[0].max_tokens, 4096);

    // Without a credential, and with an unusable one, the catalog stays empty.
    assert!(get_radius_models("radius", None).is_empty());
    let empty = OAuthCredential {
        refresh: "r".to_string(),
        access: "a".to_string(),
        expires: 0,
        extra: Map::new(),
    };
    assert!(get_radius_models("radius", Some(&empty)).is_empty());
}

#[test]
fn the_provider_starts_empty_and_is_dynamic() {
    let provider = radius_provider(RadiusProviderOptions::default());
    assert_eq!(provider.id(), "radius");
    assert_eq!(provider.name(), "Radius");
    assert!(provider.get_models().is_empty());
    assert!(provider.is_dynamic());
    assert_eq!(
        provider.auth().oauth.as_ref().expect("oauth").name(),
        "Radius"
    );

    let custom = radius_provider(RadiusProviderOptions {
        id: Some("radius-eu".to_string()),
        name: Some("Radius EU".to_string()),
        gateway: Some("gw.test/".to_string()),
    });
    assert_eq!(custom.id(), "radius-eu");
    assert_eq!(
        custom.auth().oauth.as_ref().expect("oauth").name(),
        "Radius EU"
    );
}

#[tokio::test]
async fn a_persisted_catalog_is_restored_without_network_access() {
    let store = Arc::new(InMemoryModelsStore::new());
    let entry = ModelsStoreEntry {
        models: get_radius_models(
            "radius",
            match &oauth_credential(json!({
                "baseUrl": "https://gw.test",
                "models": [gateway_model("stored-1")],
            })) {
                Credential::OAuth(credential) => Some(credential),
                _ => None,
            },
        ),
        checked_at: Some(1),
        etag: None,
        last_modified: None,
    };
    store.write("radius", entry, None).await.expect("write");

    let models = create_models(Some(CreateModelsOptions {
        models_store: Some(store),
        ..Default::default()
    }));
    let provider = radius_provider(RadiusProviderOptions::default());
    models.set_provider(provider.clone());

    let result = models
        .refresh(Some(ModelsRefreshOptions {
            allow_network: Some(false),
            providers: Some(vec!["radius".to_string()]),
            ..Default::default()
        }))
        .await;
    assert!(result.errors.is_empty());
    assert_eq!(
        provider
            .get_models()
            .iter()
            .map(|model| model.id.clone())
            .collect::<Vec<_>>(),
        vec!["stored-1".to_string()]
    );
}

#[tokio::test]
async fn the_legacy_credential_catalog_is_imported_once_and_persisted() {
    let credentials = Arc::new(InMemoryCredentialStore::new());
    credentials
        .modify(
            "radius",
            Box::new(|_current| {
                Box::pin(async {
                    Ok(Some(oauth_credential(json!({
                        "baseUrl": "https://gw.test",
                        "models": [gateway_model("legacy-1")],
                    }))))
                })
            }),
            None,
        )
        .await
        .expect("store credential");
    let store = Arc::new(InMemoryModelsStore::new());
    let models = create_models(Some(CreateModelsOptions {
        credentials: Some(credentials),
        models_store: Some(store.clone()),
        ..Default::default()
    }));
    let provider = radius_provider(RadiusProviderOptions::default());
    models.set_provider(provider.clone());

    let result = models
        .refresh(Some(ModelsRefreshOptions {
            allow_network: Some(false),
            providers: Some(vec!["radius".to_string()]),
            ..Default::default()
        }))
        .await;
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(
        provider
            .get_models()
            .iter()
            .map(|model| model.id.clone())
            .collect::<Vec<_>>(),
        vec!["legacy-1".to_string()]
    );
    let persisted = store
        .read("radius", None)
        .await
        .expect("read")
        .expect("entry");
    assert_eq!(persisted.models.len(), 1);
    assert!(persisted.checked_at.is_some());
}
