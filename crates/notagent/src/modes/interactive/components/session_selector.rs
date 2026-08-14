//! 1:1 port of
//! `packages/coding-agent/src/modes/interactive/components/session-selector.ts` (1 031 LOC).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Instant;

use notagent_tui::components::input::Input;
use notagent_tui::components::spacer::Spacer;
use notagent_tui::components::text::Text;
use notagent_tui::keybindings::keybindings_match;
use notagent_tui::tui::{Component, ComponentRef, Container, Focusable, component_ref};
use notagent_tui::utils::{truncate_to_width, truncate_to_width_opts, visible_width};

use crate::core::keybindings::KeybindingsManager;
use crate::core::session_manager::SessionInfo;
use crate::modes::interactive::theme::theme::{ThemeBg, ThemeColor, theme};
use crate::utils::paths::canonicalize_path as canonicalize_path_impl;

use super::dynamic_border::DynamicBorder;
use super::keybinding_hints::{key_hint, key_text};
use super::session_selector_search::{
    NameFilter, SortMode, filter_and_sort_sessions, has_session_name,
};

/// Which sessions the selector lists.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SessionScope {
    /// Sessions of the current working directory.
    #[default]
    Current,
    /// Every session.
    All,
}

fn shorten_path(path: &str) -> String {
    let home = dirs::home_dir()
        .map(|home| home.to_string_lossy().into_owned())
        .unwrap_or_default();
    if path.is_empty() {
        return path.to_string();
    }
    if !home.is_empty()
        && let Some(rest) = path.strip_prefix(&home)
    {
        return format!("~{rest}");
    }
    path.to_string()
}

/// `formatSessionDate(date)` — `date` is the millisecond timestamp of
/// `SessionInfo.modified`, and `now` the current time.
fn format_session_date(modified_ms: i64, now_ms: i64) -> String {
    let diff_ms = now_ms - modified_ms;
    // `Math.floor` on a negative difference rounds away from zero, which is what
    // a clock skew would produce in TypeScript as well.
    let floor_div = |value: i64, divisor: i64| (value as f64 / divisor as f64).floor() as i64;
    let diff_mins = floor_div(diff_ms, 60_000);
    let diff_hours = floor_div(diff_ms, 3_600_000);
    let diff_days = floor_div(diff_ms, 86_400_000);

    if diff_mins < 1 {
        return "now".to_string();
    }
    if diff_mins < 60 {
        return format!("{diff_mins}m");
    }
    if diff_hours < 24 {
        return format!("{diff_hours}h");
    }
    if diff_days < 7 {
        return format!("{diff_days}d");
    }
    if diff_days < 30 {
        return format!("{}w", diff_days / 7);
    }
    if diff_days < 365 {
        return format!("{}mo", diff_days / 30);
    }
    format!("{}y", diff_days / 365)
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn canonicalize_path(path: Option<&str>) -> Option<String> {
    let path = path?;
    if path.is_empty() {
        // `if (!path) return path` — an empty string stays an empty string.
        return Some(String::new());
    }
    Some(canonicalize_path_impl(path))
}

// --- header --------------------------------------------------------------------

/// Kind of the status line the header shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusKind {
    /// Neutral message.
    Info,
    /// Error message.
    Error,
}

struct StatusMessage {
    kind: StatusKind,
    message: String,
}

/// The two title rows plus the hint rows above the list.
pub struct SessionSelectorHeader {
    scope: SessionScope,
    sort_mode: SortMode,
    name_filter: NameFilter,
    request_render: Rc<dyn Fn()>,
    loading: bool,
    load_progress: Option<(usize, usize)>,
    show_path: bool,
    confirming_delete_path: Option<String>,
    status_message: Option<StatusMessage>,
    /// `setTimeout` of `setStatusMessage`; timers never call back in this port,
    /// so the deadline is polled (`status_deadline`/`tick_status`).
    status_deadline: Option<Instant>,
    show_rename_hint: bool,
}

impl SessionSelectorHeader {
    fn new(
        scope: SessionScope,
        sort_mode: SortMode,
        name_filter: NameFilter,
        request_render: Rc<dyn Fn()>,
    ) -> Self {
        Self {
            scope,
            sort_mode,
            name_filter,
            request_render,
            loading: false,
            load_progress: None,
            show_path: false,
            confirming_delete_path: None,
            status_message: None,
            status_deadline: None,
            show_rename_hint: false,
        }
    }

    fn set_scope(&mut self, scope: SessionScope) {
        self.scope = scope;
    }

    fn set_sort_mode(&mut self, sort_mode: SortMode) {
        self.sort_mode = sort_mode;
    }

    fn set_name_filter(&mut self, name_filter: NameFilter) {
        self.name_filter = name_filter;
    }

    fn set_loading(&mut self, loading: bool) {
        self.loading = loading;
        // Progress is scoped to the current load; clear whenever the loading state is set
        self.load_progress = None;
    }

    fn set_progress(&mut self, loaded: usize, total: usize) {
        self.load_progress = Some((loaded, total));
    }

    fn set_show_path(&mut self, show_path: bool) {
        self.show_path = show_path;
    }

    fn set_show_rename_hint(&mut self, show: bool) {
        self.show_rename_hint = show;
    }

    fn set_confirming_delete_path(&mut self, path: Option<String>) {
        self.confirming_delete_path = path;
    }

    fn set_status_message(&mut self, message: Option<StatusMessage>, auto_hide_ms: Option<u64>) {
        self.status_deadline = None;
        let has_message = message.is_some();
        self.status_message = message;
        let Some(auto_hide_ms) = auto_hide_ms else {
            return;
        };
        if !has_message || auto_hide_ms == 0 {
            return;
        }
        self.status_deadline =
            Some(Instant::now() + std::time::Duration::from_millis(auto_hide_ms));
    }

    /// When the status message hides itself, if one is pending.
    fn status_deadline(&self) -> Option<Instant> {
        self.status_deadline
    }

    /// Body of the auto-hide timer. `true` when the message went away.
    fn tick_status(&mut self) -> bool {
        let Some(deadline) = self.status_deadline else {
            return false;
        };
        if Instant::now() < deadline {
            return false;
        }
        self.status_message = None;
        self.status_deadline = None;
        (self.request_render)();
        true
    }
}

impl Component for SessionSelectorHeader {
    fn invalidate(&mut self) {}

