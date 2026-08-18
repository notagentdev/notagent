//! 1:1 port of
//! `packages/coding-agent/src/modes/interactive/components/bash-execution.ts` (220 LOC).
//!
//! Component for displaying bash command execution with streaming output.

use std::cell::RefCell;
use std::rc::Rc;

use notagent_tui::components::loader::Loader;
use notagent_tui::components::spacer::Spacer;
use notagent_tui::components::text::Text;
use notagent_tui::tui::{Component, ComponentRef, Container, Line, component_ref};

use crate::core::tools::truncate::{
    DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES, TruncationOptions, TruncationResult, truncate_tail,
};
use crate::modes::interactive::theme::theme::{ThemeColor, theme};
use crate::utils::ansi::strip_ansi;

use super::dynamic_border::DynamicBorder;
use super::keybinding_hints::{key_hint, key_text};
use super::visual_truncate::truncate_to_visual_lines;

/// Preview line limit when not expanded (matches tool execution behavior)
const PREVIEW_LINES: usize = 20;

/// Lifecycle of the command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Status {
    Running,
    Complete,
    Cancelled,
    Error,
}

/// Caches the truncated preview per render width, like the inline component
/// literal in TypeScript.
struct PreviewLines {
    styled_input: String,
    cached_width: Option<usize>,
    cached_lines: Option<Vec<Line>>,
}

impl Component for PreviewLines {
    fn render(&mut self, width: usize) -> Vec<Line> {
        if self.cached_lines.is_none() || self.cached_width != Some(width) {
            let result = truncate_to_visual_lines(&self.styled_input, PREVIEW_LINES, width, 1);
            self.cached_lines = Some(result.visual_lines);
            self.cached_width = Some(width);
        }
        self.cached_lines.clone().unwrap_or_default()
    }

    fn invalidate(&mut self) {
        self.cached_width = None;
        self.cached_lines = None;
    }
}

/// Bordered block showing a bash command and its streaming output.
pub struct BashExecutionComponent {
    container: Container,
    content_container: Rc<RefCell<Container>>,
    loader: ComponentRef,
    loader_handle: Rc<RefCell<Loader>>,
    command: String,
    output_lines: Vec<String>,
    status: Status,
    exit_code: Option<i64>,
    truncation_result: Option<TruncationResult>,
    full_output_path: Option<String>,
    expanded: bool,
}

impl BashExecutionComponent {
    /// `exclude_from_context` marks the `!!` prefix, which uses a dim border.
    pub fn new(command: impl Into<String>, exclude_from_context: bool) -> Self {
        let command = command.into();
        let mut container = Container::new();

        // Use dim border for excluded-from-context commands (!! prefix)
        let color_key = if exclude_from_context {
            ThemeColor::Dim
        } else {
            ThemeColor::BashMode
        };
        let border_color: Rc<dyn Fn(&str) -> String> =
            Rc::new(move |text: &str| theme().fg(color_key, text));

        // Add spacer
        container.add_child(component_ref(Spacer::new(1)));

        // Top border
        container.add_child(component_ref(DynamicBorder::new(Some(Rc::clone(
            &border_color,
        )))));

        // Content container (holds dynamic content between borders)
        let content_container = Rc::new(RefCell::new(Container::new()));
        container.add_child(Rc::clone(&content_container) as ComponentRef);

        // Command header
        let theme_instance = theme();
        let header = Text::new(
            theme_instance.fg(color_key, &theme_instance.bold(&format!("$ {command}"))),
            1,
            0,
        );
        content_container
            .borrow_mut()
            .add_child(component_ref(header));

        // Loader
        let loader_handle = Rc::new(RefCell::new(Loader::new(
            Rc::new(move |spinner: &str| theme().fg(color_key, spinner)),
            Rc::new(|text: &str| theme().fg(ThemeColor::Muted, text)),
            format!("Running... ({} to cancel)", key_text("tui.select.cancel")), // Plain text for loader
            None,
        )));
        let loader: ComponentRef = Rc::clone(&loader_handle) as ComponentRef;
        content_container.borrow_mut().add_child(Rc::clone(&loader));

        // Bottom border
        container.add_child(component_ref(DynamicBorder::new(Some(border_color))));

        Self {
            container,
            content_container,
            loader,
            loader_handle,
            command,
            output_lines: Vec::new(),
            status: Status::Running,
            exit_code: None,
            truncation_result: None,
            full_output_path: None,
            expanded: false,
        }
    }

    /// Set whether the output is expanded (shows full output) or collapsed (preview only).
    pub fn set_expanded(&mut self, expanded: bool) {
        self.expanded = expanded;
        self.update_display();
    }

    /// Append a chunk of streamed output.
    pub fn append_output(&mut self, chunk: &str) {
        // Strip ANSI codes and normalize line endings
        // Note: binary data is already sanitized in tui-renderer.rs execute_bash_command
        let clean = strip_ansi(chunk).replace("\r\n", "\n").replace('\r', "\n");

        // Append to output lines
        let new_lines: Vec<&str> = clean.split('\n').collect();
        if !self.output_lines.is_empty() && !new_lines.is_empty() {
            // Append first chunk to last line (incomplete line continuation)
            let last = self.output_lines.len() - 1;
            self.output_lines[last].push_str(new_lines[0]);
            self.output_lines
                .extend(new_lines[1..].iter().map(|line| (*line).to_string()));
        } else {
            self.output_lines
                .extend(new_lines.iter().map(|line| (*line).to_string()));
        }

        self.update_display();
    }

