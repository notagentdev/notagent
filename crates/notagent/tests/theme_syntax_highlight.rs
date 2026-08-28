use notagent::modes::interactive::theme::theme::{ThemeColor, highlight_code, init_theme, theme};
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

    assert_eq!(lines[0], theme().fg(ThemeColor::ToolDiffRemoved, "-old"));
    assert_eq!(lines[1], theme().fg(ThemeColor::ToolDiffAdded, "+new"));
}

#[test]
fn keeps_cli_highlight_default_styled_scopes_mapped_to_theme_styles() {
    setup();
    // (`\x1b[38;2;206;145;120m/foo+/gi\x1b[39m`): highlight.js gives `/foo+/gi`
    // the single scope `regexp`. tree-sitter splits it — the delimiters are
    // `operator`, the body and the flags are `string` — so the body carries the
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
    // (`\x1b[38;2;128;128;128m@decorator\x1b[39m`, scope `meta` -> `muted`):
    // tree-sitter reports the decorator as `function`, so it takes the
    // `syntaxFunction` colour, and it captures `@` and the name separately.
    assert_eq!(
        highlight_code("@decorator", Some("python"))[0],
        "\x1b[38;2;220;220;170m@\x1b[39m\x1b[38;2;220;220;170mdecorator\x1b[39m"
    );
}

#[test]
fn falls_back_to_the_code_block_colour_for_an_unsupported_language() {
    setup();
    // `supportsLanguage` is false for the languages tree-sitter does not bundle
    // every line in `mdCodeBlock`.
    let lines = highlight_code("key: value", Some("yaml"));
    assert_eq!(lines, ["\x1b[38;2;181;189;104mkey: value\x1b[39m"]);

    let lines = highlight_code("plain text", None);
    assert_eq!(lines, ["\x1b[38;2;181;189;104mplain text\x1b[39m"]);
}
