pub mod reminder;
pub mod render;

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

pub const TODO_STATUSES: [TodoStatus; 3] = [
    TodoStatus::Pending,
    TodoStatus::InProgress,
    TodoStatus::Completed,
];

impl TodoStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            TodoStatus::Pending => "pending",
            TodoStatus::InProgress => "in_progress",
            TodoStatus::Completed => "completed",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(TodoStatus::Pending),
            "in_progress" => Some(TodoStatus::InProgress),
            "completed" => Some(TodoStatus::Completed),
            _ => None,
        }
    }
}

/// Longest a task description may be, in either wording.
pub const MAX_TODO_LENGTH: usize = 1000;

/// One task.
/// Two wordings are required rather than derived. "Run tests" and "Running
/// tests" are what the list and the status line respectively need, and asking
/// the model for both is cheaper and more accurate than conjugating English.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Todo {
    pub content: String,
    /// The same task in present continuous form, shown while it runs.
    pub active_form: String,
    pub status: TodoStatus,
}

pub fn is_todo_active(todo: &Todo) -> bool {
    matches!(todo.status, TodoStatus::Pending | TodoStatus::InProgress)
}

/// Checks one replacement list.
/// Rejects rather than repairs: a task with an empty description is a bug in
/// the call, and silently dropping it would leave the model believing it
/// tracked something it did not.
/// Deviation (class 1): the ceiling counts characters where JS counts UTF-16
/// code units; the two differ only for astral characters.
pub fn validate_todo_items(items: &[Todo]) -> Result<(), String> {
    for item in items {
        if item.content.trim().is_empty() {
            return Err("Todo content cannot be empty".to_owned());
        }
        if item.content.chars().count() > MAX_TODO_LENGTH {
            return Err(format!(
                "Todo content exceeds maximum length of {MAX_TODO_LENGTH} characters"
            ));
        }
        if item.active_form.trim().is_empty() {
            return Err("Todo active form cannot be empty".to_owned());
        }
        if item.active_form.chars().count() > MAX_TODO_LENGTH {
            return Err(format!(
                "Todo active form exceeds maximum length of {MAX_TODO_LENGTH} characters"
            ));
        }
    }
    Ok(())
}

/// The stored list for one session.
/// Held here rather than derived from the transcript. Deriving it would mean
/// the list depends on a particular message surviving compaction, and
/// compaction is exactly when a long session most needs to still know what it
/// was doing.
#[derive(Debug, Default)]
pub struct TodoStore {
    todos: Vec<Todo>,
}

impl TodoStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// The whole list, in the order it was last given.
    pub fn all(&self) -> &[Todo] {
        &self.todos
    }

    /// Pending and in-progress items, which are what an unfinished turn owes.
    pub fn active(&self) -> Vec<Todo> {
        self.todos
            .iter()
            .filter(|todo| is_todo_active(todo))
            .cloned()
            .collect()
    }

    /// Replaces the stored list.
    /// Returns the replacement as it stands *before* the completed-list
    /// cleanup, so the caller can render the moment everything turned green.
    /// What is stored afterwards may be empty.
    pub fn replace(&mut self, items: Vec<Todo>) -> Result<Vec<Todo>, String> {
        validate_todo_items(&items)?;
        self.todos = items;
        let replacement = self.todos.clone();
        if !self.todos.is_empty()
            && self
                .todos
                .iter()
                .all(|todo| todo.status == TodoStatus::Completed)
        {
            self.todos.clear();
        }
        Ok(replacement)
    }

    /// Drops everything. Used when the conversation it belonged to is gone.
    pub fn clear(&mut self) {
        self.todos.clear();
    }
}

/// Pairs the entries of two lists by their text, in order.
/// Repeated descriptions are separate entries and must stay separate, so the
/// first "Run tests" in the new list pairs with the first in the old one rather
/// than with an arbitrary match. Returns, per position in `after`, the index it
/// paired with in `before`, plus the `before` positions left unpaired.
pub struct TodoPairing {
    pub paired_with: Vec<Option<usize>>,
    pub unpaired_before: Vec<usize>,
}

