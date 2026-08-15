//! Port of `packages/coding-agent/test/auth-check.test.ts`.
//!
//! `notagent auth check` answers in three states. The distinction that matters
//! to a caller is `not_ready` (nothing configured) against `invalid` (something
//! configured but unusable), and the file-system assertions guard the promise
//! that a check never creates the auth file it reads.

use std::sync::Arc;

use notagent::cli::args::parse_args;
use notagent::cli::auth_check::{
    AuthCheckReason, AuthCheckStatus, check_provider_auth, create_auth_check_model_runtime,
    get_provider_credential,
};
use notagent::cli::auth_command::{AuthCommandKind, parse_auth_command};
use notagent::core::auth_storage::{AuthStorage, AuthStorageData, ReadOnlyAuthStorage};
use notagent::core::model_runtime::{CreateModelRuntimeOptions, ModelRuntime};
use notagent::core::models_store::InMemoryCodingAgentModelsStore;
use notagent_ai::auth::types::{AuthType, CredentialStore};
use serde_json::{Value, json};

fn storage_data(value: Value) -> AuthStorageData {
    value.as_object().cloned().unwrap_or_default()
}

fn args(values: &[&str]) -> notagent::cli::args::Args {
    parse_args(
        &values
            .iter()
            .map(|value| (*value).to_owned())
            .collect::<Vec<_>>(),
    )
}

