use notagent_tui::components::markdown::StyleFn;
use notagent_tui::tui::{Component, Line};

use crate::modes::interactive::theme::theme::{ThemeColor, theme};

/// Dynamic border component that adjusts to viewport width.
pub struct DynamicBorder {
    color: StyleFn,
}

impl DynamicBorder {
    /// New border; `None` uses the theme's `borderMuted` colour, read per call
    /// which used `border` — the accent blue. A dialog's rules are furniture,
    /// not a signal, and the blue read as one; `borderMuted` is the grey the
    /// input frame already draws itself in, so the two now match.
    pub fn new(color: Option<StyleFn>) -> Self {
        Self {
            color: color.unwrap_or_else(|| {
                std::rc::Rc::new(|text: &str| theme().fg(ThemeColor::BorderMuted, text))
            }),
        }
    }
}

impl Component for DynamicBorder {
    fn invalidate(&mut self) {
        // No cached state to invalidate currently
    }

    fn render(&mut self, width: usize) -> Vec<Line> {
        vec![Line::from((self.color)(&"─".repeat(width.max(1))))]
    }
}
