use std::cell::RefCell;
use std::rc::Rc;

use crate::components::input::Input;
use crate::fuzzy::fuzzy_filter;
use crate::keybindings::keybindings_match;
use crate::tui::{Component, ComponentRef, Line, shared_lines};
use crate::utils::{truncate_to_width, truncate_to_width_opts, visible_width, wrap_text_with_ansi};

/// is the bare `done()` of a cancel, `None` means the submenu is still open.
/// the list while it is mutably borrowed for the very input that reaches the
/// submenu. The submenu therefore writes into the slot and the list reads it
/// the moment that input returns — before anything can observe the difference.
pub type SubmenuDone = Rc<RefCell<Option<Option<String>>>>;

/// `submenu?: (currentValue, done) => Component`.
pub type SubmenuFactory = Rc<dyn Fn(&str, SubmenuDone) -> ComponentRef>;

/// One settings row.
#[derive(Clone)]
pub struct SettingItem {
    /// Unique identifier.
    pub id: String,
    /// Label shown on the left.
    pub label: String,
    /// Description shown while selected.
    pub description: Option<String>,
    /// Current value shown on the right.
    pub current_value: String,
    /// Enter/Space cycles through these values.
    pub values: Option<Vec<String>>,
    /// Enter opens this submenu.
    pub submenu: Option<SubmenuFactory>,
}

/// Colouring function that also receives the selection state.
pub type SelectionAwareColorFn = Rc<dyn Fn(&str, bool) -> String>;

/// Colouring function for a plain text span.
pub type ColorFn = Rc<dyn Fn(&str) -> String>;

/// Colouring functions of a [`SettingsList`].
pub struct SettingsListTheme {
    /// Label column.
    pub label: SelectionAwareColorFn,
    /// Value column.
    pub value: SelectionAwareColorFn,
    /// Description block.
    pub description: ColorFn,
    /// Cursor prefix of the selected row.
    pub cursor: String,
    /// Hint line.
    pub hint: ColorFn,
}

/// Options of a [`SettingsList`].
#[derive(Debug, Clone, Copy, Default)]
pub struct SettingsListOptions {
    /// Show a search input above the list.
    pub enable_search: bool,
}

/// Callback invoked when a value changes.
pub type ChangeCallback = Box<dyn FnMut(&str, &str)>;

/// Scrollable list of settings.
pub struct SettingsList {
    items: Vec<SettingItem>,
    filtered_items: Vec<SettingItem>,
    theme: SettingsListTheme,
    selected_index: usize,
    max_visible: usize,
    on_change: ChangeCallback,
    on_cancel: Box<dyn FnMut()>,
    search_input: Option<Input>,
    search_enabled: bool,
    submenu_component: Option<ComponentRef>,
    submenu_item_index: Option<usize>,
    /// The continuation handed to the open submenu.
    submenu_done: SubmenuDone,
}

impl SettingsList {
    /// New settings list.
    pub fn new(
        items: Vec<SettingItem>,
        max_visible: usize,
        theme: SettingsListTheme,
        on_change: ChangeCallback,
        on_cancel: Box<dyn FnMut()>,
        options: SettingsListOptions,
    ) -> Self {
        Self {
            filtered_items: items.clone(),
            items,
            theme,
            selected_index: 0,
            max_visible,
            on_change,
            on_cancel,
            search_input: options.enable_search.then(Input::new),
            search_enabled: options.enable_search,
            submenu_component: None,
            submenu_item_index: None,
            submenu_done: Rc::new(RefCell::new(None)),
        }
    }

    /// Update an item's current value.
    pub fn update_value(&mut self, id: &str, new_value: &str) {
        if let Some(item) = self.items.iter_mut().find(|item| item.id == id) {
            item.current_value = new_value.to_string();
        }
    }

    /// Close an open submenu, optionally applying its result — the body of the
    /// `done` continuation of `activate_item`.
    fn close_submenu(&mut self, selected_value: Option<&str>) {
        if let Some(value) = selected_value
            && let Some(index) = self.submenu_item_index
        {
            let items = if self.search_enabled {
                &mut self.filtered_items
            } else {
                &mut self.items
            };
            if let Some(item) = items.get_mut(index) {
                item.current_value = value.to_string();
                let id = item.id.clone();
                self.update_value(&id, value);
                (self.on_change)(&id, value);
            }
        }
        self.submenu_component = None;
        if let Some(index) = self.submenu_item_index.take() {
            self.selected_index = index;
        }
    }

    fn display_items(&self) -> &[SettingItem] {
        if self.search_enabled {
            &self.filtered_items
        } else {
            &self.items
        }
    }

    fn add_hint_line(&self, lines: &mut Vec<String>, width: usize) {
        lines.push(String::new());
        let hint = if self.search_enabled {
            "  Type to search · Enter/Space to change · Esc to cancel"
        } else {
            "  Enter/Space to change · Esc to cancel"
        };
        lines.push(truncate_to_width(&(self.theme.hint)(hint), width));
    }

    fn render_main_list(&mut self, width: usize) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();

        if let Some(search_input) = self.search_input.as_mut() {
            // The input renders one fresh line per frame; the copy back into the
            // owned builder is a single line.
            lines.extend(
                search_input
                    .render(width)
                    .iter()
                    .map(|line| line.to_string()),
            );
            lines.push(String::new());
        }

        if self.items.is_empty() {
            lines.push((self.theme.hint)("  No settings available"));
            if self.search_enabled {
                self.add_hint_line(&mut lines, width);
            }
            return lines;
        }

