//! Selection list with a two column layout.
//!
//! 1:1 port of `packages/tui/src/components/select-list.ts` (229 LOC).

use std::rc::Rc;

use crate::keybindings::keybindings_match;
use crate::tui::Component;
use crate::utils::{truncate_to_width_opts, visible_width};

const DEFAULT_PRIMARY_COLUMN_WIDTH: usize = 32;
const PRIMARY_COLUMN_GAP: usize = 2;
const MIN_DESCRIPTION_WIDTH: usize = 10;

fn normalize_to_single_line(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut in_break = false;
    for ch in text.chars() {
        if ch == '\r' || ch == '\n' {
            if !in_break {
                result.push(' ');
                in_break = true;
            }
        } else {
            in_break = false;
            result.push(ch);
        }
    }
    result.trim().to_string()
}

/// One selectable item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectItem {
    /// Value used for filtering.
    pub value: String,
    /// Text shown in the primary column.
    pub label: String,
    /// Optional description shown in the second column.
    pub description: Option<String>,
}

/// Colouring functions of a [`SelectList`].
pub struct SelectListTheme {
    /// Prefix of the selected row.
    pub selected_prefix: Rc<dyn Fn(&str) -> String>,
    /// Whole selected row.
    pub selected_text: Rc<dyn Fn(&str) -> String>,
    /// Description column.
    pub description: Rc<dyn Fn(&str) -> String>,
    /// Scroll indicator.
    pub scroll_info: Rc<dyn Fn(&str) -> String>,
    /// "No matching commands" message.
    pub no_match: Rc<dyn Fn(&str) -> String>,
}

/// Context passed to a custom primary column truncation.
pub struct SelectListTruncatePrimaryContext<'a> {
    /// Text to truncate.
    pub text: &'a str,
    /// Available width.
    pub max_width: usize,
    /// Width of the primary column.
    pub column_width: usize,
    /// The item being rendered.
    pub item: &'a SelectItem,
    /// Whether the item is selected.
    pub is_selected: bool,
}

/// Custom truncation of the primary column.
pub type TruncatePrimaryFn = Rc<dyn Fn(SelectListTruncatePrimaryContext) -> String>;

/// Layout options of a [`SelectList`].
#[derive(Default)]
pub struct SelectListLayoutOptions {
    /// Lower bound of the primary column.
    pub min_primary_column_width: Option<usize>,
    /// Upper bound of the primary column.
    pub max_primary_column_width: Option<usize>,
    /// Custom truncation of the primary column.
    pub truncate_primary: Option<TruncatePrimaryFn>,
}

/// Callback invoked on selection events.
pub type SelectCallback = Box<dyn FnMut(&SelectItem)>;

/// Scrollable list of selectable items.
pub struct SelectList {
    items: Vec<SelectItem>,
    filtered_items: Vec<SelectItem>,
    selected_index: usize,
    max_visible: usize,
    theme: SelectListTheme,
    layout: SelectListLayoutOptions,
    /// Called when an item is confirmed.
    pub on_select: Option<SelectCallback>,
    /// Called when the list is cancelled.
    pub on_cancel: Option<Box<dyn FnMut()>>,
    /// Called when the selection moves.
    pub on_selection_change: Option<SelectCallback>,
}

impl SelectList {
    /// New list over `items`.
    pub fn new(
        items: Vec<SelectItem>,
        max_visible: usize,
        theme: SelectListTheme,
        layout: SelectListLayoutOptions,
    ) -> Self {
        Self {
            filtered_items: items.clone(),
            items,
            selected_index: 0,
            max_visible,
            theme,
            layout,
            on_select: None,
            on_cancel: None,
            on_selection_change: None,
        }
    }

    /// Filter items by a value prefix (case insensitive) and reset the selection.
    pub fn set_filter(&mut self, filter: &str) {
        let filter = filter.to_lowercase();
        self.filtered_items = self
            .items
            .iter()
            .filter(|item| item.value.to_lowercase().starts_with(&filter))
            .cloned()
            .collect();
        self.selected_index = 0;
    }

    /// Move the selection to `index` (clamped).
    pub fn set_selected_index(&mut self, index: usize) {
        self.selected_index = index.min(self.filtered_items.len().saturating_sub(1));
    }

    /// The selected item, if any.
    pub fn get_selected_item(&self) -> Option<SelectItem> {
        self.filtered_items.get(self.selected_index).cloned()
    }

    fn get_primary_column_bounds(&self) -> (usize, usize) {
        let raw_min = self
            .layout
            .min_primary_column_width
            .or(self.layout.max_primary_column_width)
            .unwrap_or(DEFAULT_PRIMARY_COLUMN_WIDTH);
        let raw_max = self
            .layout
            .max_primary_column_width
            .or(self.layout.min_primary_column_width)
            .unwrap_or(DEFAULT_PRIMARY_COLUMN_WIDTH);
        (raw_min.min(raw_max).max(1), raw_min.max(raw_max).max(1))
    }

    fn get_primary_column_width(&self) -> usize {
        let (min, max) = self.get_primary_column_bounds();
        let widest_primary = self
            .filtered_items
            .iter()
            .map(|item| visible_width(display_value(item)) + PRIMARY_COLUMN_GAP)
            .max()
            .unwrap_or(0);
        widest_primary.clamp(min, max)
    }

