//! The `chalk` styles the CLI paths use, as direct ANSI sequences.
//! substitutes direct escape sequences for it. Only the four styles the CLI and
//! the app entry point use are here — the styling inside the TUI goes through
//! the theme instead.
//! Colour support is decided the way chalk decides it: `NO_COLOR` wins, then
//! `FORCE_COLOR`, then whether standard output is a terminal. chalk asks about
//! standard output even for text that goes to standard error, and so does this.

use std::sync::OnceLock;

use crate::core::output_guard::stdout_is_tty;

fn color_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        if std::env::var("NO_COLOR").is_ok_and(|value| !value.is_empty()) {
            return false;
        }
        if let Ok(force) = std::env::var("FORCE_COLOR") {
            return !matches!(force.as_str(), "0" | "false" | "none");
        }
        if std::env::var("TERM").as_deref() == Ok("dumb") {
            return false;
        }
        stdout_is_tty()
    })
}

fn style(open: &str, close: &str, text: &str) -> String {
    if !color_enabled() {
        return text.to_owned();
    }
    format!("{open}{text}{close}")
}

pub fn red(text: &str) -> String {
    style("\u{1b}[31m", "\u{1b}[39m", text)
}

pub fn yellow(text: &str) -> String {
    style("\u{1b}[33m", "\u{1b}[39m", text)
}

pub fn cyan(text: &str) -> String {
    style("\u{1b}[36m", "\u{1b}[39m", text)
}

pub fn dim(text: &str) -> String {
    style("\u{1b}[2m", "\u{1b}[22m", text)
}
