//! 1:1 port of
//! `packages/coding-agent/src/modes/interactive/components/compaction-summary-message.ts` (59 LOC).

use std::rc::Rc;

use notagent_agent::CompactionSummaryMessage;
use notagent_tui::components::box_component::BoxComponent;
use notagent_tui::components::markdown::{DefaultTextStyle, Markdown, MarkdownTheme};
use notagent_tui::components::spacer::Spacer;
use notagent_tui::components::text::Text;
use notagent_tui::tui::{Component, component_ref};

use crate::modes::interactive::theme::theme::{
    BlockStyle, ThemeBg, ThemeColor, badge, block_style, get_markdown_theme, theme,
};

use super::keybinding_hints::key_text;
use super::to_locale_string;

/// Component that renders a compaction message with collapsed/expanded state.
/// Uses same background color as custom messages for visual consistency.
pub struct CompactionSummaryMessageComponent {
    content_box: BoxComponent,
    expanded: bool,
    message: CompactionSummaryMessage,
    markdown_theme: MarkdownTheme,
}

impl CompactionSummaryMessageComponent {
    /// `markdown_theme` defaults to [`get_markdown_theme`].
    pub fn new(message: CompactionSummaryMessage, markdown_theme: Option<MarkdownTheme>) -> Self {
        let mut component = Self {
            content_box: BoxComponent::new(
                1,
                1,
                Some(Rc::new(|text: &str| {
                    theme().bg(ThemeBg::CustomMessageBg, text)
                })),
            ),
            expanded: false,
            message,
            markdown_theme: markdown_theme.unwrap_or_else(get_markdown_theme),
        };
        component.update_display();
        component
    }

    /// Show the full summary instead of the one-line hint.
    pub fn set_expanded(&mut self, expanded: bool) {
        self.expanded = expanded;
        self.update_display();
    }

    fn update_display(&mut self) {
        // Badge style: no wash, no padding rows — a COMPACTION badge in the
        // block's colour leads (reference `compaction_summary.rs`).
        let badge_style = block_style() == BlockStyle::Badge;
        if badge_style {
            self.content_box.set_padding(1, 0);
            self.content_box.set_bg_fn(None);
        } else {
            self.content_box.set_padding(1, 1);
            self.content_box.set_bg_fn(Some(Rc::new(|text: &str| {
                theme().bg(ThemeBg::CustomMessageBg, text)
            })));
        }
        self.content_box.clear();

        let token_str = to_locale_string(self.message.tokens_before);
        let theme = theme();
        let label = if badge_style {
            badge(&theme, ThemeBg::CustomMessageBg, "compaction")
        } else {
            theme.fg(
                ThemeColor::CustomMessageLabel,
                "\x1b[1m[compaction]\x1b[22m",
            )
        };
        if self.expanded {
            self.content_box
                .add_child(component_ref(Text::new(label, 0, 0)));
            if !badge_style {
                self.content_box.add_child(component_ref(Spacer::new(1)));
            }
            let header = format!("**Compacted from {token_str} tokens**\n\n");
            self.content_box.add_child(component_ref(Markdown::new(
                format!("{header}{}", self.message.summary),
                0,
                0,
                self.markdown_theme.clone(),
                Some(DefaultTextStyle {
                    color: Some(Rc::new(|text: &str| {
                        crate::modes::interactive::theme::theme::theme()
                            .fg(ThemeColor::CustomMessageText, text)
                    })),
                    ..DefaultTextStyle::default()
                }),
                None,
            )));
        } else {
            let detail = theme.fg(
                ThemeColor::CustomMessageText,
                &format!("Compacted from {token_str} tokens ("),
            ) + &theme.fg(ThemeColor::Dim, &key_text("app.tools.expand"))
                + &theme.fg(ThemeColor::CustomMessageText, " to expand)");
            if badge_style {
                // The badge and the detail share one row (reference
                // `compaction_summary.rs`).
                self.content_box.add_child(component_ref(Text::new(
                    format!("{label} {detail}"),
                    0,
                    0,
                )));
            } else {
                self.content_box
                    .add_child(component_ref(Text::new(label, 0, 0)));
                self.content_box.add_child(component_ref(Spacer::new(1)));
                self.content_box
                    .add_child(component_ref(Text::new(detail, 0, 0)));
            }
        }
    }
}

impl Component for CompactionSummaryMessageComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.content_box.render(width)
    }

    fn invalidate(&mut self) {
        self.content_box.invalidate();
        self.update_display();
    }
}
