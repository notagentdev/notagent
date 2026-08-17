//! Port of the core cases from
//! `packages/coding-agent/test/settings-manager.test.ts`.
//!
//! The TS suite drives typed setters (`setTheme`, `setDefaultThinkingLevel`);
//! this slice of the port exposes `set_global_field`/`set_project_field` with
//! the wire names, so the tests use those.

use std::path::Path;
use std::sync::Arc;

use notagent::core::settings_manager::{
    DefaultProjectTrust, DoubleEscapeAction, InMemorySettingsStorage, QueueMode,
    ResolvedBranchSummarySettings, ResolvedCompactionSettings, ResolvedProviderRetrySettings,
    ResolvedRetrySettings, SettingsManager, SettingsManagerCreateOptions, SettingsScope,
    TreeFilterMode, WarningSettings, migrate_settings,
};

/// The editor test reads process environment variables, which are global.
fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

fn home_dir() -> std::path::PathBuf {
    dirs::home_dir().expect("home directory")
}
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
    assert_eq!(manager.get_theme_setting().as_deref(), Some("light"));
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
    assert_eq!(manager.get_theme_setting().as_deref(), Some("dark"));
    assert!(manager.set_project_field("theme", json!("blue")).is_err());

    manager.set_project_trusted(true);
    assert_eq!(manager.get_theme_setting().as_deref(), Some("light"));
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
    assert_eq!(manager.get_theme_setting().as_deref(), Some("dark"));
    manager.reload();
    assert_eq!(manager.get_theme_setting().as_deref(), Some("dark"));
}

// ---------------------------------------------------------------------------
// Typed accessors (settings-manager.test.ts:337-587)
// ---------------------------------------------------------------------------

use notagent::core::settings_manager::{
    DEFAULT_HTTP_IDLE_TIMEOUT_MS, FullscreenExitOutput, MermaidRenderingMode, ScrollViewScrollbar,
    TuiMode,
};

fn write_global(harness: &Harness, settings: Value) {
    std::fs::write(
        harness.agent_dir.join("settings.json"),
        settings.to_string(),
    )
    .expect("writes");
}

fn write_project(harness: &Harness, settings: Value) {
    std::fs::write(
        harness.project_dir.join(".notagent").join("settings.json"),
        settings.to_string(),
    )
    .expect("writes");
}

fn global_settings(harness: &Harness) -> Value {
    read_settings(&harness.agent_dir.join("settings.json"))
}

#[test]
fn http_idle_timeout_defaults_to_five_minutes() {
    let harness = harness();
    assert_eq!(
        manager(&harness).get_http_idle_timeout_ms(),
        Ok(DEFAULT_HTTP_IDLE_TIMEOUT_MS)
    );
}

#[test]
fn http_idle_timeout_uses_merged_global_and_project_settings() {
    let harness = harness();
    write_global(&harness, json!({ "httpIdleTimeoutMs": 300_000 }));
    write_project(&harness, json!({ "httpIdleTimeoutMs": 0 }));
    assert_eq!(manager(&harness).get_http_idle_timeout_ms(), Ok(0));
}

#[test]
fn http_idle_timeout_rejects_invalid_values() {
    let harness = harness();
    write_global(&harness, json!({ "httpIdleTimeoutMs": -1 }));
    let error = manager(&harness)
        .get_http_idle_timeout_ms()
        .expect_err("invalid");
    assert!(
        error.contains("Invalid httpIdleTimeoutMs setting"),
        "{error}"
    );
    // A string setting stays usable, as in TS.
    write_global(&harness, json!({ "httpIdleTimeoutMs": "disabled" }));
    assert_eq!(manager(&harness).get_http_idle_timeout_ms(), Ok(0));
    write_global(&harness, json!({ "websocketConnectTimeoutMs": " 2500 " }));
    assert_eq!(
        manager(&harness).get_websocket_connect_timeout_ms(),
        Ok(Some(2500))
    );
}

