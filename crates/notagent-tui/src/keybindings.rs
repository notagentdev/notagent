use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

use crate::keys::matches_key;

/// One entry of the registry: id, default keys, description.
pub type KeybindingDefinition = (&'static str, &'static [&'static str], &'static str);

/// User overrides: keybinding id to key list.
pub type KeybindingsConfig = HashMap<String, Vec<String>>;

/// Definitions shipped by the TUI library.
pub static TUI_KEYBINDINGS: &[KeybindingDefinition] = &[
    ("tui.editor.cursorUp", &["up"], "Move cursor up"),
    ("tui.editor.cursorDown", &["down"], "Move cursor down"),
    (
        "tui.editor.historyPrevious",
        &[],
        "Select previous prompt history entry",
    ),
    (
        "tui.editor.historyNext",
        &[],
        "Select next prompt history entry",
    ),
    (
        "tui.editor.cursorLeft",
        &["left", "ctrl+b"],
        "Move cursor left",
    ),
    (
        "tui.editor.cursorRight",
        &["right", "ctrl+f"],
        "Move cursor right",
    ),
    (
        "tui.editor.cursorWordLeft",
        &["alt+left", "ctrl+left", "alt+b"],
        "Move cursor word left",
    ),
    (
        "tui.editor.cursorWordRight",
        &["alt+right", "ctrl+right", "alt+f"],
        "Move cursor word right",
    ),
    (
        "tui.editor.cursorLineStart",
        &["home", "ctrl+home", "ctrl+a"],
        "Move to line start",
    ),
    (
        "tui.editor.cursorLineEnd",
        &["end", "ctrl+end", "ctrl+e"],
        "Move to line end",
    ),
    (
        "tui.editor.jumpForward",
        &["ctrl+]"],
        "Jump forward to character",
    ),
    (
        "tui.editor.jumpBackward",
        &["ctrl+alt+]"],
        "Jump backward to character",
    ),
    ("tui.editor.pageUp", &["pageUp", "ctrl+pageUp"], "Page up"),
    (
        "tui.editor.pageDown",
        &["pageDown", "ctrl+pageDown"],
        "Page down",
    ),
    (
        "tui.editor.deleteCharBackward",
        &["backspace"],
        "Delete character backward",
    ),
    (
        "tui.editor.deleteCharForward",
        &["delete", "ctrl+d"],
        "Delete character forward",
    ),
    (
        "tui.editor.deleteWordBackward",
        &["ctrl+w", "alt+backspace"],
        "Delete word backward",
    ),
    (
        "tui.editor.deleteWordForward",
        &["alt+d", "alt+delete"],
        "Delete word forward",
    ),
    (
        "tui.editor.deleteToLineStart",
        &["ctrl+u"],
        "Delete to line start",
    ),
    (
        "tui.editor.deleteToLineEnd",
        &["ctrl+k"],
        "Delete to line end",
    ),
    ("tui.editor.yank", &["ctrl+y"], "Yank"),
    ("tui.editor.yankPop", &["alt+y"], "Yank pop"),
    ("tui.editor.undo", &["ctrl+-"], "Undo"),
    (
        "tui.input.newLine",
        &["shift+enter", "ctrl+j"],
        "Insert newline",
    ),
    ("tui.input.submit", &["enter"], "Submit input"),
    ("tui.input.tab", &["tab"], "Tab / autocomplete"),
    ("tui.input.copy", &["ctrl+c"], "Copy selection"),
    ("tui.select.up", &["up"], "Move selection up"),
    ("tui.select.down", &["down"], "Move selection down"),
    ("tui.select.pageUp", &["pageUp"], "Selection page up"),
    ("tui.select.pageDown", &["pageDown"], "Selection page down"),
    ("tui.select.confirm", &["enter"], "Confirm selection"),
    (
        "tui.select.cancel",
        &["escape", "ctrl+c"],
        "Cancel selection",
    ),
    (
        "tui.altScreen.pageUp",
        &["pageUp"],
        "Scroll viewport up one page",
    ),
    (
        "tui.altScreen.pageDown",
        &["pageDown"],
        "Scroll viewport down one page",
    ),
    (
        "tui.altScreen.halfPageUp",
        &[],
        "Scroll viewport up half a page",
    ),
    (
        "tui.altScreen.halfPageDown",
        &[],
        "Scroll viewport down half a page",
    ),
    ("tui.altScreen.lineUp", &[], "Scroll viewport up one line"),
    (
        "tui.altScreen.lineDown",
        &[],
        "Scroll viewport down one line",
    ),
    (
        "tui.altScreen.previousPrompt",
        &["ctrl+shift+up"],
        "Jump to previous semantic prompt",
    ),
    (
        "tui.altScreen.nextPrompt",
        &["ctrl+shift+down"],
        "Jump to next semantic prompt",
    ),
    (
        "tui.altScreen.search",
        &["ctrl+shift+f"],
        "Search the primary scroll view",
    ),
    (
        "tui.altScreen.searchNext",
        &["enter", "ctrl+g"],
        "Select the next search match",
    ),
    (
        "tui.altScreen.searchPrevious",
        &["shift+enter", "ctrl+shift+g"],
        "Select the previous search match",
    ),
    (
        "tui.altScreen.searchClose",
        &["escape"],
        "Close transcript search",
    ),
    ("tui.altScreen.top", &["home"], "Scroll viewport to top"),
    (
        "tui.altScreen.bottom",
        &["end"],
        "Scroll viewport to bottom",
    ),
];

/// Two keybindings claiming the same key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeybindingConflict {
    /// The contested key.
    pub key: String,
    /// Ids claiming it.
    pub keybindings: Vec<String>,
}

