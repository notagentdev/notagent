//! Port of `packages/coding-agent/src/core/todos/render.ts`.
//!
//! What a task-list update looks like to the model.
//!
//! Two things are reported, not one. The **changes** say what moved — added,
//! updated with both statuses, removed — because that is what the model needs
//! to confirm its own call landed. The **current** list says what now stands, so
//! a model reading only the last tool result still knows the whole state and
//! does not have to reconstruct it from a chain of diffs.
//!
//! The XML shape is the reference's, verbatim down to the attribute layout: a
//! supervisor or a transcript reader built against the Rust version recognises
//! this output unchanged.

use super::{Todo, TodoStatus, pair_todos_by_content};

/// Renders one element the way the reference's `Element` does.
///
/// Attributes each go on their own indented line and the closing bracket gets a
/// line of its own; an element with neither attributes nor children closes
/// inline. This is copied rather than improved on purpose — the output is a
/// public interface.
fn render_element(
    name: &str,
    attributes: &[(&str, String)],
    text: Option<&str>,
    children: &[String],
) -> String {
    let mut result = String::new();
    if attributes.is_empty() {
        result.push_str(&format!("<{name}>"));
    } else {
        result.push_str(&format!("<{name}"));
        for (key, value) in attributes {
            result.push_str(&format!("\n  {key}=\"{value}\""));
        }
        result.push_str("\n>");
    }
    if let Some(text) = text {
        result.push_str(text);
    }
    for child in children {
        result.push('\n');
        result.push_str(child);
    }
    if children.is_empty() && attributes.is_empty() {
        result.push_str(&format!("</{name}>"));
    } else {
        result.push_str(&format!("\n</{name}>"));
    }
    result
}

/// How one entry differs from the list it replaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TodoChange {
    Added,
    Updated,
    Removed,
}

impl TodoChange {
    fn as_str(self) -> &'static str {
        match self {
            TodoChange::Added => "added",
            TodoChange::Updated => "updated",
            TodoChange::Removed => "removed",
        }
    }
}

/// The model-facing result of one replacement.
pub fn render_todos_updated(before: &[Todo], after: &[Todo]) -> String {
    let pairing = pair_todos_by_content(before, after);

    let mut change_elements: Vec<String> = Vec::new();
    for (index, todo) in after.iter().enumerate() {
        let previous = pairing.paired_with[index].and_then(|index| before.get(index));
        let change = match previous {
            None => Some(TodoChange::Added),
            Some(previous) if previous.status != todo.status => Some(TodoChange::Updated),
            Some(_) => None,
        };
        let Some(change) = change else { continue };
        let mut attributes: Vec<(&str, String)> = vec![
            ("status", todo.status.as_str().to_owned()),
            ("change", change.as_str().to_owned()),
        ];
        if let Some(previous) = previous {
            attributes.push(("prev_status", previous.status.as_str().to_owned()));
            attributes.push(("new_status", todo.status.as_str().to_owned()));
        }
        change_elements.push(render_element(
            "todo",
            &attributes,
            Some(&todo.content),
            &[],
        ));
    }
    for index in &pairing.unpaired_before {
        let Some(todo) = before.get(*index) else {
            continue;
        };
        change_elements.push(render_element(
            "todo",
            &[
                ("status", todo.status.as_str().to_owned()),
                ("change", TodoChange::Removed.as_str().to_owned()),
            ],
            Some(&todo.content),
            &[],
        ));
    }

    let current_elements: Vec<String> = after
        .iter()
        .map(|todo| {
            render_element(
                "todo",
                &[("status", todo.status.as_str().to_owned())],
                Some(&todo.content),
                &[],
            )
        })
        .collect();

    render_element(
        "todos_updated",
        &[("changes", change_elements.len().to_string())],
        None,
        &[
            render_element("changes", &[], None, &change_elements),
            render_element(
                "current",
                &[("count", after.len().to_string())],
                None,
                &current_elements,
            ),
        ],
    )
}

/// The icons the transcript uses.
///
/// Deliberately different from the panel's: a transcript line is a record of a
/// moment and reads better with checkbox glyphs, while the live panel uses
/// filled shapes that carry at a glance.
fn transcript_icon(status: TodoStatus) -> &'static str {
    match status {
        TodoStatus::Completed => "\u{f0135}",
        TodoStatus::InProgress => "\u{f0117}",
        TodoStatus::Pending => "\u{f0131}",
    }
}

/// `removed` items were dropped by this call; the rest survive it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TodoDiffKind {
    Kept,
    Changed,
    Added,
    Removed,
}

/// One line of the transcript rendering, before any styling is applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodoDiffLine {
    pub todo: Todo,
    pub icon: &'static str,
    pub kind: TodoDiffKind,
}

