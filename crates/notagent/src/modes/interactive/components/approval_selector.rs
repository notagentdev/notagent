//! 1:1 port of
//! `packages/coding-agent/src/modes/interactive/components/approval-selector.ts` (84 LOC).

use std::cell::RefCell;
use std::rc::Rc;

use notagent_tui::components::select_list::{SelectItem, SelectList, SelectListLayoutOptions};
use notagent_tui::components::text::Text;
use notagent_tui::tui::{Component, ComponentRef, Container, component_ref};

use crate::core::permissions::request::{
    APPROVAL_ANSWERS, ApprovalAnswer, ApprovalRequest, format_request_explanation,
    format_request_summary,
};
use crate::modes::interactive::theme::theme::{ThemeColor, get_select_list_theme, theme};

use super::dynamic_border::DynamicBorder;

/// Asks the user to approve one tool call.
///
/// Built on the same selector pattern as the model and theme pickers. Two
/// details differ deliberately: the deciding policy is always shown, so an
/// unexpected prompt can be traced to its rule, and cancelling resolves to a
/// denial rather than to nothing. A dismissed permission prompt must not read
/// as consent.
pub struct ApprovalSelectorComponent {
    container: Container,
    select_list: Rc<RefCell<SelectList>>,
}

impl ApprovalSelectorComponent {
    /// New dialog for `request`.
    pub fn new(request: &ApprovalRequest, on_answer: Box<dyn FnMut(ApprovalAnswer)>) -> Self {
        let theme_instance = theme();
        let mut container = Container::new();

        container.add_child(component_ref(DynamicBorder::new(None)));
        container.add_child(component_ref(Text::new(
            format!(
                "{}  {}",
                theme_instance.fg(
                    ThemeColor::Warning,
                    &theme_instance.bold("Permission required")
                ),
                theme_instance.fg(ThemeColor::Accent, &format_request_summary(request))
            ),
            1,
            0,
        )));
        container.add_child(component_ref(Text::new(
            theme_instance.fg(ThemeColor::Muted, &format_request_explanation(request)),
            1,
            0,
        )));
        if let Some(mode_id) = request.mode_id.as_deref()
            && !mode_id.is_empty()
        {
            container.add_child(component_ref(Text::new(
                theme_instance.fg(ThemeColor::Dim, &format!("mode: {mode_id}")),
                1,
                0,
            )));
        }

        let items: Vec<SelectItem> = APPROVAL_ANSWERS
            .iter()
            .map(|(answer, label)| SelectItem {
                value: answer.as_str().to_string(),
                label: (*label).to_string(),
                description: None,
            })
            .collect();
        let select_list = Rc::new(RefCell::new(SelectList::new(
            items.clone(),
            items.len(),
            get_select_list_theme(),
            SelectListLayoutOptions::default(),
        )));
        // One handler for both paths; the answer callback may only run once per
        // dialog, so it is shared through `Rc<RefCell<…>>`.
        let on_answer = Rc::new(RefCell::new(on_answer));
        {
            let mut list = select_list.borrow_mut();
            let handler = Rc::clone(&on_answer);
            list.on_select = Some(Box::new(move |item| {
                if let Some(answer) = answer_from_value(&item.value) {
                    (handler.borrow_mut())(answer);
                }
            }));
            // Escape is a refusal, not an absence of an answer.
            let handler = Rc::clone(&on_answer);
            list.on_cancel = Some(Box::new(move || {
                (handler.borrow_mut())(ApprovalAnswer::Deny);
            }));
        }

        container.add_child(Rc::clone(&select_list) as ComponentRef);
        container.add_child(component_ref(DynamicBorder::new(None)));

        Self {
            container,
            select_list,
        }
    }

    /// The inner list.
    pub fn get_select_list(&self) -> &Rc<RefCell<SelectList>> {
        &self.select_list
    }
}

/// `item.value as ApprovalAnswer` — the values come from [`APPROVAL_ANSWERS`].
fn answer_from_value(value: &str) -> Option<ApprovalAnswer> {
    APPROVAL_ANSWERS
        .iter()
        .find(|(answer, _)| answer.as_str() == value)
        .map(|(answer, _)| *answer)
}

impl Component for ApprovalSelectorComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.container.render(width)
    }

    fn invalidate(&mut self) {
        self.container.invalidate();
    }

    fn handle_input(&mut self, data: &str) {
        self.select_list.borrow_mut().handle_input(data);
    }
}
