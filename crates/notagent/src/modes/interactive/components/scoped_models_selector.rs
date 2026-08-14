//! 1:1 port of
//! `packages/coding-agent/src/modes/interactive/components/scoped-models-selector.ts` (403 LOC).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use notagent_ai::types::Model;
use notagent_tui::components::input::Input;
use notagent_tui::components::spacer::Spacer;
use notagent_tui::components::text::Text;
use notagent_tui::fuzzy::fuzzy_filter;
use notagent_tui::keybindings::keybindings_match;
use notagent_tui::keys::matches_key;
use notagent_tui::tui::{Component, ComponentRef, Container, Focusable, component_ref};

use crate::modes::interactive::model_search::{ModelSearchItem, get_model_search_text};
use crate::modes::interactive::theme::theme::{ThemeColor, theme};

use super::dynamic_border::DynamicBorder;
use super::keybinding_hints::key_text;

/// `EnabledIds`: `None` = all enabled (no filter), `Some(list)` = explicit ordered list
type EnabledIds = Option<Vec<String>>;

fn is_enabled(enabled_ids: &EnabledIds, id: &str) -> bool {
    match enabled_ids {
        None => true,
        Some(ids) => ids.iter().any(|enabled| enabled == id),
    }
}

fn toggle(enabled_ids: &EnabledIds, id: &str) -> EnabledIds {
    let Some(ids) = enabled_ids else {
        // First toggle: start with only this one
        return Some(vec![id.to_string()]);
    };
    match ids.iter().position(|enabled| enabled == id) {
        Some(index) => {
            let mut result = ids.clone();
            result.remove(index);
            Some(result)
        }
        None => {
            let mut result = ids.clone();
            result.push(id.to_string());
            Some(result)
        }
    }
}

fn enable_all(
    enabled_ids: &EnabledIds,
    all_ids: &[String],
    target_ids: Option<&[String]>,
) -> EnabledIds {
    let Some(ids) = enabled_ids else {
        // Already all enabled
        return None;
    };
    let targets: &[String] = target_ids.unwrap_or(all_ids);
    let mut result = ids.clone();
    for id in targets {
        if !result.contains(id) {
            result.push(id.clone());
        }
    }
    if result.len() == all_ids.len() && result.iter().all(|id| all_ids.contains(id)) {
        None
    } else {
        Some(result)
    }
}

fn clear_all(
    enabled_ids: &EnabledIds,
    all_ids: &[String],
    target_ids: Option<&[String]>,
) -> EnabledIds {
    let Some(ids) = enabled_ids else {
        return Some(match target_ids {
            Some(targets) => all_ids
                .iter()
                .filter(|id| !targets.contains(id))
                .cloned()
                .collect(),
            None => Vec::new(),
        });
    };
    let targets: HashSet<&String> = target_ids.unwrap_or(ids).iter().collect();
    Some(
        ids.iter()
            .filter(|id| !targets.contains(id))
            .cloned()
            .collect(),
    )
}

fn move_id(enabled_ids: &EnabledIds, id: &str, delta: isize) -> EnabledIds {
    let Some(ids) = enabled_ids else {
        return None;
    };
    let list = ids.clone();
    let Some(index) = list.iter().position(|enabled| enabled == id) else {
        return Some(list);
    };
    let new_index = index as isize + delta;
    if new_index < 0 || new_index >= list.len() as isize {
        return Some(list);
    }
    let mut result = list;
    result.swap(index, new_index as usize);
    Some(result)
}

fn get_sorted_ids(enabled_ids: &EnabledIds, all_ids: &[String]) -> Vec<String> {
    let Some(ids) = enabled_ids else {
        return all_ids.to_vec();
    };
    let enabled_set: HashSet<&String> = ids.iter().collect();
    let mut result = ids.clone();
    result.extend(
        all_ids
            .iter()
            .filter(|id| !enabled_set.contains(id))
            .cloned(),
    );
    result
}

#[derive(Clone)]
struct ModelItem {
    full_id: String,
    model: Option<Model>,
    enabled: bool,
}

/// `ModelsConfig`
pub struct ModelsConfig {
    /// Every model the runtime knows.
    pub all_models: Vec<Model>,
    /// `None` means every model is enabled.
    pub enabled_model_ids: Option<Vec<String>>,
    /// Status line below the list.
    pub refresh_status: Option<String>,
}

