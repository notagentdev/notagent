//! 1:1 port of
//! `packages/coding-agent/src/modes/interactive/components/custom-editor.ts` (96 LOC).

use std::cell::RefCell;
use std::rc::Rc;

use notagent_tui::autocomplete::AutocompleteProvider;
use notagent_tui::components::editor::{Editor, EditorOptions, EditorTheme};
use notagent_tui::editor_component::EditorComponent;
use notagent_tui::tui::{Component, Focusable, TuiCore};

use crate::core::keybindings::KeybindingsManager;

/// Handler for an app action.
///
/// TypeScript types the handler as `() => unknown` and treats only the literal
/// `false` as "declined"; the port returns `true` when the action claimed the
/// key and `false` to fall through to ordinary editor handling.
pub type AppActionHandler = Box<dyn FnMut() -> bool>;

/// Custom editor that handles app-level keybindings for coding-agent.
pub struct CustomEditor {
    editor: Editor,
    keybindings: Rc<RefCell<KeybindingsManager>>,
    /// Handlers for app actions. A handler returning `false` declines the key,
    /// which then falls through to ordinary editor handling — that is how an
    /// action can share a key with an editing binding and only claim it when it
    /// has something to do.
    ///
    /// A `Vec` rather than a map: the dispatch loop depends on the insertion
    /// order of the JavaScript `Map`.
    action_handlers: Vec<(String, AppActionHandler)>,

    // Special handlers that can be dynamically replaced
    /// Replaces the registered `app.interrupt` handler while set.
    pub on_escape: Option<Box<dyn FnMut()>>,
    /// Replaces the registered `app.exit` handler while set.
    pub on_ctrl_d: Option<Box<dyn FnMut()>>,
    /// Invoked for `app.clipboard.pasteImage`.
    pub on_paste_image: Option<Box<dyn FnMut()>>,
}

impl CustomEditor {
    /// New editor over `core`.
    pub fn new(
        core: TuiCore,
        theme: EditorTheme,
        keybindings: Rc<RefCell<KeybindingsManager>>,
        options: Option<EditorOptions>,
    ) -> Self {
        Self {
            editor: Editor::new(core, theme, options.unwrap_or_default()),
            keybindings,
            action_handlers: Vec::new(),
            on_escape: None,
            on_ctrl_d: None,
            on_paste_image: None,
        }
    }

    /// Register a handler for an app action.
    pub fn on_action(&mut self, action: &str, handler: AppActionHandler) {
        // `Map.set` keeps the position of an existing key.
        if let Some(entry) = self
            .action_handlers
            .iter_mut()
            .find(|(name, _)| name == action)
        {
            entry.1 = handler;
            return;
        }
        self.action_handlers.push((action.to_string(), handler));
    }

    /// Whether an action has a handler.
    pub fn has_action(&self, action: &str) -> bool {
        self.action_handlers.iter().any(|(name, _)| name == action)
    }

    /// The registered actions, in dispatch order.
    pub fn action_names(&self) -> Vec<String> {
        self.action_handlers
            .iter()
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// The wrapped editor.
    pub fn editor(&self) -> &Editor {
        &self.editor
    }

    /// The wrapped editor.
    pub fn editor_mut(&mut self) -> &mut Editor {
        &mut self.editor
    }

    fn matches(&self, data: &str, keybinding: &str) -> bool {
        self.keybindings.borrow().matches(data, keybinding)
    }

    fn run_action(&mut self, action: &str) -> bool {
        let Some(index) = self
            .action_handlers
            .iter()
            .position(|(name, _)| name == action)
        else {
            return false;
        };
        (self.action_handlers[index].1)();
        true
    }
}

impl Component for CustomEditor {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.editor.render(width)
    }

    fn invalidate(&mut self) {
        self.editor.invalidate();
    }

    fn as_focusable(&mut self) -> Option<&mut dyn Focusable> {
        Some(self)
    }

    fn handle_input(&mut self, data: &str) {
        // Extension-registered shortcuts (`onExtensionShortcut`) fall away with
        // the extension system.

        // Check for clipboard paste keybinding
        if self.matches(data, "app.clipboard.pasteImage") {
            if let Some(on_paste_image) = self.on_paste_image.as_mut() {
                on_paste_image();
            }
            return;
        }

        // Check app keybindings first

        // Escape/interrupt - only if autocomplete is NOT active
        if self.matches(data, "app.interrupt") {
            if !self.editor.is_showing_autocomplete() {
                // Use dynamic onEscape if set, otherwise registered handler
                if let Some(on_escape) = self.on_escape.as_mut() {
                    on_escape();
                    return;
                }
                if self.run_action("app.interrupt") {
                    return;
                }
            }
            // Let parent handle escape for autocomplete cancellation
            self.editor.handle_input(data);
            return;
        }

        // Exit (Ctrl+D) - only when editor is empty
        if self.matches(data, "app.exit") && self.editor.get_text().is_empty() {
            if let Some(on_ctrl_d) = self.on_ctrl_d.as_mut() {
                on_ctrl_d();
            } else {
                self.run_action("app.exit");
            }
            return;
            // Fall through to editor handling for delete-char-forward when not empty
        }

        // Explicit history bindings take precedence over app actions while the editor is focused.
        // This lets users bind Ctrl+P even though it cycles models by default.
        if self.matches(data, "tui.editor.historyPrevious")
            || self.matches(data, "tui.editor.historyNext")
        {
            self.editor.handle_input(data);
            return;
        }

        // Check all other app actions
        for index in 0..self.action_handlers.len() {
            let action = self.action_handlers[index].0.clone();
            if action != "app.interrupt" && action != "app.exit" && self.matches(data, &action) {
                if !(self.action_handlers[index].1)() {
                    break;
                }
                return;
            }
        }

        // Pass to parent for editor handling
        self.editor.handle_input(data);
    }
}

impl Focusable for CustomEditor {
    fn focused(&self) -> bool {
        self.editor.focused()
    }

    fn set_focused(&mut self, focused: bool) {
        self.editor.set_focused(focused);
    }
}

impl EditorComponent for CustomEditor {
    fn get_text(&self) -> String {
        self.editor.get_text()
    }

    fn set_text(&mut self, text: &str) {
        self.editor.set_text(text);
    }

    fn take_submitted(&mut self) -> Vec<String> {
        self.editor.take_submitted()
    }

    fn take_changes(&mut self) -> Vec<String> {
        self.editor.take_changes()
    }

    fn add_to_history(&mut self, text: &str) {
        self.editor.add_to_history(text);
    }

    fn insert_text_at_cursor(&mut self, text: &str) {
        self.editor.insert_text_at_cursor(text);
    }

    fn get_expanded_text(&self) -> String {
        self.editor.get_expanded_text()
    }

    fn set_autocomplete_provider(&mut self, provider: Rc<dyn AutocompleteProvider>) {
        self.editor.set_autocomplete_provider(provider);
    }

    fn set_border_color(&mut self, border_color: Rc<dyn Fn(&str) -> String>) {
        EditorComponent::set_border_color(&mut self.editor, border_color);
    }

    fn set_padding_x(&mut self, padding: usize) {
        self.editor.set_padding_x(padding);
    }

    fn set_autocomplete_max_visible(&mut self, max_visible: usize) {
        self.editor.set_autocomplete_max_visible(max_visible);
    }
}
