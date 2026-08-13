//! Port of `packages/tui/test/truncated-text.test.ts` (129 LOC).
//!
//! `chalk` is replaced by literal SGR sequences (the theme layer emits ANSI
//! directly anyway, master plan substitution).

use notagent_tui::components::truncated_text::TruncatedText;
use notagent_tui::tui::Component;
use notagent_tui::visible_width;

fn strip_sgr(line: &str) -> String {
    let mut out = String::new();
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < line.len() {
        if bytes[i] == 0x1b && line[i..].starts_with("\x1b[") {
            let rest = &line[i + 2..];
            let end = rest
                .find(|c: char| !c.is_ascii_digit() && c != ';')
                .unwrap_or(rest.len());
            if rest[end..].starts_with('m') {
                i += 2 + end + 1;
                continue;
            }
        }
        let ch = line[i..].chars().next().expect("char boundary");
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

#[test]
fn pads_output_lines_to_exactly_match_width() {
    let mut text = TruncatedText::new("Hello world", 1, 0);
    let lines = text.render(50);

    assert_eq!(lines.len(), 1);
    assert_eq!(visible_width(&lines[0]), 50);
}

#[test]
fn pads_output_with_vertical_padding_lines_to_width() {
    let mut text = TruncatedText::new("Hello", 0, 2);
    let lines = text.render(40);

    assert_eq!(lines.len(), 5);
    for line in &lines {
        assert_eq!(visible_width(line), 40);
    }
}

#[test]
fn truncates_long_text_and_pads_to_width() {
    let long_text =
        "This is a very long piece of text that will definitely exceed the available width";
    let mut text = TruncatedText::new(long_text, 1, 0);
    let lines = text.render(30);

    assert_eq!(lines.len(), 1);
    assert_eq!(visible_width(&lines[0]), 30);
    assert!(strip_sgr(&lines[0]).contains("..."));
}

#[test]
fn preserves_ansi_codes_in_output_and_pads_correctly() {
    let styled_text = "\x1b[31mHello\x1b[39m \x1b[34mworld\x1b[39m";
    let mut text = TruncatedText::new(styled_text, 1, 0);
    let lines = text.render(40);

    assert_eq!(lines.len(), 1);
    assert_eq!(visible_width(&lines[0]), 40);
    assert!(lines[0].contains("\x1b["));
}

#[test]
fn truncates_styled_text_and_adds_reset_code_before_ellipsis() {
    let long_styled_text = "\x1b[31mThis is a very long red text that will be truncated\x1b[39m";
    let mut text = TruncatedText::new(long_styled_text, 1, 0);
    let lines = text.render(20);

    assert_eq!(lines.len(), 1);
    assert_eq!(visible_width(&lines[0]), 20);
    assert!(lines[0].contains("\x1b[0m..."));
}

#[test]
fn handles_text_that_fits_exactly() {
    let mut text = TruncatedText::new("Hello world", 1, 0);
    let lines = text.render(30);

    assert_eq!(lines.len(), 1);
    assert_eq!(visible_width(&lines[0]), 30);
    assert!(!strip_sgr(&lines[0]).contains("..."));
}

#[test]
fn handles_empty_text() {
    let mut text = TruncatedText::new("", 1, 0);
    let lines = text.render(30);

    assert_eq!(lines.len(), 1);
    assert_eq!(visible_width(&lines[0]), 30);
}

#[test]
fn stops_at_newline_and_only_shows_first_line() {
    let mut text = TruncatedText::new("First line\nSecond line\nThird line", 1, 0);
    let lines = text.render(40);

    assert_eq!(lines.len(), 1);
    assert_eq!(visible_width(&lines[0]), 40);

    let stripped = strip_sgr(&lines[0]);
    let stripped = stripped.trim();
    assert!(stripped.contains("First line"));
    assert!(!stripped.contains("Second line"));
    assert!(!stripped.contains("Third line"));
}

#[test]
fn truncates_first_line_even_with_newlines_in_text() {
    let mut text = TruncatedText::new(
        "This is a very long first line that needs truncation\nSecond line",
        1,
        0,
    );
    let lines = text.render(25);

    assert_eq!(lines.len(), 1);
    assert_eq!(visible_width(&lines[0]), 25);

    let stripped = strip_sgr(&lines[0]);
    assert!(stripped.contains("..."));
    assert!(!stripped.contains("Second line"));
}
