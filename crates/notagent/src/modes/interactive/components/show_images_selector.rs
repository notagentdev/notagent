use std::cell::RefCell;
use std::rc::Rc;

use notagent_tui::components::select_list::{SelectItem, SelectList, SelectListLayoutOptions};
use notagent_tui::tui::{Component, ComponentRef, Container, Line, component_ref};

use crate::modes::interactive::theme::theme::get_select_list_theme;

use super::dynamic_border::DynamicBorder;

fn show_images_select_list_layout() -> SelectListLayoutOptions {
    SelectListLayoutOptions {
        min_primary_column_width: Some(12),
        max_primary_column_width: Some(32),
        truncate_primary: None,
    }
}

/// Component that renders a show images selector with borders
pub struct ShowImagesSelectorComponent {
    container: Container,
    select_list: Rc<RefCell<SelectList>>,
}

impl ShowImagesSelectorComponent {
    /// New selector, preselecting `current_value`.
    pub fn new(
        current_value: bool,
        on_select: Box<dyn FnMut(bool)>,
        on_cancel: Box<dyn FnMut()>,
    ) -> Self {
        let mut container = Container::new();

        let items = vec![
            SelectItem {
                value: "yes".to_string(),
                label: "Yes".to_string(),
                description: Some("Show images inline in terminal".to_string()),
            },
            SelectItem {
                value: "no".to_string(),
                label: "No".to_string(),
                description: Some("Show text placeholder instead".to_string()),
            },
        ];

        // Add top border
        container.add_child(component_ref(DynamicBorder::new(None)));

        // Create selector
        let mut select_list = SelectList::new(
            items,
            5,
            get_select_list_theme(),
            show_images_select_list_layout(),
        );

        // Preselect current value
        select_list.set_selected_index(if current_value { 0 } else { 1 });

        let mut on_select = on_select;
        select_list.on_select = Some(Box::new(move |item| {
            on_select(item.value == "yes");
        }));
        select_list.on_cancel = Some(on_cancel);

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

impl Component for ShowImagesSelectorComponent {
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
