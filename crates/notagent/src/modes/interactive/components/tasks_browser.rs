use std::time::{Duration, Instant};

use notagent_tui::tui::{Component, Line, shared_lines};
use notagent_tui::utils::{truncate_to_width_opts, visible_width};

use crate::core::tasks::types::{TaskInfo, TaskStatus, is_terminal_task_status};
use crate::modes::interactive::theme::theme::{ThemeColor, theme};

use super::tasks_panel::{now_ms, single_line};

/// Which tasks the browser lists.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TasksFilter {
    /// Everything the session knows about.
    All,
    /// Unfinished work only.
    #[default]
    Running,
}

impl TasksFilter {
    fn as_str(self) -> &'static str {
        match self {
            TasksFilter::All => "all",
            TasksFilter::Running => "running",
        }
    }
}

/// How long an armed stop waits for its confirmation before giving up.
pub const STOP_CONFIRM_TIMEOUT_MS: u64 = 5_000;

const MIN_WIDTH: usize = 48;
const MIN_HEIGHT: usize = 10;

/// Bounds on the left column, so neither a narrow nor a wide terminal ruins it.
const LIST_COLUMN_MIN: usize = 28;
const LIST_COLUMN_MAX: usize = 44;
const LIST_COLUMN_RATIO: f64 = 0.32;

/// Invoked with a task id.
pub type TaskIdCallback = Box<dyn FnMut(&str)>;

/// `TasksBrowserProps`
pub struct TasksBrowserProps {
    /// Everything the session knows about.
    pub tasks: Vec<TaskInfo>,
    /// Which of them are listed.
    pub filter: TasksFilter,
    /// The task the caller wants selected.
    pub selected_task_id: Option<String>,
    /// Output of the selected task.
    pub output: Option<String>,
    /// Whether that output is still being fetched.
    pub output_loading: bool,
    /// A message shown in the footer.
    pub notice: Option<String>,
    /// Fired when the cursor moves.
    pub on_select: TaskIdCallback,
    /// Fired on Tab.
    pub on_toggle_filter: Box<dyn FnMut()>,
    /// Fired on R.
    pub on_refresh: Box<dyn FnMut()>,
    /// Fired on Q or Escape.
    pub on_close: Box<dyn FnMut()>,
    /// Fired once a stop was confirmed.
    pub on_stop: TaskIdCallback,
    /// Fired when stop is pressed on something that cannot be stopped.
    pub on_stop_refused: Option<TaskIdCallback>,
}

fn status_colour(status: TaskStatus) -> ThemeColor {
    match status {
        TaskStatus::Running => ThemeColor::Success,
        TaskStatus::Completed => ThemeColor::Muted,
        _ => ThemeColor::Error,
    }
}

fn status_label(status: TaskStatus) -> &'static str {
    if status == TaskStatus::TimedOut {
        "timed out"
    } else {
        status.as_str()
    }
}

fn relative_time(timestamp: Option<i64>, now: i64) -> String {
    let Some(timestamp) = timestamp else {
        return String::new();
    };
    let seconds = (now - timestamp).max(0) / 1000;
    if seconds < 60 {
        return "just now".to_string();
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{minutes}m ago");
    }
    let hours = minutes / 60;
    if hours < 24 {
        return format!("{hours}h ago");
    }
    format!("{}d ago", hours / 24)
}

/// Pads or truncates to exactly `width`, so the frames line up.
fn exactly(line: &str, width: usize) -> String {
    let measured = visible_width(line);
    if measured == width {
        return line.to_string();
    }
    if measured > width {
        let cut = truncate_to_width_opts(line, width, "…", false);
        let after = visible_width(&cut);
        return if after < width {
            cut + &" ".repeat(width - after)
        } else {
            cut
        };
    }
    format!("{line}{}", " ".repeat(width - measured))
}

