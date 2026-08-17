//! 1:1 port of
//! `packages/coding-agent/src/modes/interactive/components/skill-invocation-message.ts` (55 LOC).

use std::rc::Rc;

use notagent_tui::components::box_component::BoxComponent;
use notagent_tui::components::markdown::{DefaultTextStyle, Markdown, MarkdownTheme};
use notagent_tui::components::text::Text;
use notagent_tui::tui::{Component, component_ref};

use crate::core::agent_session::ParsedSkillBlock;
use crate::modes::interactive::theme::theme::{
    BlockStyle, ThemeBg, ThemeColor, badge, block_style, get_markdown_theme, theme,
};

use super::keybinding_hints::key_text;

/// Component that renders a skill invocation message with collapsed/expanded state.
/// Uses same background color as custom messages for visual consistency.
/// Only renders the skill block itself - user message is rendered separately.
pub struct SkillInvocationMessageComponent {
    content_box: BoxComponent,
    expanded: bool,
    skill_block: ParsedSkillBlock,
    markdown_theme: MarkdownTheme,
}

impl SkillInvocationMessageComponent {
    /// `markdown_theme` defaults to [`get_markdown_theme`].
    pub fn new(skill_block: ParsedSkillBlock, markdown_theme: Option<MarkdownTheme>) -> Self {
        let mut component = Self {
            content_box: BoxComponent::new(
                1,
                1,
                Some(Rc::new(|text: &str| {
                    theme().bg(ThemeBg::CustomMessageBg, text)
                })),
            ),
            expanded: false,
            skill_block,
            markdown_theme: markdown_theme.unwrap_or_else(get_markdown_theme),
        };
        component.update_display();
        component
    }

    /// `setExpanded(expanded)`
    pub fn set_expanded(&mut self, expanded: bool) {
        self.expanded = expanded;
        self.update_display();
    }

    fn update_display(&mut self) {
        // Badge style: no wash, no padding rows — a SKILL badge in the
        // block's colour leads, the name beside it (reference pattern).
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

        let label = if badge_style {
            format!("{} ", badge(&theme(), ThemeBg::CustomMessageBg, "skill"))
        } else {
            theme().fg(ThemeColor::CustomMessageLabel, "\x1b[1m[skill]\x1b[22m ")
        };
        if self.expanded {
            // Expanded: label + skill name header + full content
            self.content_box
                .add_child(component_ref(Text::new(label.trim_end(), 0, 0)));
            let header = format!("**{}**\n\n", self.skill_block.name);
            self.content_box.add_child(component_ref(Markdown::new(
                header + &self.skill_block.content,
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
        } else {
            // Collapsed: single line - [skill] name (hint to expand)
            let line = label
                + &theme().fg(ThemeColor::CustomMessageText, &self.skill_block.name)
                + &theme().fg(
                    ThemeColor::Dim,
                    &format!(" ({} to expand)", key_text("app.tools.expand")),
                );
            self.content_box
                .add_child(component_ref(Text::new(line, 0, 0)));
        }
    }
}

impl Component for SkillInvocationMessageComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.content_box.render(width)
    }

    fn invalidate(&mut self) {
        self.content_box.invalidate();
        self.update_display();
    }
}
