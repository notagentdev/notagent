use std::rc::Rc;

use notagent_ai::types::{AssistantContent, AssistantMessage, StopReason};
use notagent_tui::components::markdown::{
    DefaultTextStyle, Markdown, MarkdownOptions, MarkdownTheme,
};
use notagent_tui::components::spacer::Spacer;
use notagent_tui::components::text::Text;
use notagent_tui::tui::{Component, Container, Line, component_ref};

use crate::modes::interactive::theme::theme::{
    BlockStyle, ThemeColor, block_style, format_elapsed_live, format_elapsed_precise,
    get_markdown_theme, theme,
};

use super::keybinding_hints::key_text;
use super::markdown_transform::{
    MarkdownMessageType, MarkdownTransformer, create_markdown_transform,
};

const OSC133_ZONE_START: &str = "\x1b]133;A\x07";
const OSC133_ZONE_END: &str = "\x1b]133;B\x07";
const OSC133_ZONE_FINAL: &str = "\x1b]133;C\x07";

/// Component that renders a complete assistant message
pub struct AssistantMessageComponent {
    /// renders it directly instead of holding it twice.
    content_container: Container,
    hide_thinking_block: bool,
    markdown_theme: MarkdownTheme,
    hidden_thinking_label: String,
    output_pad: usize,
    markdown_transformers: Vec<MarkdownTransformer>,
    last_message: Option<AssistantMessage>,
    has_tool_calls: bool,
    is_streaming: bool,
    /// Whether the compact thinking block shows its text.
    thinking_expanded: bool,
    /// When live thinking started streaming into this component.
    thinking_started: Option<std::time::Instant>,
    /// The frozen thinking duration once it ended. Replayed messages carry
    /// none and show the Thought heading without a runtime.
    thinking_finished: Option<std::time::Duration>,
    /// The last rendered timer second, so a tick rebuilds at most once per
    /// second.
    last_thinking_tick: Option<u64>,
}

impl AssistantMessageComponent {
    /// `markdownTheme = getMarkdownTheme()`, `hiddenThinkingLabel = "Thinking..."`,
    /// `outputPad = 1` and no transformers.
    pub fn new(
        message: Option<AssistantMessage>,
        hide_thinking_block: bool,
        markdown_theme: Option<MarkdownTheme>,
        hidden_thinking_label: Option<String>,
        output_pad: Option<usize>,
        markdown_transformers: Vec<MarkdownTransformer>,
    ) -> Self {
        let mut component = Self {
            // Container for text/thinking content
            content_container: Container::new(),
            hide_thinking_block,
            markdown_theme: markdown_theme.unwrap_or_else(get_markdown_theme),
            hidden_thinking_label: hidden_thinking_label
                .unwrap_or_else(|| "Thinking...".to_string()),
            output_pad: output_pad.unwrap_or(1),
            markdown_transformers,
            last_message: None,
            has_tool_calls: false,
            is_streaming: false,
            thinking_expanded: false,
            thinking_started: None,
            thinking_finished: None,
            last_thinking_tick: None,
        };
        if let Some(message) = message {
            component.update_content(message, None);
        }
        component
    }

    /// Hide or show thinking blocks.
    pub fn set_hide_thinking_block(&mut self, hide: bool) {
        self.hide_thinking_block = hide;
        if let Some(message) = self.last_message.clone() {
            self.update_content(message, None);
        }
    }

    /// Label shown instead of a hidden thinking block.
    pub fn set_hidden_thinking_label(&mut self, label: impl Into<String>) {
        self.hidden_thinking_label = label.into();
        if let Some(message) = self.last_message.clone() {
            self.update_content(message, None);
        }
    }

    /// Change the horizontal padding.
    pub fn set_output_pad(&mut self, padding: usize) {
        self.output_pad = padding;
        if let Some(message) = self.last_message.clone() {
            self.update_content(message, None);
        }
    }

