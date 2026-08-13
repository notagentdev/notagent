//! 1:1 port of `packages/coding-agent/src/core/keybindings.ts` (386 LOC).
//!
//! Ownership moved from workstream C to A with `plans/interface-requests.md`
//! O-2 (request A-7): the app keybindings are the registry the interactive
//! components resolve their hints and actions against.

use std::path::{Path, PathBuf};

use notagent_tui::keybindings::{
    KeybindingConflict, KeybindingDefinition, KeybindingsConfig,
    KeybindingsManager as TuiKeybindingsManager, TUI_KEYBINDINGS,
};
use serde_json::{Map, Value};

use crate::config::get_agent_dir;

/// `process.platform === "win32" ? [] : "ctrl+z"`
const SUSPEND_KEYS: &[&str] = if cfg!(target_os = "windows") {
    &[]
} else {
    &["ctrl+z"]
};

/// `process.platform === "win32" ? "alt+v" : "ctrl+v"`
const PASTE_IMAGE_KEYS: &[&str] = if cfg!(target_os = "windows") {
    &["alt+v"]
} else {
    &["ctrl+v"]
};

/// `process.platform === "darwin" ? ["alt+left", "ctrl+left"] : ["ctrl+left", "alt+left"]`
const TREE_FOLD_KEYS: &[&str] = if cfg!(target_os = "macos") {
    &["alt+left", "ctrl+left"]
} else {
    &["ctrl+left", "alt+left"]
};

/// `process.platform === "darwin" ? ["alt+right", "ctrl+right"] : ["ctrl+right", "alt+right"]`
const TREE_UNFOLD_KEYS: &[&str] = if cfg!(target_os = "macos") {
    &["alt+right", "ctrl+right"]
} else {
    &["ctrl+right", "alt+right"]
};

/// The app-level half of [`keybindings`]; `AppKeybindings` in TypeScript.
pub static APP_KEYBINDINGS: &[KeybindingDefinition] = &[
    ("app.interrupt", &["escape"], "Cancel or abort"),
    ("app.clear", &["ctrl+c"], "Clear editor"),
    ("app.exit", &["ctrl+d"], "Exit when editor is empty"),
    ("app.suspend", SUSPEND_KEYS, "Suspend to background"),
    // shift+tab is the operating-mode ring, matching the reference and the
    // convention users bring from other agents.
    ("app.mode.cycle", &["shift+tab"], "Cycle operating mode"),
    // The two thinking keys share a letter on purpose: ctrl+t changes how much
    // the model thinks, shift+ctrl+t changes whether you see it. Same pairing
    // as ctrl+p / shift+ctrl+p for models.
    ("app.thinking.cycle", &["ctrl+t"], "Cycle thinking level"),
    ("app.model.cycleForward", &["ctrl+p"], "Cycle to next model"),
    (
        "app.model.cycleBackward",
        &["shift+ctrl+p"],
        "Cycle to previous model",
    ),
    ("app.model.select", &["ctrl+l"], "Open model selector"),
    ("app.tools.expand", &["ctrl+o"], "Toggle tool output"),
    // The panel is a ring rather than an on/off switch, so one key covers
    // "show me the finished ones too" and "get it off my screen".
    ("app.tasks.cycle", &["alt+b"], "Cycle background task panel"),
    (
        "app.tasks.detach",
        &["ctrl+b"],
        "Move running work to the background",
    ),
    (
        "app.thinking.toggle",
        &["shift+ctrl+t"],
        "Toggle thinking blocks",
    ),
    (
        "app.session.toggleNamedFilter",
        &["ctrl+n"],
        "Toggle named session filter",
    ),
    ("app.editor.external", &["ctrl+g"], "Open external editor"),
    ("app.message.copy", &["ctrl+x"], "Copy message to clipboard"),
    (
        "app.message.followUp",
        &["alt+enter"],
        "Queue follow-up message",
    ),
    (
        "app.message.dequeue",
        &["alt+up"],
        "Restore queued messages",
    ),
    (
        "app.clipboard.pasteImage",
        PASTE_IMAGE_KEYS,
        "Paste image from clipboard (text fallback)",
    ),
    ("app.session.new", &[], "Start a new session"),
    ("app.session.tree", &[], "Open session tree"),
    ("app.session.fork", &[], "Fork current session"),
    ("app.session.resume", &[], "Resume a session"),
    (
        "app.tree.foldOrUp",
        TREE_FOLD_KEYS,
        "Fold tree branch or move up",
    ),
    (
        "app.tree.unfoldOrDown",
        TREE_UNFOLD_KEYS,
        "Unfold tree branch or move down",
    ),
    ("app.tree.editLabel", &["shift+l"], "Edit tree label"),
    (
        "app.tree.toggleLabelTimestamp",
        &["shift+t"],
        "Toggle tree label timestamps",
    ),
    (
        "app.session.togglePath",
        &["ctrl+p"],
        "Toggle session path display",
    ),
    (
        "app.session.toggleSort",
        &["ctrl+s"],
        "Toggle session sort mode",
    ),
    ("app.session.rename", &["ctrl+r"], "Rename session"),
    ("app.session.delete", &["ctrl+d"], "Delete session"),
    (
        "app.session.deleteNoninvasive",
        &["ctrl+backspace"],
        "Delete session when query is empty",
    ),
    ("app.models.save", &["ctrl+s"], "Save model selection"),
    ("app.models.enableAll", &["ctrl+a"], "Enable all models"),
    ("app.models.clearAll", &["ctrl+x"], "Clear all models"),
    (
        "app.models.toggleProvider",
        &["ctrl+p"],
        "Toggle all models for provider",
    ),
    (
        "app.models.reorderUp",
        &["alt+up"],
        "Move model up in order",
    ),
    (
        "app.models.reorderDown",
        &["alt+down"],
        "Move model down in order",
    ),
    (
        "app.tree.filter.default",
        &["ctrl+d"],
        "Tree filter: default view",
    ),
    (
        "app.tree.filter.noTools",
        &["ctrl+t"],
        "Tree filter: hide tool results",
    ),
    (
        "app.tree.filter.userOnly",
        &["ctrl+u"],
        "Tree filter: user messages only",
    ),
    (
        "app.tree.filter.labeledOnly",
        &["ctrl+l"],
        "Tree filter: labeled entries only",
    ),
    (
        "app.tree.filter.all",
        &["ctrl+a"],
        "Tree filter: show all entries",
    ),
    (
        "app.tree.filter.cycleForward",
        &["ctrl+o"],
        "Tree filter: cycle forward",
    ),
    (
        "app.tree.filter.cycleBackward",
        &["shift+ctrl+o"],
        "Tree filter: cycle backward",
    ),
];