#[test]
fn external_editor_resolves_by_precedence_and_platform() {
    let _guard = env_lock();
    let harness = harness();
    // SAFETY: the environment lock keeps concurrent tests out.
    unsafe {
        std::env::set_var("VISUAL", "vim");
        std::env::set_var("EDITOR", "nano");
    }
    write_global(&harness, json!({ "externalEditor": "code --wait" }));
    assert_eq!(
        manager(&harness).get_external_editor_command(),
        "code --wait"
    );

    write_global(&harness, json!({}));
    assert_eq!(manager(&harness).get_external_editor_command(), "vim");

    unsafe {
        std::env::remove_var("VISUAL");
        std::env::set_var("EDITOR", "emacs");
    }
    assert_eq!(manager(&harness).get_external_editor_command(), "emacs");

    unsafe { std::env::remove_var("EDITOR") };
    let expected = if cfg!(windows) { "notepad" } else { "nano" };
    assert_eq!(manager(&harness).get_external_editor_command(), expected);
}

#[test]
fn tui_mode_defaults_to_regular_and_persists_fullscreen() {
    let harness = harness();
    let manager = manager(&harness);
    assert_eq!(manager.get_tui_mode(), TuiMode::Regular);
    manager.set_tui_mode(TuiMode::Fullscreen);
    manager.flush();
    assert_eq!(manager.get_tui_mode(), TuiMode::Fullscreen);
    assert_eq!(global_settings(&harness)["tuiMode"], json!("fullscreen"));
}

#[test]
fn tui_mode_falls_back_to_regular_for_unsupported_values() {
    let harness = harness();
    write_global(&harness, json!({ "tuiMode": "other" }));
    assert_eq!(manager(&harness).get_tui_mode(), TuiMode::Regular);
    // The old `uiMode` setting is not recognized.
    write_global(&harness, json!({ "uiMode": "fullscreen" }));
    assert_eq!(manager(&harness).get_tui_mode(), TuiMode::Regular);
}

#[test]
fn validates_and_persists_fullscreen_settings() {
    let harness = harness();
    let manager = manager(&harness);
    assert_eq!(
        manager.get_fullscreen_exit_output(),
        FullscreenExitOutput::Transcript
    );
    assert_eq!(
        manager.get_fullscreen_scrollbar(),
        ScrollViewScrollbar::Auto
    );

    manager.set_fullscreen_exit_output(FullscreenExitOutput::ResumeHint);
    manager.set_fullscreen_scrollbar(ScrollViewScrollbar::Hidden);
    manager.flush();
    let saved = global_settings(&harness);
    assert_eq!(saved["fullscreenExitOutput"], json!("resume-hint"));
    assert_eq!(saved["fullscreenScrollbar"], json!("hidden"));

    write_global(
        &harness,
        json!({ "fullscreenExitOutput": "nothing", "fullscreenScrollbar": "sometimes" }),
    );
    let reloaded = self::manager(&harness);
    assert_eq!(
        reloaded.get_fullscreen_exit_output(),
        FullscreenExitOutput::Transcript
    );
    assert_eq!(
        reloaded.get_fullscreen_scrollbar(),
        ScrollViewScrollbar::Auto
    );
}

#[test]
fn output_pad_defaults_to_one_and_persists_binary_values() {
    let harness = harness();
    let manager = manager(&harness);
    assert_eq!(manager.get_output_pad(), 1);
    manager.set_output_pad(0);
    manager.flush();
    assert_eq!(manager.get_output_pad(), 0);
    assert_eq!(global_settings(&harness)["outputPad"], json!(0));

    write_global(&harness, json!({ "outputPad": 2 }));
    assert_eq!(self::manager(&harness).get_output_pad(), 1);
}

