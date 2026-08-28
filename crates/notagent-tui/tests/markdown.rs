//! Native behavior tests for markdown caching, links, and shared rendered lines.

use std::cell::RefCell;
use std::rc::Rc;

use notagent_tui::components::markdown::{
    Markdown, MarkdownOptions, MarkdownTheme, StyleFn, TransformFn,
};
use notagent_tui::terminal_image::{
    ImageProtocol, TerminalCapabilities, reset_capabilities_cache, set_capabilities,
};
use notagent_tui::tui::{Component, Line};

/// SGR reset plus OSC 8 link close — what the paint pass appends and the
/// component now bakes into its cache (`tui.rs`, `SEGMENT_RESET`).
const SEGMENT_RESET: &str = "\x1b[0m\x1b]8;;\x07";

/// Serializes the cases, which share the global terminal capabilities.
static CAPABILITY_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn lock_capabilities() -> std::sync::MutexGuard<'static, ()> {
    CAPABILITY_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A chalk style: nested occurrences of the close sequence are re-opened, so
/// nesting the same style keeps it active (chalk's `applyStyle`).
fn style(code: &'static str, reset: &'static str) -> StyleFn {
    Rc::new(move |text: &str| {
        let open = format!("\x1b[{code}m");
        let close = format!("\x1b[{reset}m");
        let inner = text.replace(&close, &format!("{close}{open}"));
        // chalk closes and re-opens the style around every newline.
        let inner = inner.replace('\n', &format!("{close}\n{open}"));
        format!("{open}{inner}{close}")
    })
}

/// `chalk.bold.cyan`: both styles wrap, outermost first.
fn bold_cyan() -> StyleFn {
    let bold = style("1", "22");
    let cyan = style("36", "39");
    Rc::new(move |text: &str| bold(&cyan(text)))
}

/// Shared high-contrast theme for the markdown component tests.
fn default_markdown_theme() -> MarkdownTheme {
    MarkdownTheme {
        heading: bold_cyan(),
        link: style("34", "39"),
        link_url: style("2", "22"),
        code: style("33", "39"),
        code_block: style("32", "39"),
        code_block_border: style("2", "22"),
        quote: style("3", "23"),
        quote_border: style("2", "22"),
        hr: style("2", "22"),
        list_bullet: style("36", "39"),
        bold: style("1", "22"),
        italic: style("3", "23"),
        strikethrough: style("9", "29"),
        underline: style("4", "24"),
        highlight_code: None,
        code_block_indent: None,
    }
}

#[test]
fn caches_transformed_markdown_by_source_and_available_width() {
    let _capabilities = lock_capabilities();
    let calls: Rc<RefCell<Vec<(String, usize)>>> = Rc::new(RefCell::new(Vec::new()));
    let recorded = calls.clone();
    let transform: TransformFn = Rc::new(move |source: &str, available_width: usize| {
        recorded
            .borrow_mut()
            .push((source.to_string(), available_width));
        format!("{source} {available_width}")
    });

    let mut markdown = Markdown::new(
        "source",
        2,
        0,
        default_markdown_theme(),
        None,
        Some(MarkdownOptions {
            transform: Some(transform),
            ..MarkdownOptions::default()
        }),
    );

    assert_eq!(strip_ansi_trimmed(&markdown.render(80)), ["source 76"]);
    markdown.render(80);
    assert_eq!(strip_ansi_trimmed(&markdown.render(60)), ["source 56"]);
    assert_eq!(
        *calls.borrow(),
        [("source".to_string(), 76), ("source".to_string(), 56)]
    );

    markdown.set_text("updated");
    assert_eq!(strip_ansi_trimmed(&markdown.render(60)), ["updated 56"]);
    assert_eq!(
        calls.borrow().last().cloned(),
        Some(("updated".to_string(), 56))
    );

    markdown.invalidate();
    markdown.render(60);
    assert_eq!(
        calls.borrow().last().cloned(),
        Some(("updated".to_string(), 56))
    );
    assert_eq!(calls.borrow().len(), 4);
}

fn strip_ansi_trimmed(lines: &[Line]) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            notagent_tui::utils::strip_terminal_sequences(line)
                .trim()
                .to_string()
        })
        .collect()
}

#[test]
fn emits_an_osc8_hyperlink_when_the_terminal_supports_it() {
    let _capabilities = lock_capabilities();
    set_capabilities(TerminalCapabilities {
        images: None::<ImageProtocol>,
        true_color: true,
        hyperlinks: true,
    });

    let mut markdown = Markdown::new(
        "[example](https://example.com)",
        0,
        0,
        default_markdown_theme(),
        None,
        None,
    );
    let output = markdown.render(80).join("\n");
    assert!(output.contains("\x1b]8;;https://example.com\x1b\\"));
    assert!(output.contains("\x1b]8;;\x1b\\"));
    assert!(!output.contains("(https://example.com)"));

    reset_capabilities_cache();
}

#[test]
fn shows_the_url_in_parentheses_without_hyperlink_support() {
    let _capabilities = lock_capabilities();
    set_capabilities(TerminalCapabilities {
        images: None::<ImageProtocol>,
        true_color: true,
        hyperlinks: false,
    });

    let mut markdown = Markdown::new(
        "[example](https://example.com)",
        0,
        0,
        default_markdown_theme(),
        None,
        None,
    );
    let output = notagent_tui::utils::strip_terminal_sequences(&markdown.render(80).join("\n"));
    assert!(output.contains("example (https://example.com)"));

    reset_capabilities_cache();
}

#[test]
fn finishes_its_cached_lines_and_a_hit_keeps_their_identity() {
    // Step 8 of the line-sharing plan: every non-image line leaves `render`
    // the way `apply_line_resets` would leave it — normalized, reset at the
    // end — so the paint pass skips it and an unchanged transcript line can
    // settle by pointer identity in the screen diff (step 7).
    let mut markdown = Markdown::new(
        "A paragraph with **bold** text.",
        1,
        1,
        default_markdown_theme(),
        None,
        None,
    );

    let first = markdown.render(32);
    assert!(!first.is_empty());
    for line in &first {
        assert!(line.ends_with(SEGMENT_RESET), "unfinished line: {line:?}");
    }

    let second = markdown.render(32);
    assert_eq!(first.len(), second.len());
    for (line, repeat) in first.iter().zip(second.iter()) {
        assert!(
            Line::ptr_eq(line, repeat),
            "a cache hit must return the same shared line: {line:?}"
        );
    }
}
