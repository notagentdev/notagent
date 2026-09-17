use crate::modes::interactive::theme::theme::{ThemeColor, theme};

#[derive(Clone, Copy)]
pub enum MarkerState {
    Queued,
    Running,
    Success,
    Error,
}

pub fn dot(state: MarkerState) -> String {
    let (color, glyph) = match state {
        MarkerState::Running => (ThemeColor::Dim, notagent_tui::activity::RUNNING_DOT),
        MarkerState::Success => (ThemeColor::Success, "●"),
        MarkerState::Error => (ThemeColor::Error, "●"),
        MarkerState::Queued => (ThemeColor::Dim, "●"),
    };
    theme().fg(color, glyph)
}

pub fn heading(label: &str, state: MarkerState) -> String {
    format!("{} {}", dot(state), theme().bold(label))
}
