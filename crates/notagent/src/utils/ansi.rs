//! Port of `packages/coding-agent/src/utils/ansi.ts`.
//!
//! That file carries an MIT notice for the `ansi-regex`/`strip-ansi` packages by
//! Sindre Sorhus; the pattern below is the same grammar expressed as a regex
//! literal, so the notice applies here as well:
//!
//! ```text
//! MIT License — Copyright (c) Sindre Sorhus <sindresorhus@gmail.com>
//! ```

use std::sync::LazyLock;

use regex::Regex;

static ANSI: LazyLock<Regex> = LazyLock::new(|| {
    // Valid string terminators are BEL, ESC \ and 0x9c.
    let terminator = r"(?:\x07|\x1b\x5c|\u{9c})";
    // OSC sequences: ESC ] … ST, non-greedy up to the first terminator.
    let osc = format!(r"(?:\x1b\][\s\S]*?{terminator})");
    // CSI and friends: ESC/C1, optional intermediates, optional params, final byte.
    let csi = r"[\x1b\u{9b}][\[\]()#;?]*(?:\d{1,4}(?:[;:]\d{0,4})*)?[\dA-PR-TZcf-nq-uy=><~]";
    Regex::new(&format!("{osc}|{csi}")).expect("ansi regex")
});

pub fn strip_ansi(value: &str) -> String {
    // Fast path: ANSI codes need the 7-bit ESC or the 8-bit CSI introducer.
    if !value.contains('\u{1b}') && !value.contains('\u{9b}') {
        return value.to_owned();
    }
    ANSI.replace_all(value, "").into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_text_without_escape_sequences() {
        assert_eq!(strip_ansi("plain text"), "plain text");
        assert_eq!(strip_ansi(""), "");
    }

    #[test]
    fn strips_colour_and_style_sequences() {
        assert_eq!(strip_ansi("\u{1b}[31mred\u{1b}[0m"), "red");
        assert_eq!(strip_ansi("\u{1b}[1;32;40mstyled\u{1b}[m"), "styled");
        assert_eq!(
            strip_ansi("\u{1b}[38;2;255;0;0mtruecolor\u{1b}[39m"),
            "truecolor"
        );
    }

    #[test]
    fn strips_cursor_and_erase_sequences() {
        assert_eq!(strip_ansi("a\u{1b}[2Kb\u{1b}[1Ac"), "abc");
        assert_eq!(strip_ansi("\u{1b}[?25lhidden\u{1b}[?25h"), "hidden");
    }

    #[test]
    fn strips_osc_sequences_with_every_terminator() {
        assert_eq!(strip_ansi("\u{1b}]0;title\u{7}text"), "text");
        assert_eq!(
            strip_ansi("\u{1b}]8;;https://example.com\u{1b}\\link"),
            "link"
        );
        assert_eq!(strip_ansi("\u{1b}]0;title\u{9c}text"), "text");
    }

    #[test]
    fn strips_the_eight_bit_csi_introducer() {
        assert_eq!(strip_ansi("\u{9b}31mred\u{9b}0m"), "red");
    }

    #[test]
    fn leaves_a_lone_escape_untouched() {
        assert_eq!(strip_ansi("\u{1b}"), "\u{1b}");
        assert_eq!(strip_ansi("a\u{1b}b"), "a\u{1b}b");
    }
}
