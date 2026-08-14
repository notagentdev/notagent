//! Port of the `theme syntax highlighting` suite of
//! `packages/coding-agent/test/syntax-highlight.test.ts` (76 LOC).
//!
//! The first `describe` of that file drives `utils/syntax-highlight.ts` and is
//! ported as unit tests inside `crates/notagent/src/utils/syntax_highlight.rs`.
//! Its own test binary because it installs global terminal capabilities.

use notagent::modes::interactive::theme::theme::{highlight_code, init_theme};
use notagent_tui::terminal_image::{TerminalCapabilities, set_capabilities};

fn setup() {
    set_capabilities(TerminalCapabilities {
        images: None,
        true_color: true,
        hyperlinks: false,
    });
    init_theme(Some("dark"), false);
}

#[test]
fn colors_diff_additions_and_deletions_in_fenced_diff_blocks() {
    setup();
    let lines = highlight_code("-old\n+new\n", Some("diff"));

    assert_eq!(lines[0], "\x1b[38;2;204;102;102m-old\x1b[39m");
    assert_eq!(lines[1], "\x1b[38;2;181;189;104m+new\x1b[39m");
}

#[test]
fn keeps_cli_highlight_default_styled_scopes_mapped_to_theme_styles() {
    setup();
    // The TypeScript expectation is the whole literal in one run
    // (`\x1b[38;2;206;145;120m/foo+/gi\x1b[39m`): highlight.js gives `/foo+/gi`
    // the single scope `regexp`. tree-sitter splits it — the delimiters are
    // `operator`, the body and the flags are `string` — so the body carries the
    // same colour but the run is shorter. Announced in interface request C-6.
    let javascript = highlight_code("const re = /foo+/gi;", Some("javascript"));
    assert!(
        javascript[0].contains("\x1b[38;2;206;145;120mfoo+\x1b[39m"),
        "{javascript:?}"
    );
    assert!(
        javascript[0].contains("\x1b[38;2;206;145;120mgi\x1b[39m"),
        "{javascript:?}"
    );

    let html = highlight_code("<div></div>", Some("html"));
    assert!(
        html[0].contains("\x1b[38;2;86;156;214mdiv\x1b[39m"),
        "{html:?}"
    );
}

#[test]
fn colors_a_python_decorator_as_a_function_scope() {
    setup();
    // Deviation from the TypeScript expectation
    // (`\x1b[38;2;128;128;128m@decorator\x1b[39m`, scope `meta` -> `muted`):
    // tree-sitter reports the decorator as `function`, so it takes the
    // `syntaxFunction` colour, and it captures `@` and the name separately.
    // Announced in interface request C-6.
    assert_eq!(
        highlight_code("@decorator", Some("python"))[0],
        "\x1b[38;2;220;220;170m@\x1b[39m\x1b[38;2;220;220;170mdecorator\x1b[39m"
    );
}

#[test]
fn falls_back_to_the_code_block_colour_for_an_unsupported_language() {
    setup();
    // `supportsLanguage` is false for the languages tree-sitter does not bundle
    // (C-6), so those take the same path as an unknown language in TypeScript:
    // every line in `mdCodeBlock`.
    let lines = highlight_code("key: value", Some("yaml"));
    assert_eq!(lines, ["\x1b[38;2;181;189;104mkey: value\x1b[39m"]);

    let lines = highlight_code("plain text", None);
    assert_eq!(lines, ["\x1b[38;2;181;189;104mplain text\x1b[39m"]);
}