/// Which tasks are listed, and in what order.
/// Foreground work is included: a delegated child the turn is waiting on is
/// exactly the thing someone opens this to look at. Running first, oldest
/// first within that, so a row does not move while being read; finished work
/// newest first, because the last thing that ended is the interesting one.
pub fn visible_tasks(tasks: &[TaskInfo], filter: TasksFilter) -> Vec<TaskInfo> {
    let mut shown: Vec<TaskInfo> = if filter == TasksFilter::All {
        tasks.to_vec()
    } else {
        tasks
            .iter()
            .filter(|task| !is_terminal_task_status(task.base().status))
            .cloned()
            .collect()
    };
    shown.sort_by(|left, right| {
        let left_done = is_terminal_task_status(left.base().status);
        let right_done = is_terminal_task_status(right.base().status);
        if left_done != right_done {
            return if left_done {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Less
            };
        }
        if !left_done {
            return left.base().started_at.cmp(&right.base().started_at);
        }
        right
            .base()
            .ended_at
            .unwrap_or(right.base().started_at)
            .cmp(&left.base().ended_at.unwrap_or(left.base().started_at))
    });
    shown
}

/// `TaskCounts`
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TaskCounts {
    /// Tasks still running.
    pub running: usize,
    /// Tasks that finished cleanly.
    pub completed: usize,
    /// Everything else.
    pub failed: usize,
}

/// Count the list by status.
pub fn count_tasks(tasks: &[TaskInfo]) -> TaskCounts {
    let mut counts = TaskCounts::default();
    for task in tasks {
        match task.base().status {
            TaskStatus::Running => counts.running += 1,
            TaskStatus::Completed => counts.completed += 1,
            _ => counts.failed += 1,
        }
    }
    counts
}

/// The full-screen browser.
pub struct TasksBrowserComponent {
    props: TasksBrowserProps,
    rows: usize,
    selected_index: usize,
    scroll: usize,
    pending_stop: Option<String>,
    /// `setTimeout(…, STOP_CONFIRM_TIMEOUT_MS)`; timers never call back in this
    /// port, so the deadline is polled.
    pending_stop_deadline: Option<Instant>,
}

impl TasksBrowserComponent {
    /// New browser over `props`, `rows` terminal rows tall.
    pub fn new(props: TasksBrowserProps, rows: usize) -> Self {
        let mut component = Self {
            props,
            rows,
            selected_index: 0,
            scroll: 0,
            pending_stop: None,
            pending_stop_deadline: None,
        };
        component.sync_selection();
        component
    }

    /// Replace the props.
    pub fn set_props(&mut self, props: TasksBrowserProps) {
        self.props = props;
        self.sync_selection();
        if let Some(pending) = self.pending_stop.clone() {
            let task = self
                .props
                .tasks
                .iter()
                .find(|entry| entry.base().task_id == pending);
            // The thing being confirmed about ended on its own; the question is moot.
            if task.is_none_or(|task| is_terminal_task_status(task.base().status)) {
                self.clear_pending_stop();
            }
        }
    }

    /// Tell the browser how tall the terminal is.
    pub fn set_rows(&mut self, rows: usize) {
        self.rows = rows;
    }

    /// Drop an armed stop.
    pub fn dispose(&mut self) {
        self.clear_pending_stop();
    }

    /// The task id whose stop is awaiting confirmation.
    pub fn pending_stop(&self) -> Option<&str> {
        self.pending_stop.as_deref()
    }

    /// When an armed stop gives up.
    pub fn stop_confirm_deadline(&self) -> Option<Instant> {
        self.pending_stop_deadline
    }

    /// Body of the confirmation timeout. `true` when the stop was disarmed.
    pub fn tick_stop_confirm(&mut self) -> bool {
        let Some(deadline) = self.pending_stop_deadline else {
            return false;
        };
        if Instant::now() < deadline {
            return false;
        }
        self.clear_pending_stop();
        true
    }

    fn list(&self) -> Vec<TaskInfo> {
        visible_tasks(&self.props.tasks, self.props.filter)
    }

    fn sync_selection(&mut self) {
        let list = self.list();
        if list.is_empty() {
            self.selected_index = 0;
            self.scroll = 0;
            return;
        }
        if let Some(selected_task_id) = self.props.selected_task_id.as_deref()
            && let Some(index) = list
                .iter()
                .position(|task| task.base().task_id == selected_task_id)
        {
            self.selected_index = index;
            return;
        }
        if self.selected_index >= list.len() {
            self.selected_index = list.len() - 1;
        }
    }