    fn render(&mut self, width: usize) -> Vec<String> {
        let theme = theme();
        let title = if self.scope == SessionScope::Current {
            "Resume Session (Current Folder)"
        } else {
            "Resume Session (All)"
        };
        let left_text = theme.bold(title);

        let sort_label = match self.sort_mode {
            SortMode::Threaded => "Threaded",
            SortMode::Recent => "Recent",
            SortMode::Relevance => "Fuzzy",
        };
        let sort_text =
            theme.fg(ThemeColor::Muted, "Sort: ") + &theme.fg(ThemeColor::Accent, sort_label);

        let name_label = if self.name_filter == NameFilter::All {
            "All"
        } else {
            "Named"
        };
        let name_text =
            theme.fg(ThemeColor::Muted, "Name: ") + &theme.fg(ThemeColor::Accent, name_label);

        let scope_text = if self.loading {
            let progress_text = match self.load_progress {
                Some((loaded, total)) => format!("{loaded}/{total}"),
                None => "...".to_string(),
            };
            theme.fg(ThemeColor::Muted, "○ Current Folder | ")
                + &theme.fg(ThemeColor::Accent, &format!("Loading {progress_text}"))
        } else if self.scope == SessionScope::Current {
            theme.fg(ThemeColor::Accent, "◉ Current Folder")
                + &theme.fg(ThemeColor::Muted, " | ○ All")
        } else {
            theme.fg(ThemeColor::Muted, "○ Current Folder | ")
                + &theme.fg(ThemeColor::Accent, "◉ All")
        };

        let right_text = truncate_to_width_opts(
            &format!("{scope_text}  {name_text}  {sort_text}"),
            width,
            "",
            false,
        );
        let available_left = width
            .saturating_sub(visible_width(&right_text))
            .saturating_sub(1);
        let left = truncate_to_width_opts(&left_text, available_left, "", false);
        let spacing = width
            .saturating_sub(visible_width(&left))
            .saturating_sub(visible_width(&right_text));

        // Build hint lines - changes based on state (all branches truncate to width)
        let (hint_line1, hint_line2) = if self.confirming_delete_path.is_some() {
            let confirm_hint = format!(
                "Delete session? {} · {}",
                key_hint("tui.select.confirm", "confirm"),
                key_hint("tui.select.cancel", "cancel")
            );
            (
                theme.fg(
                    ThemeColor::Error,
                    &truncate_to_width_opts(&confirm_hint, width, "…", false),
                ),
                String::new(),
            )
        } else if let Some(status) = self.status_message.as_ref() {
            let color = if status.kind == StatusKind::Error {
                ThemeColor::Error
            } else {
                ThemeColor::Accent
            };
            (
                theme.fg(
                    color,
                    &truncate_to_width_opts(&status.message, width, "…", false),
                ),
                String::new(),
            )
        } else {
            let path_state = if self.show_path { "(on)" } else { "(off)" };
            let separator = theme.fg(ThemeColor::Muted, " · ");
            let hint1 = key_hint("tui.input.tab", "scope")
                + &separator
                + &theme.fg(ThemeColor::Muted, "re:<pattern> regex · \"phrase\" exact");
            let mut hint2_parts = vec![
                key_hint("app.session.toggleSort", "sort"),
                key_hint("app.session.toggleNamedFilter", "named"),
                key_hint("app.session.delete", "delete"),
                key_hint("app.session.togglePath", &format!("path {path_state}")),
            ];
            if self.show_rename_hint {
                hint2_parts.push(key_hint("app.session.rename", "rename"));
            }
            let hint2 = hint2_parts.join(&separator);
            (
                truncate_to_width_opts(&hint1, width, "…", false),
                truncate_to_width_opts(&hint2, width, "…", false),
            )
        };

        vec![
            format!("{left}{}{right_text}", " ".repeat(spacing)),
            hint_line1,
            hint_line2,
        ]
    }
}

// --- session tree ----------------------------------------------------------------

/// A session tree node for hierarchical display
struct SessionTreeNode {
    session: SessionInfo,
    children: Vec<SessionTreeNode>,
    latest_activity: i64,
}

/// Flattened node for display with tree structure info
#[derive(Clone, Debug)]
struct FlatSessionNode {
    session: SessionInfo,
    depth: usize,
    is_last: bool,
    /// For each ancestor level, whether there are more siblings after it
    ancestor_continues: Vec<bool>,
}

/// Build a tree structure from sessions based on parentSessionPath.
/// Returns root nodes sorted by modified date (descending).
fn build_session_tree(sessions: &[SessionInfo]) -> Vec<SessionTreeNode> {
    // The TypeScript version links `SessionTreeNode` objects through a map; the
    // port builds the same shape by index, which avoids the shared mutable
    // references Rust would otherwise need.
    let canonical: Vec<String> = sessions
        .iter()
        .map(|session| {
            canonicalize_path(Some(&session.path)).unwrap_or_else(|| session.path.clone())
        })
        .collect();
    let index_by_path: std::collections::HashMap<&str, usize> = canonical
        .iter()
        .enumerate()
        .map(|(index, path)| (path.as_str(), index))
        // `Map.set` keeps the last write for a duplicate key.
        .collect();

    let mut children_of: Vec<Vec<usize>> = vec![Vec::new(); sessions.len()];
    let mut roots: Vec<usize> = Vec::new();
    for (index, session) in sessions.iter().enumerate() {
        let parent_path = canonicalize_path(session.parent_session_path.as_deref());
        match parent_path
            .as_deref()
            .and_then(|path| index_by_path.get(path))
        {
            Some(parent) => children_of[*parent].push(index),
            None => roots.push(index),
        }
    }

    fn build(
        index: usize,
        sessions: &[SessionInfo],
        children_of: &[Vec<usize>],
    ) -> SessionTreeNode {
        let children: Vec<SessionTreeNode> = children_of[index]
            .iter()
            .map(|child| build(*child, sessions, children_of))
            .collect();
        let latest_activity = children
            .iter()
            .map(|child| child.latest_activity)
            .fold(sessions[index].modified, i64::max);
        SessionTreeNode {
            session: sessions[index].clone(),
            children,
            latest_activity,
        }
    }

    let mut tree: Vec<SessionTreeNode> = roots
        .into_iter()
        .map(|index| build(index, sessions, &children_of))
        .collect();

    // Sort children and roots by latest activity in each subtree (descending)
    fn sort_nodes(nodes: &mut Vec<SessionTreeNode>) {
        nodes.sort_by_key(|node| std::cmp::Reverse(node.latest_activity));
        for node in nodes {
            sort_nodes(&mut node.children);
        }
    }
    sort_nodes(&mut tree);

    tree
}

