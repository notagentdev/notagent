//! 1:1 port of
//! `packages/coding-agent/src/modes/interactive/components/trust-selector.ts` (134 LOC).

use std::cell::RefCell;
use std::rc::Rc;

use notagent_tui::components::spacer::Spacer;
use notagent_tui::components::text::Text;
use notagent_tui::keybindings::keybindings_match;
use notagent_tui::tui::{Component, ComponentRef, Container, component_ref};

use crate::core::trust_manager::{
    ProjectTrustOption, ProjectTrustStoreEntry, ProjectTrustUpdate, get_project_trust_options,
};
use crate::modes::interactive::theme::theme::{ThemeColor, theme};

use super::dynamic_border::DynamicBorder;
use super::keybinding_hints::{key_hint, raw_key_hint};

/// `Pick<ProjectTrustOption, "trusted" | "updates">`
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrustSelection {
    /// Whether the project is trusted.
    pub trusted: bool,
    /// The store writes the decision implies.
    pub updates: Vec<ProjectTrustUpdate>,
}

/// `TrustSelectorOptions`
pub struct TrustSelectorOptions {
    /// The project directory.
    pub cwd: String,
    /// The decision on file, if any.
    pub saved_decision: Option<ProjectTrustStoreEntry>,
    /// Whether the running session treats the project as trusted.
    pub project_trusted: bool,
    /// Invoked with the confirmed selection.
    pub on_select: Box<dyn FnMut(TrustSelection)>,
    /// Invoked on cancel.
    pub on_cancel: Box<dyn FnMut()>,
}

fn format_decision(trust_path: Option<&str>, decision: Option<&ProjectTrustStoreEntry>) -> String {
    let Some(decision) = decision else {
        return "none".to_string();
    };
    let label = if decision.decision {
        "trusted"
    } else {
        "untrusted"
    };
    if let Some(trust_path) = trust_path
        && decision.path != trust_path
    {
        return format!("{label} (inherited from {})", decision.path);
    }
    format!("{label} ({})", decision.path)
}

/// The project-trust dialog.
pub struct TrustSelectorComponent {
    container: Container,
    selected_index: usize,
    list_container: Rc<RefCell<Container>>,
    trust_options: Vec<ProjectTrustOption>,
    saved_decision: Option<ProjectTrustStoreEntry>,
    on_select_callback: Box<dyn FnMut(TrustSelection)>,
    on_cancel_callback: Box<dyn FnMut()>,
}

impl TrustSelectorComponent {
    /// New dialog for `options.cwd`.
    pub fn new(options: TrustSelectorOptions) -> Self {
        let theme_instance = theme();
        let saved_decision = options.saved_decision;
        let trust_options = get_project_trust_options(&options.cwd, false);
        // `Math.max(0, findIndex(...))` — no match falls back to the first option.
        let selected_index = trust_options
            .iter()
            .position(|option| is_saved_option(option, saved_decision.as_ref()))
            .unwrap_or(0);

        let mut container = Container::new();
        container.add_child(component_ref(DynamicBorder::new(None)));
        container.add_child(component_ref(Spacer::new(1)));
        container.add_child(component_ref(Text::new(
            theme_instance.fg(ThemeColor::Accent, &theme_instance.bold("Project trust")),
            1,
            0,
        )));
        container.add_child(component_ref(Text::new(
            theme_instance.fg(ThemeColor::Muted, &options.cwd),
            1,
            0,
        )));
        container.add_child(component_ref(Spacer::new(1)));
        container.add_child(component_ref(Text::new(
            theme_instance.fg(
                ThemeColor::Muted,
                &format!(
                    "Saved decision: {}",
                    format_decision(
                        trust_options
                            .first()
                            .and_then(|option| option.saved_path.as_deref()),
                        saved_decision.as_ref()
                    )
                ),
            ),
            1,
            0,
        )));
        container.add_child(component_ref(Text::new(
            theme_instance.fg(
                ThemeColor::Muted,
                &format!(
                    "Current session: {}",
                    if options.project_trusted {
                        "trusted"
                    } else {
                        "untrusted"
                    }
                ),
            ),
            1,
            0,
        )));
        container.add_child(component_ref(Spacer::new(1)));

        let list_container = Rc::new(RefCell::new(Container::new()));
        container.add_child(Rc::clone(&list_container) as ComponentRef);
        container.add_child(component_ref(Spacer::new(1)));
        container.add_child(component_ref(Text::new(
            raw_key_hint("↑↓", "navigate")
                + "  "
                + &key_hint("tui.select.confirm", "save")
                + "  "
                + &key_hint("tui.select.cancel", "cancel"),
            1,
            0,
        )));
        container.add_child(component_ref(Spacer::new(1)));
        container.add_child(component_ref(DynamicBorder::new(None)));

        let mut selector = Self {
            container,
            selected_index,
            list_container,
            trust_options,
            saved_decision,
            on_select_callback: options.on_select,
            on_cancel_callback: options.on_cancel,
        };
        selector.update_list();
        selector
    }

    fn update_list(&mut self) {
        let theme_instance = theme();
        let mut list_container = self.list_container.borrow_mut();
        list_container.clear();
        for (index, option) in self.trust_options.iter().enumerate() {
            let is_selected = index == self.selected_index;
            let is_current = is_saved_option(option, self.saved_decision.as_ref());
            let checkmark = if is_current {
                theme_instance.fg(ThemeColor::Success, " ✓")
            } else {
                String::new()
            };
            let prefix = if is_selected {
                theme_instance.fg(ThemeColor::Accent, "→ ")
            } else {
                "  ".to_string()
            };
            let label = if is_selected {
                theme_instance.fg(ThemeColor::Accent, &option.label)
            } else {
                theme_instance.fg(ThemeColor::Text, &option.label)
            };
            list_container.add_child(component_ref(Text::new(
                format!("{prefix}{label}{checkmark}"),
                1,
                0,
            )));
        }
    }
}

fn is_saved_option(option: &ProjectTrustOption, saved: Option<&ProjectTrustStoreEntry>) -> bool {
    let Some(saved_path) = option.saved_path.as_deref() else {
        return false;
    };
    saved.is_some_and(|saved| saved.decision == option.trusted && saved.path == saved_path)
}

impl Component for TrustSelectorComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.container.render(width)
    }

    fn invalidate(&mut self) {
        self.container.invalidate();
    }

    fn handle_input(&mut self, key_data: &str) {
        if keybindings_match(key_data, "tui.select.up") || key_data == "k" {
            self.selected_index = self.selected_index.saturating_sub(1);
            self.update_list();
        } else if keybindings_match(key_data, "tui.select.down") || key_data == "j" {
            self.selected_index =
                (self.selected_index + 1).min(self.trust_options.len().saturating_sub(1));
            self.update_list();
        } else if keybindings_match(key_data, "tui.select.confirm") || key_data == "\n" {
            if let Some(selected) = self.trust_options.get(self.selected_index) {
                let selection = TrustSelection {
                    trusted: selected.trusted,
                    updates: selected.updates.clone(),
                };
                (self.on_select_callback)(selection);
            }
        } else if keybindings_match(key_data, "tui.select.cancel") {
            (self.on_cancel_callback)();
        }
    }
}
