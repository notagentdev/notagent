//! 1:1 port of
//! `packages/coding-agent/src/modes/interactive/components/user-message-selector.ts` (155 LOC).

use std::cell::RefCell;
use std::rc::Rc;

use notagent_tui::components::spacer::Spacer;
use notagent_tui::components::text::Text;
use notagent_tui::keybindings::keybindings_match;
use notagent_tui::tui::{Component, ComponentRef, Container, component_ref};
use notagent_tui::utils::truncate_to_width;

use crate::modes::interactive::theme::theme::{ThemeColor, theme};

use super::dynamic_border::DynamicBorder;

/// One selectable user message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserMessageItem {
    /// Entry ID in the session
    pub id: String,
    /// The message text
    pub text: String,
    /// Optional timestamp if available
    pub timestamp: Option<String>,
}

/// Invoked with the entry id of the confirmed message.
pub type UserMessageSelectCallback = Box<dyn FnMut(&str)>;

/// Custom user message list component with selection
pub struct UserMessageList {
    messages: Vec<UserMessageItem>,
    selected_index: usize,
    /// Invoked with the entry id of the confirmed message.
    pub on_select: Option<UserMessageSelectCallback>,
    /// Invoked on cancel.
    pub on_cancel: Option<Box<dyn FnMut()>>,
    /// Max messages visible
    max_visible: usize,
}

impl UserMessageList {
    /// New list; selection starts at `initial_selected_id` or the newest message.
    pub fn new(messages: Vec<UserMessageItem>, initial_selected_id: Option<&str>) -> Self {
        // Store messages in chronological order (oldest to newest)
        let initial_index =
            initial_selected_id.and_then(|id| messages.iter().position(|message| message.id == id));
        // Start with selected message if provided, else default to the most recent
        let selected_index = initial_index.unwrap_or_else(|| messages.len().saturating_sub(1));
        Self {
            messages,
            selected_index,
            on_select: None,
            on_cancel: None,
            max_visible: 10,
        }
    }
}

impl Component for UserMessageList {
    fn invalidate(&mut self) {
        // No cached state to invalidate currently
    }

    fn render(&mut self, width: usize) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();
        let theme_instance = theme();

        if self.messages.is_empty() {
            lines.push(theme_instance.fg(ThemeColor::Muted, "  No user messages found"));
            return lines;
        }

        // Calculate visible range with scrolling
        let centered = (self.selected_index as i64) - (self.max_visible as i64) / 2;
        let last_page = (self.messages.len() as i64) - (self.max_visible as i64);
        let start_index = centered.min(last_page).max(0) as usize;
        let end_index = (start_index + self.max_visible).min(self.messages.len());

        // Render visible messages (2 lines per message + blank line)
        for index in start_index..end_index {
            let message = &self.messages[index];
            let is_selected = index == self.selected_index;

            // Normalize message to single line
            let normalized_message = message.text.replace('\n', " ").trim().to_string();

            // First line: cursor + message
            let cursor = if is_selected {
                theme_instance.fg(ThemeColor::Accent, "› ")
            } else {
                "  ".to_string()
            };
            let max_msg_width = width.saturating_sub(2); // Account for cursor (2 chars)
            let truncated_msg = truncate_to_width(&normalized_message, max_msg_width);
            let message_line = if is_selected {
                cursor + &theme_instance.bold(&truncated_msg)
            } else {
                cursor + &truncated_msg
            };

            lines.push(message_line);

            // Second line: metadata (position in history)
            let position = index + 1;
            let metadata = format!("  Message {position} of {}", self.messages.len());
            lines.push(theme_instance.fg(ThemeColor::Muted, &metadata));
            lines.push(String::new()); // Blank line between messages
        }

        // Add scroll indicator if needed
        if start_index > 0 || end_index < self.messages.len() {
            lines.push(theme_instance.fg(
                ThemeColor::Muted,
                &format!("  ({}/{})", self.selected_index + 1, self.messages.len()),
            ));
        }

        lines
    }

    fn handle_input(&mut self, key_data: &str) {
        // Up arrow - go to previous (older) message, wrap to bottom when at top
        if keybindings_match(key_data, "tui.select.up") {
            self.selected_index = if self.selected_index == 0 {
                self.messages.len().saturating_sub(1)
            } else {
                self.selected_index - 1
            };
        }
        // Down arrow - go to next (newer) message, wrap to top when at bottom
        else if keybindings_match(key_data, "tui.select.down") {
            self.selected_index = if self.selected_index == self.messages.len().saturating_sub(1) {
                0
            } else {
                self.selected_index + 1
            };
        }
        // Enter - select message and branch
        else if keybindings_match(key_data, "tui.select.confirm") {
            if let Some(selected) = self.messages.get(self.selected_index) {
                let id = selected.id.clone();
                if let Some(on_select) = self.on_select.as_mut() {
                    on_select(&id);
                }
            }
        }
        // Escape - cancel
        else if keybindings_match(key_data, "tui.select.cancel")
            && let Some(on_cancel) = self.on_cancel.as_mut()
        {
            on_cancel();
        }
    }
}

/// Component that renders a user message selector for branching
pub struct UserMessageSelectorComponent {
    container: Container,
    message_list: Rc<RefCell<UserMessageList>>,
    /// `true` when the constructor found no messages; TypeScript schedules an
    /// auto-cancel with `setTimeout(..., 100)`, which the caller drives here.
    empty: bool,
}

impl UserMessageSelectorComponent {
    /// New selector over `messages`.
    pub fn new(
        messages: Vec<UserMessageItem>,
        on_select: UserMessageSelectCallback,
        on_cancel: Box<dyn FnMut()>,
        initial_selected_id: Option<&str>,
    ) -> Self {
        let mut container = Container::new();
        let theme_instance = theme();

        // Add header
        container.add_child(component_ref(Spacer::new(1)));
        container.add_child(component_ref(Text::new(
            theme_instance.bold("Fork from Message"),
            1,
            0,
        )));
        container.add_child(component_ref(Text::new(
            theme_instance.fg(
                ThemeColor::Muted,
                "Select a user message to copy the active path up to that point into a new session",
            ),
            1,
            0,
        )));
        container.add_child(component_ref(Spacer::new(1)));
        container.add_child(component_ref(DynamicBorder::new(None)));
        container.add_child(component_ref(Spacer::new(1)));

        // Create message list
        let empty = messages.is_empty();
        let mut message_list = UserMessageList::new(messages, initial_selected_id);
        message_list.on_select = Some(on_select);
        message_list.on_cancel = Some(on_cancel);
        let message_list = Rc::new(RefCell::new(message_list));

        container.add_child(Rc::clone(&message_list) as ComponentRef);

        // Add bottom border
        container.add_child(component_ref(Spacer::new(1)));
        container.add_child(component_ref(DynamicBorder::new(None)));

        Self {
            container,
            message_list,
            empty,
        }
    }

    /// Whether the selector opened without any messages.
    pub fn is_empty(&self) -> bool {
        self.empty
    }

    /// The wrapped list, which the caller focuses.
    pub fn get_message_list(&self) -> Rc<RefCell<UserMessageList>> {
        Rc::clone(&self.message_list)
    }
}

impl Component for UserMessageSelectorComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.container.render(width)
    }

    fn handle_input(&mut self, data: &str) {
        self.message_list.borrow_mut().handle_input(data);
    }

    fn invalidate(&mut self) {
        self.container.invalidate();
    }
}