/// Flatten tree into display list with tree structure metadata.
fn flatten_session_tree(roots: &[SessionTreeNode]) -> Vec<FlatSessionNode> {
    fn walk(
        node: &SessionTreeNode,
        depth: usize,
        ancestor_continues: Vec<bool>,
        is_last: bool,
        result: &mut Vec<FlatSessionNode>,
    ) {
        result.push(FlatSessionNode {
            session: node.session.clone(),
            depth,
            is_last,
            ancestor_continues: ancestor_continues.clone(),
        });

        for (index, child) in node.children.iter().enumerate() {
            let child_is_last = index == node.children.len() - 1;
            // Only show continuation line for non-root ancestors
            let continues = if depth > 0 { !is_last } else { false };
            let mut child_ancestors = ancestor_continues.clone();
            child_ancestors.push(continues);
            walk(child, depth + 1, child_ancestors, child_is_last, result);
        }
    }

    let mut result: Vec<FlatSessionNode> = Vec::new();
    for (index, root) in roots.iter().enumerate() {
        walk(root, 0, Vec::new(), index == roots.len() - 1, &mut result);
    }
    result
}

// --- session list ------------------------------------------------------------------

/// Requests the list raises that only the selector can carry out.
///
/// TypeScript wires these to callbacks that close over the selector; the port
/// records them and the selector drains them right after dispatch (class 1,
/// same shape as the tree selector).
#[derive(Clone, Debug, PartialEq, Eq)]
enum SessionListEvent {
    ToggleScope,
    ToggleSort,
    ToggleNameFilter,
    RenameSession(String),
    DeleteSession(String),
}

/// Invoked with a session path.
pub type SessionPathCallback = Box<dyn FnMut(&str)>;
/// Invoked with the path being confirmed for deletion, or `None`.
pub type DeleteConfirmationCallback = Box<dyn FnMut(Option<&str>)>;
/// Invoked with the path column state.
pub type TogglePathCallback = Box<dyn FnMut(bool)>;
/// Renames a session: path and new name.
pub type RenameSessionCallback = Box<dyn FnMut(&str, &str)>;

/// Custom session list component with multi-line items and search
pub struct SessionList {
    all_sessions: Vec<SessionInfo>,
    filtered_sessions: Vec<FlatSessionNode>,
    selected_index: usize,
    search_input: Input,
    show_cwd: bool,
    sort_mode: SortMode,
    name_filter: NameFilter,
    keybindings: Rc<RefCell<KeybindingsManager>>,
    show_path: bool,
    confirming_delete_path: Option<String>,
    current_session_canonical_path: Option<String>,
    /// Invoked with the path of the confirmed session.
    pub on_select: Option<SessionPathCallback>,
    /// Invoked on cancel.
    pub on_cancel: Option<Box<dyn FnMut()>>,
    /// Wired by the selector; never invoked by the list in TypeScript either.
    pub on_exit: Option<Box<dyn FnMut()>>,
    /// Invoked when the delete confirmation is raised or dropped.
    pub on_delete_confirmation_change: Option<DeleteConfirmationCallback>,
    /// Replaces the selector's own deletion when set — `SessionList.onDeleteSession`
    /// holds one handler, and whoever sets it last wins (TypeScript sets it from
    /// the selector's constructor, tests overwrite it).
    pub on_delete_session: Option<SessionPathCallback>,
    /// Invoked when the path column is toggled.
    pub on_toggle_path: Option<TogglePathCallback>,
    /// Invoked with a user-facing error.
    pub on_error: Option<SessionPathCallback>,
    /// Set when the search input submitted; drained by the selector.
    submitted: Rc<Cell<bool>>,
    events: Vec<SessionListEvent>,
    /// Max sessions visible (one line each)
    max_visible: usize,

    // Focusable implementation - propagate to searchInput for IME cursor positioning
    focused: bool,
}

impl SessionList {
    fn new(
        sessions: Vec<SessionInfo>,
        show_cwd: bool,
        sort_mode: SortMode,
        name_filter: NameFilter,
        keybindings: Rc<RefCell<KeybindingsManager>>,
        current_session_file_path: Option<&str>,
    ) -> Self {
        let mut search_input = Input::new();
        let submitted = Rc::new(Cell::new(false));
        {
            let flag = Rc::clone(&submitted);
            search_input.on_submit = Some(Box::new(move |_value| flag.set(true)));
        }
        let mut list = Self {
            all_sessions: sessions,
            filtered_sessions: Vec::new(),
            selected_index: 0,
            search_input,
            show_cwd,
            sort_mode,
            name_filter,
            keybindings,
            show_path: false,
            confirming_delete_path: None,
            current_session_canonical_path: canonicalize_path(current_session_file_path),
            on_select: None,
            on_cancel: None,
            on_exit: None,
            on_delete_confirmation_change: None,
            on_delete_session: None,
            on_toggle_path: None,
            on_error: None,
            submitted,
            events: Vec::new(),
            max_visible: 10,
            focused: false,
        };
        list.filter_sessions("");
        list
    }

    /// Path of the session under the cursor.
    pub fn get_selected_session_path(&self) -> Option<&str> {
        self.filtered_sessions
            .get(self.selected_index)
            .map(|node| node.session.path.as_str())
    }

    fn set_sort_mode(&mut self, sort_mode: SortMode) {
        self.sort_mode = sort_mode;
        let query = self.search_input.get_value().to_string();
        self.filter_sessions(&query);
    }

    fn set_name_filter(&mut self, name_filter: NameFilter) {
        self.name_filter = name_filter;
        let query = self.search_input.get_value().to_string();
        self.filter_sessions(&query);
    }

    fn set_sessions(&mut self, sessions: Vec<SessionInfo>, show_cwd: bool) {
        self.all_sessions = sessions;
        self.show_cwd = show_cwd;
        let query = self.search_input.get_value().to_string();
        self.filter_sessions(&query);
    }