/// Invoked with the enabled model ids.
pub type EnabledIdsCallback = Box<dyn FnMut(Option<Vec<String>>)>;

/// `ModelsCallbacks`
pub struct ModelsCallbacks {
    /// Called whenever the enabled model set or order changes (session-only, no persist)
    pub on_change: EnabledIdsCallback,
    /// Called when user wants to persist current selection to settings
    pub on_persist: EnabledIdsCallback,
    /// Called on cancel.
    pub on_cancel: Box<dyn FnMut()>,
}

/// Colour of the refresh status line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefreshStatusKind {
    /// `muted`
    Muted,
    /// `success`
    Success,
    /// `warning`
    Warning,
}

/// Component for enabling/disabling models for Ctrl+P cycling.
/// Changes are session-only until explicitly persisted with Ctrl+S.
pub struct ScopedModelsSelectorComponent {
    container: Container,
    models_by_id: HashMap<String, Model>,
    all_ids: Vec<String>,
    enabled_ids: EnabledIds,
    filtered_items: Vec<ModelItem>,
    selected_index: usize,
    search_input: Rc<RefCell<Input>>,
    list_container: Rc<RefCell<Container>>,
    footer_text: Rc<RefCell<Text>>,
    callbacks: ModelsCallbacks,
    max_visible: usize,
    is_dirty: bool,
    refresh_status_text: Option<Rc<RefCell<Text>>>,

    // Focusable implementation - propagate to searchInput for IME cursor positioning
    focused: bool,
}

impl ScopedModelsSelectorComponent {
    /// New selector over `config`.
    pub fn new(config: ModelsConfig, callbacks: ModelsCallbacks) -> Self {
        let theme_instance = theme();
        let mut models_by_id = HashMap::new();
        let mut all_ids = Vec::new();
        for model in config.all_models {
            let full_id = format!("{}/{}", model.provider, model.id);
            models_by_id.insert(full_id.clone(), model);
            all_ids.push(full_id);
        }

        let enabled_ids = config.enabled_model_ids;
        let mut container = Container::new();

        // Header
        container.add_child(component_ref(DynamicBorder::new(None)));
        container.add_child(component_ref(Spacer::new(1)));
        container.add_child(component_ref(Text::new(
            theme_instance.fg(
                ThemeColor::Accent,
                &theme_instance.bold("Model Configuration"),
            ),
            0,
            0,
        )));
        container.add_child(component_ref(Text::new(
            theme_instance.fg(
                ThemeColor::Muted,
                &format!(
                    "Session-only. {} to save to settings.",
                    key_text("app.models.save")
                ),
            ),
            0,
            0,
        )));
        container.add_child(component_ref(Spacer::new(1)));

        // Search input
        let search_input = Rc::new(RefCell::new(Input::new()));
        container.add_child(Rc::clone(&search_input) as ComponentRef);
        container.add_child(component_ref(Spacer::new(1)));

        // List container
        let list_container = Rc::new(RefCell::new(Container::new()));
        container.add_child(Rc::clone(&list_container) as ComponentRef);

        // Footer hint
        container.add_child(component_ref(Spacer::new(1)));
        let refresh_status_text = config.refresh_status.map(|status| {
            let text = Rc::new(RefCell::new(Text::new(
                theme_instance.fg(ThemeColor::Muted, &format!("  {status}")),
                0,
                0,
            )));
            container.add_child(Rc::clone(&text) as ComponentRef);
            text
        });
        let footer_text = Rc::new(RefCell::new(Text::new(String::new(), 0, 0)));
        container.add_child(Rc::clone(&footer_text) as ComponentRef);

        container.add_child(component_ref(DynamicBorder::new(None)));

        let mut selector = Self {
            container,
            models_by_id,
            all_ids,
            enabled_ids,
            filtered_items: Vec::new(),
            selected_index: 0,
            search_input,
            list_container,
            footer_text,
            callbacks,
            max_visible: 8,
            is_dirty: false,
            refresh_status_text,
            focused: false,
        };
        selector.filtered_items = selector.build_items();
        let footer = selector.get_footer_text();
        selector.footer_text.borrow_mut().set_text(footer);
        selector.update_list();
        selector
    }