    fn clear_pending_stop(&mut self) {
        self.pending_stop = None;
        self.pending_stop_deadline = None;
    }

    fn emit_select(&mut self) {
        let list = self.list();
        if let Some(task) = list.get(self.selected_index) {
            let task_id = task.base().task_id.clone();
            (self.props.on_select)(&task_id);
        }
    }
}

impl Component for TasksBrowserComponent {
    fn invalidate(&mut self) {}

    fn handle_input(&mut self, data: &str) {
        // An armed stop swallows the next key: anything but yes is no. Being
        // one keystroke from killing a build deserves that much.
        if let Some(task_id) = self.pending_stop.clone() {
            self.clear_pending_stop();
            if data == "y" || data == "Y" {
                (self.props.on_stop)(&task_id);
            }
            return;
        }

        if data == "\x1b" || data == "q" || data == "Q" {
            (self.props.on_close)();
            return;
        }
        if data == "\x1b[A" || data == "k" {
            self.selected_index = self.selected_index.saturating_sub(1);
            self.emit_select();
            return;
        }
        if data == "\x1b[B" || data == "j" {
            self.selected_index =
                (self.selected_index + 1).min(self.list().len().saturating_sub(1));
            self.emit_select();
            return;
        }
        if data == "\t" {
            (self.props.on_toggle_filter)();
            return;
        }
        if data == "r" || data == "R" {
            (self.props.on_refresh)();
            return;
        }
        if data == "s" || data == "S" {
            let list = self.list();
            let Some(task) = list.get(self.selected_index) else {
                return;
            };
            let task_id = task.base().task_id.clone();
            if is_terminal_task_status(task.base().status) {
                if let Some(on_stop_refused) = self.props.on_stop_refused.as_mut() {
                    on_stop_refused(&task_id);
                }
                return;
            }
            self.pending_stop = Some(task_id);
            self.pending_stop_deadline =
                Some(Instant::now() + Duration::from_millis(STOP_CONFIRM_TIMEOUT_MS));
        }
    }

    fn render(&mut self, width: usize) -> Vec<Line> {
        let theme_instance = theme();
        let height = self.rows.max(1);
        if width < MIN_WIDTH || height < MIN_HEIGHT {
            return vec![Line::from(exactly(
                &theme_instance.fg(
                    ThemeColor::Error,
                    &format!("Terminal too small (need ≥ {MIN_WIDTH} × {MIN_HEIGHT})"),
                ),
                width,
            ))];
        }

        let body_height = height - 2;
        let list_width = ((width as f64 * LIST_COLUMN_RATIO).floor() as usize)
            .clamp(LIST_COLUMN_MIN, LIST_COLUMN_MAX);
        let right_width = width - list_width;

        let left = self.render_list(list_width, body_height);
        let right = self.render_right(right_width, body_height);

        let mut lines = vec![self.render_header(width)];
        for index in 0..body_height {
            let left_line = left
                .get(index)
                .cloned()
                .unwrap_or_else(|| " ".repeat(list_width));
            let right_line = right
                .get(index)
                .cloned()
                .unwrap_or_else(|| " ".repeat(right_width));
            lines.push(left_line + &right_line);
        }
        lines.push(self.render_footer(width));
        shared_lines(lines)
    }
}

impl TasksBrowserComponent {
    fn render_header(&self, width: usize) -> String {
        let theme_instance = theme();
        let shown = self.list();
        let counts = count_tasks(&shown);
        let mut segments = vec![
            theme_instance.bold(&theme_instance.fg(ThemeColor::Accent, " TASKS ")),
            theme_instance.fg(
                ThemeColor::Muted,
                &format!(
                    " filter={} ",
                    if self.props.filter == TasksFilter::All {
                        "ALL"
                    } else {
                        "RUNNING"
                    }
                ),
            ),
        ];
        if counts.running > 0 {
            segments.push(theme_instance.fg(
                ThemeColor::Success,
                &format!(" {} running ", counts.running),
            ));
        }
        if counts.completed > 0 {
            segments.push(theme_instance.fg(
                ThemeColor::Muted,
                &format!(" {} completed ", counts.completed),
            ));
        }
        if counts.failed > 0 {
            segments.push(theme_instance.fg(
                ThemeColor::Error,
                &format!(" {} ended badly ", counts.failed),
            ));
        }
        segments.push(theme_instance.fg(ThemeColor::Muted, &format!(" {} total ", shown.len())));
        exactly(&segments.join(""), width)
    }

