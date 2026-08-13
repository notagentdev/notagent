//! 1:1 port of
//! `packages/coding-agent/src/modes/interactive/components/dynamic-border.ts` (25 LOC).

use notagent_tui::components::markdown::StyleFn;
use notagent_tui::tui::Component;

use crate::modes::interactive::theme::theme::{ThemeColor, theme};

/// Dynamic border component that adjusts to viewport width.
pub struct DynamicBorder {
    color: StyleFn,
}

impl DynamicBorder {
    /// New border; `None` uses the theme's `border` colour, like the TypeScript
    /// default parameter (which also reads the global theme per call).
    pub fn new(color: Option<StyleFn>) -> Self {
        Self {
            color: color.unwrap_or_else(|| {
                std::rc::Rc::new(|text: &str| theme().fg(ThemeColor::Border, text))
            }),
        }
    }
}

impl Component for DynamicBorder {
    fn invalidate(&mut self) {
        // No cached state to invalidate currently
    }

    fn render(&mut self, width: usize) -> Vec<String> {
        vec![(self.color)(&"─".repeat(width.max(1)))]
    }
}