/// `KEYBINDINGS` — the TUI registry plus the app entries, in that order.
pub fn keybindings() -> &'static [KeybindingDefinition] {
    static ALL: std::sync::OnceLock<Vec<KeybindingDefinition>> = std::sync::OnceLock::new();
    ALL.get_or_init(|| {
        let mut all = TUI_KEYBINDINGS.to_vec();
        all.extend_from_slice(APP_KEYBINDINGS);
        all
    })
}

/// `KEYBINDING_NAME_MIGRATIONS` — legacy names of the flat keybindings.json.
const KEYBINDING_NAME_MIGRATIONS: &[(&str, &str)] = &[
    ("cursorUp", "tui.editor.cursorUp"),
    ("cursorDown", "tui.editor.cursorDown"),
    ("cursorLeft", "tui.editor.cursorLeft"),
    ("cursorRight", "tui.editor.cursorRight"),
    ("cursorWordLeft", "tui.editor.cursorWordLeft"),
    ("cursorWordRight", "tui.editor.cursorWordRight"),
    ("cursorLineStart", "tui.editor.cursorLineStart"),
    ("cursorLineEnd", "tui.editor.cursorLineEnd"),
    ("jumpForward", "tui.editor.jumpForward"),
    ("jumpBackward", "tui.editor.jumpBackward"),
    ("pageUp", "tui.editor.pageUp"),
    ("pageDown", "tui.editor.pageDown"),
    ("deleteCharBackward", "tui.editor.deleteCharBackward"),
    ("deleteCharForward", "tui.editor.deleteCharForward"),
    ("deleteWordBackward", "tui.editor.deleteWordBackward"),
    ("deleteWordForward", "tui.editor.deleteWordForward"),
    ("deleteToLineStart", "tui.editor.deleteToLineStart"),
    ("deleteToLineEnd", "tui.editor.deleteToLineEnd"),
    ("yank", "tui.editor.yank"),
    ("yankPop", "tui.editor.yankPop"),
    ("undo", "tui.editor.undo"),
    ("newLine", "tui.input.newLine"),
    ("submit", "tui.input.submit"),
    ("tab", "tui.input.tab"),
    ("copy", "tui.input.copy"),
    ("selectUp", "tui.select.up"),
    ("selectDown", "tui.select.down"),
    ("selectPageUp", "tui.select.pageUp"),
    ("selectPageDown", "tui.select.pageDown"),
    ("selectConfirm", "tui.select.confirm"),
    ("selectCancel", "tui.select.cancel"),
    ("interrupt", "app.interrupt"),
    ("clear", "app.clear"),
    ("exit", "app.exit"),
    ("suspend", "app.suspend"),
    ("cycleThinkingLevel", "app.thinking.cycle"),
    ("cycleModelForward", "app.model.cycleForward"),
    ("cycleModelBackward", "app.model.cycleBackward"),
    ("selectModel", "app.model.select"),
    ("expandTools", "app.tools.expand"),
    ("toggleThinking", "app.thinking.toggle"),
    ("toggleSessionNamedFilter", "app.session.toggleNamedFilter"),
    ("externalEditor", "app.editor.external"),
    ("followUp", "app.message.followUp"),
    ("dequeue", "app.message.dequeue"),
    ("pasteImage", "app.clipboard.pasteImage"),
    ("newSession", "app.session.new"),
    ("tree", "app.session.tree"),
    ("fork", "app.session.fork"),
    ("resume", "app.session.resume"),
    ("treeFoldOrUp", "app.tree.foldOrUp"),
    ("treeUnfoldOrDown", "app.tree.unfoldOrDown"),
    ("treeEditLabel", "app.tree.editLabel"),
    ("treeToggleLabelTimestamp", "app.tree.toggleLabelTimestamp"),
    ("toggleSessionPath", "app.session.togglePath"),
    ("toggleSessionSort", "app.session.toggleSort"),
    ("renameSession", "app.session.rename"),
    ("deleteSession", "app.session.delete"),
    ("deleteSessionNoninvasive", "app.session.deleteNoninvasive"),
];