    fn filter_sessions(&mut self, query: &str) {
        let trimmed = query.trim();
        let name_filtered: Vec<SessionInfo> = if self.name_filter == NameFilter::All {
            self.all_sessions.clone()
        } else {
            self.all_sessions
                .iter()
                .filter(|session| has_session_name(session))
                .cloned()
                .collect()
        };

        if self.sort_mode == SortMode::Threaded && trimmed.is_empty() {
            // Threaded mode without search: show tree structure
            let roots = build_session_tree(&name_filtered);
            self.filtered_sessions = flatten_session_tree(&roots);
        } else {
            // Other modes or with search: flat list
            let filtered =
                filter_and_sort_sessions(&name_filtered, query, self.sort_mode, NameFilter::All);
            self.filtered_sessions = filtered
                .into_iter()
                .map(|session| FlatSessionNode {
                    session,
                    depth: 0,
                    is_last: true,
                    ancestor_continues: Vec::new(),
                })
                .collect();
        }
        self.selected_index = self
            .selected_index
            .min(self.filtered_sessions.len().saturating_sub(1));
    }

    fn set_confirming_delete_path(&mut self, path: Option<String>) {
        self.confirming_delete_path = path.clone();
        if let Some(callback) = self.on_delete_confirmation_change.as_mut() {
            callback(path.as_deref());
        }
    }

    fn start_delete_confirmation_for_selected_session(&mut self) {
        let Some(node) = self.filtered_sessions.get(self.selected_index) else {
            return;
        };
        let path = node.session.path.clone();

        // Prevent deleting current session
        if self.is_current_session_path(&path) {
            if let Some(on_error) = self.on_error.as_mut() {
                on_error("Cannot delete the currently active session");
            }
            return;
        }

        self.set_confirming_delete_path(Some(path));
    }

    fn is_current_session_path(&self, path: &str) -> bool {
        let Some(current) = self.current_session_canonical_path.as_deref() else {
            return false;
        };
        canonicalize_path(Some(path)).as_deref().unwrap_or(path) == current
    }

    fn build_tree_prefix(node: &FlatSessionNode) -> String {
        if node.depth == 0 {
            return String::new();
        }

        let parts: String = node
            .ancestor_continues
            .iter()
            .map(|continues| if *continues { "│  " } else { "   " })
            .collect();
        let branch = if node.is_last { "└─ " } else { "├─ " };
        parts + branch
    }

    fn take_events(&mut self) -> Vec<SessionListEvent> {
        std::mem::take(&mut self.events)
    }
}

impl Component for SessionList {
    fn invalidate(&mut self) {}

    fn render(&mut self, width: usize) -> Vec<String> {
        let theme = theme();
        let mut lines: Vec<String> = Vec::new();

        // Render search input
        lines.extend(self.search_input.render(width));
        lines.push(String::new()); // Blank line after search

        if self.filtered_sessions.is_empty() {
            let empty_message = if self.name_filter == NameFilter::Named {
                let toggle_key = key_text("app.session.toggleNamedFilter");
                if self.show_cwd {
                    format!("  No named sessions found. Press {toggle_key} to show all.")
                } else {
                    format!(
                        "  No named sessions in current folder. Press {toggle_key} to show all, or Tab to view all."
                    )
                }
            } else if self.show_cwd {
                // "All" scope - no sessions anywhere that match filter
                "  No sessions found".to_string()
            } else {
                // "Current folder" scope - hint to try "all"
                "  No sessions in current folder. Press Tab to view all.".to_string()
            };
            lines.push(theme.fg(
                ThemeColor::Muted,
                &truncate_to_width_opts(&empty_message, width, "…", false),
            ));
            return lines;
        }

        // Calculate visible range with scrolling
        let start_index = (self.selected_index as isize - (self.max_visible / 2) as isize)
            .min(self.filtered_sessions.len() as isize - self.max_visible as isize)
            .max(0) as usize;
        let end_index = (start_index + self.max_visible).min(self.filtered_sessions.len());

        let now = now_ms();
        // Render visible sessions (one line each with tree structure)
        for index in start_index..end_index {
            let node = &self.filtered_sessions[index];
            let session = &node.session;
            let is_selected = index == self.selected_index;
            let is_confirming_delete =
                Some(session.path.as_str()) == self.confirming_delete_path.as_deref();
            let is_current = self.is_current_session_path(&session.path);

            // Build tree prefix
            let prefix = Self::build_tree_prefix(node);

            // Session display text (name or first message)
            let has_name = session.name.is_some();
            let display_text = session.name.as_deref().unwrap_or(&session.first_message);
            let normalized_message: String = display_text
                .chars()
                .map(|character| {
                    if (character as u32) <= 0x1f || character as u32 == 0x7f {
                        ' '
                    } else {
                        character
                    }
                })
                .collect::<String>()
                .trim()
                .to_string();

            // Right side: message count and age
            let age = format_session_date(session.modified, now);
            let mut right_part = format!("{} {age}", session.message_count);
            if self.show_cwd && !session.cwd.is_empty() {
                right_part = format!("{} {right_part}", shorten_path(&session.cwd));
            }
            if self.show_path {
                right_part = format!("{} {right_part}", shorten_path(&session.path));
            }

            // Cursor
            let cursor = if is_selected {
                theme.fg(ThemeColor::Accent, "› ")
            } else {
                "  ".to_string()
            };

            // Calculate available width for message
            let prefix_width = visible_width(&prefix);
            let right_width = visible_width(&right_part) + 2; // +2 for spacing
            // `width - 2 - prefixWidth - rightWidth` can go negative in
            // TypeScript; the `Math.max(10, …)` below absorbs it.
            let available_for_msg =
                width as isize - 2 - prefix_width as isize - right_width as isize; // -2 for cursor

            let truncated_msg = truncate_to_width_opts(
                &normalized_message,
                available_for_msg.max(10) as usize,
                "…",
                false,
            );

            // Style message
            let message_color = if is_confirming_delete {
                Some(ThemeColor::Error)
            } else if is_current {
                Some(ThemeColor::Accent)
            } else if has_name {
                Some(ThemeColor::Warning)
            } else {
                None
            };
            let mut styled_msg = match message_color {
                Some(color) => theme.fg(color, &truncated_msg),
                None => truncated_msg,
            };
            if is_selected {
                styled_msg = theme.bold(&styled_msg);
            }

            // Build line
            let left_part = cursor + &theme.fg(ThemeColor::Dim, &prefix) + &styled_msg;
            let left_width = visible_width(&left_part);
            let spacing = width
                .saturating_sub(left_width)
                .saturating_sub(visible_width(&right_part))
                .max(1);
            let styled_right = theme.fg(
                if is_confirming_delete {
                    ThemeColor::Error
                } else {
                    ThemeColor::Dim
                },
                &right_part,
            );

            let mut line = left_part + &" ".repeat(spacing) + &styled_right;
            if is_selected {
                line = theme.bg(ThemeBg::SelectedBg, &line);
            }
            lines.push(truncate_to_width(&line, width));
        }

        // Add scroll indicator if needed
        if start_index > 0 || end_index < self.filtered_sessions.len() {
            let scroll_text = format!(
                "  ({}/{})",
                self.selected_index + 1,
                self.filtered_sessions.len()
            );
            lines.push(theme.fg(
                ThemeColor::Muted,
                &truncate_to_width_opts(&scroll_text, width, "", false),
            ));
        }

        lines
    }

