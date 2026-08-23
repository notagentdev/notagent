//! 1:1 port of
//! `packages/coding-agent/src/modes/interactive/components/custom-message.ts` (113 LOC).
//!
//! The optional `MessageRenderer` is dropped: it is only ever supplied by
//! `extensionRunner.getMessageRenderer` (`interactive-mode.ts:3728`), and
//! `plans/facts/extension-boundary.md` lists message renderer overrides as
//! dropped without replacement. Everything the default rendering path does is
//! kept, because `CustomMessage` itself is a core message type.

use std::rc::Rc;

use notagent_agent::CustomMessage;
use notagent_ai::types::{TextOrImageContent, UserContent};
use notagent_tui::components::box_component::BoxComponent;
use notagent_tui::components::markdown::{DefaultTextStyle, Markdown, MarkdownTheme};
use notagent_tui::components::spacer::Spacer;
use notagent_tui::components::text::Text;
use notagent_tui::tui::{Component, Container, Line, component_ref};

use crate::modes::interactive::theme::theme::{
    BlockStyle, ThemeBg, ThemeColor, badge, block_style, get_markdown_theme, theme,
};

/// Component that renders a custom message entry.
/// Uses distinct styling to differentiate from user messages.
pub struct CustomMessageComponent {
    container: Container,
    message: CustomMessage,
    markdown_theme: MarkdownTheme,
    expanded: bool,
    output_pad: usize,
}

impl CustomMessageComponent {
    /// `markdown_theme` defaults to [`get_markdown_theme`] and `output_pad` to 1.
    pub fn new(
        message: CustomMessage,
        markdown_theme: Option<MarkdownTheme>,
        output_pad: Option<usize>,
    ) -> Self {
        let mut component = Self {
            container: Container::new(),
            message,
            markdown_theme: markdown_theme.unwrap_or_else(get_markdown_theme),
            expanded: false,
            output_pad: output_pad.unwrap_or(1),
        };
        component.rebuild();
        component
    }

    /// Expand or collapse. Kept for API parity: only the dropped custom renderer
    /// ever varied its output by this flag.
    pub fn set_expanded(&mut self, expanded: bool) {
        if self.expanded != expanded {
            self.expanded = expanded;
            self.rebuild();
        }
    }

    /// Change the horizontal padding.
    pub fn set_output_pad(&mut self, output_pad: usize) {
        if self.output_pad != output_pad {
            self.output_pad = output_pad;
            self.rebuild();
        }
    }

    fn rebuild(&mut self) {
        self.container.clear();
        self.container.add_child(component_ref(Spacer::new(1)));

        // Standard: box with purple background. Badge style: no wash, no
        // padding rows — the label becomes a badge in the block's colour and
        // the content follows directly (reference compaction pattern).
        let badge_style = block_style() == BlockStyle::Badge;
        let mut content_box = if badge_style {
            BoxComponent::new(0, 0, None)
        } else {
            BoxComponent::new(
                1,
                1,
                Some(Rc::new(|text: &str| {
                    theme().bg(ThemeBg::CustomMessageBg, text)
                })),
            )
        };

        // Default rendering: label + content
        let label = if badge_style {
            badge(
                &theme(),
                ThemeBg::CustomMessageBg,
                &self.message.custom_type,
            )
        } else {
            theme().fg(
                ThemeColor::CustomMessageLabel,
                &format!("\x1b[1m[{}]\x1b[22m", self.message.custom_type),
            )
        };
        content_box.add_child(component_ref(Text::new(label, 0, 0)));
        if !badge_style {
            content_box.add_child(component_ref(Spacer::new(1)));
        }

        // Extract text content
        let text = match &self.message.content {
            UserContent::Text(text) => text.clone(),
            UserContent::Blocks(blocks) => blocks
                .iter()
                .filter_map(|block| match block {
                    TextOrImageContent::Text(content) => Some(content.text.clone()),
                    TextOrImageContent::Image(_) => None,
                })
                .collect::<Vec<_>>()
                .join("\n"),
        };

        content_box.add_child(component_ref(Markdown::new(
            text,
            0,
            0,
            self.markdown_theme.clone(),
            Some(DefaultTextStyle {
                color: Some(Rc::new(|text: &str| {
                    theme().fg(ThemeColor::CustomMessageText, text)
                })),
                ..DefaultTextStyle::default()
            }),
            None,
        )));
        self.container.add_child(component_ref(content_box));
    }
}

impl Component for CustomMessageComponent {
    fn render(&mut self, width: usize) -> Vec<Line> {
        self.container.render(width)
    }

    fn invalidate(&mut self) {
        self.container.invalidate();
        self.rebuild();
    }
}
