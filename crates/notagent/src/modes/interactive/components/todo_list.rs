//! The task list above the editor.
//!
//! 1:1 port of
//! `packages/coding-agent/src/modes/interactive/components/todo-list.ts` (216 LOC).
//!
//! Two behaviours here are worth stating because both were chosen against the
//! obvious alternative.
//!
//! The window is anchored on the task being worked on, as the *last* visible
//! row. A list longer than the screen otherwise shows its beginning, which is
//! finished work — the one part nobody needs. Anchoring on the active task puts
//! what is happening now at the bottom edge, with its immediate history above
//! it, which is how a person reads a checklist they are working through.
//!
//! And a list that has just gone all-green stays up for five seconds before it
//! disappears. Vanishing at the instant of completion denies the user the one
//! moment the list existed for.

use std::time::{Duration, Instant};

use notagent_tui::tui::{Component, Line};
use notagent_tui::utils::truncate_to_width_opts;

use crate::core::todos::{Todo, TodoStatus};
use crate::modes::interactive::theme::theme::{ThemeColor, theme};

/// How long an all-completed list stays up before it hides itself.
pub const TODO_COMPLETED_HIDE_DELAY_MS: u64 = 5_000;

/// `TodoStatusCounts`
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TodoStatusCounts {
    /// Tasks that are done.
    pub completed: usize,
    /// The task being worked on.
    pub in_progress: usize,
    /// Tasks not started yet.
    pub pending: usize,
}

/// `TodoDisplay`
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TodoDisplay {
    /// The window of the list, in the list's own order.
    pub visible: Vec<Todo>,
    /// How many of the list fell outside the window.
    pub hidden: usize,
    /// Counts over the whole list, not just the window.
    pub counts: TodoStatusCounts,
}

/// How many rows the list may occupy.
///
/// Nothing at all below eleven rows: on a terminal that small the list would
/// crowd out the conversation it is describing. Above that it grows to ten,
/// always leaving fourteen rows for everything else.
pub fn todo_display_limit(terminal_rows: usize) -> usize {
    if terminal_rows <= 10 {
        return 0;
    }
    10.min(terminal_rows.saturating_sub(14).max(3))
}

/// Count the list by status.
pub fn count_todos(todos: &[Todo]) -> TodoStatusCounts {
    let mut counts = TodoStatusCounts::default();
    for todo in todos {
        match todo.status {
            TodoStatus::Completed => counts.completed += 1,
            TodoStatus::InProgress => counts.in_progress += 1,
            TodoStatus::Pending => counts.pending += 1,
        }
    }
    counts
}

/// The window of the list to show, in the list's own order.
pub fn build_todo_display(todos: &[Todo], terminal_rows: usize) -> TodoDisplay {
    let counts = count_todos(todos);
    let limit = todo_display_limit(terminal_rows);
    if limit == 0 {
        return TodoDisplay {
            visible: Vec::new(),
            hidden: todos.len(),
            counts,
        };
    }
    if todos.len() <= limit {
        return TodoDisplay {
            visible: todos.to_vec(),
            hidden: 0,
            counts,
        };
    }

    // Anchor on the task being worked on; without one, stay at the top.
    let anchor = todos
        .iter()
        .position(|todo| todo.status == TodoStatus::InProgress)
        .unwrap_or(0);
    let start = (anchor.saturating_sub(limit - 1)).min(todos.len() - limit);
    TodoDisplay {
        visible: todos[start..start + limit].to_vec(),
        hidden: todos.len() - limit,
        counts,
    }
}

/// The one-line summary shown above a standalone list.
pub fn format_todo_summary(todos: &[Todo]) -> String {
    let counts = count_todos(todos);
    let mut summary = format!("{} tasks ({} done", todos.len(), counts.completed);
    if counts.in_progress > 0 {
        summary += &format!(", {} in progress", counts.in_progress);
    }
    summary += &format!(", {} open)", counts.pending);
    summary
}

/// The line below a truncated list, counting the whole list rather than the gap.
pub fn format_hidden_todo_summary(display: &TodoDisplay) -> Option<String> {
    if display.hidden == 0 {
        return None;
    }
    let mut parts: Vec<String> = Vec::new();
    if display.counts.pending > 0 {
        parts.push(format!("{} pending", display.counts.pending));
    }
    if display.counts.completed > 0 {
        parts.push(format!("{} completed", display.counts.completed));
    }
    if parts.is_empty() {
        return Some(format!("... +{}", display.hidden));
    }
    Some(format!("... +{} {}", display.hidden, parts.join(", ")))
}

/// Filled shapes rather than checkboxes: they carry at a glance.
fn panel_icon(status: TodoStatus) -> &'static str {
    match status {
        TodoStatus::Completed => "●",
        TodoStatus::InProgress => "◐",
        TodoStatus::Pending => "○",
    }
}

/// The column every row starts in, so the list lines up with the transcript
/// around it rather than hanging one character to its left.
const ROW_INDENT: &str = " ";

/// One rendered row.
///
/// The shape carries the status; it takes no colour of its own. A green dot
/// beside struck-through text and a purple one beside bold text say the same
/// thing twice, and the second telling is the one that pulls the eye away from
/// the task the user is actually reading.
pub fn format_todo_line(todo: &Todo) -> String {
    let theme = theme();
    let icon = panel_icon(todo.status);
    match todo.status {
        TodoStatus::Completed => format!(
            "{ROW_INDENT}{} {}",
            theme.fg(ThemeColor::Muted, icon),
            theme.fg(ThemeColor::Muted, &theme.strikethrough(&todo.content))
        ),
        TodoStatus::InProgress => format!(
            "{ROW_INDENT}{} {}",
            theme.fg(ThemeColor::Text, icon),
            theme.bold(&todo.content)
        ),
        TodoStatus::Pending => format!(
            "{ROW_INDENT}{} {}",
            theme.fg(ThemeColor::Muted, icon),
            todo.content
        ),
    }
}

