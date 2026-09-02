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
fn headings_hide_markdown_markers_at_every_level() {
    let _capabilities = lock_capabilities();
    let mut markdown = Markdown::new(
        "# H1\n## H2\n### H3\n#### H4\n##### H5\n###### H6",
        0,
        0,
        default_markdown_theme(),
        None,
        None,
    );

    let visible_lines: Vec<String> = strip_ansi_trimmed(&markdown.render(80))
        .into_iter()
        .filter(|line| !line.is_empty())
        .collect();

    assert_eq!(visible_lines, ["H1", "H2", "H3", "H4", "H5", "H6"]);
}

#[test]
fn a_systemically_cramped_table_renders_as_stacked_records() {
    let _capabilities = lock_capabilities();
    let mut markdown = Markdown::new(
        "| Key | Notes |\n| --- | --- |\n| firstlongid | A readable explanatory sentence for this row. |\n| secondlongid | Another readable explanatory sentence for this row. |\n| short | A final readable explanatory sentence for this row. |",
        0,
        0,
        default_markdown_theme(),
        None,
        None,
    );

    let visible_lines: Vec<String> = strip_ansi_trimmed(&markdown.render(17))
        .into_iter()
        .filter(|line| !line.is_empty())
        .collect();

    assert!(
        !visible_lines.iter().any(|line| line.starts_with('┌')),
        "a cramped table must not retain its grid: {visible_lines:?}"
    );
    assert_eq!(
        visible_lines
            .iter()
            .filter(|line| line.as_str() == "Key")
            .count(),
        3,
        "each source row must become one labelled record: {visible_lines:?}"
    );
    assert_eq!(
        visible_lines
            .iter()
            .filter(|line| line.chars().all(|character| character == '─'))
            .count(),
        2,
        "stacked records must remain visually separable: {visible_lines:?}"
    );
    assert!(
        visible_lines.iter().any(|line| line == "firstlongid"),
        "record values must remain visible: {visible_lines:?}"
    );
}

#[test]
fn cramped_tables_keep_labels_and_values_aligned_when_space_allows() {
    let _capabilities = lock_capabilities();
    let mut markdown = Markdown::new(
        "| A | B | C | D | E | F |\n| --- | --- | --- | --- | --- | --- |\n| alphabet | birthday | calendar | document | elephant | fountain |\n| airplane | building | cardinal | dinosaur | envelope | festival |",
        0,
        0,
        default_markdown_theme(),
        None,
        None,
    );

    let visible_lines = strip_ansi_trimmed(&markdown.render(30));

    assert!(
        visible_lines.iter().any(|line| line == "A  alphabet"),
        "record fields should remain aligned when their values have enough width: {visible_lines:?}"
    );
    assert!(
        !visible_lines.iter().any(|line| line.starts_with('┌')),
        "a systemically fragmented grid must use records: {visible_lines:?}"
    );
}

#[test]
fn one_mildly_fragmented_row_keeps_the_table_grid() {
    let _capabilities = lock_capabilities();
    let mut markdown = Markdown::new(
        "| Key | Date | State |\n| --- | --- | --- |\n| short | 2025-01-01 | Ready |\n| verylongidentifier | 2025-02-02 | Ready |\n| final | 2025-03-03 | Done |",
        0,
        0,
        default_markdown_theme(),
        None,
        None,
    );

    let visible_lines = strip_ansi_trimmed(&markdown.render(40));

    assert!(
        visible_lines.iter().any(|line| line.starts_with('┌')),
        "one exceptional cell must not collapse an otherwise readable grid: {visible_lines:?}"
    );
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
