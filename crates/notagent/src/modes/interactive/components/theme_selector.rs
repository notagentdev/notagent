//! 1:1 port of
//! `packages/coding-agent/src/modes/interactive/components/theme-selector.ts` (67 LOC).

use std::cell::RefCell;
use std::rc::Rc;

use notagent_tui::components::select_list::{SelectItem, SelectList, SelectListLayoutOptions};
use notagent_tui::tui::{Component, ComponentRef, Container, Line, component_ref};

use crate::modes::interactive::theme::theme::{get_available_themes, get_select_list_theme};

use super::dynamic_border::DynamicBorder;

fn theme_select_list_layout() -> SelectListLayoutOptions {
    SelectListLayoutOptions {
        min_primary_column_width: Some(12),
        max_primary_column_width: Some(32),
        truncate_primary: None,
    }
}

/// Component that renders a theme selector
pub struct ThemeSelectorComponent {
    container: Container,
    select_list: Rc<RefCell<SelectList>>,
}

impl ThemeSelectorComponent {
    /// New selector; `on_preview` runs on every selection change.
    pub fn new(
        current_theme: &str,
        on_select: Box<dyn FnMut(String)>,
        on_cancel: Box<dyn FnMut()>,
        on_preview: Box<dyn FnMut(String)>,
    ) -> Self {
        let mut container = Container::new();

        // Get available themes and create select items
        let themes = get_available_themes();
        let theme_items: Vec<SelectItem> = themes
            .iter()
            .map(|name| SelectItem {
                value: name.clone(),
                label: name.clone(),
                description: if name == current_theme {
                    Some("(current)".to_string())
                } else {
                    None
                },
            })
            .collect();

        // Add top border
        container.add_child(component_ref(DynamicBorder::new(None)));

        // Create selector
        let mut select_list = SelectList::new(
            theme_items,
            10,
            get_select_list_theme(),
            theme_select_list_layout(),
        );

        // Preselect current theme
        if let Some(current_index) = themes.iter().position(|name| name == current_theme) {
            select_list.set_selected_index(current_index);
        }

        let mut on_select = on_select;
        select_list.on_select = Some(Box::new(move |item| {
            on_select(item.value.clone());
        }));
        select_list.on_cancel = Some(on_cancel);
        let mut on_preview = on_preview;
        select_list.on_selection_change = Some(Box::new(move |item| {
            on_preview(item.value.clone());
        }));

        let select_list = Rc::new(RefCell::new(select_list));
        container.add_child(Rc::clone(&select_list) as ComponentRef);

        // Add bottom border
        container.add_child(component_ref(DynamicBorder::new(None)));

        Self {
            container,
            select_list,
        }
    }

    /// The wrapped list, which the caller focuses.
    pub fn get_select_list(&self) -> Rc<RefCell<SelectList>> {
        Rc::clone(&self.select_list)
    }
}

impl Component for ThemeSelectorComponent {
    fn render(&mut self, width: usize) -> Vec<Line> {
        self.container.render(width)
    }

    fn handle_input(&mut self, data: &str) {
        self.select_list.borrow_mut().handle_input(data);
    }

    fn invalidate(&mut self) {
        self.container.invalidate();
    }
}