/// The lines a transcript shows for one replacement.
///
/// Walks the previous list in its own order so surviving items keep their
/// place, then appends what is new. An item that was dropped is still shown,
/// struck through — a task disappearing without a trace is the one case where
/// the list silently loses information the user cared about.
pub fn build_todo_diff_lines(before: &[Todo], after: &[Todo]) -> Vec<TodoDiffLine> {
    let pairing = pair_todos_by_content(before, after);
    let mut after_for_before: Vec<Option<usize>> = vec![None; before.len()];
    for (after_index, before_index) in pairing.paired_with.iter().enumerate() {
        if let Some(before_index) = before_index {
            after_for_before[*before_index] = Some(after_index);
        }
    }

    let mut lines: Vec<TodoDiffLine> = Vec::new();
    for (before_index, before_todo) in before.iter().enumerate() {
        let Some(after_index) = after_for_before[before_index] else {
            lines.push(TodoDiffLine {
                todo: before_todo.clone(),
                icon: transcript_icon(before_todo.status),
                kind: TodoDiffKind::Removed,
            });
            continue;
        };
        let Some(after_todo) = after.get(after_index) else {
            continue;
        };
        lines.push(TodoDiffLine {
            todo: after_todo.clone(),
            icon: transcript_icon(after_todo.status),
            kind: if before_todo.status == after_todo.status {
                TodoDiffKind::Kept
            } else {
                TodoDiffKind::Changed
            },
        });
    }
    for (index, todo) in after.iter().enumerate() {
        if pairing.paired_with[index].is_some() {
            continue;
        }
        lines.push(TodoDiffLine {
            todo: todo.clone(),
            icon: transcript_icon(todo.status),
            kind: TodoDiffKind::Added,
        });
    }
    lines
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
    fn reports_an_added_item_as_added() {
        let output = render_todos_updated(&[], &[item("Task A", TodoStatus::Pending)]);
        assert!(output.contains("change=\"added\""), "{output}");
        assert!(output.contains("changes=\"1\""), "{output}");
    }

    #[test]
    fn reports_a_status_move_with_both_statuses() {
        let output = render_todos_updated(
            &[item("Task A", TodoStatus::Pending)],
            &[item("Task A", TodoStatus::InProgress)],
        );
        assert!(output.contains("change=\"updated\""), "{output}");
        assert!(output.contains("prev_status=\"pending\""), "{output}");
        assert!(output.contains("new_status=\"in_progress\""), "{output}");
    }

    #[test]
    fn reports_an_omitted_item_as_removed() {
        let output = render_todos_updated(&[item("Task A", TodoStatus::Pending)], &[]);
        assert!(output.contains("change=\"removed\""), "{output}");
    }

    #[test]
    fn says_nothing_changed_when_nothing_did() {
        let output = render_todos_updated(
            &[item("Task A", TodoStatus::Pending)],
            &[item("Task A", TodoStatus::Pending)],
        );
        assert!(output.contains("changes=\"0\""), "{output}");
    }

    #[test]
    fn always_carries_the_whole_current_list() {
        let output = render_todos_updated(
            &[item("Task A", TodoStatus::Pending)],
            &[
                item("Task A", TodoStatus::Completed),
                item("Task B", TodoStatus::Pending),
            ],
        );
        assert!(output.contains("<current\n  count=\"2\""), "{output}");
        assert!(output.contains("Task A"), "{output}");
        assert!(output.contains("Task B"), "{output}");
    }

    #[test]
    fn keeps_surviving_items_in_the_previous_order_and_appends_what_is_new() {
        let lines = build_todo_diff_lines(
            &[
                item("First", TodoStatus::Pending),
                item("Second", TodoStatus::InProgress),
            ],
            &[
                item("Second", TodoStatus::Completed),
                item("First", TodoStatus::Pending),
                item("Third", TodoStatus::Pending),
            ],
        );
        assert_eq!(
            lines
                .iter()
                .map(|line| (line.todo.content.as_str(), line.kind))
                .collect::<Vec<_>>(),
            vec![
                ("First", TodoDiffKind::Kept),
                ("Second", TodoDiffKind::Changed),
                ("Third", TodoDiffKind::Added),
            ]
        );
    }

    #[test]
    fn still_shows_an_item_that_was_dropped() {
        let lines = build_todo_diff_lines(&[item("Cancelled", TodoStatus::InProgress)], &[]);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].kind, TodoDiffKind::Removed);
        // Its own status icon, not the pending one — what it was still matters.
        assert_eq!(lines[0].icon, "\u{f0117}");
    }
}
