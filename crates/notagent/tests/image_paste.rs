//! Images pasted into the input box.
//!
//! A paste used to put its temporary file path into the editor. Now it inserts
//! `[Image #1]` and the bytes ride along with the message. What is pinned here
//! is the parsing half — which markers resolve, in what order, and that a
//! marker the user typed themselves resolves to nothing.

use notagent::modes::interactive::interactive_mode::{format_image_marker, parse_image_markers};

#[test]
fn numbers_markers_from_one() {
    assert_eq!(format_image_marker(1), "[Image #1]");
    assert_eq!(format_image_marker(12), "[Image #12]");
}

#[test]
fn reads_the_markers_in_the_order_they_appear() {
    let text = format!(
        "compare {} against {}",
        format_image_marker(2),
        format_image_marker(1)
    );
    assert_eq!(parse_image_markers(&text), vec![2, 1]);
}

/// The same image mentioned twice is still one attachment; the caller
/// de-duplicates, so the parse keeps both mentions.
#[test]
fn keeps_every_mention() {
    let text = format!("{a} and {a}", a = format_image_marker(3));
    assert_eq!(parse_image_markers(&text), vec![3, 3]);
}

/// `#0` is never written, so a text containing one is the user's own.
#[test]
fn ignores_a_zero_marker() {
    assert!(parse_image_markers("see [Image #0]").is_empty());
}

#[test]
fn ignores_text_that_only_looks_like_a_marker() {
    assert!(parse_image_markers("[Image #]").is_empty());
    assert!(parse_image_markers("[image #1]").is_empty(), "case matters");
    assert!(parse_image_markers("[Image 1]").is_empty());
    assert!(parse_image_markers("[paste #1 +3 lines]").is_empty());
}

#[test]
fn finds_a_marker_with_no_spaces_around_it() {
    assert_eq!(parse_image_markers("a[Image #7]b"), vec![7]);
}