    fn handle_input(&mut self, key_data: &str) {
        // Handle delete confirmation state first - intercept all keys
        if let Some(path_to_delete) = self.confirming_delete_path.clone() {
            if keybindings_match(key_data, "tui.select.confirm") {
                self.set_confirming_delete_path(None);
                match self.on_delete_session.as_mut() {
                    Some(callback) => callback(&path_to_delete),
                    None => self
                        .events
                        .push(SessionListEvent::DeleteSession(path_to_delete)),
                }
                return;
            }
            if keybindings_match(key_data, "tui.select.cancel") {
                self.set_confirming_delete_path(None);
                return;
            }
            // Ignore all other keys while confirming
            return;
        }

        if keybindings_match(key_data, "tui.input.tab") {
            self.events.push(SessionListEvent::ToggleScope);
            return;
        }

        if keybindings_match(key_data, "app.session.toggleSort") {
            self.events.push(SessionListEvent::ToggleSort);
            return;
        }

        // The only lookup TypeScript makes through the injected manager rather
        // than the global registry.
        if self
            .keybindings
            .borrow()
            .matches(key_data, "app.session.toggleNamedFilter")
        {
            self.events.push(SessionListEvent::ToggleNameFilter);
            return;
        }

        // Ctrl+P: toggle path display
        if keybindings_match(key_data, "app.session.togglePath") {
            self.show_path = !self.show_path;
            let show_path = self.show_path;
            if let Some(callback) = self.on_toggle_path.as_mut() {
                callback(show_path);
            }
            return;
        }

        // Ctrl+D: initiate delete confirmation (useful on terminals that don't distinguish Ctrl+Backspace from Backspace)
        if keybindings_match(key_data, "app.session.delete") {
            self.start_delete_confirmation_for_selected_session();
            return;
        }

        // Rename selected session
        if keybindings_match(key_data, "app.session.rename") {
            if let Some(node) = self.filtered_sessions.get(self.selected_index) {
                let path = node.session.path.clone();
                self.events.push(SessionListEvent::RenameSession(path));
            }
            return;
        }

        // Ctrl+Backspace: non-invasive convenience alias for delete
        // Only triggers deletion when the query is empty; otherwise it is forwarded to the input
        if keybindings_match(key_data, "app.session.deleteNoninvasive") {
            if !self.search_input.get_value().is_empty() {
                self.search_input.handle_input(key_data);
                let query = self.search_input.get_value().to_string();
                self.filter_sessions(&query);
                return;
            }

            self.start_delete_confirmation_for_selected_session();
            return;
        }

        // Up arrow
        if keybindings_match(key_data, "tui.select.up") {
            self.selected_index = self.selected_index.saturating_sub(1);
        }
        // Down arrow
        else if keybindings_match(key_data, "tui.select.down") {
            self.selected_index =
                (self.selected_index + 1).min(self.filtered_sessions.len().saturating_sub(1));
        }
        // Page up - jump up by maxVisible items
        else if keybindings_match(key_data, "tui.select.pageUp") {
            self.selected_index = self.selected_index.saturating_sub(self.max_visible);
        }
        // Page down - jump down by maxVisible items
        else if keybindings_match(key_data, "tui.select.pageDown") {
            self.selected_index = (self.selected_index + self.max_visible)
                .min(self.filtered_sessions.len().saturating_sub(1));
        }
        // Enter
        else if keybindings_match(key_data, "tui.select.confirm") {
            if let Some(node) = self.filtered_sessions.get(self.selected_index) {
                let path = node.session.path.clone();
                if let Some(on_select) = self.on_select.as_mut() {
                    on_select(&path);
                }
            }
        }
        // Escape - cancel
        else if keybindings_match(key_data, "tui.select.cancel") {
            if let Some(on_cancel) = self.on_cancel.as_mut() {
                on_cancel();
            }
        }
        // Pass everything else to search input
        else {
            self.search_input.handle_input(key_data);
            // `searchInput.onSubmit` selects the current item; it fires inside
            // `handleInput`, before the filter below runs.
            if self.submitted.replace(false)
                && let Some(node) = self.filtered_sessions.get(self.selected_index)
            {
                let path = node.session.path.clone();
                if let Some(on_select) = self.on_select.as_mut() {
                    on_select(&path);
                }
            }
            let query = self.search_input.get_value().to_string();
            self.filter_sessions(&query);
        }
    }

    fn as_focusable(&mut self) -> Option<&mut dyn Focusable> {
        Some(self)
    }
}

impl Focusable for SessionList {
    fn focused(&self) -> bool {
        self.focused
    }

    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
        self.search_input.set_focused(focused);
    }
}

// --- deletion ----------------------------------------------------------------------

/// How a session file was removed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeleteMethod {
    /// The `trash` CLI took it.
    Trash,
    /// Removed outright.
    Unlink,
}

/// Result of [`delete_session_file`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeleteOutcome {
    /// Whether the file is gone.
    pub ok: bool,
    /// Which path removed it.
    pub method: DeleteMethod,
    /// Failure text shown in the header.
    pub error: Option<String>,
}