#[test]
fn mermaid_defaults_to_streaming_and_persists_rendering_modes() {
    let harness = harness();
    let manager = manager(&harness);
    assert_eq!(
        manager.get_mermaid_rendering_mode(),
        MermaidRenderingMode::Streaming
    );
    manager.set_mermaid_rendering_mode(MermaidRenderingMode::Final);
    manager.flush();
    assert_eq!(
        manager.get_mermaid_rendering_mode(),
        MermaidRenderingMode::Final
    );
    assert_eq!(
        global_settings(&harness)["markdown"]["mermaid"],
        json!("final")
    );

    write_global(&harness, json!({ "markdown": { "mermaid": "sometimes" } }));
    assert_eq!(
        self::manager(&harness).get_mermaid_rendering_mode(),
        MermaidRenderingMode::Streaming
    );
}

#[test]
fn shell_command_prefix_loads_and_survives_unrelated_writes() {
    let harness = harness();
    write_global(
        &harness,
        json!({ "shellCommandPrefix": "shopt -s expand_aliases" }),
    );
    let manager = manager(&harness);
    assert_eq!(
        manager.get_shell_command_prefix().as_deref(),
        Some("shopt -s expand_aliases")
    );

    manager.set_theme("light");
    manager.flush();
    let saved = global_settings(&harness);
    assert_eq!(
        saved["shellCommandPrefix"],
        json!("shopt -s expand_aliases")
    );
    assert_eq!(saved["theme"], json!("light"));

    write_global(&harness, json!({ "theme": "dark" }));
    assert_eq!(self::manager(&harness).get_shell_command_prefix(), None);
}

#[test]
fn session_dir_resolves_scopes_and_expands_tilde() {
    let harness = harness();
    write_global(&harness, json!({ "theme": "dark" }));
    assert_eq!(manager(&harness).get_session_dir(), None);

    write_global(&harness, json!({ "sessionDir": "/tmp/sessions" }));
    assert_eq!(
        manager(&harness).get_session_dir().as_deref(),
        Some("/tmp/sessions")
    );

    write_global(&harness, json!({ "sessionDir": "/global/sessions" }));
    write_project(&harness, json!({ "sessionDir": "./sessions" }));
    assert_eq!(
        manager(&harness).get_session_dir().as_deref(),
        Some("./sessions")
    );

    write_project(&harness, json!({}));
    write_global(&harness, json!({ "sessionDir": "~/sessions" }));
    assert_eq!(
        manager(&harness).get_session_dir(),
        Some(home_dir().join("sessions").to_string_lossy().into_owned())
    );
}

#[test]
fn shell_path_expands_tilde() {
    let harness = harness();
    write_global(&harness, json!({ "theme": "dark" }));
    assert_eq!(manager(&harness).get_shell_path(), None);

    write_global(&harness, json!({ "shellPath": "/bin/zsh" }));
    assert_eq!(
        manager(&harness).get_shell_path().as_deref(),
        Some("/bin/zsh")
    );

    write_global(
        &harness,
        json!({ "shellPath": "~/.local/bin/agent-shell-sandbox" }),
    );
    assert_eq!(
        manager(&harness).get_shell_path(),
        Some(
            home_dir()
                .join(".local/bin/agent-shell-sandbox")
                .to_string_lossy()
                .into_owned()
        )
    );

    write_global(&harness, json!({ "shellPath": "~" }));
    assert_eq!(
        manager(&harness).get_shell_path(),
        Some(home_dir().to_string_lossy().into_owned())
    );
}

#[test]
fn theme_settings_separate_automatic_pairs_from_fixed_names() {
    let harness = harness();
    write_global(&harness, json!({ "theme": "light/dark" }));
    let manager = manager(&harness);
    assert_eq!(manager.get_theme(), None);
    assert_eq!(manager.get_theme_setting().as_deref(), Some("light/dark"));

    manager.set_theme("solarized-light/tokyo-night");
    manager.flush();
    assert_eq!(
        global_settings(&harness)["theme"],
        json!("solarized-light/tokyo-night")
    );
}

#[test]
fn default_project_trust_reads_the_global_scope_only() {
    let harness = harness();
    write_global(&harness, json!({ "defaultProjectTrust": "always" }));
    write_project(&harness, json!({ "defaultProjectTrust": "never" }));
    let manager = SettingsManager::create(
        &harness.project_dir,
        Some(&harness.agent_dir),
        SettingsManagerCreateOptions {
            project_trusted: Some(true),
        },
    );
    assert_eq!(
        manager.get_default_project_trust(),
        DefaultProjectTrust::Always
    );

    write_global(&harness, json!({ "defaultProjectTrust": "sometimes" }));
    assert_eq!(
        self::manager(&harness).get_default_project_trust(),
        DefaultProjectTrust::Ask
    );
}

