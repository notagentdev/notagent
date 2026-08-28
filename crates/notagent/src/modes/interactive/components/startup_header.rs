use std::rc::Rc;

use notagent_tui::tui::{Component, Line, shared_lines};
use notagent_tui::utils::{truncate_to_width_opts, visible_width};

use crate::modes::interactive::theme::theme::{ThemeColor, theme};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartupHeaderData {
    pub directory: String,
    pub git_branch: Option<String>,
    pub model: Option<String>,
}

pub struct StartupHeader {
    data: Rc<dyn Fn() -> StartupHeaderData>,
}

impl StartupHeader {
    pub fn from_provider(data: Rc<dyn Fn() -> StartupHeaderData>) -> Self {
        Self { data }
    }

    fn field(label: &str, value: &str, color: ThemeColor) -> String {
        let theme = theme();
        format!(
            "{} {}",
            theme.bold(&theme.fg(ThemeColor::Muted, label)),
            theme.fg(color, value)
        )
    }

    fn body_line(content: &str, card_width: usize) -> String {
        let theme = theme();
        let content_width = card_width.saturating_sub(4);
        let clipped = truncate_to_width_opts(content, content_width, "…", false);
        let padding = " ".repeat(content_width.saturating_sub(visible_width(&clipped)));
        format!(
            " {} {clipped}{padding} {} ",
            theme.fg(ThemeColor::Accent, "│"),
            theme.fg(ThemeColor::Accent, "│")
        )
    }
}

impl Component for StartupHeader {
    fn render(&mut self, width: usize) -> Vec<Line> {
        let data = (self.data)();
        let theme = theme();
        let title = format!(
            "{}{}",
            theme.fg(ThemeColor::Text, "Welcome to "),
            theme.bold(&theme.fg(ThemeColor::Accent, "notagent!"))
        );
        let help = format!(
            "{}{}{}",
            theme.fg(ThemeColor::Text, "Send "),
            theme.bold(&theme.fg(ThemeColor::Accent, "/help")),
            theme.fg(ThemeColor::Muted, " for help information.")
        );

        if width < 8 {
            return shared_lines(vec![truncate_to_width_opts(&title, width, "", false)]);
        }

        let card_width = width - 2;
        let horizontal = theme.fg(ThemeColor::Accent, &"─".repeat(card_width - 2));
        let top = format!(
            " {}{horizontal}{} ",
            theme.fg(ThemeColor::Accent, "╭"),
            theme.fg(ThemeColor::Accent, "╮")
        );
        let bottom = format!(
            " {}{horizontal}{} ",
            theme.fg(ThemeColor::Accent, "╰"),
            theme.fg(ThemeColor::Accent, "╯")
        );
        let git = data.git_branch.as_deref().unwrap_or("not a repository");
        let model = data
            .model
            .as_deref()
            .unwrap_or("not set, use /model to select");
        let lines = vec![
            top,
            Self::body_line(&title, card_width),
            Self::body_line(&help, card_width),
            Self::body_line("", card_width),
            Self::body_line(
                &Self::field("Directory:", &data.directory, ThemeColor::Text),
                card_width,
            ),
            Self::body_line(&Self::field("Git:", git, ThemeColor::Accent), card_width),
            Self::body_line(
                &Self::field("Model:", model, ThemeColor::Accent),
                card_width,
            ),
            bottom,
        ];
        shared_lines(lines)
    }

    fn invalidate(&mut self) {}
}

#[cfg(test)]
mod tests {
    use notagent_tui::utils::{strip_terminal_sequences, visible_width};

    use super::*;
    use crate::modes::interactive::theme::theme::{init_theme, test_lock};

    #[test]
    fn the_card_uses_the_available_width_and_shows_runtime_details() {
        let _guard = test_lock();
        init_theme(Some("dark"), false);
        let data = StartupHeaderData {
            directory: "/workspace/notagent".to_owned(),
            git_branch: Some("feature/start-screen".to_owned()),
            model: Some("openai/gpt-5".to_owned()),
        };
        let mut card = StartupHeader::from_provider(Rc::new(move || data.clone()));

        let lines: Vec<String> = card
            .render(64)
            .iter()
            .map(|line| strip_terminal_sequences(line))
            .collect();

        assert_eq!(lines.len(), 8, "the card has a stable compact height");
        assert!(
            lines.iter().all(|line| visible_width(line) == 64),
            "every card line fills, but does not exceed, the render width: {lines:?}"
        );
        assert!(lines[0].starts_with(" ╭"), "the top border is rounded");
        assert!(lines[7].starts_with(" ╰"), "the bottom border is rounded");
        assert!(lines[1].contains("Welcome to notagent!"), "{lines:?}");
        assert!(
            lines[2].contains("Send /help for help information."),
            "{lines:?}"
        );
        assert!(
            lines[4].contains("Directory: /workspace/notagent"),
            "{lines:?}"
        );
        assert!(lines[5].contains("Git: feature/start-screen"), "{lines:?}");
        assert!(lines[6].contains("Model: openai/gpt-5"), "{lines:?}");
    }
}