/// Whether the summary line is shown above the rows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TodoListMode {
    /// Rows only.
    #[default]
    Status,
    /// Summary line above the rows.
    Standalone,
}

/// The panel itself.
pub struct TodoListComponent {
    todos: Vec<Todo>,
    rows: usize,
    mode: TodoListMode,
}

impl TodoListComponent {
    /// New panel; the TypeScript defaults are `("status", 24)`.
    pub fn new(mode: TodoListMode, terminal_rows: usize) -> Self {
        Self {
            todos: Vec::new(),
            rows: terminal_rows,
            mode,
        }
    }

    /// Replace the list.
    pub fn set_todos(&mut self, todos: Vec<Todo>) {
        self.todos = todos;
    }

    /// Tell the panel how tall the terminal is.
    pub fn set_terminal_rows(&mut self, rows: usize) {
        self.rows = rows;
    }
}

impl Default for TodoListComponent {
    fn default() -> Self {
        Self::new(TodoListMode::Status, 24)
    }
}

impl Component for TodoListComponent {
    fn invalidate(&mut self) {}

    fn render(&mut self, width: usize) -> Vec<Line> {
        if self.todos.is_empty() {
            return Vec::new();
        }
        let display = build_todo_display(&self.todos, self.rows);
        let mut lines: Vec<String> = Vec::new();
        if self.mode == TodoListMode::Standalone {
            lines.push(theme().fg(
                ThemeColor::Muted,
                &format!("{ROW_INDENT}{}", format_todo_summary(&self.todos)),
            ));
        }
        for todo in &display.visible {
            lines.push(format_todo_line(todo));
        }
        if let Some(hidden) = format_hidden_todo_summary(&display) {
            lines.push(theme().fg(ThemeColor::Muted, &format!("{ROW_INDENT}{hidden}")));
        }
        lines
            .iter()
            .map(|line| Line::from(truncate_to_width_opts(line, width, "…", false)))
            .collect()
    }
}

/// Decides whether the panel is up, and hides a finished list on a delay.
///
/// The delay is guarded by an epoch rather than a cancellable timer: every
/// update bumps it, and a timer that fires against a stale epoch does nothing.
/// A list that changed while the timer ran therefore cannot be hidden by it.
///
/// Timers never call back in this port (see `plans/interface-requests.md` A-4),
/// so the scheduled hide is exposed as [`TodoVisibility::hide_deadline`] plus
/// [`TodoVisibility::tick`]; the body of `tick` is the TypeScript timer body,
/// epoch check included.
pub struct TodoVisibility {
    todos: Vec<Todo>,
    shown: bool,
    epoch: u64,
    on_change: Box<dyn FnMut()>,
    delay: Duration,
    /// Deadline of the pending hide plus the epoch it was scheduled for.
    hide_at: Option<(Instant, u64)>,
}

impl TodoVisibility {
    /// New visibility gate; the TypeScript default delay is
    /// [`TODO_COMPLETED_HIDE_DELAY_MS`].
    pub fn new(on_change: Box<dyn FnMut()>, delay_ms: u64) -> Self {
        Self {
            todos: Vec::new(),
            shown: false,
            epoch: 0,
            on_change,
            delay: Duration::from_millis(delay_ms),
            hide_at: None,
        }
    }

    /// Whether the panel is up.
    pub fn visible(&self) -> bool {
        self.shown && !self.todos.is_empty()
    }

    /// The list the panel shows.
    pub fn current(&self) -> &[Todo] {
        &self.todos
    }

    /// Feeds a new list.
    ///
    /// `force_visible` is what a fresh tool call passes: an all-completed list
    /// arriving from a call should be seen once, while the same list arriving
    /// from a replay should not raise a panel the user had already dismissed.
    pub fn update(&mut self, todos: Vec<Todo>, force_visible: bool) {
        self.todos = todos;
        self.epoch += 1;

        if self.todos.is_empty() {
            self.shown = false;
            return;
        }
        if self
            .todos
            .iter()
            .any(|todo| todo.status != TodoStatus::Completed)
        {
            self.shown = true;
            return;
        }
        if !force_visible && !self.shown {
            self.shown = false;
            return;
        }

        self.shown = true;
        self.hide_at = Some((Instant::now() + self.delay, self.epoch));
    }

    /// When the scheduled hide is due, if one is pending.
    pub fn hide_deadline(&self) -> Option<Instant> {
        self.hide_at.map(|(deadline, _)| deadline)
    }

    /// Run a due hide. Returns `true` when the panel went down.
    pub fn tick(&mut self) -> bool {
        let Some((deadline, epoch)) = self.hide_at else {
            return false;
        };
        if Instant::now() < deadline {
            return false;
        }
        self.hide_at = None;
        if epoch != self.epoch {
            return false;
        }
        if self.todos.is_empty() {
            return false;
        }
        if !self
            .todos
            .iter()
            .all(|todo| todo.status == TodoStatus::Completed)
        {
            return false;
        }
        self.shown = false;
        (self.on_change)();
        true
    }

    /// Drops the list outright. Used when the turn it belonged to is gone.
    pub fn reset(&mut self) {
        self.todos = Vec::new();
        self.shown = false;
        self.epoch += 1;
    }
}