fn normalize_keys(keys: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for key in keys {
        if seen.insert(key.clone()) {
            result.push(key.clone());
        }
    }
    result
}

/// Resolves keybinding ids to keys, honouring user overrides.
pub struct KeybindingsManager {
    definitions: Vec<KeybindingDefinition>,
    user_bindings: KeybindingsConfig,
    keys_by_id: HashMap<String, Vec<String>>,
    conflicts: Vec<KeybindingConflict>,
}

impl KeybindingsManager {
    /// New manager over `definitions` with optional user overrides.
    pub fn new(definitions: &[KeybindingDefinition], user_bindings: KeybindingsConfig) -> Self {
        let mut manager = Self {
            definitions: definitions.to_vec(),
            user_bindings,
            keys_by_id: HashMap::new(),
            conflicts: Vec::new(),
        };
        manager.rebuild();
        manager
    }

    fn rebuild(&mut self) {
        self.keys_by_id.clear();
        self.conflicts.clear();

        let mut user_claims: HashMap<String, Vec<String>> = HashMap::new();
        for (keybinding, keys) in &self.user_bindings {
            if !self
                .definitions
                .iter()
                .any(|(id, _, _)| *id == keybinding.as_str())
            {
                continue;
            }
            for key in normalize_keys(keys) {
                let claimants = user_claims.entry(key).or_default();
                if !claimants.contains(keybinding) {
                    claimants.push(keybinding.clone());
                }
            }
        }
        for (key, keybindings) in user_claims {
            if keybindings.len() > 1 {
                self.conflicts.push(KeybindingConflict { key, keybindings });
            }
        }

        for (id, default_keys, _) in &self.definitions {
            let keys = match self.user_bindings.get(*id) {
                Some(user_keys) => normalize_keys(user_keys),
                None => normalize_keys(
                    &default_keys
                        .iter()
                        .map(|key| (*key).to_string())
                        .collect::<Vec<_>>(),
                ),
            };
            self.keys_by_id.insert((*id).to_string(), keys);
        }
    }

    /// Whether `data` matches the given keybinding.
    pub fn matches(&self, data: &str, keybinding: &str) -> bool {
        self.keys_by_id
            .get(keybinding)
            .is_some_and(|keys| keys.iter().any(|key| matches_key(data, key)))
    }

    /// Keys bound to a keybinding.
    pub fn get_keys(&self, keybinding: &str) -> Vec<String> {
        self.keys_by_id.get(keybinding).cloned().unwrap_or_default()
    }

    /// Definition of a keybinding.
    pub fn get_definition(&self, keybinding: &str) -> Option<KeybindingDefinition> {
        self.definitions
            .iter()
            .find(|(id, _, _)| *id == keybinding)
            .copied()
    }

    /// Keys claimed by more than one keybinding.
    pub fn get_conflicts(&self) -> Vec<KeybindingConflict> {
        self.conflicts.clone()
    }

    /// Replace the user overrides.
    pub fn set_user_bindings(&mut self, user_bindings: KeybindingsConfig) {
        self.user_bindings = user_bindings;
        self.rebuild();
    }

    /// Current user overrides.
    pub fn get_user_bindings(&self) -> KeybindingsConfig {
        self.user_bindings.clone()
    }

    /// Resolved keys of every keybinding.
    pub fn get_resolved_bindings(&self) -> KeybindingsConfig {
        self.definitions
            .iter()
            .map(|(id, _, _)| ((*id).to_string(), self.get_keys(id)))
            .collect()
    }
}

fn global_keybindings() -> &'static Mutex<KeybindingsManager> {
    static CELL: OnceLock<Mutex<KeybindingsManager>> = OnceLock::new();
    CELL.get_or_init(|| {
        Mutex::new(KeybindingsManager::new(
            TUI_KEYBINDINGS,
            KeybindingsConfig::new(),
        ))
    })
}

/// Install the global keybindings manager.
pub fn set_keybindings(keybindings: KeybindingsManager) {
    *global_keybindings()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = keybindings;
}

/// Whether `data` matches `keybinding` in the global manager.
pub fn keybindings_match(data: &str, keybinding: &str) -> bool {
    global_keybindings()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .matches(data, keybinding)
}

/// Keys bound to `keybinding` in the global manager.
pub fn keybinding_keys(keybinding: &str) -> Vec<String> {
    global_keybindings()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get_keys(keybinding)
}
