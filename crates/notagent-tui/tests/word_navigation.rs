use std::collections::HashMap;

use notagent_tui::word_navigation::{WordNavigationOptions, find_word_backward, find_word_forward};

fn plain() -> WordNavigationOptions<'static> {
    WordNavigationOptions::default()
}

// describe("findWordBackward")

#[test]
fn backward_basic_words() {
    let text = "hello world";
    assert_eq!(find_word_backward(text, 11, &plain()), 6);
    assert_eq!(find_word_backward(text, 6, &plain()), 0);
}

#[test]
fn backward_dotted() {
    let text = "foo.bar";
    assert_eq!(find_word_backward(text, 7, &plain()), 4);
    assert_eq!(find_word_backward(text, 4, &plain()), 3);
    assert_eq!(find_word_backward(text, 3, &plain()), 0);
}

#[test]
fn backward_colon() {
    let text = "foo:bar";
    assert_eq!(find_word_backward(text, 7, &plain()), 4);
    assert_eq!(find_word_backward(text, 4, &plain()), 3);
    assert_eq!(find_word_backward(text, 3, &plain()), 0);
}

#[test]
fn backward_path() {
    let text = "path/to/file";
    assert_eq!(find_word_backward(text, 12, &plain()), 8);
    assert_eq!(find_word_backward(text, 8, &plain()), 7);
    assert_eq!(find_word_backward(text, 7, &plain()), 5);
    assert_eq!(find_word_backward(text, 5, &plain()), 4);
    assert_eq!(find_word_backward(text, 4, &plain()), 0);
}

#[test]
fn backward_cjk_mixed() {
    // Documented deviation (class 3): ICU segments Chinese with a dictionary
    // ("你好" / "世界"), UAX #29 in `unicode-segmentation` splits per character.
    let text = "你好世界 test";
    assert_eq!(find_word_backward(text, text.len(), &plain()), 13);
    assert_eq!(find_word_backward(text, 13, &plain()), 9);
    assert_eq!(find_word_backward(text, 9, &plain()), 6);
    assert_eq!(find_word_backward(text, 6, &plain()), 3);
    assert_eq!(find_word_backward(text, 3, &plain()), 0);
}

#[test]
fn backward_whitespace_at_boundaries() {
    let text = "  hello  ";
    assert_eq!(find_word_backward(text, 9, &plain()), 2);
    assert_eq!(find_word_backward(text, 2, &plain()), 0);
}

#[test]
fn backward_punctuation_run() {
    let text = "foo...bar";
    assert_eq!(find_word_backward(text, 9, &plain()), 6);
    assert_eq!(find_word_backward(text, 6, &plain()), 3);
    assert_eq!(find_word_backward(text, 3, &plain()), 0);
}

#[test]
fn backward_cursor_at_0_returns_0() {
    assert_eq!(find_word_backward("hello", 0, &plain()), 0);
}

// describe("findWordForward")

#[test]
fn forward_basic_words() {
    let text = "hello world";
    assert_eq!(find_word_forward(text, 0, &plain()), 5);
    assert_eq!(find_word_forward(text, 5, &plain()), 11);
}

#[test]
fn forward_dotted() {
    let text = "foo.bar";
    assert_eq!(find_word_forward(text, 0, &plain()), 3);
    assert_eq!(find_word_forward(text, 3, &plain()), 4);
    assert_eq!(find_word_forward(text, 4, &plain()), 7);
}

#[test]
fn forward_colon() {
    let text = "foo:bar";
    assert_eq!(find_word_forward(text, 0, &plain()), 3);
    assert_eq!(find_word_forward(text, 3, &plain()), 4);
    assert_eq!(find_word_forward(text, 4, &plain()), 7);
}

#[test]
fn forward_path() {
    let text = "path/to/file";
    assert_eq!(find_word_forward(text, 0, &plain()), 4);
    assert_eq!(find_word_forward(text, 4, &plain()), 5);
    assert_eq!(find_word_forward(text, 5, &plain()), 7);
    assert_eq!(find_word_forward(text, 7, &plain()), 8);
    assert_eq!(find_word_forward(text, 8, &plain()), 12);
}

#[test]
fn forward_cjk_mixed() {
    let text = "你好世界 test";
    let first_end = find_word_forward(text, 0, &plain());
    assert!(first_end > 0);
    // Four CJK characters are at most twelve bytes.
    assert!(first_end <= 12);

    let mut pos = 0;
    while pos < text.len() {
        let next = find_word_forward(text, pos, &plain());
        if next == pos {
            break;
        }
        pos = next;
    }
    assert_eq!(pos, text.len());
}

#[test]
fn forward_whitespace_at_boundaries() {
    let text = "  hello  ";
    assert_eq!(find_word_forward(text, 0, &plain()), 7);
    assert_eq!(find_word_forward(text, 7, &plain()), 9);
}

#[test]
fn forward_punctuation_run() {
    let text = "foo...bar";
    assert_eq!(find_word_forward(text, 0, &plain()), 3);
    assert_eq!(find_word_forward(text, 3, &plain()), 6);
    assert_eq!(find_word_forward(text, 6, &plain()), 9);
}

#[test]
fn forward_cursor_at_end_returns_end() {
    assert_eq!(find_word_forward("hello", 5, &plain()), 5);
}

// describe("atomic segments")

#[test]
fn atomic_segments_are_skipped_as_one_unit() {
    let marker = "[paste #1 +5 lines]";
    let text = format!("hello {marker} world");
    let is_atomic = |segment: &str| segment == marker;

    // The functions slice the text before segmenting, so each expected slice is
    let mut segment_map: HashMap<String, Vec<(usize, String)>> = HashMap::new();
    segment_map.insert(
        text.clone(),
        vec![
            (0, "hello".to_string()),
            (5, " ".to_string()),
            (6, marker.to_string()),
            (25, " ".to_string()),
            (26, "world".to_string()),
        ],
    );
    segment_map.insert(
        text[..26].to_string(),
        vec![
            (0, "hello".to_string()),
            (5, " ".to_string()),
            (6, marker.to_string()),
            (25, " ".to_string()),
        ],
    );
    segment_map.insert(
        text[6..].to_string(),
        vec![
            (0, marker.to_string()),
            (19, " ".to_string()),
            (20, "world".to_string()),
        ],
    );

    let segment = |input: &str| segment_map.get(input).cloned().unwrap_or_default();
    let options = WordNavigationOptions {
        segment: Some(&segment),
        is_atomic_segment: Some(&is_atomic),
    };

    assert_eq!(find_word_backward(&text, text.len(), &options), 26);
    assert_eq!(find_word_backward(&text, 26, &options), 6);
    assert_eq!(find_word_forward(&text, 6, &options), 6 + marker.len());
}
