//! Port of `packages/tui/test/select-list.test.ts` (116 LOC).

use std::rc::Rc;

use notagent_tui::components::select_list::{
    SelectItem, SelectList, SelectListLayoutOptions, SelectListTheme,
};
use notagent_tui::tui::Component;
use notagent_tui::visible_width;

fn test_theme() -> SelectListTheme {
    let identity = || -> Rc<dyn Fn(&str) -> String> { Rc::new(|text: &str| text.to_string()) };
    SelectListTheme {
        selected_prefix: identity(),
        selected_text: identity(),
        description: identity(),
        scroll_info: identity(),
        no_match: identity(),
    }
}

fn item(value: &str, description: Option<&str>) -> SelectItem {
    SelectItem {
        value: value.to_string(),
        label: value.to_string(),
        description: description.map(str::to_string),
    }
}

/// `String.prototype.indexOf` counts UTF-16 code units, not bytes.
fn utf16_index_of(line: &str, text: &str) -> Option<usize> {
    let byte_index = line.find(text)?;
    Some(line[..byte_index].encode_utf16().count())
}

fn visible_index_of(line: &str, text: &str) -> usize {
    let index = line.find(text).expect("text is present");
    visible_width(&line[..index])
}

#[test]
fn normalizes_multiline_descriptions_to_single_line() {
    let items = vec![item("test", Some("Line one\nLine two\nLine three"))];
    let mut list = SelectList::new(items, 5, test_theme(), SelectListLayoutOptions::default());
    let rendered = list.render(100);

    assert!(!rendered.is_empty());
    assert!(!rendered[0].contains('\n'));
    assert!(rendered[0].contains("Line one Line two Line three"));
}

#[test]
fn keeps_descriptions_aligned_when_the_primary_text_is_truncated() {
    let items = vec![
        item("short", Some("short description")),
        item(
            "very-long-command-name-that-needs-truncation",
            Some("long description"),
        ),
    ];
    let mut list = SelectList::new(items, 5, test_theme(), SelectListLayoutOptions::default());
    let rendered = list.render(80);

    assert_eq!(
        visible_index_of(&rendered[0], "short description"),
        visible_index_of(&rendered[1], "long description")
    );
}

#[test]
fn uses_the_configured_minimum_primary_column_width() {
    let items = vec![item("a", Some("first")), item("bb", Some("second"))];
    let mut list = SelectList::new(
        items,
        5,
        test_theme(),
        SelectListLayoutOptions {
            min_primary_column_width: Some(12),
            max_primary_column_width: Some(20),
            truncate_primary: None,
        },
    );
    let rendered = list.render(80);

    assert_eq!(utf16_index_of(&rendered[0], "first"), Some(14));
    assert_eq!(utf16_index_of(&rendered[1], "second"), Some(14));
}

#[test]
fn uses_the_configured_maximum_primary_column_width() {
    let items = vec![
        item(
            "very-long-command-name-that-needs-truncation",
            Some("first"),
        ),
        item("short", Some("second")),
    ];
    let mut list = SelectList::new(
        items,
        5,
        test_theme(),
        SelectListLayoutOptions {
            min_primary_column_width: Some(12),
            max_primary_column_width: Some(20),
            truncate_primary: None,
        },
    );
    let rendered = list.render(80);

    assert_eq!(visible_index_of(&rendered[0], "first"), 22);
    assert_eq!(visible_index_of(&rendered[1], "second"), 22);
}

#[test]
fn allows_overriding_primary_truncation_while_preserving_description_alignment() {
    let items = vec![
        item(
            "very-long-command-name-that-needs-truncation",
            Some("first"),
        ),
        item("short", Some("second")),
    ];
    let mut list = SelectList::new(
        items,
        5,
        test_theme(),
        SelectListLayoutOptions {
            min_primary_column_width: Some(12),
            max_primary_column_width: Some(12),
            truncate_primary: Some(Rc::new(|context| {
                // TS compares `text.length` (UTF-16 units); the fixtures are ASCII.
                if context.text.chars().count() <= context.max_width {
                    return context.text.to_string();
                }
                let keep: String = context
                    .text
                    .chars()
                    .take(context.max_width.saturating_sub(1))
                    .collect();
                format!("{keep}…")
            })),
        },
    );
    let rendered = list.render(80);

    assert!(rendered[0].contains('…'));
    assert_eq!(
        visible_index_of(&rendered[0], "first"),
        visible_index_of(&rendered[1], "second")
    );
}