async fn create_runtime(credentials: Arc<dyn CredentialStore>) -> Arc<ModelRuntime> {
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
async fn reports_a_configured_provider_as_ready() {
    let credentials = Arc::new(AuthStorage::in_memory(storage_data(json!({
        "openai": { "type": "api_key", "key": "test-key" }
    }))));
    let runtime = create_runtime(credentials).await;

    let result = check_provider_auth(&args(&["--provider", "openai"]), &runtime, false)
        .await
        .expect("check");

    assert_eq!(result.status, AuthCheckStatus::Ready);
    assert_eq!(result.provider, "openai");
    assert_eq!(result.reason, None);
    assert_eq!(result.auth_type, Some(AuthType::ApiKey));
}

#[tokio::test]
async fn resolves_the_provider_from_model() {
    let credentials = Arc::new(AuthStorage::in_memory(storage_data(json!({
        "openai": { "type": "api_key", "key": "test-key" }
    }))));
    let runtime = create_runtime(credentials).await;

    let result = check_provider_auth(&args(&["--model", "openai/gpt-5.5"]), &runtime, false)
        .await
        .expect("check");
    assert_eq!(result.status, AuthCheckStatus::Ready);
    assert_eq!(result.provider, "openai");
    assert_eq!(result.auth_type, Some(AuthType::ApiKey));

    let result = check_provider_auth(
        &args(&["--provider", "openai", "--model", "gpt-5.5"]),
        &runtime,
        false,
    )
    .await
    .expect("check");
    assert_eq!(result.status, AuthCheckStatus::Ready);
    assert_eq!(result.provider, "openai");
}

#[tokio::test]
async fn reads_credentials_without_refreshing_oauth_when_requested() {
    let api_credentials = Arc::new(AuthStorage::in_memory(storage_data(json!({
        "openai": { "type": "api_key", "key": "test-key" }
    }))));
    let api_runtime =
        create_runtime(Arc::clone(&api_credentials) as Arc<dyn CredentialStore>).await;
    assert_eq!(
        get_provider_credential("openai", &api_runtime, api_credentials.as_ref(), false).await,
        Some("test-key".to_owned())
    );

    // An expired OAuth credential is returned verbatim when refreshing is off;
    // refreshing it would need the network the check promised not to use.
    let credentials = Arc::new(AuthStorage::in_memory(storage_data(json!({
        "openai-codex": {
            "type": "oauth",
            "access": "old-token",
            "refresh": "refresh-token",
            "expires": 0
        }
    }))));
    let oauth_runtime = create_runtime(Arc::clone(&credentials) as Arc<dyn CredentialStore>).await;
    assert!(
        oauth_runtime.get_provider("openai-codex").is_some(),
        "OpenAI Codex OAuth provider is not registered"
    );

    assert_eq!(
        get_provider_credential("openai-codex", &oauth_runtime, credentials.as_ref(), false).await,
        Some("old-token".to_owned())
    );
}

#[tokio::test]
async fn reports_an_unknown_provider_as_not_ready() {
    let credentials = Arc::new(AuthStorage::in_memory(AuthStorageData::new()));
    let runtime = create_runtime(credentials).await;

    let result = check_provider_auth(&args(&["--provider", "not-installed"]), &runtime, false)
        .await
        .expect("check");

    assert_eq!(result.status, AuthCheckStatus::NotReady);
    assert_eq!(result.provider, "not-installed");
    assert_eq!(result.reason, Some(AuthCheckReason::ProviderNotFound));
}

#[tokio::test]
async fn does_not_treat_an_unresolved_stored_environment_reference_as_configured() {
    let temp = tempfile::tempdir().expect("temp dir");
    let auth_path = temp.path().join("auth.json");
    std::fs::write(
        &auth_path,
        json!({ "openai": { "type": "api_key", "key": "$MISSING_AUTH_CHECK_KEY" } }).to_string(),
    )
    .expect("write auth");
    let credentials =
        Arc::new(ReadOnlyAuthStorage::new(&auth_path.to_string_lossy()).expect("storage"));
    let runtime = create_runtime(credentials).await;

    let result = check_provider_auth(&args(&["--provider", "openai"]), &runtime, false)
        .await
        .expect("check");

    assert_eq!(result.status, AuthCheckStatus::NotReady);
    assert_eq!(result.provider, "openai");
    assert_eq!(
        result.reason,
        Some(AuthCheckReason::CredentialsNotConfigured)
    );
}

#[tokio::test]
async fn reports_malformed_auth_state_as_invalid() {
    let temp = tempfile::tempdir().expect("temp dir");
    let auth_path = temp.path().join("auth.json");
    std::fs::write(&auth_path, "{invalid-json").expect("write auth");
    let credentials =
        Arc::new(ReadOnlyAuthStorage::new(&auth_path.to_string_lossy()).expect("storage"));
    let runtime = create_runtime(credentials).await;

    let result = check_provider_auth(&args(&["--provider", "openai"]), &runtime, false)
        .await
        .expect("check");

    assert_eq!(result.status, AuthCheckStatus::Invalid);
    assert_eq!(result.provider, "openai");
    assert_eq!(result.reason, Some(AuthCheckReason::InvalidState));
}

#[tokio::test]
async fn does_not_create_an_auth_file_or_its_parent_directory() {
    let temp = tempfile::tempdir().expect("temp dir");
    let agent_dir = temp.path().join("agent");
    let auth_path = agent_dir.join("auth.json");
    let credentials =
        Arc::new(ReadOnlyAuthStorage::new(&auth_path.to_string_lossy()).expect("storage"));
    let runtime = create_runtime(credentials).await;

    let result = check_provider_auth(&args(&["--provider", "openai"]), &runtime, false)
        .await
        .expect("check");

    assert_eq!(result.status, AuthCheckStatus::NotReady);
    assert_eq!(
        result.reason,
        Some(AuthCheckReason::CredentialsNotConfigured)
    );
    assert!(!auth_path.exists());
    assert!(!agent_dir.exists());
}

#[test]
fn accepts_optional_json_output_credential_output_and_no_refresh() {
    let command = parse_auth_command(&[
        "auth".to_owned(),
        "check".to_owned(),
        "--provider".to_owned(),
        "openai".to_owned(),
    ])
    .expect("parse")
    .expect("command");
    assert_eq!(command.kind, AuthCommandKind::Check);
    assert_eq!(
        command.args,
        vec!["--provider".to_owned(), "openai".to_owned()]
    );
    assert!(!command.json);
    assert!(!command.credentials);
    assert!(!command.no_refresh);
    assert_eq!(command.min_expiry_ms, None);

    let command = parse_auth_command(
        &[
            "auth",
            "check",
            "--json",
            "--credentials",
            "--no-refresh",
            "--provider",
            "openai",
        ]
        .map(str::to_owned),
    )
    .expect("parse")
    .expect("command");
    assert_eq!(command.kind, AuthCommandKind::Check);
    assert_eq!(
        command.args,
        vec!["--provider".to_owned(), "openai".to_owned()]
    );
    assert!(command.json);
    assert!(command.credentials);
    assert!(command.no_refresh);
}

#[tokio::test]
async fn creates_an_auth_check_runtime_without_catalog_storage() {
    let runtime =
        create_auth_check_model_runtime(Arc::new(AuthStorage::in_memory(AuthStorageData::new())))
            .await
            .expect("runtime");

    assert!(runtime.get_provider("openai").is_some());
}