/// Delete a session file, trying the `trash` CLI first, then falling back to unlink
///
/// `spawnSync` and the awaited `unlink` are both blocking in TypeScript too, so
/// the port stays synchronous.
pub fn delete_session_file(session_path: &str) -> DeleteOutcome {
    // Try `trash` first (if installed)
    let trash_args: Vec<&str> = if session_path.starts_with('-') {
        vec!["--", session_path]
    } else {
        vec![session_path]
    };
    let trash_result = std::process::Command::new("trash")
        .args(&trash_args)
        .output();

    let trash_error_hint = || -> Option<String> {
        let mut parts: Vec<String> = Vec::new();
        match trash_result.as_ref() {
            Err(error) => parts.push(error.to_string()),
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let stderr = stderr.trim();
                if !stderr.is_empty() {
                    parts.push(stderr.split('\n').next().unwrap_or(stderr).to_string());
                }
            }
        }
        if parts.is_empty() {
            return None;
        }
        let joined = parts.join(" · ");
        Some(format!("trash: {}", js_slice_chars(&joined, 200)))
    };

    // If trash reports success, or the file is gone afterwards, treat it as successful
    let trash_succeeded = trash_result
        .as_ref()
        .is_ok_and(|output| output.status.success());
    if trash_succeeded || !std::path::Path::new(session_path).exists() {
        return DeleteOutcome {
            ok: true,
            method: DeleteMethod::Trash,
            error: None,
        };
    }

    // Fallback to permanent deletion
    match std::fs::remove_file(session_path) {
        Ok(()) => DeleteOutcome {
            ok: true,
            method: DeleteMethod::Unlink,
            error: None,
        },
        Err(unlink_error) => {
            let error = match trash_error_hint() {
                Some(hint) => format!("{unlink_error} ({hint})"),
                None => unlink_error.to_string(),
            };
            DeleteOutcome {
                ok: false,
                method: DeleteMethod::Unlink,
                error: Some(error),
            }
        }
    }
}

/// `String.prototype.slice(0, limit)` in UTF-16 code units.
fn js_slice_chars(text: &str, limit: usize) -> &str {
    let mut units = 0usize;
    for (offset, character) in text.char_indices() {
        let next = units + character.len_utf16();
        if next > limit {
            return &text[..offset];
        }
        units = next;
    }
    text
}

// --- selector ----------------------------------------------------------------------

/// Why a load was started.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadReason {
    /// The load the constructor starts.
    Initial,
    /// After a delete or a rename.
    Refresh,
    /// The user switched scope.
    Toggle,
}

/// A load the caller has to run.
///
/// TypeScript awaits the loader inside the component; the port hands the request
/// out and takes the result back, which keeps every state transition (scope and
/// sequence checks included) inside the component (class 1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadRequest {
    /// Which loader to run.
    pub scope: SessionScope,
    /// Why the load was started.
    pub reason: LoadReason,
    /// Sequence number of an `all` load; `None` for `current`.
    seq: Option<u64>,
}

/// Optional constructor arguments.
#[derive(Default)]
pub struct SessionSelectorOptions {
    /// Renames a session; `None` disables the rename mode.
    ///
    /// Synchronous, unlike the TypeScript promise: session writes are
    /// synchronous throughout this port.
    pub rename_session: Option<RenameSessionCallback>,
    /// Overrides whether the rename hint is shown.
    pub show_rename_hint: Option<bool>,
    /// The registry the named-filter binding is resolved against.
    pub keybindings: Option<Rc<RefCell<KeybindingsManager>>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SelectorMode {
    List,
    Rename,
}

/// Component that renders a session selector
pub struct SessionSelectorComponent {
    container: Container,
    can_rename: bool,
    session_list: Rc<RefCell<SessionList>>,
    header: Rc<RefCell<SessionSelectorHeader>>,
    scope: SessionScope,
    sort_mode: SortMode,
    name_filter: NameFilter,
    current_sessions: Option<Vec<SessionInfo>>,
    all_sessions: Option<Vec<SessionInfo>>,
    request_render: Rc<dyn Fn()>,
    rename_session: Option<RenameSessionCallback>,
    current_loading: bool,
    all_loading: bool,
    all_load_seq: u64,
    pending_loads: Vec<LoadRequest>,

    mode: SelectorMode,
    rename_input: Rc<RefCell<Input>>,
    rename_submitted: Rc<Cell<Option<String>>>,
    rename_target_path: Option<String>,

    // Focusable implementation - propagate to sessionList for IME cursor positioning
    focused: bool,
}

impl SessionSelectorComponent {
    /// New selector. The loaders are run by the caller (see [`LoadRequest`]).
    pub fn new(
        on_select: SessionPathCallback,
        on_cancel: Box<dyn FnMut()>,
        on_exit: Box<dyn FnMut()>,
        request_render: Rc<dyn Fn()>,
        options: SessionSelectorOptions,
        current_session_file_path: Option<&str>,
    ) -> Self {
        let keybindings = options
            .keybindings
            .unwrap_or_else(|| Rc::new(RefCell::new(KeybindingsManager::create(None))));
        let scope = SessionScope::default();
        let sort_mode = SortMode::Threaded;
        let name_filter = NameFilter::All;
        let header = Rc::new(RefCell::new(SessionSelectorHeader::new(
            scope,
            sort_mode,
            name_filter,
            Rc::clone(&request_render),
        )));
        let rename_session = options.rename_session;
        let can_rename = rename_session.is_some();
        header
            .borrow_mut()
            .set_show_rename_hint(options.show_rename_hint.unwrap_or(can_rename));

        // Create session list (starts empty, will be populated after load)
        let session_list = Rc::new(RefCell::new(SessionList::new(
            Vec::new(),
            false,
            sort_mode,
            name_filter,
            Rc::clone(&keybindings),
            current_session_file_path,
        )));

        let rename_input = Rc::new(RefCell::new(Input::new()));
        let rename_submitted: Rc<Cell<Option<String>>> = Rc::new(Cell::new(None));
        {
            let sink = Rc::clone(&rename_submitted);
            rename_input.borrow_mut().on_submit =
                Some(Box::new(move |value| sink.set(Some(value.to_string()))));
        }

        // Ensure header status timeouts are cleared when leaving the selector
        {
            let mut list = session_list.borrow_mut();
            let mut on_select = on_select;
            let clear = Rc::clone(&header);
            list.on_select = Some(Box::new(move |path| {
                clear.borrow_mut().set_status_message(None, None);
                on_select(path);
            }));
            let mut on_cancel = on_cancel;
            let clear = Rc::clone(&header);
            list.on_cancel = Some(Box::new(move || {
                clear.borrow_mut().set_status_message(None, None);
                on_cancel();
            }));
            let mut on_exit = on_exit;
            let clear = Rc::clone(&header);
            list.on_exit = Some(Box::new(move || {
                clear.borrow_mut().set_status_message(None, None);
                on_exit();
            }));

            // Sync list events to header
            let sync_header = Rc::clone(&header);
            let render = Rc::clone(&request_render);
            list.on_toggle_path = Some(Box::new(move |show_path| {
                sync_header.borrow_mut().set_show_path(show_path);
                render();
            }));
            let sync_header = Rc::clone(&header);
            let render = Rc::clone(&request_render);
            list.on_delete_confirmation_change = Some(Box::new(move |path| {
                sync_header
                    .borrow_mut()
                    .set_confirming_delete_path(path.map(str::to_string));
                render();
            }));
            let sync_header = Rc::clone(&header);
            let render = Rc::clone(&request_render);
            list.on_error = Some(Box::new(move |message| {
                sync_header.borrow_mut().set_status_message(
                    Some(StatusMessage {
                        kind: StatusKind::Error,
                        message: message.to_string(),
                    }),
                    Some(3000),
                );
                render();
            }));
        }

        let mut selector = Self {
            container: Container::new(),
            can_rename,
            session_list,
            header,
            scope,
            sort_mode,
            name_filter,
            current_sessions: None,
            all_sessions: None,
            request_render,
            rename_session,
            current_loading: false,
            all_loading: false,
            all_load_seq: 0,
            pending_loads: Vec::new(),
            mode: SelectorMode::List,
            rename_input,
            rename_submitted,
            rename_target_path: None,
            focused: false,
        };

        let content = Rc::clone(&selector.session_list) as ComponentRef;
        selector.build_base_layout(content, true);

        // Start loading current sessions immediately
        selector.load_current_sessions();
        selector
    }