        if self.display_items().is_empty() {
            lines.push(truncate_to_width(
                &(self.theme.hint)("  No matching settings"),
                width,
            ));
            self.add_hint_line(&mut lines, width);
            return lines;
        }

        let display_items = self.display_items().to_vec();
        let start_index = (self.selected_index as i64 - (self.max_visible / 2) as i64)
            .min(display_items.len() as i64 - self.max_visible as i64)
            .max(0) as usize;
        let end_index = (start_index + self.max_visible).min(display_items.len());

        let max_label_width = self
            .items
            .iter()
            .map(|item| visible_width(&item.label))
            .max()
            .unwrap_or(0)
            .min(30);

        for (index, item) in display_items
            .iter()
            .enumerate()
            .take(end_index)
            .skip(start_index)
        {
            let is_selected = index == self.selected_index;
            let prefix = if is_selected {
                self.theme.cursor.clone()
            } else {
                "  ".to_string()
            };
            let prefix_width = visible_width(&prefix);

            let label_padded = format!(
                "{}{}",
                item.label,
                " ".repeat(max_label_width.saturating_sub(visible_width(&item.label)))
            );
            let label_text = (self.theme.label)(&label_padded, is_selected);

            let separator = "  ";
            let used_width = prefix_width + max_label_width + visible_width(separator);
            let value_max_width = width.saturating_sub(used_width + 2);
            let value_text = (self.theme.value)(
                &truncate_to_width_opts(&item.current_value, value_max_width, "", false),
                is_selected,
            );

            lines.push(truncate_to_width(
                &format!("{prefix}{label_text}{separator}{value_text}"),
                width,
            ));
        }

        if start_index > 0 || end_index < display_items.len() {
            let scroll_text = format!("  ({}/{})", self.selected_index + 1, display_items.len());
            lines.push((self.theme.hint)(&truncate_to_width_opts(
                &scroll_text,
                width.saturating_sub(2),
                "",
                false,
            )));
        }

        if let Some(description) = display_items
            .get(self.selected_index)
            .and_then(|item| item.description.clone())
        {
            lines.push(String::new());
            for line in wrap_text_with_ansi(&description, width.saturating_sub(4)) {
                lines.push((self.theme.description)(&format!("  {line}")));
            }
        }

        self.add_hint_line(&mut lines, width);
        lines
    }

    fn activate_item(&mut self) {
        let index = self.selected_index;
        let Some(item) = self.display_items().get(index).cloned() else {
            return;
        };

        if let Some(submenu) = item.submenu.clone() {
            // Open the submenu; the current value lets it pre-select correctly.
            self.submenu_item_index = Some(index);
            *self.submenu_done.borrow_mut() = None;
            self.submenu_component =
                Some(submenu(&item.current_value, Rc::clone(&self.submenu_done)));
            return;
        }
        if let Some(values) = item.values.clone()
            && !values.is_empty()
        {
            let current_index = values.iter().position(|value| *value == item.current_value);
            let next_index = current_index.map_or(0, |index| (index + 1) % values.len());
            let new_value = values[next_index].clone();
            if self.search_enabled
                && let Some(filtered) = self.filtered_items.get_mut(index)
            {
                filtered.current_value = new_value.clone();
            }
            self.update_value(&item.id, &new_value);
            if !self.search_enabled
                && let Some(direct) = self.items.get_mut(index)
            {
                direct.current_value = new_value.clone();
            }
            (self.on_change)(&item.id, &new_value);
        }
    }

    fn apply_filter(&mut self, query: &str) {
        self.filtered_items = fuzzy_filter(&self.items, query, |item| item.label.clone());
        self.selected_index = 0;
    }
}

impl Component for SettingsList {
    fn render(&mut self, width: usize) -> Vec<Line> {
        if let Some(submenu) = self.submenu_component.clone() {
            return submenu.borrow_mut().render(width);
        }
        shared_lines(self.render_main_list(width))
    }

    fn handle_input(&mut self, data: &str) {
        // While a submenu is open all input goes to it. Its `done` lands in the
        if let Some(submenu) = self.submenu_component.clone() {
            submenu.borrow_mut().handle_input(data);
            let done = self.submenu_done.borrow_mut().take();
            if let Some(selected_value) = done {
                self.close_submenu(selected_value.as_deref());
            }
            return;
        }

        let display_len = self.display_items().len();
        if keybindings_match(data, "tui.select.up") {
            if display_len == 0 {
                return;
            }
            self.selected_index = if self.selected_index == 0 {
                display_len - 1
            } else {
                self.selected_index - 1
            };
        } else if keybindings_match(data, "tui.select.down") {
            if display_len == 0 {
                return;
            }
            self.selected_index = if self.selected_index == display_len - 1 {
                0
            } else {
                self.selected_index + 1
            };
        } else if keybindings_match(data, "tui.select.confirm")
            || (data == " "
                && (!self.search_enabled
                    || self
                        .search_input
                        .as_ref()
                        .is_some_and(|input| input.get_value().is_empty())))
        {
            self.activate_item();
        } else if keybindings_match(data, "tui.select.cancel") {
            (self.on_cancel)();
        } else if self.search_enabled {
            let query = {
                let Some(search_input) = self.search_input.as_mut() else {
                    return;
                };
                search_input.handle_input(data);
                search_input.get_value().to_string()
            };
            self.apply_filter(&query);
        }
    }

    fn invalidate(&mut self) {
        if let Some(submenu) = &self.submenu_component {
            submenu.borrow_mut().invalidate();
        }
    }
}
