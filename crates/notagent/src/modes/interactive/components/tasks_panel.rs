//! Shared helpers for the dock panels.
//! The background-shell roster that lived here is gone: the footer counts the
//! running shells and `/tasks` holds the detail, so a panel between them showed
//! the same work twice (user decision 2026-08-31). Subagents render in
//! `subagent_panel`.

/// `text.replace(/\s+/g, " ").trim()`
pub(crate) fn single_line(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut in_whitespace = false;
    for character in text.chars() {
        if character.is_whitespace() {
            in_whitespace = true;
            continue;
        }
        if in_whitespace && !result.is_empty() {
            result.push(' ');
        }
        in_whitespace = false;
        result.push(character);
    }
    result
}

/// Milliseconds since the epoch, as `Date.now()`.
pub(crate) fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
