//! Port of the parser part of `packages/tui/test/terminal-colors.test.ts` (252 LOC).
//!
//! The `TUI.queryTerminalBackgroundColor` cases drive `TuiMainScreen` and follow
//! with task 6.

use notagent_tui::terminal_colors::{
    RgbColor, TerminalColorScheme, parse_osc11_background_color, parse_terminal_color_scheme_report,
};

#[test]
fn parses_16_bit_osc_11_rgb_responses() {
    assert_eq!(
        parse_osc11_background_color("\x1b]11;rgb:0000/8000/ffff\x07"),
        Some(RgbColor {
            r: 0,
            g: 128,
            b: 255
        })
    );
}

#[test]
fn parses_osc_11_hex_responses() {
    assert_eq!(
        parse_osc11_background_color("\x1b]11;#ffffff\x1b\\"),
        Some(RgbColor {
            r: 255,
            g: 255,
            b: 255
        })
    );
    assert_eq!(
        parse_osc11_background_color("\x1b]11;#000000\x07"),
        Some(RgbColor { r: 0, g: 0, b: 0 })
    );
}

#[test]
fn rejects_non_strict_osc_11_responses() {
    assert_eq!(parse_osc11_background_color("x\x1b]11;#ffffff\x07"), None);
    assert_eq!(parse_osc11_background_color("\x1b]10;#ffffff\x07"), None);
    assert_eq!(parse_osc11_background_color("\x1b]11;#ffffff\x07x"), None);
}

#[test]
fn parses_color_scheme_reports() {
    assert_eq!(
        parse_terminal_color_scheme_report("\x1b[?997;1n"),
        Some(TerminalColorScheme::Dark)
    );
    assert_eq!(
        parse_terminal_color_scheme_report("\x1b[?997;2n"),
        Some(TerminalColorScheme::Light)
    );
    assert_eq!(
        parse_terminal_color_scheme_report("\x1b[?997;2n\x1b[?997;1n\x1b[?997;1n"),
        Some(TerminalColorScheme::Dark)
    );
    assert_eq!(
        parse_terminal_color_scheme_report("\x1b[?997;1n\x1b[?997;2n\x1b[?997;2n"),
        Some(TerminalColorScheme::Light)
    );
    assert_eq!(parse_terminal_color_scheme_report("\x1b[?997;3n"), None);
    assert_eq!(parse_terminal_color_scheme_report("\x1b[?996n"), None);
    assert_eq!(parse_terminal_color_scheme_report("x\x1b[?997;1n"), None);
}