    fn render_footer(&self, width: usize) -> String {
        let theme_instance = theme();
        let key = |text: &str| theme_instance.bold(&theme_instance.fg(ThemeColor::Accent, text));
        let dim = |text: &str| theme_instance.fg(ThemeColor::Muted, text);

        if let Some(pending_stop) = self.pending_stop.as_deref() {
            return exactly(
                &format!(
                    " {} {}?  {} {}  {}{}{} {} ",
                    theme_instance.bold(&theme_instance.fg(ThemeColor::Warning, "Stop")),
                    theme_instance.fg(ThemeColor::Text, pending_stop),
                    key("Y"),
                    dim("confirm"),
                    key("N"),
                    dim("/"),
                    key("esc"),
                    dim("cancel")
                ),
                width,
            );
        }

        let list = self.list();
        let selected = list.get(self.selected_index);
        let mut parts = vec![format!(" {} {}", key("↑↓"), dim("select"))];
        // `S` is advertised only when it would do something. A key that looks
        // available and does nothing reads as broken.
        if selected.is_some_and(|task| !is_terminal_task_status(task.base().status)) {
            parts.push(format!("{} {}", key("S"), dim("stop")));
        }
        parts.push(format!("{} {}", key("R"), dim("refresh")));
        parts.push(format!("{} {}", key("Tab"), dim("filter")));
        parts.push(format!("{} {} ", key("Q/Esc"), dim("close")));

        let left = parts.join("  ");
        if let Some(notice) = self.props.notice.as_deref()
            && !notice.is_empty()
        {
            let styled = theme_instance.fg(ThemeColor::Warning, &format!(" {notice} "));
            let total = visible_width(&left) + visible_width(&styled);
            if total <= width {
                return left + &" ".repeat(width - total) + &styled;
            }
        }
        exactly(&left, width)
    }

    /// A framed box of exactly `width` × `height`, with its title in the top rule.
    fn frame(&self, title: &str, content: &[String], width: usize, height: usize) -> Vec<String> {
        let theme_instance = theme();
        if height < 2 || width < 4 {
            return vec![" ".repeat(width); height];
        }
        let inner_width = width - 2;
        let styled_title = theme_instance.bold(&theme_instance.fg(ThemeColor::Text, title));
        let title_segment = format!("─ {styled_title} ");
        let top = if visible_width(&title_segment) <= inner_width {
            theme_instance.fg(ThemeColor::Muted, "┌")
                + &theme_instance.fg(ThemeColor::Muted, "─ ")
                + &styled_title
                + " "
                + &theme_instance.fg(
                    ThemeColor::Muted,
                    &"─".repeat(inner_width.saturating_sub(visible_width(&title_segment))),
                )
                + &theme_instance.fg(ThemeColor::Muted, "┐")
        } else {
            theme_instance.fg(ThemeColor::Muted, &format!("┌{}┐", "─".repeat(inner_width)))
        };

        let mut lines = vec![top];
        for index in 0..height - 2 {
            let content_line = content.get(index).map(String::as_str).unwrap_or("");
            lines.push(
                theme_instance.fg(ThemeColor::Muted, "│")
                    + &exactly(content_line, inner_width)
                    + &theme_instance.fg(ThemeColor::Muted, "│"),
            );
        }
        lines.push(theme_instance.fg(ThemeColor::Muted, &format!("└{}┘", "─".repeat(inner_width))));
        lines
    }

