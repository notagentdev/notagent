//! 1:1 port of
//! `packages/coding-agent/src/modes/interactive/components/extension-selector.ts` (112 LOC).
//!
//! Renamed to `list_selector` per interface request C-5: despite its name the
//! component is a generic list selector that four core call sites use
//! (`interactive-mode.ts:2434,5579,5862`, `cli/startup-ui.ts:152`).

use std::cell::RefCell;
use std::rc::Rc;

use notagent_tui::components::spacer::Spacer;
use notagent_tui::components::text::Text;
use notagent_tui::keybindings::keybindings_match;
use notagent_tui::tui::{Component, ComponentRef, Container, Line, component_ref};

use crate::modes::interactive::theme::theme::{ThemeColor, theme};

use super::countdown_timer::CountdownTimer;
use super::dynamic_border::DynamicBorder;
use super::keybinding_hints::{key_hint, raw_key_hint};

/// `ExtensionSelectorOptions` without the `tui` handle, which the polled
/// countdown no longer needs.
#[derive(Default)]
pub struct ListSelectorOptions {
    /// Auto-cancel after this many milliseconds.
    pub timeout: Option<u64>,
    /// Invoked for `app.tools.expand`.
    pub on_toggle_tools_expanded: Option<Box<dyn FnMut()>>,
}

/// Generic selector component.
/// Displays a list of string options with keyboard navigation.
pub struct ListSelectorComponent {
    container: Container,
    options: Vec<String>,
    selected_index: usize,
    list_container: Rc<RefCell<Container>>,
    title_text: Rc<RefCell<Text>>,
    base_title: String,
    countdown: Option<CountdownTimer>,
    on_select: Box<dyn FnMut(String)>,
    on_cancel: Box<dyn FnMut()>,
    on_toggle_tools_expanded: Option<Box<dyn FnMut()>>,
}

impl ListSelectorComponent {
    /// New selector over `options`.
    pub fn new(
        title: impl Into<String>,
        options: Vec<String>,
        on_select: Box<dyn FnMut(String)>,
        on_cancel: Box<dyn FnMut()>,
        opts: Option<ListSelectorOptions>,
    ) -> Self {
        let title = title.into();
        let opts = opts.unwrap_or_default();
        let mut container = Container::new();

        container.add_child(component_ref(DynamicBorder::new(None)));
        container.add_child(component_ref(Spacer::new(1)));

        let theme_instance = theme();
        let title_text = Rc::new(RefCell::new(Text::new(
            theme_instance.fg(ThemeColor::Accent, &theme_instance.bold(&title)),
            1,
            0,
        )));
        container.add_child(Rc::clone(&title_text) as ComponentRef);
        container.add_child(component_ref(Spacer::new(1)));

        let countdown = match opts.timeout {
            Some(timeout) if timeout > 0 => Some(CountdownTimer::new(timeout)),
            _ => None,
        };

        let list_container = Rc::new(RefCell::new(Container::new()));
        container.add_child(Rc::clone(&list_container) as ComponentRef);
        container.add_child(component_ref(Spacer::new(1)));
        container.add_child(component_ref(Text::new(
            format!(
                "{}  {}  {}",
                raw_key_hint("↑↓", "navigate"),
                key_hint("tui.select.confirm", "select"),
                key_hint("tui.select.cancel", "cancel")
            ),
            1,
            0,
        )));
        container.add_child(component_ref(Spacer::new(1)));
        container.add_child(component_ref(DynamicBorder::new(None)));

        let mut selector = Self {
            container,
            options,
            selected_index: 0,
            list_container,
            title_text,
            base_title: title,
            countdown,
            on_select,
            on_cancel,
            on_toggle_tools_expanded: opts.on_toggle_tools_expanded,
        };
        if selector.countdown.is_some() {
            selector.update_title_countdown();
        }
        selector.update_list();
        selector
    }

    /// When the countdown is next due.
    pub fn countdown_deadline(&self) -> Option<std::time::Instant> {
        self.countdown.as_ref().and_then(CountdownTimer::deadline)
    }

    /// Advance the countdown; `true` when the title changed and the caller must
    /// request a render.
    pub fn tick_countdown(&mut self) -> bool {
        let Some(countdown) = self.countdown.as_mut() else {
            return false;
        };
        let Some(tick) = countdown.tick() else {
            return false;
        };
        self.update_title_countdown();
        if tick.expired {
            self.countdown = None;
            (self.on_cancel)();
        }
        true
    }

    fn update_title_countdown(&mut self) {
        let Some(countdown) = self.countdown.as_ref() else {
            return;
        };
        let seconds = countdown.remaining_seconds();
        let theme_instance = theme();
        self.title_text.borrow_mut().set_text(theme_instance.fg(
            ThemeColor::Accent,
            &theme_instance.bold(&format!("{} ({seconds}s)", self.base_title)),
        ));
    }

    fn update_list(&mut self) {
        let theme_instance = theme();
        let mut list_container = self.list_container.borrow_mut();
        list_container.clear();
        for (index, option) in self.options.iter().enumerate() {
            let is_selected = index == self.selected_index;
            let text = if is_selected {
                theme_instance.fg(ThemeColor::Accent, "→ ")
                    + &theme_instance.fg(ThemeColor::Accent, option)
            } else {
                format!("  {}", theme_instance.fg(ThemeColor::Text, option))
            };
            list_container.add_child(component_ref(Text::new(text, 1, 0)));
        }
    }

    /// Stop the countdown.
    pub fn dispose(&mut self) {
        if let Some(countdown) = self.countdown.as_mut() {
            countdown.dispose();
        }
    }
}

impl Component for ListSelectorComponent {
    fn render(&mut self, width: usize) -> Vec<Line> {
        self.container.render(width)
    }

    fn handle_input(&mut self, key_data: &str) {
        if keybindings_match(key_data, "app.tools.expand") {
            if let Some(on_toggle) = self.on_toggle_tools_expanded.as_mut() {
                on_toggle();
            }
        } else if keybindings_match(key_data, "tui.select.up") || key_data == "k" {
            self.selected_index = self.selected_index.saturating_sub(1);
            self.update_list();
        } else if keybindings_match(key_data, "tui.select.down") || key_data == "j" {
            self.selected_index =
                (self.selected_index + 1).min(self.options.len().saturating_sub(1));
            self.update_list();
        } else if keybindings_match(key_data, "tui.select.confirm") || key_data == "\n" {
            if let Some(selected) = self.options.get(self.selected_index).cloned()
                && !selected.is_empty()
            {
                (self.on_select)(selected);
            }
        } else if keybindings_match(key_data, "tui.select.cancel") {
            (self.on_cancel)();
        }
    }

    fn invalidate(&mut self) {
        self.container.invalidate();
    }
}
