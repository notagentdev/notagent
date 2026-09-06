use std::rc::Rc;

use notagent_tui::tui::{Component, Line, shared_lines};
use notagent_tui::utils::{truncate_to_width_opts, visible_width};

use crate::config::VERSION;
use crate::modes::interactive::theme::theme::{ThemeColor, theme};

const MAX_CARD_WIDTH: usize = 80;

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
            theme.fg(ThemeColor::Text, "│"),
            theme.fg(ThemeColor::Text, "│")
        )
    }
}

impl Component for StartupHeader {
    fn render(&mut self, width: usize) -> Vec<Line> {
        let data = (self.data)();
        let theme = theme();
        let title = format!(
            "{}{}{}",
            theme.fg(ThemeColor::Text, "Welcome to "),
            theme.bold(&theme.fg(ThemeColor::Accent, "notagent")),
            theme.fg(ThemeColor::Dim, &format!(" v{VERSION}"))
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

        let git = data.git_branch.as_deref().unwrap_or("not a repository");
        let model = data
            .model
            .as_deref()
            .unwrap_or("not set, use /model to select");
        let directory = Self::field("Directory:", &data.directory, ThemeColor::Text);
        let git = Self::field("Git:", git, ThemeColor::Accent);
        let model = Self::field("Model:", model, ThemeColor::Accent);
        let content_width = [&title, &help, &directory, &git, &model]
            .into_iter()
            .map(|line| visible_width(line))
            .max()
            .unwrap_or_default();
        let rendered_width = width
            .min(MAX_CARD_WIDTH)
            .min(content_width.saturating_add(6));
        let card_width = rendered_width.saturating_sub(2);
        let horizontal = theme.fg(ThemeColor::Text, &"─".repeat(card_width - 2));
        let top = format!(
            " {}{horizontal}{} ",
            theme.fg(ThemeColor::Text, "╭"),
            theme.fg(ThemeColor::Text, "╮")
        );
        let bottom = format!(
            " {}{horizontal}{} ",
            theme.fg(ThemeColor::Text, "╰"),
            theme.fg(ThemeColor::Text, "╯")
        );
        let lines = vec![
            top,
            Self::body_line(&title, card_width),
            Self::body_line(&help, card_width),
            Self::body_line("", card_width),
            Self::body_line(&directory, card_width),
            Self::body_line(&git, card_width),
            Self::body_line(&model, card_width),
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
    fn model_changes_keep_every_border_at_the_same_column() {
        let _guard = test_lock();
        init_theme(Some("dark"), false);
        let data = Rc::new(std::cell::RefCell::new(StartupHeaderData {
            directory: "~/projects/notagent".to_owned(),
            git_branch: Some("main".to_owned()),
            model: Some("openai/gpt-5".to_owned()),
        }));
        let source = Rc::clone(&data);
        let mut card = StartupHeader::from_provider(Rc::new(move || source.borrow().clone()));
        let terminal = notagent_tui::test_terminal::VirtualTerminal::new(120, 24);
        let mut screen =
            notagent_tui::tui_main_screen::TuiMainScreen::new(Box::new(terminal.clone()));
        let source = Rc::clone(&data);
        screen.core().add_child(notagent_tui::tui::component_ref(
            StartupHeader::from_provider(Rc::new(move || source.borrow().clone())),
        ));
        screen.start();
        for model in ["openai/gpt-5", "anthropic/claude-fable-5-1", "openai/gpt-5"] {
            data.borrow_mut().model = Some(model.to_owned());
            for width in 8..=120 {
                let lines: Vec<_> = card
                    .render(width)
                    .iter()
                    .map(|line| strip_terminal_sequences(line))
                    .collect();
                let columns = lines[0].chars().count();
                assert!(columns <= width, "header must fit width {width}: {lines:?}");
                assert!(
                    lines.iter().all(|line| line.chars().count() == columns),
                    "all ASCII-content rows must align with both borders at width {width}: {lines:?}"
                );
                assert!(
                    lines[1..7]
                        .iter()
                        .all(|line| line.chars().nth(columns - 2) == Some('│')),
                    "right border must stay in the same cell at width {width}: {lines:?}"
                );
            }
            for width in [120, 40, 38, 80, 24, 60, 120] {
                terminal.resize(width, 24);
                screen.core().request_immediate_render();
                screen.render_now(false);
                let viewport = terminal.get_viewport();
                let expected = card.render(width);
                for (actual, expected) in viewport.iter().zip(expected.iter()) {
                    assert_eq!(
                        actual.trim_end(),
                        strip_terminal_sequences(expected).trim_end(),
                        "model changes and resizing must repaint the whole card at width {width}: {viewport:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_card_fits_its_content_and_shows_runtime_details() {
        let _guard = test_lock();
        init_theme(Some("dark"), false);
        let data = StartupHeaderData {
            directory: "/workspace/notagent".to_owned(),
            git_branch: Some("feature/start-screen".to_owned()),
            model: Some("openai/gpt-5".to_owned()),
        };
        let mut card = StartupHeader::from_provider(Rc::new(move || data.clone()));

        let rendered = card.render(100);
        assert!(
            rendered[0].contains(&theme().fg(ThemeColor::Text, "╭")),
            "the border uses the normal text color"
        );
        let lines: Vec<String> = rendered
            .iter()
            .map(|line| strip_terminal_sequences(line))
            .collect();

        assert_eq!(lines.len(), 8, "the card has a stable compact height");
        let card_width = visible_width(&lines[0]);
        assert!(
            card_width < 100,
            "the card should fit its content instead of filling the render width: {lines:?}"
        );
        assert!(
            lines.iter().all(|line| visible_width(line) == card_width),
            "every card line has the same content-based width: {lines:?}"
        );
        assert!(lines[0].starts_with(" ╭"), "the top border is rounded");
        assert!(lines[7].starts_with(" ╰"), "the bottom border is rounded");
        assert!(
            lines[1].contains(&format!("Welcome to notagent v{}", crate::config::VERSION)),
            "{lines:?}"
        );
        assert!(
            !lines[1].contains(&format!("v{}!", crate::config::VERSION)),
            "{lines:?}"
        );
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