fn legacy_keybinding_name(key: &str) -> Option<&'static str> {
    KEYBINDING_NAME_MIGRATIONS
        .iter()
        .find(|(legacy, _)| *legacy == key)
        .map(|(_, current)| *current)
}

/// Keep only string and string-array entries; `toKeybindingsConfig`.
fn to_keybindings_config(value: &Map<String, Value>) -> KeybindingsConfig {
    let mut config = KeybindingsConfig::new();
    for (key, binding) in value {
        match binding {
            // A single `KeyId` becomes a one-element list: the Rust registry
            // stores key lists only (deviation of the tui port, class 1).
            Value::String(key_id) => {
                config.insert(key.clone(), vec![key_id.clone()]);
            }
            Value::Array(entries) if entries.iter().all(Value::is_string) => {
                config.insert(
                    key.clone(),
                    entries
                        .iter()
                        .map(|entry| entry.as_str().unwrap_or_default().to_string())
                        .collect(),
                );
            }
            _ => {}
        }
    }
    config
}

/// The migrated config plus whether anything changed.
pub struct MigratedKeybindings {
    /// The rewritten config, in registry order.
    pub config: Map<String, Value>,
    /// Whether a legacy name was rewritten or dropped.
    pub migrated: bool,
}

/// Rewrite legacy keybinding names to their namespaced ids.
pub fn migrate_keybindings_config(raw_config: &Map<String, Value>) -> MigratedKeybindings {
    let mut config: Map<String, Value> = Map::new();
    let mut migrated = false;

    for (key, value) in raw_config {
        let next_key = legacy_keybinding_name(key).unwrap_or(key.as_str());
        if next_key != key {
            migrated = true;
        }
        if key != next_key && raw_config.contains_key(next_key) {
            migrated = true;
            continue;
        }
        config.insert(next_key.to_string(), value.clone());
    }

    MigratedKeybindings {
        config: order_keybindings_config(&config),
        migrated,
    }
}

fn order_keybindings_config(config: &Map<String, Value>) -> Map<String, Value> {
    let mut ordered: Map<String, Value> = Map::new();
    for (keybinding, _, _) in keybindings() {
        if let Some(value) = config.get(*keybinding) {
            ordered.insert((*keybinding).to_string(), value.clone());
        }
    }

    let mut extras: Vec<&String> = config
        .keys()
        .filter(|key| !ordered.contains_key(key.as_str()))
        .collect();
    extras.sort();
    for key in extras {
        ordered.insert(key.clone(), config[key].clone());
    }

    ordered
}