    /// Whether this component's thinking is still streaming — the heading
    /// shows Thinking with a live timer and needs the one-second tick.
    pub fn has_running_thinking(&self) -> bool {
        self.thinking_started.is_some() && self.thinking_finished.is_none()
    }

    /// Advances the Thinking heading's timer. Returns `true` when the visible
    /// second changed and the caller should request a render (reference
    /// `tick_thinking_timer`).
    pub fn tick_thinking_timer(&mut self) -> bool {
        if !self.has_running_thinking() {
            return false;
        }
        let Some(started) = self.thinking_started else {
            return false;
        };
        let second = started.elapsed().as_secs();
        if self.last_thinking_tick == Some(second) {
            return false;
        }
        self.last_thinking_tick = Some(second);
        if let Some(message) = self.last_message.clone() {
            self.update_content(message, None);
        }
        true
    }

    /// Opens or closes the compact thinking block through the chat's expandables.
    pub fn set_expanded(&mut self, expanded: bool) {
        if self.thinking_expanded == expanded {
            return;
        }
        self.thinking_expanded = expanded;
        if let Some(message) = self.last_message.clone() {
            self.update_content(message, None);
        }
    }

    /// The thinking timer runs from the first streamed thinking content until
    /// visible text or a tool call follows it (or streaming ends). Derived
    fn derive_thinking_timer(&mut self, message: &AssistantMessage) {
        let has_thinking = message.content.iter().any(|content| {
            matches!(content, AssistantContent::Thinking(thinking) if !thinking.thinking.trim().is_empty())
        });
        if self.is_streaming {
            if has_thinking && self.thinking_started.is_none() && self.thinking_finished.is_none() {
                self.thinking_started = Some(std::time::Instant::now());
            }
            if let Some(started) = self.thinking_started
                && self.thinking_finished.is_none()
            {
                let last_thinking = message.content.iter().rposition(|content| {
                    matches!(content, AssistantContent::Thinking(thinking) if !thinking.thinking.trim().is_empty())
                });
                let followed = last_thinking.is_some_and(|index| {
                    message.content[index + 1..].iter().any(|content| {
                        matches!(content, AssistantContent::Text(text) if !text.text.trim().is_empty())
                            || matches!(content, AssistantContent::ToolCall(_))
                    })
                });
                if followed {
                    self.thinking_finished = Some(started.elapsed());
                }
            }
        } else if let Some(started) = self.thinking_started
            && self.thinking_finished.is_none()
        {
            self.thinking_finished = Some(started.elapsed());
        }
    }