    fn build_base_layout(&mut self, content: ComponentRef, show_header: bool) {
        let border = || {
            component_ref(DynamicBorder::new(Some(Rc::new(|text: &str| {
                theme().fg(ThemeColor::Accent, text)
            }))))
        };
        self.container.clear();
        self.container.add_child(component_ref(Spacer::new(1)));
        self.container.add_child(border());
        self.container.add_child(component_ref(Spacer::new(1)));
        if show_header {
            self.container
                .add_child(Rc::clone(&self.header) as ComponentRef);
            self.container.add_child(component_ref(Spacer::new(1)));
        }
        self.container.add_child(content);
        self.container.add_child(component_ref(Spacer::new(1)));
        self.container.add_child(border());
    }

    /// The list; the TypeScript tests reach for it to drive the component.
    pub fn get_session_list(&self) -> &Rc<RefCell<SessionList>> {
        &self.session_list
    }

    /// Whether a rename handler was supplied.
    pub fn can_rename(&self) -> bool {
        self.can_rename
    }

    fn load_current_sessions(&mut self) {
        self.load_scope(SessionScope::Current, LoadReason::Initial);
    }

    /// `loadScope` up to the `await`: marks the scope loading and queues the request.
    fn load_scope(&mut self, scope: SessionScope, reason: LoadReason) {
        // Mark loading
        let seq = match scope {
            SessionScope::Current => {
                self.current_loading = true;
                None
            }
            SessionScope::All => {
                self.all_loading = true;
                self.all_load_seq += 1;
                Some(self.all_load_seq)
            }
        };
        self.header.borrow_mut().set_scope(scope);
        self.header.borrow_mut().set_loading(true);
        (self.request_render)();
        self.pending_loads.push(LoadRequest { scope, reason, seq });
    }

    /// The next load the caller has to run.
    pub fn take_pending_load(&mut self) -> Option<LoadRequest> {
        if self.pending_loads.is_empty() {
            return None;
        }
        Some(self.pending_loads.remove(0))
    }

    /// `onProgress` of a running load.
    pub fn report_load_progress(&mut self, request: &LoadRequest, loaded: usize, total: usize) {
        if request.scope != self.scope {
            return;
        }
        if request.seq.is_some() && request.seq != Some(self.all_load_seq) {
            return;
        }
        self.header.borrow_mut().set_progress(loaded, total);
        (self.request_render)();
    }

    /// `loadScope` from the `await` onwards.
    pub fn apply_load_result(
        &mut self,
        request: LoadRequest,
        result: Result<Vec<SessionInfo>, String>,
    ) {
        let show_cwd = request.scope == SessionScope::All;
        match result {
            Ok(sessions) => {
                match request.scope {
                    SessionScope::Current => {
                        self.current_sessions = Some(sessions.clone());
                        self.current_loading = false;
                    }
                    SessionScope::All => {
                        self.all_sessions = Some(sessions.clone());
                        self.all_loading = false;
                    }
                }

                if request.scope != self.scope {
                    return;
                }
                if request.seq.is_some() && request.seq != Some(self.all_load_seq) {
                    return;
                }

                self.header.borrow_mut().set_loading(false);
                self.session_list
                    .borrow_mut()
                    .set_sessions(sessions, show_cwd);
                (self.request_render)();
            }
            Err(message) => {
                match request.scope {
                    SessionScope::Current => self.current_loading = false,
                    SessionScope::All => self.all_loading = false,
                }

                if request.scope != self.scope {
                    return;
                }
                if request.seq.is_some() && request.seq != Some(self.all_load_seq) {
                    return;
                }

                self.header.borrow_mut().set_loading(false);
                self.header.borrow_mut().set_status_message(
                    Some(StatusMessage {
                        kind: StatusKind::Error,
                        message: format!("Failed to load sessions: {message}"),
                    }),
                    Some(4000),
                );

                if request.reason == LoadReason::Initial {
                    self.session_list
                        .borrow_mut()
                        .set_sessions(Vec::new(), show_cwd);
                }
                (self.request_render)();
            }
        }
    }

    /// When the header's status message hides itself.
    pub fn status_deadline(&self) -> Option<Instant> {
        self.header.borrow().status_deadline()
    }

    /// Run a due status auto-hide. `true` when the message went away.
    pub fn tick_status(&mut self) -> bool {
        self.header.borrow_mut().tick_status()
    }

    fn toggle_sort_mode(&mut self) {
        // Cycle: threaded -> recent -> relevance -> threaded
        self.sort_mode = match self.sort_mode {
            SortMode::Threaded => SortMode::Recent,
            SortMode::Recent => SortMode::Relevance,
            SortMode::Relevance => SortMode::Threaded,
        };
        self.header.borrow_mut().set_sort_mode(self.sort_mode);
        self.session_list.borrow_mut().set_sort_mode(self.sort_mode);
        (self.request_render)();
    }

    fn toggle_name_filter(&mut self) {
        self.name_filter = match self.name_filter {
            NameFilter::All => NameFilter::Named,
            NameFilter::Named => NameFilter::All,
        };
        self.header.borrow_mut().set_name_filter(self.name_filter);
        self.session_list
            .borrow_mut()
            .set_name_filter(self.name_filter);
        (self.request_render)();
    }

    fn refresh_sessions_after_mutation(&mut self) {
        self.load_scope(self.scope, LoadReason::Refresh);
    }

