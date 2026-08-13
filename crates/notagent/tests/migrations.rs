//! Tests for the port of `packages/coding-agent/src/migrations.ts`.
//!
//! The TS suite drives the migrations through the real agent directory
//! (`getAgentDir()`); the Rust port exposes directory-scoped variants so the
//! cases run against a temp directory without touching the user's home.

use serde_json::{Value, json};

fn read_json(path: &std::path::Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).expect("reads")).expect("parses")
}

#[test]
fn migrates_oauth_and_api_keys_into_auth_json() {
    let directory = tempfile::Builder::new()
        .prefix("notagent-migrations-")
        .tempdir()
        .expect("temp dir");
    let agent_dir = directory.path();
    std::fs::write(
        agent_dir.join("oauth.json"),
        json!({ "anthropic": { "access": "token", "refresh": "refresh" } }).to_string(),
    )
    .expect("writes");
    std::fs::write(
        agent_dir.join("settings.json"),
        json!({ "theme": "dark", "apiKeys": { "openai": "sk-test", "anthropic": "ignored" } })
            .to_string(),
    )
    .expect("writes");

    let providers = notagent::migrations::testing::migrate_auth_to_auth_json_in(agent_dir);

    assert!(providers.contains(&"anthropic".to_owned()));
    assert!(providers.contains(&"openai".to_owned()));
    let auth = read_json(&agent_dir.join("auth.json"));
    assert_eq!(auth["anthropic"]["type"], "oauth");
    assert_eq!(auth["anthropic"]["access"], "token");
    // An existing oauth credential wins over the settings apiKey.
    assert_eq!(
        auth["openai"],
        json!({ "type": "api_key", "key": "sk-test" })
    );
    // The legacy files are retired and the apiKeys block is removed.
    assert!(agent_dir.join("oauth.json.migrated").exists());
    assert!(!agent_dir.join("oauth.json").exists());
    let settings = read_json(&agent_dir.join("settings.json"));
    assert!(settings.get("apiKeys").is_none());
    assert_eq!(settings["theme"], "dark");
}

#[test]
fn skips_the_auth_migration_when_auth_json_exists() {
    let directory = tempfile::Builder::new()
        .prefix("notagent-migrations-")
        .tempdir()
        .expect("temp dir");
    let agent_dir = directory.path();
    std::fs::write(
        agent_dir.join("auth.json"),
        json!({ "existing": { "type": "api_key" } }).to_string(),
    )
    .expect("writes");
    std::fs::write(
        agent_dir.join("oauth.json"),
        json!({ "anthropic": {} }).to_string(),
    )
    .expect("writes");

    assert!(notagent::migrations::testing::migrate_auth_to_auth_json_in(agent_dir).is_empty());
    assert!(agent_dir.join("oauth.json").exists());
    assert_eq!(
        read_json(&agent_dir.join("auth.json"))["existing"]["type"],
        "api_key"
    );
}

#[test]
fn moves_stray_sessions_into_their_cwd_directory() {
    let directory = tempfile::Builder::new()
        .prefix("notagent-migrations-")
        .tempdir()
        .expect("temp dir");
    let agent_dir = directory.path();
    let session = "{\"type\":\"session\",\"version\":3,\"id\":\"s1\",\"cwd\":\"/Users/dev/project\"}\n{\"id\":\"a\"}\n";
    std::fs::write(agent_dir.join("2026-01-01_s1.jsonl"), session).expect("writes");
    // A file without a session header stays where it is.
    std::fs::write(agent_dir.join("other.jsonl"), "{\"type\":\"other\"}\n").expect("writes");

    notagent::migrations::testing::migrate_sessions_from_agent_root_in(agent_dir);

    let moved = agent_dir
        .join("sessions")
        .join("--Users-dev-project--")
        .join("2026-01-01_s1.jsonl");
    assert!(moved.exists(), "session was not moved");
    assert!(!agent_dir.join("2026-01-01_s1.jsonl").exists());
    assert!(agent_dir.join("other.jsonl").exists());
}

#[test]
fn keeps_an_existing_target_session_file() {
    let directory = tempfile::Builder::new()
        .prefix("notagent-migrations-")
        .tempdir()
        .expect("temp dir");
    let agent_dir = directory.path();
    let session = "{\"type\":\"session\",\"version\":3,\"id\":\"s1\",\"cwd\":\"/work\"}\n";
    std::fs::write(agent_dir.join("s1.jsonl"), session).expect("writes");
    let target_dir = agent_dir.join("sessions").join("--work--");
    std::fs::create_dir_all(&target_dir).expect("creates");
    std::fs::write(target_dir.join("s1.jsonl"), "existing").expect("writes");

    notagent::migrations::testing::migrate_sessions_from_agent_root_in(agent_dir);

    assert_eq!(
        std::fs::read_to_string(target_dir.join("s1.jsonl")).expect("reads"),
        "existing"
    );
    assert!(agent_dir.join("s1.jsonl").exists());
}
