use std::fs;

use notagent::core::keybindings::{KeybindingsManager, migrate_keybindings_config_file};
use serde_json::{Value, json};

fn create_agent_dir(config: Value) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("temp dir");
    fs::write(
        dir.path().join("keybindings.json"),
        format!("{}\n", serde_json::to_string_pretty(&config).unwrap()),
    )
    .expect("write keybindings.json");
    dir
}

fn read_config(dir: &tempfile::TempDir) -> Value {
    let text = fs::read_to_string(dir.path().join("keybindings.json")).expect("read");
    serde_json::from_str(&text).expect("parse")
}

#[test]
fn rewrites_old_key_names_to_namespaced_ids() {
    let dir = create_agent_dir(json!({
        "cursorUp": ["up", "ctrl+p"],
        "expandTools": "ctrl+x",
    }));

    // directory-scoped step directly (`run_migrations` resolves the agent dir
    // from the environment).
    migrate_keybindings_config_file(dir.path());

    assert_eq!(
        read_config(&dir),
        json!({
            "tui.editor.cursorUp": ["up", "ctrl+p"],
            "app.tools.expand": "ctrl+x",
        })
    );
}

#[test]
fn keeps_the_namespaced_value_when_old_and_new_names_both_exist() {
    let dir = create_agent_dir(json!({
        "expandTools": "ctrl+x",
        "app.tools.expand": "ctrl+y",
    }));

    migrate_keybindings_config_file(dir.path());

    assert_eq!(read_config(&dir), json!({ "app.tools.expand": "ctrl+y" }));
}

#[test]
fn loads_old_key_names_in_memory_before_the_file_is_rewritten() {
    let dir = create_agent_dir(json!({
        "selectConfirm": "enter",
        "interrupt": "ctrl+x",
    }));

    let keybindings = KeybindingsManager::create(Some(dir.path()));

    let user_bindings = keybindings.get_user_bindings();
    assert_eq!(user_bindings.len(), 2);
    assert_eq!(user_bindings["tui.select.confirm"], vec!["enter"]);
    assert_eq!(user_bindings["app.interrupt"], vec!["ctrl+x"]);

    let effective = keybindings.get_effective_config();
    assert_eq!(effective["tui.select.confirm"], vec!["enter"]);
    assert_eq!(effective["app.interrupt"], vec!["ctrl+x"]);
}

#[test]
fn leaves_a_config_without_legacy_names_untouched() {
    // `migrated === false` short-circuits the rewrite, so the file keeps its
    // original formatting.
    let dir = create_agent_dir(json!({ "app.tools.expand": "ctrl+y" }));
    let before = fs::read_to_string(dir.path().join("keybindings.json")).unwrap();

    migrate_keybindings_config_file(dir.path());

    assert_eq!(
        fs::read_to_string(dir.path().join("keybindings.json")).unwrap(),
        before
    );
}

#[test]
fn orders_the_rewritten_config_by_the_registry_and_sorts_unknown_keys() {
    let dir = create_agent_dir(json!({
        "zzz.unknown": "ctrl+z",
        "expandTools": "ctrl+x",
        "aaa.unknown": "ctrl+a",
        "cursorUp": "up",
    }));

    migrate_keybindings_config_file(dir.path());

    let text = fs::read_to_string(dir.path().join("keybindings.json")).unwrap();
    let keys: Vec<&str> = text
        .lines()
        .filter_map(|line| line.trim().strip_prefix('"'))
        .filter_map(|line| line.split('"').next())
        .collect();
    assert_eq!(
        keys,
        [
            "tui.editor.cursorUp",
            "app.tools.expand",
            "aaa.unknown",
            "zzz.unknown"
        ]
    );
}

#[test]
fn resolves_the_platform_defaults_of_the_app_registry() {
    let keybindings = KeybindingsManager::default();

    assert_eq!(keybindings.get_keys("app.message.copy"), vec!["ctrl+x"]);
    assert!(keybindings.matches("\x18", "app.message.copy"));
    assert!(keybindings.matches("\x15", "app.tree.filter.userOnly"));

    let fold = keybindings.get_keys("app.tree.foldOrUp");
    if cfg!(target_os = "macos") {
        assert_eq!(fold, vec!["alt+left", "ctrl+left"]);
    } else {
        assert_eq!(fold, vec!["ctrl+left", "alt+left"]);
    }

    let suspend = keybindings.get_keys("app.suspend");
    if cfg!(target_os = "windows") {
        assert!(suspend.is_empty());
    } else {
        assert_eq!(suspend, vec!["ctrl+z"]);
    }
}
