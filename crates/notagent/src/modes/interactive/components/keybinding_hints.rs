//! 1:1 port of
//! `packages/coding-agent/src/modes/interactive/components/keybinding-hints.ts` (48 LOC).
//!
//! Utilities for formatting keybinding hints in the UI.

use notagent_tui::keybinding_keys;

use crate::modes::interactive::theme::theme::{ThemeColor, theme};

/// Formatting options of [`format_key_text`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeyTextFormatOptions {
    /// Upper-case the first letter of every part.
    pub capitalize: bool,
}

fn format_key_part(part: &str, options: KeyTextFormatOptions) -> String {
    let display_part = if cfg!(target_os = "macos") && part.to_lowercase() == "alt" {
        "option"
    } else {
        part
    };
    if options.capitalize {
        let mut chars = display_part.chars();
        match chars.next() {
            Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            None => String::new(),
        }
    } else {
        display_part.to_string()
    }
}

/// Format a key description such as `ctrl+c/escape`.
pub fn format_key_text(key: &str, options: KeyTextFormatOptions) -> String {
    key.split('/')
        .map(|combination| {
            combination
                .split('+')
                .map(|part| format_key_part(part, options))
                .collect::<Vec<_>>()
                .join("+")
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn format_keys(keys: &[String], options: KeyTextFormatOptions) -> String {
    if keys.is_empty() {
        return String::new();
    }
    format_key_text(&keys.join("/"), options)
}

/// The keys bound to `keybinding`, formatted for display.
pub fn key_text(keybinding: &str) -> String {
    format_keys(
        &keybinding_keys(keybinding),
        KeyTextFormatOptions::default(),
    )
}

/// Like [`key_text`], with every part capitalised.
pub fn key_display_text(keybinding: &str) -> String {
    format_keys(
        &keybinding_keys(keybinding),
        KeyTextFormatOptions { capitalize: true },
    )
}

/// `<keys> <description>`, themed.
pub fn key_hint(keybinding: &str, description: &str) -> String {
    let theme = theme();
    theme.fg(ThemeColor::Dim, &key_text(keybinding))
        + &theme.fg(ThemeColor::Muted, &format!(" {description}"))
}

/// Like [`key_hint`] for a literal key description.
pub fn raw_key_hint(key: &str, description: &str) -> String {
    let theme = theme();
    theme.fg(
        ThemeColor::Dim,
        &format_key_text(key, KeyTextFormatOptions::default()),
    ) + &theme.fg(ThemeColor::Muted, &format!(" {description}"))
}
