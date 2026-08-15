//! ANSI colouring of finished art.
//!
//! Port of `grok-mermaid/src/ansi.ts` (34 LOC).

use super::types::{Cls, MermaidArt};

const ESC: char = '\x1b';

/// SGR parameter per semantic class, e.g. `"2"` for dim, `"36"` for cyan,
/// `"38;5;244"` for a 256-colour index. A class left out is printed unstyled.
#[derive(Debug, Clone, Default)]
pub struct AnsiTheme {
    pub border: Option<&'static str>,
    pub text: Option<&'static str>,
    pub edge: Option<&'static str>,
    pub edge_label: Option<&'static str>,
    pub title: Option<&'static str>,
    pub none: Option<&'static str>,
}

impl AnsiTheme {
    fn sgr(&self, cls: Cls) -> Option<&'static str> {
        match cls {
            Cls::Border => self.border,
            Cls::Text => self.text,
            Cls::Edge => self.edge,
            Cls::EdgeLabel => self.edge_label,
            Cls::Title => self.title,
            Cls::None => self.none,
        }
    }
}

/// Dim frame, plain labels, cyan connectors. Readable on light and dark.
pub fn default_theme() -> AnsiTheme {
    AnsiTheme {
        border: Some("2"),
        text: None,
        edge: Some("36"),
        edge_label: Some("2;36"),
        title: Some("1"),
        none: None,
    }
}

/// Render art to ANSI-coloured lines.
///
/// A convenience over mapping `art.styled` yourself — reach for that directly
/// when your TUI has its own styling model.
pub fn to_ansi(art: &MermaidArt, theme: &AnsiTheme) -> Vec<String> {
    art.styled
        .iter()
        .map(|row| {
            row.iter()
                .map(|span| match theme.sgr(span.cls) {
                    None => span.text.clone(),
                    Some(sgr) => format!("{ESC}[{sgr}m{}{ESC}[0m", span.text),
                })
                .collect()
        })
        .collect()
}