fn load_raw_config(path: &Path) -> Option<Map<String, Value>> {
    if !path.exists() {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(map)) => Some(map),
        _ => None,
    }
}

/// The app's keybindings registry; `KeybindingsManager extends TuiKeybindingsManager`.
///
/// Rust has no inheritance, so the port composes the TUI manager and forwards
/// its API (class 1).
pub struct KeybindingsManager {
    inner: TuiKeybindingsManager,
    config_path: Option<PathBuf>,
}

impl Default for KeybindingsManager {
    fn default() -> Self {
        Self::new(KeybindingsConfig::new(), None)
    }
}

impl KeybindingsManager {
    /// New manager over the app registry.
    pub fn new(user_bindings: KeybindingsConfig, config_path: Option<PathBuf>) -> Self {
        Self {
            inner: TuiKeybindingsManager::new(keybindings(), user_bindings),
            config_path,
        }
    }

    /// Manager backed by `<agentDir>/keybindings.json`.
    pub fn create(agent_dir: Option<&Path>) -> Self {
        let config_path = agent_dir
            .map(Path::to_path_buf)
            .unwrap_or_else(get_agent_dir)
            .join("keybindings.json");
        let user_bindings = Self::load_from_file(&config_path);
        Self::new(user_bindings, Some(config_path))
    }

    /// Re-read the config file.
    pub fn reload(&mut self) {
        let Some(config_path) = self.config_path.clone() else {
            return;
        };
        self.set_user_bindings(Self::load_from_file(&config_path));
    }

    /// Resolved keys of every keybinding.
    pub fn get_effective_config(&self) -> KeybindingsConfig {
        self.inner.get_resolved_bindings()
    }

    fn load_from_file(path: &Path) -> KeybindingsConfig {
        let Some(raw_config) = load_raw_config(path) else {
            return KeybindingsConfig::new();
        };
        to_keybindings_config(&migrate_keybindings_config(&raw_config).config)
    }

    /// The registry to install with `notagent_tui::keybindings::set_keybindings`.
    ///
    /// TypeScript installs the manager itself; the global registry of the port
    /// owns its manager, so the app hands it an equivalent one (class 1).
    pub fn to_tui(&self) -> TuiKeybindingsManager {
        TuiKeybindingsManager::new(keybindings(), self.inner.get_user_bindings())
    }

    /// Whether `data` matches the given keybinding.
    pub fn matches(&self, data: &str, keybinding: &str) -> bool {
        self.inner.matches(data, keybinding)
    }

    /// Keys bound to a keybinding.
    pub fn get_keys(&self, keybinding: &str) -> Vec<String> {
        self.inner.get_keys(keybinding)
    }

    /// Definition of a keybinding.
    pub fn get_definition(&self, keybinding: &str) -> Option<KeybindingDefinition> {
        self.inner.get_definition(keybinding)
    }

    /// Keys claimed by more than one keybinding.
    pub fn get_conflicts(&self) -> Vec<KeybindingConflict> {
        self.inner.get_conflicts()
    }

    /// Replace the user overrides.
    pub fn set_user_bindings(&mut self, user_bindings: KeybindingsConfig) {
        self.inner.set_user_bindings(user_bindings);
    }

    /// Current user overrides.
    pub fn get_user_bindings(&self) -> KeybindingsConfig {
        self.inner.get_user_bindings()
    }

    /// Resolved keys of every keybinding.
    pub fn get_resolved_bindings(&self) -> KeybindingsConfig {
        self.inner.get_resolved_bindings()
    }
}

/// Rewrite `<agentDir>/keybindings.json` in place; `migrateKeybindingsConfigFile`
/// of `migrations.ts`, which lives here because it needs the registry order.
pub fn migrate_keybindings_config_file(agent_dir: &Path) {
    let config_path = agent_dir.join("keybindings.json");
    if !config_path.exists() {
        return;
    }

    let Some(parsed) = load_raw_config(&config_path) else {
        // Ignore malformed files during migration
        return;
    };
    let result = migrate_keybindings_config(&parsed);
    if !result.migrated {
        return;
    }
    if let Ok(text) = serde_json::to_string_pretty(&Value::Object(result.config)) {
        let _ = std::fs::write(&config_path, format!("{text}\n"));
    }
}
