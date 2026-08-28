use std::rc::Rc;

use crate::autocomplete::AutocompleteProvider;
use crate::tui::Component;

/// An editor usable in place of [`crate::components::editor::Editor`].
/// methods with default implementations; the `onSubmit`/`onChange` callbacks
/// become polled queues, since a callback would need `&mut` access to the owner.
pub trait EditorComponent: Component {
    /// The current text.
    fn get_text(&self) -> String;

    /// Replace the text.
    fn set_text(&mut self, text: &str);

    /// Text submitted since the last call (`onSubmit`).
    fn take_submitted(&mut self) -> Vec<String>;

    /// Text of every change since the last call (`onChange`).
    fn take_changes(&mut self) -> Vec<String>;

    /// Add an entry to the history for up/down navigation.
    fn add_to_history(&mut self, _text: &str) {}

    /// Insert text at the cursor.
    fn insert_text_at_cursor(&mut self, _text: &str) {}

    /// Text with all markers expanded; defaults to [`Self::get_text`].
    fn get_expanded_text(&self) -> String {
        self.get_text()
    }

    /// Install the autocomplete provider.
    fn set_autocomplete_provider(&mut self, _provider: Rc<dyn AutocompleteProvider>) {}

    /// Replace the border colour.
    fn set_border_color(&mut self, _border_color: Rc<dyn Fn(&str) -> String>) {}

    /// Change the horizontal padding.
    fn set_padding_x(&mut self, _padding: usize) {}

    /// Change the maximum number of visible autocomplete entries.
    fn set_autocomplete_max_visible(&mut self, _max_visible: usize) {}
}