    /// Replace the model list, optionally with a new enabled set.
    pub fn update_models(
        &mut self,
        models: &[Model],
        enabled_model_ids: Option<Option<Vec<String>>>,
    ) {
        let selected_id = self
            .filtered_items
            .get(self.selected_index)
            .map(|item| item.full_id.clone());
        if let Some(enabled) = enabled_model_ids {
            self.enabled_ids = enabled;
        }
        self.models_by_id.clear();
        self.all_ids = Vec::new();
        for model in models {
            let full_id = format!("{}/{}", model.provider, model.id);
            self.models_by_id.insert(full_id.clone(), model.clone());
            self.all_ids.push(full_id);
        }
        self.refresh();
        let refreshed_index = selected_id.and_then(|id| {
            self.filtered_items
                .iter()
                .position(|item| item.full_id == id)
        });
        if let Some(index) = refreshed_index {
            self.selected_index = index;
            self.update_list();
        }
    }

    /// Replace the status line below the list.
    pub fn set_refresh_status(&mut self, message: &str, kind: RefreshStatusKind) {
        let color = match kind {
            RefreshStatusKind::Muted => ThemeColor::Muted,
            RefreshStatusKind::Success => ThemeColor::Success,
            RefreshStatusKind::Warning => ThemeColor::Warning,
        };
        if let Some(text) = self.refresh_status_text.as_ref() {
            text.borrow_mut()
                .set_text(theme().fg(color, &format!("  {message}")));
        }
    }

    fn build_items(&self) -> Vec<ModelItem> {
        get_sorted_ids(&self.enabled_ids, &self.all_ids)
            .into_iter()
            .map(|id| ModelItem {
                model: self.models_by_id.get(&id).cloned(),
                enabled: is_enabled(&self.enabled_ids, &id),
                full_id: id,
            })
            .collect()
    }

    fn get_footer_text(&self) -> String {
        let theme_instance = theme();
        let enabled_count = match self.enabled_ids.as_ref() {
            Some(ids) => ids
                .iter()
                .filter(|id| self.models_by_id.contains_key(*id))
                .count(),
            None => self.all_ids.len(),
        };
        let unavailable_count = match self.enabled_ids.as_ref() {
            Some(ids) => ids
                .iter()
                .filter(|id| !self.models_by_id.contains_key(*id))
                .count(),
            None => 0,
        };
        let all_enabled = self.enabled_ids.is_none();
        let count_text = if all_enabled {
            "all enabled".to_string()
        } else {
            format!(
                "{enabled_count}/{} enabled{}",
                self.all_ids.len(),
                if unavailable_count > 0 {
                    format!(" · {unavailable_count} unavailable")
                } else {
                    String::new()
                }
            )
        };
        let parts = [
            format!("{} toggle", key_text("tui.select.confirm")),
            format!("{} all", key_text("app.models.enableAll")),
            format!("{} clear", key_text("app.models.clearAll")),
            format!("{} provider", key_text("app.models.toggleProvider")),
            format!(
                "{}/{} reorder",
                key_text("app.models.reorderUp"),
                key_text("app.models.reorderDown")
            ),
            format!("{} save", key_text("app.models.save")),
            count_text,
        ];
        let joined = parts.join(" · ");
        if self.is_dirty {
            theme_instance.fg(ThemeColor::Dim, &format!("  {joined} "))
                + &theme_instance.fg(ThemeColor::Warning, "(unsaved)")
        } else {
            theme_instance.fg(ThemeColor::Dim, &format!("  {joined}"))
        }
    }

    fn refresh(&mut self) {
        let query = self.search_input.borrow().get_value().to_string();
        let items = self.build_items();
        self.filtered_items = if query.is_empty() {
            items
        } else {
            fuzzy_filter(&items, &query, |item| match item.model.as_ref() {
                Some(model) => get_model_search_text(&ModelSearchItem::new(
                    model.id.clone(),
                    model.provider.clone(),
                    Some(model.name.clone()),
                )),
                None => item.full_id.clone(),
            })
        };
        self.selected_index = self
            .selected_index
            .min(self.filtered_items.len().saturating_sub(1));
        self.update_list();
        let footer = self.get_footer_text();
        self.footer_text.borrow_mut().set_text(footer);
    }

