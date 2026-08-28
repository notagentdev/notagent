use super::shells::ShellId;

/// Theme colour key per shell: read-only is calm, worker is active.
pub fn indicator_color_key(shell: ShellId) -> &'static str {
    match shell {
        ShellId::ReadOnly => "success",
        ShellId::Worker => "warning",
    }
}

/// Short label for the footer, e.g. `plan (read-only)`. The shell is spelled
/// out for a read-only mode because that is the restriction worth stating; a
/// worker mode shows only its name to keep the footer short.
pub fn format_mode_label(id: &str, shell: ShellId) -> String {
    match shell {
        ShellId::ReadOnly => format!("{id} (read-only)"),
        ShellId::Worker => id.to_string(),
    }
}

/// Message shown after a mode switch. Includes the injected token volume,
/// because several large skills in one folder silently negate the benefit of a
/// short system prompt and the author needs to see that cost.
pub fn format_mode_switch_notice(id: &str, shell: ShellId, injected_tokens: usize) -> String {
    let scope = match shell {
        ShellId::ReadOnly => "read-only",
        ShellId::Worker => "full access",
    };
    format!(
        "mode: {id} — {scope}, {} injected",
        format_token_count(injected_tokens)
    )
}

fn format_token_count(tokens: usize) -> String {
    if tokens < 1000 {
        return format!("{tokens} tokens");
    }
    // `toFixed(1)` rounds half away from zero; Rust's `{:.1}` rounds half to
    // even, which differs for values such as 2050 tokens.
    let thousands = tokens as f64 / 1000.0;
    let rounded = (thousands * 10.0).round() / 10.0;
    format!("{rounded:.1}k tokens")
}

/// Rough token estimate for injected mode text. Deliberately cheap: this drives
/// a visibility hint, not a budget decision, so a tokenizer round trip would
/// cost more than the number is worth.
pub fn estimate_injected_tokens(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    text.encode_utf16().count().div_ceil(4)
}