pub fn pair_todos_by_content(before: &[Todo], after: &[Todo]) -> TodoPairing {
    let mut queues: HashMap<&str, Vec<usize>> = HashMap::new();
    for (index, todo) in before.iter().enumerate() {
        queues.entry(&todo.content).or_default().push(index);
    }
    let mut paired_before = vec![false; before.len()];
    let paired_with = after
        .iter()
        .map(|todo| {
            let queue = queues.get_mut(todo.content.as_str())?;
            if queue.is_empty() {
                return None;
            }
            let index = queue.remove(0);
            paired_before[index] = true;
            Some(index)
        })
        .collect();
    let unpaired_before = paired_before
        .iter()
        .enumerate()
        .filter_map(|(index, paired)| (!paired).then_some(index))
        .collect();
    TodoPairing {
        paired_with,
        unpaired_before,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(content: &str, status: TodoStatus) -> Todo {
        Todo {
            content: content.to_owned(),
            active_form: format!("Doing {content}"),
            status,
        }
    }

    #[test]
    fn stores_what_it_was_given_in_the_order_it_was_given() {
        let mut store = TodoStore::new();
        let returned = store
            .replace(vec![
                item("Task A", TodoStatus::Pending),
                item("Task B", TodoStatus::InProgress),
            ])
            .expect("valid");
        assert_eq!(
            returned
                .iter()
                .map(|todo| todo.content.as_str())
                .collect::<Vec<_>>(),
            vec!["Task A", "Task B"]
        );
        assert_eq!(store.all(), returned.as_slice());
    }

    #[test]
    fn updates_the_status_of_an_item_whose_text_is_unchanged() {
        let mut store = TodoStore::new();
        store
            .replace(vec![item("Task A", TodoStatus::Pending)])
            .expect("valid");
        let returned = store
            .replace(vec![item("Task A", TodoStatus::InProgress)])
            .expect("valid");
        assert_eq!(returned, vec![item("Task A", TodoStatus::InProgress)]);
    }

    #[test]
    fn keeps_duplicate_descriptions_as_separate_entries_in_order() {
        let mut store = TodoStore::new();
        store
            .replace(vec![
                item("Repeated", TodoStatus::Pending),
                item("Other", TodoStatus::Pending),
                item("Repeated", TodoStatus::InProgress),
            ])
            .expect("valid");
        let replacement = vec![
            item("Repeated", TodoStatus::Completed),
            item("Repeated", TodoStatus::Pending),
            item("Other", TodoStatus::InProgress),
        ];
        let returned = store.replace(replacement.clone()).expect("valid");
        assert_eq!(returned, replacement);
    }

    #[test]
    fn removes_what_the_next_call_leaves_out() {
        let mut store = TodoStore::new();
        store
            .replace(vec![
                item("Task A", TodoStatus::Pending),
                item("Task B", TodoStatus::Pending),
            ])
            .expect("valid");
        let returned = store
            .replace(vec![item("Task A", TodoStatus::InProgress)])
            .expect("valid");
        assert_eq!(returned.len(), 1);
        assert_eq!(returned[0].content, "Task A");
        assert_eq!(store.all().len(), 1);
    }

    #[test]
    fn clears_the_list_on_an_empty_replacement() {
        let mut store = TodoStore::new();
        store
            .replace(vec![item("Task A", TodoStatus::InProgress)])
            .expect("valid");
        assert_eq!(store.replace(Vec::new()).expect("valid"), Vec::new());
        assert_eq!(store.all(), &[] as &[Todo]);
    }

    #[test]
    fn reports_an_all_completed_list_once_and_then_forgets_it() {
        let mut store = TodoStore::new();
        store
            .replace(vec![
                item("Task A", TodoStatus::InProgress),
                item("Task B", TodoStatus::Pending),
            ])
            .expect("valid");
        let returned = store
            .replace(vec![
                item("Task A", TodoStatus::Completed),
                item("Task B", TodoStatus::Completed),
            ])
            .expect("valid");
        // The caller still gets the completed state to render …
        assert_eq!(
            returned,
            vec![
                item("Task A", TodoStatus::Completed),
                item("Task B", TodoStatus::Completed),
            ]
        );
        // … but finished work does not follow the user into the next request.
        assert_eq!(store.all(), &[] as &[Todo]);
    }

    #[test]
    fn preserves_a_large_replacements_order_and_statuses() {
        let mut store = TodoStore::new();
        store
            .replace(
                (0..15)
                    .map(|index| item(&format!("Task {index}"), TodoStatus::Pending))
                    .collect(),
            )
            .expect("valid");
        let replacement: Vec<Todo> = (0..15)
            .map(|index| {
                let position = 14 - index;
                let status = match position % 3 {
                    0 => TodoStatus::Completed,
                    1 => TodoStatus::InProgress,
                    _ => TodoStatus::Pending,
                };
                item(&format!("Task {position}"), status)
            })
            .collect();
        let returned = store.replace(replacement.clone()).expect("valid");
        assert_eq!(returned, replacement);
    }

    #[test]
    fn refuses_an_empty_description() {
        let mut store = TodoStore::new();
        let error = store
            .replace(vec![item("   ", TodoStatus::Pending)])
            .expect_err("refused");
        assert!(error.contains("content cannot be empty"), "{error}");
    }

    #[test]
    fn refuses_an_empty_active_form() {
        let mut store = TodoStore::new();
        let error = store
            .replace(vec![Todo {
                content: "Task".to_owned(),
                active_form: " ".to_owned(),
                status: TodoStatus::Pending,
            }])
            .expect_err("refused");
        assert!(error.contains("active form cannot be empty"), "{error}");
    }

    #[test]
    fn refuses_a_description_past_the_ceiling() {
        let mut store = TodoStore::new();
        let error = store
            .replace(vec![item(
                &"x".repeat(MAX_TODO_LENGTH + 1),
                TodoStatus::Pending,
            )])
            .expect_err("refused");
        assert!(error.contains("maximum length"), "{error}");
    }

    #[test]
    fn leaves_the_stored_list_untouched_by_a_refusal() {
        let mut store = TodoStore::new();
        store
            .replace(vec![item("Task A", TodoStatus::Pending)])
            .expect("valid");
        store
            .replace(vec![item("", TodoStatus::Pending)])
            .expect_err("refused");
        assert_eq!(store.all(), &[item("Task A", TodoStatus::Pending)]);
    }
}
