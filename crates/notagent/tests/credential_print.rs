use std::sync::Arc;

use notagent::cli::args::parse_args;
use notagent::cli::auth_command::{AuthCommandKind, is_auth_command_help, parse_auth_command};
use notagent::cli::credential_print::resolve_credential_for_print;
use notagent::core::auth_storage::{AuthStorage, AuthStorageData};
use notagent::core::model_runtime::{CreateModelRuntimeOptions, ModelRuntime};
use notagent::core::models_store::InMemoryCodingAgentModelsStore;
use notagent_ai::auth::types::CredentialStore;
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

fn owned(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn hour_from_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
        + 60 * 60 * 1000
}

async fn create_runtime(credentials: Arc<dyn CredentialStore>) -> Arc<ModelRuntime> {
    ModelRuntime::create(CreateModelRuntimeOptions {
        credentials: Some(credentials),
        models_path: Some(None),
        models_store: Some(Arc::new(InMemoryCodingAgentModelsStore::new())),
        allow_model_network: Some(false),
        ..CreateModelRuntimeOptions::default()
    })
    .await
    .expect("model runtime")
}

#[tokio::test]
async fn prints_a_resolved_api_key() {
    let runtime = create_runtime(Arc::new(AuthStorage::in_memory(storage_data(json!({
        "openai": { "type": "api_key", "key": "test-api-key" }
    })))))
    .await;

    let credential = resolve_credential_for_print(
        &args(&["--provider", "openai"]),
        &runtime,
        AuthCommandKind::ApiKey,
        None,
        None,
    )
    .await
    .expect("credential");

    assert_eq!(credential, "test-api-key");
}

#[tokio::test]
async fn prints_bearer_tokens_resolved_from_an_authorization_header() {
    let runtime = create_runtime(Arc::new(AuthStorage::in_memory(storage_data(json!({
        "kimi-coding": {
            "type": "oauth",
            "access": "header-test-token",
            "refresh": "test-refresh-token",
            "expires": hour_from_now(),
        }
    })))))
    .await;

    let credential = resolve_credential_for_print(
        &args(&["--provider", "kimi-coding"]),
        &runtime,
        AuthCommandKind::BearerToken,
        None,
        None,
    )
    .await
    .expect("credential");

    assert_eq!(credential, "header-test-token");
}

#[test]
fn parses_credential_commands() {
    let command = parse_auth_command(&owned(&["auth", "print-api-key", "--provider", "openai"]))
        .expect("parse")
        .expect("command");
    assert_eq!(command.kind, AuthCommandKind::ApiKey);
    assert_eq!(command.args, owned(&["--provider", "openai"]));
    assert!(!command.json);
    assert!(!command.credentials);
    assert!(!command.no_refresh);
    assert_eq!(command.min_expiry_ms, None);

    let command = parse_auth_command(&owned(&["auth", "print-bearer-token"]))
        .expect("parse")
        .expect("command");
    assert_eq!(command.kind, AuthCommandKind::BearerToken);

    let command = parse_auth_command(&owned(&[
        "auth",
        "print-bearer-token",
        "--min-expiry",
        "30m",
    ]))
    .expect("parse")
    .expect("command");
    assert_eq!(command.kind, AuthCommandKind::BearerToken);
    assert!(command.args.is_empty());
    assert_eq!(command.min_expiry_ms, Some(30 * 60_000));

    let error = parse_auth_command(&owned(&["auth", "print-api-key", "--min-expiry", "30m"]))
        .expect_err("rejects --min-expiry");
    assert!(error.0.contains("only supported by print-bearer-token"));

    assert!(is_auth_command_help(&owned(&["auth", "--help"])));
    assert!(is_auth_command_help(&owned(&[
        "auth",
        "print-api-key",
        "--help"
    ])));
    assert!(is_auth_command_help(&owned(&[
        "auth",
        "print-bearer-token",
        "-h"
    ])));
    assert!(is_auth_command_help(&owned(&["auth", "check", "--help"])));
    assert!(parse_auth_command(&owned(&["auth", "unknown"])).is_err());
}

#[tokio::test]
async fn rejects_invalid_arguments_and_credential_types() {
    let runtime = create_runtime(Arc::new(AuthStorage::in_memory(storage_data(json!({
        "openai-codex": {
            "type": "oauth",
            "access": "test-token-not-to-be-printed",
            "refresh": "test-refresh-token",
            "expires": hour_from_now(),
        }
    })))))
    .await;

    let error =
        resolve_credential_for_print(&args(&[]), &runtime, AuthCommandKind::ApiKey, None, None)
            .await
            .expect_err("requires a target");
    assert!(
        error
            .0
            .contains("requires --provider <provider> or --model <model>")
    );

    let error = resolve_credential_for_print(
        &args(&["--provider", "openai-codex"]),
        &runtime,
        AuthCommandKind::ApiKey,
        None,
        None,
    )
    .await
    .expect_err("refuses an OAuth provider");
    assert!(error.0.contains("configured with OAuth"));
}