    fn notify_change(&mut self) {
        (self.callbacks.on_change)(self.enabled_ids.clone());
    }

    fn update_list(&mut self) {
        let theme_instance = theme();
        let mut list_container = self.list_container.borrow_mut();
        list_container.clear();

        if self.filtered_items.is_empty() {
            list_container.add_child(component_ref(Text::new(
                theme_instance.fg(ThemeColor::Muted, "  No matching models"),
                0,
                0,
            )));
            return;
        }

        let start_index = (self.selected_index as isize - (self.max_visible / 2) as isize)
            .min(self.filtered_items.len() as isize - self.max_visible as isize)
            .max(0) as usize;
        let end_index = (start_index + self.max_visible).min(self.filtered_items.len());
        let all_enabled = self.enabled_ids.is_none();

        for index in start_index..end_index {
            let item = &self.filtered_items[index];
            let is_selected = index == self.selected_index;
            let prefix = if is_selected {
                theme_instance.fg(ThemeColor::Accent, "→ ")
            } else {
                "  ".to_string()
            };
            let id = item
                .model
                .as_ref()
                .map(|model| model.id.clone())
                .unwrap_or_else(|| item.full_id.clone());
            let model_text = if is_selected {
                theme_instance.fg(ThemeColor::Accent, &id)
            } else {
                id
            };
            let provider_badge = theme_instance.fg(
                ThemeColor::Muted,
                &match item.model.as_ref() {
                    Some(model) => format!(" [{}]", model.provider),
                    None => " [unavailable]".to_string(),
                },
            );
            let status = match item.model.as_ref() {
                Some(_) if all_enabled => String::new(),
                Some(_) if item.enabled => theme_instance.fg(ThemeColor::Success, " ✓"),
                _ => theme_instance.fg(ThemeColor::Dim, " ✗"),
            };
            list_container.add_child(component_ref(Text::new(
                format!("{prefix}{model_text}{provider_badge}{status}"),
                0,
                0,
            )));
        }

        // Add scroll indicator if needed
        if start_index > 0 || end_index < self.filtered_items.len() {
            list_container.add_child(component_ref(Text::new(
                theme_instance.fg(
                    ThemeColor::Muted,
                    &format!(
                        "  ({}/{})",
                        self.selected_index + 1,
                        self.filtered_items.len()
                    ),
                ),
                0,
                0,
            )));
        }

        if !self.filtered_items.is_empty() {
            let selected = &self.filtered_items[self.selected_index];
            list_container.add_child(component_ref(Spacer::new(1)));
            list_container.add_child(component_ref(Text::new(
                theme_instance.fg(
                    ThemeColor::Muted,
                    &match selected.model.as_ref() {
                        Some(model) => format!("  Model Name: {}", model.name),
                        None => "  Model unavailable".to_string(),
                    },
                ),
                0,
                0,
            )));
        }
    }

    /// The search input; `getSearchInput()`.
    pub fn get_search_input(&self) -> &Rc<RefCell<Input>> {
        &self.search_input
    }
}

