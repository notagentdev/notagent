use notagent_agent::types::AgentMessage;
use notagent_ai::types::{TextContent, TextOrImageContent, UserContent};
use serde::{Deserialize, Serialize};

use crate::core::todos::{Todo, TodoStatus, is_todo_active};

/// Marks the message this module creates, in the transcript and in the UI.
pub const TODO_REMINDER_TYPE: &str = "pending_todos";

/// The open items one reminder was raised for, in order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoReminderDetails {
    pub contents: Vec<String>,
}

fn status_label(status: TodoStatus) -> &'static str {
    match status {
        TodoStatus::Pending => "PENDING",
        TodoStatus::InProgress => "IN_PROGRESS",
        // `status.toUpperCase()`, which no live reminder ever reaches because
        // completed items are filtered out first.
        TodoStatus::Completed => "COMPLETED",
    }
}

/// The text handed back to the model.
pub fn render_pending_todos_reminder(active: &[Todo]) -> String {
    let lines = active
        .iter()
        .filter(|todo| is_todo_active(todo))
        .map(|todo| format!("- [{}] {}", status_label(todo.status), todo.content));
    let mut out = vec![
        "<system_reminder>".to_string(),
        "You have pending todo items that must be completed before finishing the task:".to_string(),
        String::new(),
    ];
    out.extend(lines);
    out.push(String::new());
    out.push("Please complete all pending items before finishing.".to_string());
    out.push("</system_reminder>".to_string());
    out.join("\n")
}

/// The message this module builds, minus the fields the caller stamps.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingTodosReminder {
    pub custom_type: String,
    pub content: UserContent,
    pub display: bool,
    pub details: TodoReminderDetails,
}

/// Builds the message, or nothing when the same set was already reminded of.
pub fn build_pending_todos_reminder(
    active: &[Todo],
    transcript: &[AgentMessage],
) -> Option<PendingTodosReminder> {
    let open: Vec<&Todo> = active.iter().filter(|todo| is_todo_active(todo)).collect();
    if open.is_empty() {
        return None;
    }

    let contents: Vec<String> = open.iter().map(|todo| todo.content.clone()).collect();
    if same_as_last_reminder(&contents, transcript) {
        return None;
    }

    let owned: Vec<Todo> = open.into_iter().cloned().collect();
    Some(PendingTodosReminder {
        custom_type: TODO_REMINDER_TYPE.to_string(),
        content: UserContent::Blocks(vec![TextOrImageContent::Text(TextContent::new(
            render_pending_todos_reminder(&owned),
        ))]),
        // Hidden: it is addressed to the model, and the user is already looking
        // at the same open items in the panel above the editor.
        display: false,
        details: TodoReminderDetails { contents },
    })
}

/// Whether the most recent reminder covered exactly these items.
/// Only the most recent one is consulted, deliberately. An older reminder for
/// the same set is not evidence the model has seen it recently — the set having
/// survived a further attempt to stop is what matters.
fn same_as_last_reminder(contents: &[String], transcript: &[AgentMessage]) -> bool {
    for message in transcript.iter().rev() {
        let AgentMessage::Custom(message) = message else {
            continue;
        };
        if message.custom_type != TODO_REMINDER_TYPE {
            continue;
        }
        let Some(details) = message.details.as_ref() else {
            return false;
        };
        let Some(previous) = details.get("contents").and_then(|value| value.as_array()) else {
            return false;
        };
        if previous.len() != contents.len() {
            return false;
        }
        let mut left: Vec<String> = previous
            .iter()
            .map(|value| match value.as_str() {
                Some(text) => text.to_string(),
                None => value.to_string(),
            })
            .collect();
        let mut right: Vec<String> = contents.to_vec();
        left.sort();
        right.sort();
        return left == right;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use notagent_agent::harness::messages::create_custom_message;
    use serde_json::json;

    fn item(content: &str, status: TodoStatus) -> Todo {
        Todo {
            content: content.to_string(),
            active_form: format!("Doing {content}"),
            status,
        }
    }

    /// The transcript shape the reminder consults: previous reminders, each
    /// carrying the set of open items it was raised for.
    fn transcript(contents: Option<&[&str]>) -> Vec<AgentMessage> {
        match contents {
            None => Vec::new(),
            Some(contents) => vec![AgentMessage::Custom(create_custom_message(
                TODO_REMINDER_TYPE,
                UserContent::Blocks(vec![TextOrImageContent::Text(TextContent::new(""))]),
                false,
                Some(json!({ "contents": contents })),
                0,
            ))],
        }
    }

    fn text_of(reminder: &PendingTodosReminder) -> String {
        match &reminder.content {
            UserContent::Text(text) => text.clone(),
            UserContent::Blocks(blocks) => blocks
                .iter()
                .filter_map(|block| match block {
                    TextOrImageContent::Text(text) => Some(text.text.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(""),
        }
    }

    #[test]
    fn is_raised_when_items_are_still_open() {
        let reminder = build_pending_todos_reminder(
            &[item("Task A", TodoStatus::InProgress)],
            &transcript(None),
        )
        .expect("reminder");
        assert!(text_of(&reminder).contains("[IN_PROGRESS] Task A"));
    }

    #[test]
    fn is_not_raised_when_everything_is_done() {
        assert!(
            build_pending_todos_reminder(
                &[item("Task A", TodoStatus::Completed)],
                &transcript(None)
            )
            .is_none()
        );
    }

    #[test]
    fn is_not_repeated_for_the_same_set_of_open_items() {
        assert!(
            build_pending_todos_reminder(
                &[item("Task A", TodoStatus::Pending)],
                &transcript(Some(&["Task A"]))
            )
            .is_none()
        );
    }

    #[test]
    fn is_raised_again_once_the_set_of_open_items_changes() {
        assert!(
            build_pending_todos_reminder(
                &[
                    item("Task A", TodoStatus::Pending),
                    item("Task B", TodoStatus::Pending),
                ],
                &transcript(Some(&["Task A"]))
            )
            .is_some()
        );
    }

    #[test]
    fn stays_out_of_the_transcript_being_addressed_to_the_model() {
        let reminder =
            build_pending_todos_reminder(&[item("Task A", TodoStatus::Pending)], &transcript(None))
                .expect("reminder");
        assert!(!reminder.display);
    }

    /// Only the most recent reminder counts: an older one for the same set is
    /// not evidence the model has seen it since.
    #[test]
    fn consults_only_the_most_recent_reminder() {
        let mut messages = transcript(Some(&["Task A"]));
        messages.extend(transcript(Some(&["Task B"])));
        assert!(
            build_pending_todos_reminder(&[item("Task A", TodoStatus::Pending)], &messages)
                .is_some()
        );
    }

    #[test]
    fn renders_both_open_states_and_leaves_completed_items_out() {
        let text = render_pending_todos_reminder(&[
            item("Task A", TodoStatus::Pending),
            item("Task B", TodoStatus::InProgress),
            item("Task C", TodoStatus::Completed),
        ]);
        assert!(text.contains("- [PENDING] Task A"));
        assert!(text.contains("- [IN_PROGRESS] Task B"));
        assert!(!text.contains("Task C"));
        assert!(text.starts_with("<system_reminder>"));
        assert!(text.ends_with("</system_reminder>"));
    }
}