#[test]
fn resolves_the_remaining_defaults() {
    let harness = harness();
    let manager = manager(&harness);
    assert_eq!(manager.get_steering_mode(), QueueMode::OneAtATime);
    assert_eq!(manager.get_follow_up_mode(), QueueMode::OneAtATime);
    assert_eq!(manager.get_transport(), "auto");
    assert_eq!(
        manager.get_compaction_settings(),
        ResolvedCompactionSettings {
            enabled: true,
            reserve_tokens: 16_384,
            keep_recent_tokens: 20_000
        }
    );
    assert_eq!(
        manager.get_branch_summary_settings(),
        ResolvedBranchSummarySettings {
            reserve_tokens: 16_384,
            skip_prompt: false
        }
    );
    assert_eq!(
        manager.get_retry_settings(),
        ResolvedRetrySettings {
            enabled: true,
            max_retries: 3,
            base_delay_ms: 2_000
        }
    );
    assert_eq!(
        manager.get_provider_retry_settings(),
        ResolvedProviderRetrySettings {
            timeout_ms: None,
            max_retries: None,
            max_retry_delay_ms: 60_000
        }
    );
    assert!(!manager.get_hide_thinking_block());
    assert!(!manager.get_show_cache_miss_notices());
    assert!(!manager.get_quiet_startup());
    assert!(manager.get_enable_install_telemetry());
    assert!(!manager.get_enable_analytics());
    assert!(manager.get_enable_skill_commands());
    assert!(manager.get_show_images());
    assert_eq!(manager.get_image_width_cells(), 60);
    assert!(!manager.get_show_terminal_progress());
    assert!(manager.get_image_auto_resize());
    assert!(!manager.get_block_images());
    assert_eq!(manager.get_double_escape_action(), DoubleEscapeAction::Tree);
    assert_eq!(manager.get_tree_filter_mode(), TreeFilterMode::Default);
    assert_eq!(manager.get_editor_padding_x(), 0);
    assert_eq!(manager.get_autocomplete_max_visible(), 5);
    assert_eq!(manager.get_code_block_indent(), "  ");
    assert_eq!(manager.get_warnings(), WarningSettings::default());
    assert_eq!(manager.get_packages(), Vec::new());
    assert_eq!(manager.get_skill_paths(), Vec::<String>::new());
    assert_eq!(manager.get_prompt_template_paths(), Vec::<String>::new());
    assert_eq!(manager.get_theme_paths(), Vec::<String>::new());
    assert_eq!(manager.get_enabled_models(), None);
    assert_eq!(manager.get_npm_command(), None);
    assert_eq!(manager.get_last_changelog_version(), None);
    assert_eq!(manager.get_thinking_budgets(), None);
}

#[test]
fn clamps_and_persists_numeric_settings() {
    let harness = harness();
    let manager = manager(&harness);
    manager.set_editor_padding_x(9.0);
    manager.set_autocomplete_max_visible(1.0);
    manager.set_image_width_cells(0.0);
    manager.set_http_idle_timeout_ms(1234.9).expect("valid");
    manager.flush();
    assert_eq!(manager.get_editor_padding_x(), 3);
    assert_eq!(manager.get_autocomplete_max_visible(), 3);
    assert_eq!(manager.get_image_width_cells(), 1);
    assert_eq!(manager.get_http_idle_timeout_ms(), Ok(1234));
    assert!(manager.set_http_idle_timeout_ms(-1.0).is_err());
    let saved = global_settings(&harness);
    assert_eq!(saved["editorPaddingX"], json!(3));
    assert_eq!(saved["autocompleteMaxVisible"], json!(3));
    assert_eq!(saved["terminal"]["imageWidthCells"], json!(1));
}

