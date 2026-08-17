//! 1:1 port of
//! `packages/coding-agent/src/modes/interactive/components/user-message.ts` (70 LOC).

use std::rc::Rc;

use notagent_tui::components::box_component::BoxComponent;
use notagent_tui::components::markdown::{
    DefaultTextStyle, Markdown, MarkdownOptions, MarkdownTheme,
};
use notagent_tui::tui::{Component, Container, component_ref};

use crate::modes::interactive::theme::theme::{
    BlockStyle, ThemeBg, ThemeColor, block_style, get_markdown_theme, theme,
};

use super::markdown_transform::{
    MarkdownMessageType, MarkdownTransformer, create_markdown_transform,
};

const OSC133_ZONE_START: &str = "\x1b]133;A\x07";
const OSC133_ZONE_END: &str = "\x1b]133;B\x07";
const OSC133_ZONE_FINAL: &str = "\x1b]133;C\x07";

/// Component that renders a user message: a boxed, washed block in the
/// standard style, the washed text lines without padding rows in the badge
/// style (the user message keeps its wash in both styles — reference
/// `user_message.rs`).
pub struct UserMessageComponent {
    container: Container,
    text: String,
    markdown_theme: MarkdownTheme,
    output_pad: usize,
    markdown_transformers: Vec<MarkdownTransformer>,
    /// The block style the layout was built for; a mismatch at render time
    /// rebuilds, so a live style switch restyles the message.
    built_style: BlockStyle,
}

impl UserMessageComponent {
    /// `markdown_theme` defaults to [`get_markdown_theme`], `output_pad` to 1
    /// and `markdown_transformers` to empty, like the TypeScript parameters.
    pub fn new(
        text: impl Into<String>,
        markdown_theme: Option<MarkdownTheme>,
        output_pad: Option<usize>,
        markdown_transformers: Vec<MarkdownTransformer>,
    ) -> Self {
        let mut component = Self {
            container: Container::new(),
            text: text.into(),
            markdown_theme: markdown_theme.unwrap_or_else(get_markdown_theme),
            output_pad: output_pad.unwrap_or(1),
            markdown_transformers,
            built_style: block_style(),
        };
        component.rebuild();
        component
    }

    /// Change the horizontal padding.
    pub fn set_output_pad(&mut self, padding: usize) {
        self.output_pad = padding;
        self.rebuild();
    }

    fn rebuild(&mut self) {
        self.built_style = block_style();
        self.container.clear();
        // The badge style sheds the padding rows but keeps the wash, so the
        // user's turns stay visually anchored (reference `user_message.rs`).
        let padding_y = if self.built_style == BlockStyle::Badge {
            0
        } else {
            1
        };
        let mut content_box = BoxComponent::new(
            self.output_pad,
            padding_y,
            Some(Rc::new(|content: &str| {
                theme().bg(ThemeBg::UserMessageBg, content)
            })),
        );
        content_box.add_child(component_ref(Markdown::new(
            self.text.clone(),
            0,
            0,
            self.markdown_theme.clone(),
            Some(DefaultTextStyle {
                color: Some(Rc::new(|content: &str| {
                    theme().fg(ThemeColor::UserMessageText, content)
                })),
                ..DefaultTextStyle::default()
            }),
            Some(MarkdownOptions {
                preserve_ordered_list_markers: true,
                preserve_backslash_escapes: true,
                transform: Some(create_markdown_transform(
                    MarkdownMessageType::User,
                    false,
                    self.markdown_transformers.clone(),
                )),
                ..MarkdownOptions::default()
            }),
        )));
        self.container.add_child(component_ref(content_box));
    }
}

impl Component for UserMessageComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        if self.built_style != block_style() {
            self.rebuild();
        }
        let mut lines = self.container.render(width);
        if lines.is_empty() {
            return lines;
        }

        if lines.len() == 1 {
            // A single line carries the whole zone in order: start, end,
            // final, then the message (reference `user_message.rs`).
            lines[0] = format!(
                "{OSC133_ZONE_START}{OSC133_ZONE_END}{OSC133_ZONE_FINAL}{}",
                lines[0]
            );
        } else {
            lines[0] = format!("{OSC133_ZONE_START}{}", lines[0]);
            let last = lines.len() - 1;
            lines[last] = format!("{OSC133_ZONE_END}{OSC133_ZONE_FINAL}{}", lines[last]);
        }
        lines
    }

    fn invalidate(&mut self) {
        self.container.invalidate();
        self.rebuild();
    }
}
