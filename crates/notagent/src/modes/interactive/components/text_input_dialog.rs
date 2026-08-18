//! 1:1 port of
//! `packages/coding-agent/src/modes/interactive/components/extension-editor.ts` (132 LOC).
//!
//! Renamed to `text_input_dialog` per interface request C-23, for the reason
//! that renamed `extension-selector.ts` to [`super::list_selector`]: the file
//! sits among the extension UI, but a core dialog uses it. The tree selector
//! answers "Summarize with custom prompt" with
//! `showExtensionEditor("Custom summarization instructions")`
//! (`interactive-mode.ts:5285-5292` and `:2549-2577`), and that is the only
//! caller left once the extension system is gone.
//!
//! Deviations:
//!   * Class 1: `Editor` reports a submit through `take_submitted` instead of
//!     an `onSubmit` callback (workstream A's editor port), so the dialog
//!     drains it right after handing the key on — TypeScript's callback fires
//!     inside that same call.
//!   * Class 1: `tui.stop()` / `tui.start()` around the external editor is a
//!     [`ExternalEditorRunner`] the owner supplies. Restarting the TUI means
//!     re-registering the input pump, which a component cannot reach in the
//!     port (`RendererCell` belongs to the mode); the command itself is still
//!     resolved here, exactly as TypeScript does.
//!   * Class 2: `EditorOptions` and the prefill stay, the extension plumbing
//!     around them does not.

use std::cell::RefCell;
use std::rc::Rc;

use notagent_tui::components::editor::{Editor, EditorOptions};
use notagent_tui::components::spacer::Spacer;
use notagent_tui::components::text::Text;
use notagent_tui::keybindings::keybindings_match;
use notagent_tui::tui::{
    Component, ComponentRef, Container, Focusable, Line, TuiCore, component_ref,
};

use crate::core::keybindings::KeybindingsManager;
use crate::modes::interactive::theme::theme::{ThemeColor, get_editor_theme, theme};

use super::dynamic_border::DynamicBorder;
use super::keybinding_hints::key_hint;

/// Runs `command` over `content` while the TUI is stopped and returns the
/// edited text, or `None` when the editor did not complete.
///
/// The port of the `tui.stop()` / `editInExternalEditor` / `tui.start()` block
/// of `handleOpenExternalEditor` (`extension-editor.ts:113-131`).
pub type ExternalEditorRunner = Box<dyn FnMut(&str, &str) -> Option<String>>;

/// Everything `ExtensionEditorComponent` takes after its callbacks.
#[derive(Default)]
pub struct TextInputDialogOptions {
    /// Text the editor starts with.
    pub prefill: Option<String>,
    /// Passed to the [`Editor`].
    pub editor: Option<EditorOptions>,
    /// `externalEditorCommand`; `VISUAL`, `EDITOR` and finally `nano`
    /// (`notepad` on Windows) fill in.
    pub external_editor_command: Option<String>,
    /// Opens the external editor. Without one, `app.editor.external` does
    /// nothing — as in TypeScript, where the key only ever reaches a mounted
    /// dialog.
    pub on_external_editor: Option<ExternalEditorRunner>,
}

/// Multi-line editor dialog with a title, a hint line and borders.
pub struct TextInputDialogComponent {
    container: Container,
    editor: Rc<RefCell<Editor>>,
    keybindings: Rc<RefCell<KeybindingsManager>>,
    external_editor_command: String,
    on_external_editor: Option<ExternalEditorRunner>,
    on_submit: Box<dyn FnMut(String)>,
    on_cancel: Box<dyn FnMut()>,
    focused: bool,
}

impl TextInputDialogComponent {
    /// New dialog over `core`.
    pub fn new(
        core: TuiCore,
        keybindings: Rc<RefCell<KeybindingsManager>>,
        title: &str,
        on_submit: Box<dyn FnMut(String)>,
        on_cancel: Box<dyn FnMut()>,
        options: Option<TextInputDialogOptions>,
    ) -> Self {
        let options = options.unwrap_or_default();
        let theme_instance = theme();
        let mut container = Container::new();

        // Add top border
        container.add_child(component_ref(DynamicBorder::new(None)));
        container.add_child(component_ref(Spacer::new(1)));

        // Add title
        container.add_child(component_ref(Text::new(
            theme_instance.fg(ThemeColor::Accent, title),
            1,
            0,
        )));
        container.add_child(component_ref(Spacer::new(1)));

        // Create editor
        let mut editor = Editor::new(core, get_editor_theme(), options.editor.unwrap_or_default());
        if let Some(prefill) = options
            .prefill
            .as_deref()
            .filter(|prefill| !prefill.is_empty())
        {
            editor.set_text(prefill);
        }
        let editor = Rc::new(RefCell::new(editor));
        container.add_child(Rc::clone(&editor) as ComponentRef);

        container.add_child(component_ref(Spacer::new(1)));

        // Add hint
        let hint = format!(
            "{}  {}  {}  {}",
            key_hint("tui.select.confirm", "submit"),
            key_hint("tui.input.newLine", "newline"),
            key_hint("tui.select.cancel", "cancel"),
            key_hint("app.editor.external", "external editor"),
        );
        container.add_child(component_ref(Text::new(hint, 1, 0)));

        container.add_child(component_ref(Spacer::new(1)));

        // Add bottom border
        container.add_child(component_ref(DynamicBorder::new(None)));

        Self {
            container,
            editor,
            keybindings,
            external_editor_command: options
                .external_editor_command
                .filter(|command| !command.is_empty())
                .or_else(|| std::env::var("VISUAL").ok().filter(|it| !it.is_empty()))
                .or_else(|| std::env::var("EDITOR").ok().filter(|it| !it.is_empty()))
                .unwrap_or_else(|| if cfg!(windows) { "notepad" } else { "nano" }.to_string()),
            on_external_editor: options.on_external_editor,
            on_submit,
            on_cancel,
            focused: false,
        }
    }

    /// The text in the editor.
    pub fn text(&self) -> String {
        self.editor.borrow().get_text()
    }

    /// `handleOpenExternalEditor()`.
    fn handle_open_external_editor(&mut self) {
        let Some(run) = self.on_external_editor.as_mut() else {
            return;
        };
        let content = self.editor.borrow().get_text();
        if let Some(edited) = run(&self.external_editor_command, &content) {
            self.editor.borrow_mut().set_text(&edited);
        }
    }
}

impl Component for TextInputDialogComponent {
    fn render(&mut self, width: usize) -> Vec<Line> {
        self.container.render(width)
    }

    fn invalidate(&mut self) {
        self.container.invalidate();
    }

    fn as_focusable(&mut self) -> Option<&mut dyn Focusable> {
        Some(self)
    }

    fn handle_input(&mut self, data: &str) {
        // Escape or Ctrl+C to cancel
        if keybindings_match(data, "tui.select.cancel") {
            (self.on_cancel)();
            return;
        }

        // External editor (app keybinding)
        if self
            .keybindings
            .borrow()
            .matches(data, "app.editor.external")
        {
            self.handle_open_external_editor();
            return;
        }

        // Forward to editor
        let submitted = {
            let mut editor = self.editor.borrow_mut();
            editor.handle_input(data);
            editor.take_submitted()
        };
        // Wire up Enter to submit (Shift+Enter for newlines, like the main editor)
        for text in submitted {
            (self.on_submit)(text);
        }
    }
}

impl Focusable for TextInputDialogComponent {
    fn focused(&self) -> bool {
        self.focused
    }

    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
        self.editor.borrow_mut().set_focused(focused);
    }
}