    fn truncate_primary(
        &self,
        item: &SelectItem,
        is_selected: bool,
        max_width: usize,
        column_width: usize,
    ) -> String {
        let display = display_value(item);
        let truncated_value = match &self.layout.truncate_primary {
            Some(truncate) => truncate(SelectListTruncatePrimaryContext {
                text: display,
                max_width,
                column_width,
                item,
                is_selected,
            }),
            None => truncate_to_width_opts(display, max_width, "", false),
        };
        truncate_to_width_opts(&truncated_value, max_width, "", false)
    }

    fn render_item(
        &self,
        item: &SelectItem,
        is_selected: bool,
        width: usize,
        description_single_line: Option<&str>,
        primary_column_width: usize,
    ) -> String {
        let prefix = if is_selected { "→ " } else { "  " };
        let prefix_width = visible_width(prefix);

        if let Some(description) = description_single_line
            && width > 40
        {
            let effective_primary_column_width = primary_column_width
                .min(width.saturating_sub(prefix_width + 4))
                .max(1);
            let max_primary_width = effective_primary_column_width
                .saturating_sub(PRIMARY_COLUMN_GAP)
                .max(1);
            let truncated_value = self.truncate_primary(
                item,
                is_selected,
                max_primary_width,
                effective_primary_column_width,
            );
            let truncated_value_width = visible_width(&truncated_value);
            let spacing = " ".repeat(
                effective_primary_column_width
                    .saturating_sub(truncated_value_width)
                    .max(1),
            );
            let description_start = prefix_width + truncated_value_width + spacing.len();
            let remaining_width = width as i64 - description_start as i64 - 2; // -2 for safety

            if remaining_width > MIN_DESCRIPTION_WIDTH as i64 {
                let truncated_desc =
                    truncate_to_width_opts(description, remaining_width as usize, "", false);
                if is_selected {
                    return (self.theme.selected_text)(&format!(
                        "{prefix}{truncated_value}{spacing}{truncated_desc}"
                    ));
                }
                let desc_text = (self.theme.description)(&format!("{spacing}{truncated_desc}"));
                return format!("{prefix}{truncated_value}{desc_text}");
            }
        }

        let max_width = width.saturating_sub(prefix_width + 2);
        let truncated_value = self.truncate_primary(item, is_selected, max_width, max_width);
        if is_selected {
            return (self.theme.selected_text)(&format!("{prefix}{truncated_value}"));
        }
        format!("{prefix}{truncated_value}")
    }

    fn notify_selection_change(&mut self) {
        let Some(item) = self.filtered_items.get(self.selected_index).cloned() else {
            return;
        };
        if let Some(callback) = self.on_selection_change.as_mut() {
            callback(&item);
        }
    }
}

fn display_value(item: &SelectItem) -> &str {
    if item.label.is_empty() {
        &item.value
    } else {
        &item.label
    }
}

impl Component for SelectList {
    fn render(&mut self, width: usize) -> Vec<String> {
        if self.filtered_items.is_empty() {
            return vec![(self.theme.no_match)("  No matching commands")];
        }

        let primary_column_width = self.get_primary_column_width();
        let start_index = (self.selected_index as i64 - (self.max_visible / 2) as i64)
            .min(self.filtered_items.len() as i64 - self.max_visible as i64)
            .max(0) as usize;
        let end_index = (start_index + self.max_visible).min(self.filtered_items.len());

        let mut lines: Vec<String> = Vec::new();
        for index in start_index..end_index {
            let item = self.filtered_items[index].clone();
            let is_selected = index == self.selected_index;
            let description_single_line = item.description.as_deref().map(normalize_to_single_line);
            lines.push(self.render_item(
                &item,
                is_selected,
                width,
                description_single_line.as_deref(),
                primary_column_width,
            ));
        }

        if start_index > 0 || end_index < self.filtered_items.len() {
            let scroll_text = format!(
                "  ({}/{})",
                self.selected_index + 1,
                self.filtered_items.len()
            );
            lines.push((self.theme.scroll_info)(&truncate_to_width_opts(
                &scroll_text,
                width.saturating_sub(2),
                "",
                false,
            )));
        }

        lines
    }

    fn handle_input(&mut self, data: &str) {
        // Up arrow — wrap to the bottom when at the top.
        if keybindings_match(data, "tui.select.up") {
            self.selected_index = if self.selected_index == 0 {
                self.filtered_items.len().saturating_sub(1)
            } else {
                self.selected_index - 1
            };
            self.notify_selection_change();
        }
        // Down arrow — wrap to the top when at the bottom.
        else if keybindings_match(data, "tui.select.down") {
            self.selected_index =
                if self.selected_index == self.filtered_items.len().saturating_sub(1) {
                    0
                } else {
                    self.selected_index + 1
                };
            self.notify_selection_change();
        } else if keybindings_match(data, "tui.select.confirm") {
            if let Some(item) = self.filtered_items.get(self.selected_index).cloned()
                && let Some(callback) = self.on_select.as_mut()
            {
                callback(&item);
            }
        } else if keybindings_match(data, "tui.select.cancel")
            && let Some(callback) = self.on_cancel.as_mut()
        {
            callback();
        }
    }

    fn invalidate(&mut self) {
        // No cached state to invalidate currently.
    }
}