    fn toggle_scope(&mut self) {
        if self.scope == SessionScope::Current {
            self.scope = SessionScope::All;
            self.header.borrow_mut().set_scope(self.scope);

            if let Some(sessions) = self.all_sessions.clone() {
                self.header.borrow_mut().set_loading(false);
                self.session_list.borrow_mut().set_sessions(sessions, true);
                (self.request_render)();
                return;
            }

            if !self.all_loading {
                self.load_scope(SessionScope::All, LoadReason::Toggle);
            }
            return;
        }

        self.scope = SessionScope::Current;
        self.header.borrow_mut().set_scope(self.scope);
        let loading = self.current_loading;
        self.header.borrow_mut().set_loading(loading);
        let sessions = self.current_sessions.clone().unwrap_or_default();
        self.session_list.borrow_mut().set_sessions(sessions, false);
        (self.request_render)();
    }

    fn rename_selected_session(&mut self, session_path: &str) {
        if self.rename_session.is_none() {
            return;
        }
        if self.scope == SessionScope::Current && self.current_loading {
            return;
        }
        if self.scope == SessionScope::All && self.all_loading {
            return;
        }

        let sessions = if self.scope == SessionScope::All {
            self.all_sessions.as_deref().unwrap_or_default()
        } else {
            self.current_sessions.as_deref().unwrap_or_default()
        };
        let name = sessions
            .iter()
            .find(|session| session.path == session_path)
            .and_then(|session| session.name.clone());
        self.enter_rename_mode(session_path, name.as_deref());
    }

    fn delete_selected_session(&mut self, session_path: &str) {
        let result = delete_session_file(session_path);

        if result.ok {
            if let Some(sessions) = self.current_sessions.as_mut() {
                sessions.retain(|session| session.path != session_path);
            }
            if let Some(sessions) = self.all_sessions.as_mut() {
                sessions.retain(|session| session.path != session_path);
            }

            let sessions = if self.scope == SessionScope::All {
                self.all_sessions.clone().unwrap_or_default()
            } else {
                self.current_sessions.clone().unwrap_or_default()
            };
            let show_cwd = self.scope == SessionScope::All;
            self.session_list
                .borrow_mut()
                .set_sessions(sessions, show_cwd);

            let message = if result.method == DeleteMethod::Trash {
                "Session moved to trash"
            } else {
                "Session deleted"
            };
            self.header.borrow_mut().set_status_message(
                Some(StatusMessage {
                    kind: StatusKind::Info,
                    message: message.to_string(),
                }),
                Some(2000),
            );
            self.refresh_sessions_after_mutation();
        } else {
            let error_message = result.error.unwrap_or_else(|| "Unknown error".to_string());
            self.header.borrow_mut().set_status_message(
                Some(StatusMessage {
                    kind: StatusKind::Error,
                    message: format!("Failed to delete: {error_message}"),
                }),
                Some(3000),
            );
        }

        (self.request_render)();
    }

    fn enter_rename_mode(&mut self, session_path: &str, current_name: Option<&str>) {
        self.mode = SelectorMode::Rename;
        self.rename_target_path = Some(session_path.to_string());
        self.rename_input
            .borrow_mut()
            .set_value(current_name.unwrap_or(""));
        self.rename_input.borrow_mut().set_focused(true);

        let theme = theme();
        let mut panel = Container::new();
        panel.add_child(component_ref(Text::new(theme.bold("Rename Session"), 1, 0)));
        panel.add_child(component_ref(Spacer::new(1)));
        panel.add_child(Rc::clone(&self.rename_input) as ComponentRef);
        panel.add_child(component_ref(Spacer::new(1)));
        panel.add_child(component_ref(Text::new(
            theme.fg(
                ThemeColor::Muted,
                &format!(
                    "{} to save · {} to cancel",
                    key_text("tui.select.confirm"),
                    key_text("tui.select.cancel")
                ),
            ),
            1,
            0,
        )));

        self.build_base_layout(component_ref(panel), false);
        (self.request_render)();
    }

    fn exit_rename_mode(&mut self) {
        self.mode = SelectorMode::List;
        self.rename_target_path = None;

        let content = Rc::clone(&self.session_list) as ComponentRef;
        self.build_base_layout(content, true);

        (self.request_render)();
    }

    fn confirm_rename(&mut self, value: &str) {
        let next = value.trim().to_string();
        if next.is_empty() {
            return;
        }
        let Some(target) = self.rename_target_path.clone() else {
            self.exit_rename_mode();
            return;
        };

        // Find current name for callback
        if self.rename_session.is_none() {
            self.exit_rename_mode();
            return;
        }

        if let Some(rename_session) = self.rename_session.as_mut() {
            rename_session(&target, &next);
        }
        self.refresh_sessions_after_mutation();
        self.exit_rename_mode();
    }

    fn drain_list_events(&mut self) {
        loop {
            let events = self.session_list.borrow_mut().take_events();
            if events.is_empty() {
                return;
            }
            for event in events {
                match event {
                    SessionListEvent::ToggleScope => self.toggle_scope(),
                    SessionListEvent::ToggleSort => self.toggle_sort_mode(),
                    SessionListEvent::ToggleNameFilter => self.toggle_name_filter(),
                    SessionListEvent::RenameSession(path) => self.rename_selected_session(&path),
                    SessionListEvent::DeleteSession(path) => self.delete_selected_session(&path),
                }
            }
        }
    }
}

impl Component for SessionSelectorComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.container.render(width)
    }

    fn invalidate(&mut self) {
        self.container.invalidate();
    }

    fn handle_input(&mut self, data: &str) {
        if self.mode == SelectorMode::Rename {
            if keybindings_match(data, "tui.select.cancel") {
                self.exit_rename_mode();
                return;
            }
            self.rename_input.borrow_mut().handle_input(data);
            if let Some(value) = self.rename_submitted.take() {
                self.confirm_rename(&value);
            }
            return;
        }

        self.session_list.borrow_mut().handle_input(data);
        self.drain_list_events();
    }

    fn as_focusable(&mut self) -> Option<&mut dyn Focusable> {
        Some(self)
    }
}

impl Focusable for SessionSelectorComponent {
    fn focused(&self) -> bool {
        self.focused
    }

    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
        self.session_list.borrow_mut().set_focused(focused);
        self.rename_input.borrow_mut().set_focused(focused);
        if focused && self.mode == SelectorMode::Rename {
            self.rename_input.borrow_mut().set_focused(true);
        }
    }
}