#[test]
fn enabling_analytics_generates_a_tracking_id_once() {
    let harness = harness();
    let manager = manager(&harness);
    assert_eq!(manager.get_tracking_id(), None);
    manager.set_enable_analytics(true);
    manager.flush();
    let tracking_id = manager.get_tracking_id().expect("tracking id");
    assert_eq!(tracking_id.len(), 36);
    manager.set_enable_analytics(false);
    manager.set_enable_analytics(true);
    assert_eq!(
        manager.get_tracking_id().as_deref(),
        Some(tracking_id.as_str())
    );
    assert!(manager.get_enable_analytics());
}

#[test]
fn stores_and_clears_the_subagent_model() {
    // `/subagent-model` (port addition, v0.1.6): persisted like the default
    // model; clearing drops the keys from settings.json entirely.
    let harness = harness();
    let manager = manager(&harness);
    assert_eq!(manager.get_subagent_model(), None);
    manager.set_subagent_model_and_provider("openrouter", "cheap-model");
    manager.flush();
    assert_eq!(
        manager.get_subagent_provider().as_deref(),
        Some("openrouter")
    );
    assert_eq!(manager.get_subagent_model().as_deref(), Some("cheap-model"));
    let saved = global_settings(&harness);
    assert_eq!(saved["subagentProvider"], json!("openrouter"));
    assert_eq!(saved["subagentModel"], json!("cheap-model"));

    manager.clear_subagent_model();
    manager.flush();
    assert_eq!(manager.get_subagent_provider(), None);
    assert_eq!(manager.get_subagent_model(), None);
    let saved = global_settings(&harness);
    assert!(saved.get("subagentModel").is_none(), "{saved}");
    assert!(saved.get("subagentProvider").is_none(), "{saved}");
}

#[test]
fn writes_typed_list_and_model_settings() {
    let harness = harness();
    let manager = manager(&harness);
    manager.set_default_model_and_provider("anthropic", "claude-sonnet");
    manager.set_skill_paths(&["~/skills".to_owned()]);
    manager.set_enabled_models(Some(&["anthropic/*".to_owned()]));
    manager.set_npm_command(Some(&["npm".to_owned(), "--global".to_owned()]));
    manager.flush();
    assert_eq!(manager.get_default_provider().as_deref(), Some("anthropic"));
    assert_eq!(
        manager.get_default_model().as_deref(),
        Some("claude-sonnet")
    );
    assert_eq!(manager.get_skill_paths(), vec!["~/skills".to_owned()]);
    assert_eq!(
        manager.get_enabled_models(),
        Some(vec!["anthropic/*".to_owned()])
    );
    assert_eq!(
        manager.get_npm_command(),
        Some(vec!["npm".to_owned(), "--global".to_owned()])
    );
    let saved = global_settings(&harness);
    assert_eq!(saved["defaultProvider"], json!("anthropic"));
    assert_eq!(saved["defaultModel"], json!("claude-sonnet"));
    assert_eq!(saved["enabledModels"], json!(["anthropic/*"]));

    manager.set_enabled_models(None);
    manager.flush();
    assert_eq!(manager.get_enabled_models(), None);
    assert!(global_settings(&harness).get("enabledModels").is_none());
}

/// Atomic file leases (port addition, v0.1.19): off unless the user turns them
/// on, and persisted under the `atomicLeases` wire name that `/leases` writes.
#[test]
fn atomic_leases_are_off_until_they_are_turned_on() {
    let harness = harness();
    let manager = manager(&harness);

    assert!(!manager.get_atomic_leases_enabled());

    manager.set_atomic_leases_enabled(true);
    manager.flush();
    assert!(manager.get_atomic_leases_enabled());
    assert_eq!(global_settings(&harness)["atomicLeases"], json!(true));

    manager.set_atomic_leases_enabled(false);
    manager.flush();
    assert!(!manager.get_atomic_leases_enabled());
    assert_eq!(global_settings(&harness)["atomicLeases"], json!(false));
}