    /// Rebuild the rendered content; `is_streaming` defaults to the current value.
    pub fn update_content(&mut self, message: AssistantMessage, is_streaming: Option<bool>) {
        self.last_message = Some(message.clone());
        self.is_streaming = is_streaming.unwrap_or(self.is_streaming);
        self.derive_thinking_timer(&message);

        // Clear content container
        self.content_container.clear();

        let has_visible_content = message.content.iter().any(is_visible_content);

        if has_visible_content {
            self.content_container
                .add_child(component_ref(Spacer::new(1)));
        }

        // Render content in order
        let mut index = 0;
        while index < message.content.len() {
            match &message.content[index] {
                AssistantContent::Text(content) if !content.text.trim().is_empty() => {
                    // Assistant text messages with no background - trim the text
                    // Set paddingY=0 to avoid extra spacing before tool executions
                    self.content_container
                        .add_child(component_ref(Markdown::new(
                            content.text.trim(),
                            self.output_pad,
                            0,
                            self.markdown_theme.clone(),
                            None,
                            Some(MarkdownOptions {
                                transform: Some(create_markdown_transform(
                                    MarkdownMessageType::Assistant,
                                    self.is_streaming,
                                    self.markdown_transformers.clone(),
                                )),
                                ..MarkdownOptions::default()
                            }),
                        )));
                    index += 1;
                }
                AssistantContent::Thinking(_) => {
                    let run_start = index;
                    let mut thinking_blocks: Vec<String> = Vec::new();
                    while index < message.content.len() {
                        let AssistantContent::Thinking(thinking_content) = &message.content[index]
                        else {
                            break;
                        };
                        let thinking = thinking_content.thinking.trim();
                        if !thinking.is_empty() {
                            thinking_blocks.push(thinking.to_string());
                        }
                        index += 1;
                    }

                    if thinking_blocks.is_empty() {
                        continue;
                    }

                    // Hidden thinking is hidden in every style. The compact
                    // style has no static label to fall back to, so hiding
                    // drops the block whole.
                    if self.hide_thinking_block && block_style() == BlockStyle::Badge {
                        continue;
                    }

                    // Add spacing only when another visible assistant content block follows.
                    // This avoids a superfluous blank line before separately-rendered tool
                    // execution blocks.
                    let has_visible_content_after =
                        message.content[index..].iter().any(is_visible_content);

                    if block_style() == BlockStyle::Badge {
                        // The compact style renders thinking as its own block:
                        // a muted heading with a live timer while it streams,
                        // then the text itself behind the expand toggle.
                        let is_last_run = !message.content[index..]
                            .iter()
                            .any(|content| matches!(content, AssistantContent::Thinking(_)));
                        let running = is_last_run && self.has_running_thinking();
                        let theme_instance = theme();
                        let mut heading = format!(
                            "{}{}",
                            theme_instance.fg(ThemeColor::Muted, "*"),
                            theme_instance.fg(
                                ThemeColor::Muted,
                                if running { "Thinking" } else { "Thought" },
                            ),
                        );
                        // The runtime sits next to the heading — live it stays
                        // invisible below one second, final it reads as ms
                        // only under a second.
                        let duration_text = if running {
                            self.thinking_started
                                .and_then(|started| format_elapsed_live(started.elapsed()))
                        } else if is_last_run {
                            self.thinking_finished.map(format_elapsed_precise)
                        } else {
                            None
                        };
                        if let Some(duration_text) = duration_text {
                            heading.push_str(&format!(
                                " {}{}{}",
                                theme_instance.fg(ThemeColor::Dim, "("),
                                theme_instance.fg(ThemeColor::ThinkingText, &duration_text),
                                theme_instance.fg(ThemeColor::Dim, ")")
                            ));
                        }
                        // A thinking run following earlier sections of the
                        // same message needs its own breathing room — without
                        // it the heading sticks to the preceding answer text.
                        if run_start > 0 {
                            self.content_container
                                .add_child(component_ref(Spacer::new(1)));
                        }
                        self.content_container
                            .add_child(component_ref(Text::new(heading, 0, 0)));
                        if self.thinking_expanded {
                            self.content_container
                                .add_child(component_ref(Markdown::new(
                                    thinking_blocks.join("\n\n"),
                                    self.output_pad,
                                    0,
                                    self.markdown_theme.clone(),
                                    Some(DefaultTextStyle {
                                        color: Some(Rc::new(|text: &str| {
                                            theme().fg(ThemeColor::ThinkingText, text)
                                        })),
                                        italic: true,
                                        ..DefaultTextStyle::default()
                                    }),
                                    Some(MarkdownOptions {
                                        transform: Some(create_markdown_transform(
                                            MarkdownMessageType::AssistantThinking,
                                            self.is_streaming,
                                            self.markdown_transformers.clone(),
                                        )),
                                        ..MarkdownOptions::default()
                                    }),
                                )));
                        }
                        // The info line closes the block: under the heading
                        // collapsed and under the thinking text expanded; the
                        // runtime lives on the heading line.
                        if !running {
                            self.content_container.add_child(component_ref(Text::new(
                                theme_instance.fg(
                                    ThemeColor::Dim,
                                    &format!(
                                        "({} to {})",
                                        key_text("app.tools.expand"),
                                        if self.thinking_expanded {
                                            "collapse"
                                        } else {
                                            "expand"
                                        }
                                    ),
                                ),
                                self.output_pad,
                                0,
                            )));
                        }
                    } else if self.hide_thinking_block {
                        // Show one static label for each run of thinking blocks when hidden.
                        let theme = theme();
                        self.content_container.add_child(component_ref(Text::new(
                            theme.italic(
                                &theme.fg(ThemeColor::ThinkingText, &self.hidden_thinking_label),
                            ),
                            self.output_pad,
                            0,
                        )));
                    } else {
                        // Render each run of thinking blocks as one Markdown section.
                        self.content_container
                            .add_child(component_ref(Markdown::new(
                                thinking_blocks.join("\n\n"),
                                self.output_pad,
                                0,
                                self.markdown_theme.clone(),
                                Some(DefaultTextStyle {
                                    color: Some(Rc::new(|text: &str| {
                                        theme().fg(ThemeColor::ThinkingText, text)
                                    })),
                                    italic: true,
                                    ..DefaultTextStyle::default()
                                }),
                                Some(MarkdownOptions {
                                    transform: Some(create_markdown_transform(
                                        MarkdownMessageType::AssistantThinking,
                                        self.is_streaming,
                                        self.markdown_transformers.clone(),
                                    )),
                                    ..MarkdownOptions::default()
                                }),
                            )));
                    }
                    if has_visible_content_after {
                        self.content_container
                            .add_child(component_ref(Spacer::new(1)));
                    }
                }
                _ => index += 1,
            }
        }

        // Check if incomplete/failed - show after partial content.
        // For aborted/error tool calls, tool execution components show the error.
        // Length stops can happen before a tool call is complete, so surface them here too.
        let has_tool_calls = message
            .content
            .iter()
            .any(|content| matches!(content, AssistantContent::ToolCall(_)));
        self.has_tool_calls = has_tool_calls;
        if message.stop_reason == StopReason::Length {
            self.content_container
                .add_child(component_ref(Spacer::new(1)));
            self.content_container.add_child(component_ref(Text::new(
                theme().fg(
                    ThemeColor::Error,
                    "Response was truncated before completion.",
                ),
                self.output_pad,
                0,
            )));
        } else if !has_tool_calls {
            if message.stop_reason == StopReason::Aborted {
                let abort_message = match message.error_message.as_deref() {
                    Some(error_message)
                        if !error_message.is_empty() && error_message != "Request was aborted" =>
                    {
                        error_message
                    }
                    _ => "Operation aborted",
                };
                self.content_container
                    .add_child(component_ref(Spacer::new(1)));
                self.content_container.add_child(component_ref(Text::new(
                    theme().fg(ThemeColor::Error, abort_message),
                    self.output_pad,
                    0,
                )));
            } else if message.stop_reason == StopReason::Error {
                let error_message = match message.error_message.as_deref() {
                    Some(error_message) if !error_message.is_empty() => error_message,
                    _ => "Unknown error",
                };
                self.content_container
                    .add_child(component_ref(Spacer::new(1)));
                self.content_container.add_child(component_ref(Text::new(
                    theme().fg(ThemeColor::Error, &format!("Error: {error_message}")),
                    self.output_pad,
                    0,
                )));
            }
        }
    }
}

fn is_visible_content(content: &AssistantContent) -> bool {
    match content {
        AssistantContent::Text(text) => !text.text.trim().is_empty(),
        AssistantContent::Thinking(thinking) => !thinking.thinking.trim().is_empty(),
        AssistantContent::ToolCall(_) => false,
    }
}

impl Component for AssistantMessageComponent {
    fn render(&mut self, width: usize) -> Vec<Line> {
        let mut lines = self.content_container.render(width);
        if self.has_tool_calls || lines.is_empty() {
            return lines;
        }

        lines[0] = Line::from(format!("{OSC133_ZONE_START}{}", lines[0]));
        let last = lines.len() - 1;
        lines[last] = Line::from(format!(
            "{OSC133_ZONE_END}{OSC133_ZONE_FINAL}{}",
            lines[last]
        ));
        lines
    }

    fn invalidate(&mut self) {
        self.content_container.invalidate();
        if let Some(message) = self.last_message.clone() {
            self.update_content(message, None);
        }
    }
}
