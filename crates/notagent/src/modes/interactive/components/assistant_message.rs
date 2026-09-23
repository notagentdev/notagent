use std::rc::Rc;

use notagent_ai::types::{
    AssistantContent, AssistantMessage, StopReason, TextContent, ThinkingContent,
};
use notagent_tui::components::markdown::{
    DefaultTextStyle, Markdown, MarkdownOptions, MarkdownTheme,
};
use notagent_tui::components::spacer::Spacer;
use notagent_tui::components::text::Text;
use notagent_tui::components::truncated_text::TruncatedText;
use notagent_tui::tui::{Component, Container, Line, component_ref};

use crate::modes::interactive::theme::theme::{
    ThemeColor, block_style, format_elapsed_live, format_elapsed_precise, get_markdown_theme, theme,
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
    regular_streaming: bool,
    stream_committed_block: usize,
    stream_committed_source: String,
    stream_scan_offset: usize,
    stream_structural_holdback: bool,
    /// Rows of the committed render already handed to native history.
    stream_emitted_lines: usize,
    /// Byte range, in `stream_committed_source`, of the chunk committed last.
    stream_last_chunk: Option<(usize, usize)>,
    /// Width those rows were rendered at.
    stream_width: Option<usize>,
    stream_pending_scan: bool,
    stream_needs_reflow: bool,
    stream_preview_cache: Option<(usize, Vec<Line>)>,
    marker: &'static str,
    leading_spacer: bool,
    zone_markers: bool,
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
            regular_streaming: false,
            stream_committed_block: 0,
            stream_committed_source: String::new(),
            stream_scan_offset: 0,
            stream_structural_holdback: false,
            stream_emitted_lines: 0,
            stream_last_chunk: None,
            stream_width: None,
            stream_pending_scan: true,
            stream_needs_reflow: false,
            stream_preview_cache: None,
            marker: "\u{283f}",
            leading_spacer: true,
            zone_markers: true,
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

    pub fn set_regular_streaming(&mut self, regular: bool) {
        if self.regular_streaming != regular {
            self.reset_stream_history();
        }
        self.regular_streaming = regular;
    }

    pub fn has_stream_history(&self) -> bool {
        self.stream_emitted_lines > 0
    }

    pub fn take_stream_reflow(&mut self) -> bool {
        std::mem::take(&mut self.stream_needs_reflow)
    }

    pub fn append_stream_delta(
        &mut self,
        content_index: usize,
        delta: &str,
        thinking: bool,
    ) -> bool {
        if !self.regular_streaming || !self.is_streaming {
            return false;
        }
        let Some(block) = self
            .last_message
            .as_mut()
            .and_then(|message| message.content.get_mut(content_index))
        else {
            return false;
        };
        match (block, thinking) {
            (AssistantContent::Text(text), false) => text.text.push_str(delta),
            (AssistantContent::Thinking(content), true) => content.thinking.push_str(delta),
            _ => return false,
        }
        if content_index < self.stream_committed_block {
            self.reset_stream_history();
            self.stream_needs_reflow = true;
        }
        if !delta.trim().is_empty() {
            if thinking && self.thinking_started.is_none() && self.thinking_finished.is_none() {
                self.thinking_started = Some(std::time::Instant::now());
            } else if !thinking
                && let Some(started) = self.thinking_started
                && self.thinking_finished.is_none()
            {
                self.thinking_finished = Some(started.elapsed());
            }
        }
        self.stream_preview_cache = None;
        self.stream_pending_scan = true;
        true
    }

    fn render_stream_message(
        &self,
        message: AssistantMessage,
        width: usize,
        first: bool,
    ) -> Vec<Line> {
        let mut fragment = Self::new(
            None,
            self.hide_thinking_block,
            Some(self.markdown_theme.clone()),
            Some(self.hidden_thinking_label.clone()),
            Some(self.output_pad),
            self.markdown_transformers.clone(),
        );
        fragment.marker = if first { "\u{283f}" } else { " " };
        fragment.leading_spacer = first;
        fragment.zone_markers = first;
        fragment.thinking_expanded = self.thinking_expanded;
        fragment.thinking_started = self.thinking_started;
        fragment.thinking_finished = self.thinking_finished;
        fragment.update_content(message, Some(true));
        fragment.render(width)
    }

    fn content_fragment(
        message: &AssistantMessage,
        content: Vec<AssistantContent>,
    ) -> AssistantMessage {
        AssistantMessage {
            content,
            api: message.api.clone(),
            provider: message.provider.clone(),
            model: message.model.clone(),
            response_model: message.response_model.clone(),
            response_id: message.response_id.clone(),
            diagnostics: message.diagnostics.clone(),
            usage: message.usage,
            stop_reason: message.stop_reason,
            deferred: message.deferred.clone(),
            error_message: message.error_message.clone(),
            raw_stop_reason: message.raw_stop_reason.clone(),
            end_turn: message.end_turn,
            timestamp: message.timestamp,
        }
    }

    /// The committed part of the stream rendered exactly as the finished
    /// message will render it, so its lines are a prefix of the final render
    /// and the end of the stream only appends the rest. Rendering each
    /// paragraph on its own drifted from the whole-message layout and forced a
    /// scrollback rebuild after every streamed answer.
    fn render_committed_stream(&self, message: &AssistantMessage, width: usize) -> Vec<Line> {
        let mut content = message.content[..self.stream_committed_block].to_vec();
        if !self.stream_committed_source.is_empty() {
            content.push(AssistantContent::Text(TextContent::new(
                self.stream_committed_source.as_str(),
            )));
        }
        let mut committed = Self::content_fragment(message, content);
        // A failure notice belongs after the whole message, not after a prefix.
        committed.stop_reason = StopReason::Stop;
        committed.error_message = None;
        let mut fragment = Self::new(
            None,
            self.hide_thinking_block,
            Some(self.markdown_theme.clone()),
            Some(self.hidden_thinking_label.clone()),
            Some(self.output_pad),
            self.markdown_transformers.clone(),
        );
        // The zone end belongs to the final row, which a prefix never has.
        fragment.zone_markers = false;
        fragment.thinking_expanded = self.thinking_expanded;
        fragment.thinking_started = self.thinking_started;
        fragment.thinking_finished = self.thinking_finished;
        fragment.update_content(committed, Some(false));
        let mut lines = fragment.render(width);
        if !fragment.has_tool_calls
            && let Some(first) = lines.first_mut()
        {
            *first = Line::from(format!("{OSC133_ZONE_START}{first}"));
        }
        lines
    }

    /// Rows `appended` adds after `previous` in the same text block, rendered
    /// from those two chunks alone. Committed chunks are plain paragraphs, so
    /// the pair renders as `previous` followed by the new rows; the check
    /// that it does keeps a long answer from re-rendering everything above it
    /// on every paragraph, and a pair that fails it falls back to the whole.
    fn render_appended_rows(
        &self,
        message: &AssistantMessage,
        previous: &str,
        appended: &str,
        width: usize,
    ) -> Option<Vec<Line>> {
        if !self.markdown_transformers.is_empty() {
            // A transformer sees the whole text and may join across chunks.
            return None;
        }
        let render = |text: &str| {
            let mut fragment = Self::new(
                None,
                self.hide_thinking_block,
                Some(self.markdown_theme.clone()),
                Some(self.hidden_thinking_label.clone()),
                Some(self.output_pad),
                Vec::new(),
            );
            fragment.leading_spacer = false;
            fragment.zone_markers = false;
            let mut chunk = Self::content_fragment(
                message,
                vec![AssistantContent::Text(TextContent::new(text))],
            );
            chunk.stop_reason = StopReason::Stop;
            chunk.error_message = None;
            fragment.update_content(chunk, Some(false));
            fragment.render(width)
        };
        let alone = render(previous);
        let pair = render(&format!("{previous}{appended}"));
        (pair.len() >= alone.len() && pair[..alone.len()] == alone[..])
            .then(|| pair[alone.len()..].to_vec())
    }

    fn reset_stream_history(&mut self) {
        self.stream_committed_block = 0;
        self.stream_committed_source.clear();
        self.stream_scan_offset = 0;
        self.stream_structural_holdback = false;
        self.stream_emitted_lines = 0;
        self.stream_last_chunk = None;
        self.stream_width = None;
        self.stream_pending_scan = true;
        self.stream_preview_cache = None;
    }

    fn stream_preview(&self, width: usize) -> Vec<Line> {
        let Some(message) = self.last_message.as_ref() else {
            return Vec::new();
        };
        let mut remaining = 2_048;
        let mut content = Vec::new();
        for (index, block) in message.content.iter().enumerate().rev() {
            if index < self.stream_committed_block || remaining == 0 {
                break;
            }
            match block {
                AssistantContent::Text(text) => {
                    let source = if index == self.stream_committed_block {
                        text.text
                            .strip_prefix(&self.stream_committed_source)
                            .unwrap_or(&text.text)
                    } else {
                        &text.text
                    };
                    let suffix = bounded_suffix(source, remaining);
                    remaining = remaining.saturating_sub(suffix.chars().count());
                    content.push(AssistantContent::Text(TextContent {
                        text: suffix.to_owned(),
                        text_signature: text.text_signature.clone(),
                        extra: text.extra.clone(),
                    }));
                }
                AssistantContent::Thinking(thinking) => {
                    let suffix = bounded_suffix(&thinking.thinking, remaining);
                    remaining = remaining.saturating_sub(suffix.chars().count());
                    content.push(AssistantContent::Thinking(ThinkingContent {
                        thinking: suffix.to_owned(),
                        thinking_signature: thinking.thinking_signature.clone(),
                        redacted: thinking.redacted,
                        extra: thinking.extra.clone(),
                    }));
                }
                AssistantContent::ToolCall(_) => {}
            }
        }
        content.reverse();
        let continues = self.stream_emitted_lines > 0;
        let mut lines =
            self.render_stream_message(Self::content_fragment(message, content), width, !continues);
        // The final render separates the committed paragraph from the next
        // one; showing that gap now keeps the tail from jumping at the end.
        if continues && !lines.is_empty() {
            lines.insert(0, Line::from(""));
        }
        lines
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
        if self.regular_streaming && !matches!(is_streaming, Some(false)) {
            let settled_changed = self.stream_committed_block > 0
                && self.last_message.as_ref().is_some_and(|previous| {
                    previous.content.get(..self.stream_committed_block)
                        != message.content.get(..self.stream_committed_block)
                });
            let current_changed = !self.stream_committed_source.is_empty()
                && !matches!(message.content.get(self.stream_committed_block), Some(AssistantContent::Text(text)) if text.text.starts_with(&self.stream_committed_source));
            if settled_changed || current_changed {
                self.reset_stream_history();
                self.stream_needs_reflow = true;
            }
        }
        self.is_streaming = is_streaming.unwrap_or(self.is_streaming);
        self.derive_thinking_timer(&message);
        self.stream_preview_cache = None;
        if self.regular_streaming && self.is_streaming {
            self.stream_scan_offset = self.stream_committed_source.len();
            self.stream_structural_holdback = false;
            self.stream_pending_scan = true;
            self.last_message = Some(message);
            self.content_container.clear();
            return;
        }
        self.last_message = Some(message.clone());

        // Clear content container
        self.content_container.clear();

        let has_visible_content = message.content.iter().any(is_visible_content);

        if has_visible_content && self.leading_spacer {
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
                    self.content_container.add_child(component_ref(
                        super::message_marker::MessageMarker {
                            marker: self.marker,
                            padding: self.output_pad,
                            content: Markdown::new(
                                content.text.trim(),
                                0,
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
                            ),
                        },
                    ));
                    index += 1;
                }
                AssistantContent::Thinking(_) => {
                    let thinking_padding = self.output_pad.saturating_add(1).max(2);
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
                    if self.hide_thinking_block && block_style().is_compact() {
                        continue;
                    }

                    // Add spacing only when another visible assistant content block follows.
                    // This avoids a superfluous blank line before separately-rendered tool
                    // execution blocks.
                    let has_visible_content_after =
                        message.content[index..].iter().any(is_visible_content);

                    if block_style().is_compact() {
                        // The compact style renders thinking as its own block:
                        // a muted heading with a live timer while it streams,
                        // then the text itself behind the expand toggle.
                        let is_last_run = !message.content[index..]
                            .iter()
                            .any(|content| matches!(content, AssistantContent::Thinking(_)));
                        let running = is_last_run && self.has_running_thinking();
                        let theme_instance = theme();
                        let mut heading = format!(
                            "{} {}",
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
                        if !running {
                            heading.push_str(&theme_instance.fg(
                                ThemeColor::Dim,
                                &format!(
                                    " ({} to {})",
                                    key_text("app.tools.expand"),
                                    if self.thinking_expanded {
                                        "collapse"
                                    } else {
                                        "expand"
                                    }
                                ),
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
                            .add_child(component_ref(TruncatedText::new(heading, 0, 0)));
                        if self.thinking_expanded {
                            self.content_container
                                .add_child(component_ref(Markdown::new(
                                    thinking_blocks.join("\n\n"),
                                    thinking_padding,
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
                    } else if self.hide_thinking_block {
                        // Show one static label for each run of thinking blocks when hidden.
                        let theme = theme();
                        self.content_container.add_child(component_ref(Text::new(
                            theme.italic(
                                &theme.fg(ThemeColor::ThinkingText, &self.hidden_thinking_label),
                            ),
                            thinking_padding,
                            0,
                        )));
                    } else {
                        // Render each run of thinking blocks as one Markdown section.
                        self.content_container
                            .add_child(component_ref(Markdown::new(
                                thinking_blocks.join("\n\n"),
                                thinking_padding,
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

fn bounded_suffix(source: &str, chars: usize) -> &str {
    if chars == 0 {
        return "";
    }
    source
        .char_indices()
        .rev()
        .nth(chars)
        .map_or(source, |(index, _)| {
            &source[index + source[index..].chars().next().map_or(0, char::len_utf8)..]
        })
}

fn plain_stream_block(source: &str) -> bool {
    source
        .lines()
        .filter(|line| !line.trim().is_empty())
        .all(|line| {
            let trimmed = line.trim_start();
            let numbered_list = trimmed
                .split_once(['.', ')'])
                .is_some_and(|(prefix, rest)| {
                    !prefix.is_empty()
                        && prefix.bytes().all(|byte| byte.is_ascii_digit())
                        && rest.starts_with(' ')
                });
            !numbered_list
                && !trimmed.starts_with(['#', '-', '*', '+', '>', '|', '<', '`', '~', '='])
                && !trimmed.starts_with("$$")
                && !trimmed.starts_with("\\[")
                && !trimmed.contains(['|', '`', '<', '>', '[', ']', '!'])
        })
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
        if self.regular_streaming && self.is_streaming {
            if let Some((cached_width, lines)) = &self.stream_preview_cache
                && *cached_width == width
            {
                return lines.clone();
            }
            let lines = self.stream_preview(width);
            self.stream_preview_cache = Some((width, lines.clone()));
            return lines;
        }
        let mut lines = self.content_container.render(width);
        if self.has_tool_calls || lines.is_empty() || !self.zone_markers {
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
        self.reset_stream_history();
        self.content_container.invalidate();
        if let Some(message) = self.last_message.clone() {
            self.update_content(message, None);
        }
    }

    fn prepare_reflow(&mut self, _width: usize) {
        self.reset_stream_history();
    }

    fn take_stream_history(&mut self, width: usize) -> Vec<Line> {
        if !self.regular_streaming || !self.is_streaming || !self.stream_pending_scan {
            return Vec::new();
        }
        self.stream_pending_scan = false;
        if self
            .stream_width
            .is_some_and(|committed| committed != width)
        {
            // Rows at another width are already in scrollback; only a replay
            // can bring the transcript back in line.
            self.reset_stream_history();
            self.stream_needs_reflow = true;
            return Vec::new();
        }
        let Some(message) = self.last_message.as_ref() else {
            return Vec::new();
        };
        let previous = (
            self.stream_committed_block,
            self.stream_committed_source.len(),
        );
        while let Some(block) = message.content.get(self.stream_committed_block) {
            let settled = self.stream_committed_block + 1 < message.content.len();
            match block {
                AssistantContent::Text(text) => {
                    let source = &text.text;
                    let start = self.stream_committed_source.len();
                    let mut boundary = start;
                    if settled {
                        boundary = source.len();
                    } else if !self.stream_structural_holdback {
                        // One byte back finds a "\n\n" split across two deltas;
                        // a delta can end inside a multi-byte character.
                        let scan_from = source
                            .floor_char_boundary(
                                self.stream_scan_offset.saturating_sub(1).min(source.len()),
                            )
                            .max(start);
                        for (offset, _) in source[scan_from..].match_indices("\n\n") {
                            let end = scan_from + offset + 2;
                            let chunk = &source[boundary..end];
                            if chunk.trim().is_empty() || !plain_stream_block(chunk) {
                                self.stream_structural_holdback = true;
                                break;
                            }
                            boundary = end;
                        }
                        self.stream_scan_offset = source.len();
                    }
                    self.stream_committed_source
                        .push_str(&source[start..boundary]);
                }
                // A thought is final once its runtime is frozen; before that
                // its heading still reads as running.
                AssistantContent::Thinking(_)
                    if settled
                        && (self.thinking_finished.is_some()
                            || self.thinking_started.is_none()) => {}
                AssistantContent::ToolCall(_) if settled => {}
                _ => break,
            }
            if !settled {
                break;
            }
            self.stream_committed_block += 1;
            self.stream_committed_source.clear();
            self.stream_scan_offset = 0;
            self.stream_structural_holdback = false;
        }
        if (
            self.stream_committed_block,
            self.stream_committed_source.len(),
        ) == previous
        {
            return Vec::new();
        }
        let same_block = self.stream_committed_block == previous.0;
        let chunk_start = if same_block { previous.1 } else { 0 };
        let committed = &self.stream_committed_source;
        let appended = self
            .stream_last_chunk
            .filter(|(_, end)| same_block && *end == previous.1 && self.stream_emitted_lines > 0)
            .and_then(|(start, end)| {
                self.render_appended_rows(message, &committed[start..end], &committed[end..], width)
            });
        let lines = match appended {
            Some(rows) => {
                self.stream_emitted_lines += rows.len();
                rows
            }
            None => {
                let rendered = self.render_committed_stream(message, width);
                let rows = rendered
                    .get(self.stream_emitted_lines..)
                    .map(<[Line]>::to_vec)
                    .unwrap_or_default();
                self.stream_emitted_lines = self.stream_emitted_lines.max(rendered.len());
                rows
            }
        };
        self.stream_last_chunk = (self.stream_committed_source.len() > chunk_start)
            .then_some((chunk_start, self.stream_committed_source.len()));
        self.stream_width = Some(width);
        self.stream_preview_cache = None;
        lines
    }
}