impl Component for ScopedModelsSelectorComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.container.render(width)
    }

    fn invalidate(&mut self) {
        self.container.invalidate();
    }

    fn as_focusable(&mut self) -> Option<&mut dyn Focusable> {
        Some(self)
    }

    fn handle_input(&mut self, data: &str) {
        // Navigation
        if keybindings_match(data, "tui.select.up") {
            if self.filtered_items.is_empty() {
                return;
            }
            self.selected_index = if self.selected_index == 0 {
                self.filtered_items.len() - 1
            } else {
                self.selected_index - 1
            };
            self.update_list();
            return;
        }
        if keybindings_match(data, "tui.select.down") {
            if self.filtered_items.is_empty() {
                return;
            }
            self.selected_index = if self.selected_index == self.filtered_items.len() - 1 {
                0
            } else {
                self.selected_index + 1
            };
            self.update_list();
            return;
        }

        // Reorder enabled models
        let reorder_up = keybindings_match(data, "app.models.reorderUp");
        let reorder_down = keybindings_match(data, "app.models.reorderDown");
        if reorder_up || reorder_down {
            let Some(enabled_ids) = self.enabled_ids.clone() else {
                return;
            };
            let Some(item) = self.filtered_items.get(self.selected_index) else {
                return;
            };
            let full_id = item.full_id.clone();
            if is_enabled(&self.enabled_ids, &full_id) {
                let delta: isize = if reorder_up { -1 } else { 1 };
                let current_index = enabled_ids
                    .iter()
                    .position(|id| *id == full_id)
                    .unwrap_or_default() as isize;
                let new_index = current_index + delta;
                // Only move if within bounds
                if new_index >= 0 && new_index < enabled_ids.len() as isize {
                    self.enabled_ids = move_id(&self.enabled_ids, &full_id, delta);
                    self.is_dirty = true;
                    self.selected_index = (self.selected_index as isize + delta).max(0) as usize;
                    self.refresh();
                    self.notify_change();
                }
            }
            return;
        }

        // Toggle on Enter
        if keybindings_match(data, "tui.select.confirm") {
            if let Some(item) = self.filtered_items.get(self.selected_index) {
                let full_id = item.full_id.clone();
                self.enabled_ids = toggle(&self.enabled_ids, &full_id);
                self.is_dirty = true;
                self.refresh();
                self.notify_change();
            }
            return;
        }

        // Enable all (filtered if search active, otherwise all)
        if keybindings_match(data, "app.models.enableAll") {
            let target_ids = self.search_target_ids();
            self.enabled_ids = enable_all(&self.enabled_ids, &self.all_ids, target_ids.as_deref());
            self.is_dirty = true;
            self.refresh();
            self.notify_change();
            return;
        }

        // Clear all (filtered if search active, otherwise all)
        if keybindings_match(data, "app.models.clearAll") {
            let target_ids = self.search_target_ids();
            self.enabled_ids = clear_all(&self.enabled_ids, &self.all_ids, target_ids.as_deref());
            self.is_dirty = true;
            self.refresh();
            self.notify_change();
            return;
        }

        // Toggle provider of current item
        if keybindings_match(data, "app.models.toggleProvider") {
            let provider = self
                .filtered_items
                .get(self.selected_index)
                .and_then(|item| item.model.as_ref())
                .map(|model| model.provider.clone());
            if let Some(provider) = provider {
                let provider_ids: Vec<String> = self
                    .all_ids
                    .iter()
                    .filter(|id| {
                        self.models_by_id
                            .get(*id)
                            .is_some_and(|model| model.provider == provider)
                    })
                    .cloned()
                    .collect();
                let all_enabled = provider_ids
                    .iter()
                    .all(|id| is_enabled(&self.enabled_ids, id));
                self.enabled_ids = if all_enabled {
                    clear_all(&self.enabled_ids, &self.all_ids, Some(&provider_ids))
                } else {
                    enable_all(&self.enabled_ids, &self.all_ids, Some(&provider_ids))
                };
                self.is_dirty = true;
                self.refresh();
                self.notify_change();
            }
            return;
        }

        // Save/persist to settings
        if keybindings_match(data, "app.models.save") {
            (self.callbacks.on_persist)(self.enabled_ids.clone());
            self.is_dirty = false;
            let footer = self.get_footer_text();
            self.footer_text.borrow_mut().set_text(footer);
            return;
        }

        // Ctrl+C - clear search or cancel if empty
        if matches_key(data, "ctrl+c") {
            if !self.search_input.borrow().get_value().is_empty() {
                self.search_input.borrow_mut().set_value("");
                self.refresh();
            } else {
                (self.callbacks.on_cancel)();
            }
            return;
        }

        // Escape - cancel
        if matches_key(data, "escape") {
            (self.callbacks.on_cancel)();
            return;
        }

        // Pass everything else to search input
        self.search_input.borrow_mut().handle_input(data);
        self.refresh();
    }
}

impl ScopedModelsSelectorComponent {
    /// `this.searchInput.getValue() ? this.filteredItems.map((i) => i.fullId) : undefined`
    fn search_target_ids(&self) -> Option<Vec<String>> {
        if self.search_input.borrow().get_value().is_empty() {
            return None;
        }
        Some(
            self.filtered_items
                .iter()
                .map(|item| item.full_id.clone())
                .collect(),
        )
    }
}

impl Focusable for ScopedModelsSelectorComponent {
    fn focused(&self) -> bool {
        self.focused
    }

    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
        self.search_input.borrow_mut().set_focused(focused);
    }
}
