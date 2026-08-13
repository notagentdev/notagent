//! Port of the core cases from
//! `packages/coding-agent/test/settings-manager.test.ts`.
//!
//! The TS suite drives typed setters (`setTheme`, `setDefaultThinkingLevel`);
//! this slice of the port exposes `set_global_field`/`set_project_field` with
//! the wire names, so the tests use those.

use std::path::Path;
use std::sync::Arc;

use notagent::core::settings_manager::{
    InMemorySettingsStorage, SettingsManager, SettingsManagerCreateOptions, SettingsScope,
    migrate_settings,
};
use serde_json::{Value, json};

fn read_settings(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).expect("reads")).expect("parses")
}

struct Harness {
    _directory: tempfile::TempDir,
    agent_dir: std::path::PathBuf,
    project_dir: std::path::PathBuf,
}

fn harness() -> Harness {
    let directory = tempfile::Builder::new()
        .prefix("notagent-settings-")
        .tempdir()
        .expect("temp dir");
    let agent_dir = directory.path().join("agent");
    let project_dir = directory.path().join("project");
    std::fs::create_dir_all(&agent_dir).expect("creates");
    std::fs::create_dir_all(project_dir.join(".notagent")).expect("creates");
    Harness {
        _directory: directory,
        agent_dir,
        project_dir,
    }
}

fn manager(harness: &Harness) -> SettingsManager {
    SettingsManager::create(
        &harness.project_dir,
        Some(&harness.agent_dir),
        SettingsManagerCreateOptions::default(),
    )
}

#[test]
fn preserves_externally_added_settings_when_changing_a_field() {
    let harness = harness();
    let settings_path = harness.agent_dir.join("settings.json");
    std::fs::write(
        &settings_path,
        json!({ "theme": "dark", "defaultModel": "claude-sonnet" }).to_string(),
    )
    .expect("writes");

    let manager = manager(&harness);

    // The user edits settings.json externally while the manager is running.
    let mut current = read_settings(&settings_path);
    current["enabledModels"] = json!(["claude-opus-4-5", "gpt-5.2-codex"]);
    std::fs::write(
        &settings_path,
        serde_json::to_string_pretty(&current).expect("serializes"),
    )
    .expect("writes");

    manager.set_global_field("defaultThinkingLevel", json!("high"));
    manager.flush();

    let saved = read_settings(&settings_path);
    assert_eq!(
        saved["enabledModels"],
        json!(["claude-opus-4-5", "gpt-5.2-codex"])
    );
    assert_eq!(saved["defaultThinkingLevel"], "high");
    assert_eq!(saved["theme"], "dark");
    assert_eq!(saved["defaultModel"], "claude-sonnet");
}

#[test]
fn preserves_unknown_keys_including_dropped_extension_settings() {
    let harness = harness();
    let settings_path = harness.agent_dir.join("settings.json");
    std::fs::write(
        &settings_path,
        json!({ "defaultModel": "claude-sonnet" }).to_string(),
    )
    .expect("writes");

    let manager = manager(&harness);
    let mut current = read_settings(&settings_path);
    current["shellPath"] = json!("/bin/zsh");
    // `extensions` is dropped from the typed settings with the extension system;
    // it must still survive as an unknown key.
    current["extensions"] = json!(["/path/to/extension.ts"]);
    std::fs::write(
        &settings_path,
        serde_json::to_string_pretty(&current).expect("serializes"),
    )
    .expect("writes");

    manager.set_global_field("theme", json!("light"));
    manager.flush();

    let saved = read_settings(&settings_path);
    assert_eq!(saved["shellPath"], "/bin/zsh");
    assert_eq!(saved["extensions"], json!(["/path/to/extension.ts"]));
    assert_eq!(saved["theme"], "light");
}

#[test]
fn lets_in_memory_changes_override_file_changes_for_the_same_key() {
    let harness = harness();
    let settings_path = harness.agent_dir.join("settings.json");
    std::fs::write(&settings_path, json!({ "theme": "dark" }).to_string()).expect("writes");

    let manager = manager(&harness);
    let mut current = read_settings(&settings_path);
    current["defaultThinkingLevel"] = json!("low");
    std::fs::write(
        &settings_path,
        serde_json::to_string_pretty(&current).expect("serializes"),
    )
    .expect("writes");

    manager.set_global_field("defaultThinkingLevel", json!("high"));
    manager.flush();

    assert_eq!(
        read_settings(&settings_path)["defaultThinkingLevel"],
        "high"
    );
}