    /// Mark the command as finished.
    pub fn set_complete(
        &mut self,
        exit_code: Option<i64>,
        cancelled: bool,
        truncation_result: Option<TruncationResult>,
        full_output_path: Option<String>,
    ) {
        self.exit_code = exit_code;
        self.status = if cancelled {
            Status::Cancelled
        } else if exit_code.is_some_and(|code| code != 0) {
            Status::Error
        } else {
            Status::Complete
        };
        self.truncation_result = truncation_result;
        self.full_output_path = full_output_path;

        // Stop loader
        self.loader_handle.borrow_mut().stop();

        self.update_display();
    }

    fn update_display(&mut self) {
        // Apply truncation for LLM context limits (same limits as bash tool)
        let full_output = self.output_lines.join("\n");
        let context_truncation = truncate_tail(
            &full_output,
            TruncationOptions {
                max_lines: Some(DEFAULT_MAX_LINES),
                max_bytes: Some(DEFAULT_MAX_BYTES),
            },
        );

        // Get the lines to potentially display (after context truncation)
        let available_lines: Vec<&str> = if context_truncation.content.is_empty() {
            Vec::new()
        } else {
            context_truncation.content.split('\n').collect()
        };

        // Apply preview truncation based on expanded state
        let preview_start = available_lines.len().saturating_sub(PREVIEW_LINES);
        let preview_logical_lines = &available_lines[preview_start..];
        let hidden_line_count = available_lines.len() - preview_logical_lines.len();

        // Rebuild content container
        let mut content_container = self.content_container.borrow_mut();
        content_container.clear();

        // Command header. TypeScript always uses `bashMode` here, even when the
        // constructor drew the borders in `dim` for a `!!` command (bug-compat).
        let theme_instance = theme();
        let header = Text::new(
            theme_instance.fg(
                ThemeColor::BashMode,
                &theme_instance.bold(&format!("$ {}", self.command)),
            ),
            1,
            0,
        );
        content_container.add_child(component_ref(header));

        // Output
        if !available_lines.is_empty() {
            if self.expanded {
                // Show all lines
                let display_text = available_lines
                    .iter()
                    .map(|line| theme_instance.fg(ThemeColor::Muted, line))
                    .collect::<Vec<_>>()
                    .join("\n");
                content_container.add_child(component_ref(Text::new(
                    format!("\n{display_text}"),
                    1,
                    0,
                )));
            } else {
                // Use shared visual truncation utility with width-aware caching
                let styled_output = preview_logical_lines
                    .iter()
                    .map(|line| theme_instance.fg(ThemeColor::Muted, line))
                    .collect::<Vec<_>>()
                    .join("\n");
                content_container.add_child(component_ref(PreviewLines {
                    styled_input: format!("\n{styled_output}"),
                    cached_width: None,
                    cached_lines: None,
                }));
            }
        }

        // Loader or status
        if self.status == Status::Running {
            content_container.add_child(Rc::clone(&self.loader));
        } else {
            let mut status_parts: Vec<String> = Vec::new();

            // Show how many lines are hidden (collapsed preview)
            if hidden_line_count > 0 {
                if self.expanded {
                    status_parts.push(format!(
                        "{}{}{}",
                        theme_instance.fg(ThemeColor::Muted, "("),
                        key_hint("app.tools.expand", "to collapse"),
                        theme_instance.fg(ThemeColor::Muted, ")")
                    ));
                } else {
                    status_parts.push(format!(
                        "{}{}{}",
                        theme_instance.fg(
                            ThemeColor::Muted,
                            &format!("... {hidden_line_count} more lines (")
                        ),
                        key_hint("app.tools.expand", "to expand"),
                        theme_instance.fg(ThemeColor::Muted, ")")
                    ));
                }
            }

            if self.status == Status::Cancelled {
                status_parts.push(theme_instance.fg(ThemeColor::Warning, "(cancelled)"));
            } else if self.status == Status::Error {
                status_parts.push(theme_instance.fg(
                    ThemeColor::Error,
                    &format!("(exit {})", format_exit_code(self.exit_code)),
                ));
            }

            // Add truncation warning (context truncation, not preview truncation)
            let was_truncated = self
                .truncation_result
                .as_ref()
                .is_some_and(|result| result.truncated)
                || context_truncation.truncated;
            if was_truncated && let Some(full_output_path) = self.full_output_path.as_ref() {
                status_parts.push(theme_instance.fg(
                    ThemeColor::Warning,
                    &format!("Output truncated. Full output: {full_output_path}"),
                ));
            }

            if !status_parts.is_empty() {
                content_container.add_child(component_ref(Text::new(
                    format!("\n{}", status_parts.join("\n")),
                    1,
                    0,
                )));
            }
        }
    }

    /// Get the raw output for creating BashExecutionMessage.
    pub fn get_output(&self) -> String {
        self.output_lines.join("\n")
    }

    /// Get the command that was executed.
    pub fn get_command(&self) -> &str {
        &self.command
    }
}

/// `${this.exitCode}` for an exit code that may be absent.
fn format_exit_code(exit_code: Option<i64>) -> String {
    match exit_code {
        Some(exit_code) => exit_code.to_string(),
        None => "undefined".to_string(),
    }
}

impl Component for BashExecutionComponent {
    fn render(&mut self, width: usize) -> Vec<Line> {
        self.container.render(width)
    }

    fn invalidate(&mut self) {
        self.container.invalidate();
        self.update_display();
    }
}
