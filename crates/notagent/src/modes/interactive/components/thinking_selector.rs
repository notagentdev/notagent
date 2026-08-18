//! 1:1 port of
//! `packages/coding-agent/src/modes/interactive/components/thinking-selector.ts` (75 LOC).

use std::cell::RefCell;
use std::rc::Rc;

use notagent_agent::ThinkingLevel;
use notagent_tui::components::select_list::{SelectItem, SelectList, SelectListLayoutOptions};
use notagent_tui::tui::{Component, ComponentRef, Container, Line, component_ref};

use crate::modes::interactive::theme::theme::get_select_list_theme;

use super::dynamic_border::DynamicBorder;

fn thinking_select_list_layout() -> SelectListLayoutOptions {
    SelectListLayoutOptions {
        min_primary_column_width: Some(12),
        max_primary_column_width: Some(32),
        truncate_primary: None,
    }
}

/// `LEVEL_DESCRIPTIONS`.
fn level_description(level: ThinkingLevel) -> &'static str {
    match level {
        ThinkingLevel::Off => "No reasoning",
        ThinkingLevel::Minimal => "Very brief reasoning (~1k tokens)",
        ThinkingLevel::Low => "Light reasoning (~2k tokens)",
        ThinkingLevel::Medium => "Moderate reasoning (~8k tokens)",
        ThinkingLevel::High => "Deep reasoning (~16k tokens)",
        ThinkingLevel::Xhigh => "Extra-high reasoning (~32k tokens)",
        ThinkingLevel::Max => "Maximum reasoning",
    }
}

/// The `ThinkingLevel` literal, which is also the label.
fn level_value(level: ThinkingLevel) -> &'static str {
    match level {
        ThinkingLevel::Off => "off",
        ThinkingLevel::Minimal => "minimal",
        ThinkingLevel::Low => "low",
        ThinkingLevel::Medium => "medium",
        ThinkingLevel::High => "high",
        ThinkingLevel::Xhigh => "xhigh",
        ThinkingLevel::Max => "max",
    }
}

fn level_from_value(value: &str) -> Option<ThinkingLevel> {
    [
        ThinkingLevel::Off,
        ThinkingLevel::Minimal,
        ThinkingLevel::Low,
        ThinkingLevel::Medium,
        ThinkingLevel::High,
        ThinkingLevel::Xhigh,
        ThinkingLevel::Max,
    ]
    .into_iter()
    .find(|level| level_value(*level) == value)
}

/// Component that renders a thinking level selector with borders
pub struct ThinkingSelectorComponent {
    container: Container,
    select_list: Rc<RefCell<SelectList>>,
}

impl ThinkingSelectorComponent {
    /// New selector over `available_levels`, preselecting `current_level`.
    pub fn new(
        current_level: ThinkingLevel,
        available_levels: Vec<ThinkingLevel>,
        on_select: Box<dyn FnMut(ThinkingLevel)>,
        on_cancel: Box<dyn FnMut()>,
    ) -> Self {
        let mut container = Container::new();

        let thinking_levels: Vec<SelectItem> = available_levels
            .iter()
            .map(|level| SelectItem {
                value: level_value(*level).to_string(),
                label: level_value(*level).to_string(),
                description: Some(level_description(*level).to_string()),
            })
            .collect();

        // Add top border
        container.add_child(component_ref(DynamicBorder::new(None)));

        // Create selector
        let mut select_list = SelectList::new(
            thinking_levels.clone(),
            thinking_levels.len(),
            get_select_list_theme(),
            thinking_select_list_layout(),
        );

        // Preselect current level
        if let Some(current_index) = thinking_levels
            .iter()
            .position(|item| item.value == level_value(current_level))
        {
            select_list.set_selected_index(current_index);
        }

        let mut on_select = on_select;
        select_list.on_select = Some(Box::new(move |item| {
            if let Some(level) = level_from_value(&item.value) {
                on_select(level);
            }
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

impl Component for ThinkingSelectorComponent {
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