#[test]
fn merges_project_settings_over_global_settings() {
    let harness = harness();
    std::fs::write(
        harness.agent_dir.join("settings.json"),
        json!({ "theme": "dark", "compaction": { "enabled": true, "reserveTokens": 16384 } })
            .to_string(),
    )
    .expect("writes");
    std::fs::write(
        harness.project_dir.join(".notagent").join("settings.json"),
        json!({ "theme": "light", "compaction": { "reserveTokens": 4096 } }).to_string(),
    )
    .expect("writes");

    let manager = manager(&harness);
    let settings = manager.settings();
    assert_eq!(settings.theme.as_deref(), Some("light"));
    let compaction = settings.compaction.expect("compaction");
    // Nested objects merge instead of replacing.
    assert_eq!(compaction.enabled, Some(true));
    assert_eq!(compaction.reserve_tokens, Some(4096));
}

#[test]
fn ignores_project_settings_until_the_project_is_trusted() {
    let harness = harness();
    std::fs::write(
        harness.agent_dir.join("settings.json"),
        json!({ "theme": "dark" }).to_string(),
    )
    .expect("writes");
    std::fs::write(
        harness.project_dir.join(".notagent").join("settings.json"),
        json!({ "theme": "light" }).to_string(),
    )
    .expect("writes");

    let manager = SettingsManager::create(
        &harness.project_dir,
        Some(&harness.agent_dir),
        SettingsManagerCreateOptions {
            project_trusted: Some(false),
        },
    );
    assert_eq!(manager.settings().theme.as_deref(), Some("dark"));
    assert!(manager.set_project_field("theme", json!("blue")).is_err());

    manager.set_project_trusted(true);
    assert_eq!(manager.settings().theme.as_deref(), Some("light"));
    manager
        .set_project_field("theme", json!("blue"))
        .expect("writes project settings");
    manager.flush();
    let saved = read_settings(&harness.project_dir.join(".notagent").join("settings.json"));
    assert_eq!(saved["theme"], "blue");
}

#[test]
fn records_and_drains_load_errors_without_losing_the_file() {
    let harness = harness();
    let settings_path = harness.agent_dir.join("settings.json");
    std::fs::write(&settings_path, "{ not json").expect("writes");

    let manager = manager(&harness);
    let errors = manager.drain_errors();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].scope, SettingsScope::Global);
    assert!(manager.drain_errors().is_empty());

    // A broken file is never overwritten.
    manager.set_global_field("theme", json!("light"));
    manager.flush();
    assert_eq!(
        std::fs::read_to_string(&settings_path).expect("reads"),
        "{ not json"
    );
}

#[test]
fn writes_nested_fields_without_dropping_sibling_keys() {
    let harness = harness();
    let settings_path = harness.agent_dir.join("settings.json");
    std::fs::write(
        &settings_path,
        json!({ "compaction": { "enabled": true, "reserveTokens": 16384 } }).to_string(),
    )
    .expect("writes");

    let manager = manager(&harness);
    manager.set_global_nested_field("compaction", "keepRecentTokens", json!(20000));
    manager.flush();

    let saved = read_settings(&settings_path);
    assert_eq!(saved["compaction"]["enabled"], true);
    assert_eq!(saved["compaction"]["reserveTokens"], 16384);
    assert_eq!(saved["compaction"]["keepRecentTokens"], 20000);
}

#[test]
fn migrates_legacy_settings_shapes() {
    // queueMode -> steeringMode
    let migrated = migrate_settings(
        json!({ "queueMode": "one-at-a-time" })
            .as_object()
            .unwrap()
            .clone(),
    );
    assert_eq!(migrated["steeringMode"], "one-at-a-time");
    assert!(!migrated.contains_key("queueMode"));

    // websockets -> transport
    let migrated = migrate_settings(json!({ "websockets": true }).as_object().unwrap().clone());
    assert_eq!(migrated["transport"], "websocket");
    let migrated = migrate_settings(json!({ "websockets": false }).as_object().unwrap().clone());
    assert_eq!(migrated["transport"], "sse");

    // skills object -> array plus enableSkillCommands
    let migrated = migrate_settings(
        json!({ "skills": { "enableSkillCommands": false, "customDirectories": ["/a"] } })
            .as_object()
            .unwrap()
            .clone(),
    );
    assert_eq!(migrated["skills"], json!(["/a"]));
    assert_eq!(migrated["enableSkillCommands"], false);

    // retry.maxDelayMs -> retry.provider.maxRetryDelayMs
    let migrated = migrate_settings(
        json!({ "retry": { "maxDelayMs": 1234 } })
            .as_object()
            .unwrap()
            .clone(),
    );
    assert_eq!(migrated["retry"]["provider"]["maxRetryDelayMs"], 1234);
    assert!(
        !migrated["retry"]
            .as_object()
            .unwrap()
            .contains_key("maxDelayMs")
    );
}

#[test]
fn in_memory_manager_never_touches_the_filesystem() {
    let storage = Arc::new(InMemorySettingsStorage::default());
    let manager = SettingsManager::from_storage(storage, SettingsManagerCreateOptions::default());
    manager.set_global_field("theme", json!("dark"));
    assert_eq!(manager.settings().theme.as_deref(), Some("dark"));
    manager.reload();
    assert_eq!(manager.settings().theme.as_deref(), Some("dark"));
}