    fn render_list(&mut self, width: usize, height: usize) -> Vec<String> {
        let theme_instance = theme();
        let list = self.list();
        let inner_height = height.saturating_sub(2);
        let title = format!("Tasks [{}]", self.props.filter.as_str());

        if list.is_empty() {
            let empty = if self.props.filter == TasksFilter::Running {
                "Nothing running. Tab shows all."
            } else {
                "No background tasks in this session."
            };
            return self.frame(
                &title,
                &[theme_instance.fg(ThemeColor::Muted, empty)],
                width,
                height,
            );
        }

        self.adjust_scroll(inner_height, list.len());
        let inner_width = width - 2;
        let rows: Vec<String> = list
            .iter()
            .skip(self.scroll)
            .take(inner_height)
            .enumerate()
            .map(|(offset, task)| {
                self.render_row(
                    task,
                    self.scroll + offset == self.selected_index,
                    inner_width,
                )
            })
            .collect();
        self.frame(&title, &rows, width, height)
    }

    fn render_row(&self, task: &TaskInfo, selected: bool, inner_width: usize) -> String {
        let theme_instance = theme();
        let base = task.base();
        let pointer = theme_instance.fg(
            if selected {
                ThemeColor::Accent
            } else {
                ThemeColor::Muted
            },
            if selected { "❯ " } else { "  " },
        );
        let id_colour = if selected {
            ThemeColor::Accent
        } else if matches!(task, TaskInfo::Subagent(_)) {
            ThemeColor::Success
        } else {
            ThemeColor::ToolTitle
        };
        let id = if selected {
            theme_instance.bold(&theme_instance.fg(id_colour, &base.task_id))
        } else {
            theme_instance.fg(id_colour, &base.task_id)
        };
        let status = theme_instance.fg(status_colour(base.status), status_label(base.status));
        let prefix = format!(
            "{pointer}{id}{} {status}",
            " ".repeat(16usize.saturating_sub(base.task_id.chars().count()))
        );

        let budget = inner_width as isize - visible_width(&prefix) as isize - 1;
        if budget < 4 {
            return exactly(&prefix, inner_width);
        }
        let label = {
            let description = single_line(&base.description);
            match task {
                // A subagent leads with its star name: scanning the column for
                // "which one is Vega" is the reason the name exists at all.
                TaskInfo::Subagent(subagent) if !description.is_empty() => {
                    format!("{} · {description}", subagent.alias)
                }
                TaskInfo::Subagent(subagent) => subagent.alias.clone(),
                TaskInfo::Shell(shell) if description.is_empty() => single_line(&shell.command),
                TaskInfo::Shell(_) => description,
            }
        };
        let label = if label.is_empty() {
            "(no description)".to_string()
        } else {
            label
        };
        exactly(
            &format!(
                "{prefix} {}",
                theme_instance.fg(
                    ThemeColor::Text,
                    &truncate_to_width_opts(&label, budget as usize, "…", false)
                )
            ),
            inner_width,
        )
    }

    fn adjust_scroll(&mut self, visible_rows: usize, total: usize) {
        if visible_rows == 0 {
            self.scroll = 0;
            return;
        }
        if self.selected_index < self.scroll {
            self.scroll = self.selected_index;
        } else if self.selected_index >= self.scroll + visible_rows {
            self.scroll = self.selected_index - visible_rows + 1;
        }
        self.scroll = self.scroll.min(total.saturating_sub(visible_rows));
    }

    fn render_right(&self, width: usize, height: usize) -> Vec<String> {
        // The detail pane wants about ten rows for a subagent's fields; the
        // preview takes the rest, but never less than its own frame plus a line.
        let detail_height = (10.max(
            ((height as f64 * 0.4).floor() as isize)
                .min(height as isize - 5)
                .max(0) as usize,
        ))
        .min(3.max(height.saturating_sub(3)));
        let mut lines = self.render_detail(width, detail_height);
        lines.extend(self.render_preview(width, height - detail_height));
        lines
    }

    fn render_detail(&self, width: usize, height: usize) -> Vec<String> {
        let theme_instance = theme();
        let list = self.list();
        let Some(task) = list.get(self.selected_index) else {
            return self.frame(
                "Detail",
                &[theme_instance.fg(ThemeColor::Muted, "Select a task on the left.")],
                width,
                height,
            );
        };
        let base = task.base();

        let label = |text: &str| theme_instance.fg(ThemeColor::Muted, &format!("{text:<14}"));
        let value = |text: &str| theme_instance.fg(ThemeColor::Text, text);
        let mut lines = vec![
            format!("{}{}", label("Task:"), value(&base.task_id)),
            format!(
                "{}{}",
                label("Status:"),
                theme_instance.fg(status_colour(base.status), status_label(base.status))
            ),
            format!(
                "{}{}",
                label("Description:"),
                value(&{
                    let description = single_line(&base.description);
                    if description.is_empty() {
                        "—".to_string()
                    } else {
                        description
                    }
                })
            ),
        ];
        match task {
            TaskInfo::Shell(shell) => {
                if !shell.command.is_empty() && shell.command != base.description {
                    lines.push(format!(
                        "{}{}",
                        label("Command:"),
                        value(&single_line(&shell.command))
                    ));
                }
                if shell.pid > 0 {
                    lines.push(format!(
                        "{}{}",
                        label("Pid:"),
                        theme_instance.fg(ThemeColor::Muted, &shell.pid.to_string())
                    ));
                }
                if let Some(exit_code) = shell.exit_code {
                    lines.push(format!(
                        "{}{}",
                        label("Exit code:"),
                        theme_instance.fg(ThemeColor::Muted, &exit_code.to_string())
                    ));
                }
            }
            TaskInfo::Subagent(subagent) => {
                lines.push(format!(
                    "{}{}",
                    label("Session:"),
                    value(&subagent.session_id)
                ));
                lines.push(format!("{}{}", label("Name:"), value(&subagent.alias)));
                lines.push(format!("{}{}", label("Agent:"), value(&subagent.agent)));
                if subagent.tokens > 0 {
                    lines.push(format!(
                        "{}{}",
                        label("Tokens:"),
                        theme_instance.fg(ThemeColor::Muted, &subagent.tokens.to_string())
                    ));
                }
            }
        }
        lines.push(format!(
            "{}{}",
            label("Waiting call:"),
            theme_instance.fg(
                ThemeColor::Muted,
                if base.detached == Some(false) {
                    "yes"
                } else {
                    "no"
                }
            )
        ));
        let now = now_ms();
        let timing = if base.status == TaskStatus::Running {
            format!("started {}", relative_time(Some(base.started_at), now))
        } else if base.ended_at.is_some_and(|ended_at| ended_at != 0) {
            format!("ended {}", relative_time(base.ended_at, now))
        } else {
            String::new()
        };
        if !timing.is_empty() {
            lines.push(format!(
                "{}{}",
                label("Time:"),
                theme_instance.fg(ThemeColor::Muted, &timing)
            ));
        }
        if let Some(stop_reason) = base.stop_reason.as_deref()
            && !stop_reason.is_empty()
        {
            lines.push(format!(
                "{}{}",
                label("Reason:"),
                theme_instance.fg(ThemeColor::Muted, &single_line(stop_reason))
            ));
        }
        self.frame("Detail", &lines, width, height)
    }

    fn render_preview(&self, width: usize, height: usize) -> Vec<String> {
        let theme_instance = theme();
        let inner_height = height.saturating_sub(2);
        let list = self.list();
        if list.get(self.selected_index).is_none() {
            return self.frame(
                "Output",
                &[theme_instance.fg(ThemeColor::Muted, "No task selected.")],
                width,
                height,
            );
        }

        let body = if self.props.output_loading {
            "[loading…]"
        } else {
            match self.props.output.as_deref() {
                None | Some("") => "[no output]",
                Some(output) => output,
            }
        };
        // The tail rather than the head: what a running task just printed is
        // what someone opened this to see.
        let all_lines: Vec<&str> = body.split('\n').collect();
        let lines: Vec<String> = all_lines
            .iter()
            .skip(all_lines.len().saturating_sub(inner_height))
            .map(|line| theme_instance.fg(ThemeColor::Muted, line))
            .collect();
        self.frame("Output", &lines, width, height)
    }
}
