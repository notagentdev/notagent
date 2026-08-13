//! Word-wise cursor navigation.
//!
//! 1:1 port of `packages/tui/src/word-navigation.ts` (117 LOC). `Intl.Segmenter`
//! with word granularity becomes `unicode-segmentation`'s word bounds; a segment
//! counts as word-like when it contains an alphanumeric character, which matches
//! ICU's `isWordLike`.

use crate::utils::{is_punctuation_char, is_whitespace_char, word_segments};

/// Predicate marking segments that must be treated as single units
/// (for example the editor's paste markers).
pub type IsAtomicSegment<'a> = &'a dyn Fn(&str) -> bool;

/// Custom segmentation returning word segments with their byte offsets.
pub type SegmentFn<'a> = &'a dyn Fn(&str) -> Vec<(usize, String)>;

/// Options of the word navigation functions.
#[derive(Default)]
pub struct WordNavigationOptions<'a> {
    /// Custom segmentation returning word segments with their byte offsets.
    pub segment: Option<SegmentFn<'a>>,
    /// Segments to treat as atomic.
    pub is_atomic_segment: Option<IsAtomicSegment<'a>>,
}

fn is_word_like(segment: &str) -> bool {
    segment.chars().any(char::is_alphanumeric)
}

fn segments_of(text: &str, options: &WordNavigationOptions<'_>) -> Vec<String> {
    match options.segment {
        Some(segment) => segment(text).into_iter().map(|(_, s)| s).collect(),
        None => word_segments(text).map(str::to_string).collect(),
    }
}

fn is_atomic(options: &WordNavigationOptions<'_>, segment: &str) -> bool {
    options
        .is_atomic_segment
        .map(|predicate| predicate(segment))
        .unwrap_or(false)
}

/// Cursor position after moving one word backward.
pub fn find_word_backward(text: &str, cursor: usize, options: &WordNavigationOptions<'_>) -> usize {
    if cursor == 0 {
        return 0;
    }

    let text_before_cursor = &text[..cursor];
    let mut segments = segments_of(text_before_cursor, options);
    let mut new_cursor = cursor;

    // Skip trailing whitespace.
    while let Some(last) = segments.last() {
        if is_atomic(options, last) || !is_whitespace_char(last) {
            break;
        }
        new_cursor -= last.len();
        segments.pop();
    }

    let Some(last) = segments.last().cloned() else {
        return new_cursor;
    };

    if is_atomic(options, &last) {
        // Skip one atomic segment.
        new_cursor -= last.len();
    } else if is_word_like(&last) {
        // Skip inside one word-like segment, preserving ASCII punctuation boundaries.
        let last_punctuation = last
            .char_indices()
            .rfind(|(_, c)| is_punctuation_char(&c.to_string()));
        match last_punctuation {
            None => new_cursor -= last.len(),
            Some((index, c)) => new_cursor -= last.len() - (index + c.len_utf8()),
        }
    } else {
        // Skip a run of punctuation.
        while let Some(segment) = segments.last() {
            if is_atomic(options, segment) || is_word_like(segment) || is_whitespace_char(segment) {
                break;
            }
            new_cursor -= segment.len();
            segments.pop();
        }
    }

    new_cursor
}

/// Cursor position after moving one word forward.
pub fn find_word_forward(text: &str, cursor: usize, options: &WordNavigationOptions<'_>) -> usize {
    if cursor >= text.len() {
        return text.len();
    }

    let text_after_cursor = &text[cursor..];
    let segments = segments_of(text_after_cursor, options);
    let mut iter = segments.into_iter().peekable();
    let mut new_cursor = cursor;

    // Skip leading whitespace.
    while let Some(segment) = iter.peek() {
        if is_atomic(options, segment) || !is_whitespace_char(segment) {
            break;
        }
        new_cursor += segment.len();
        iter.next();
    }

    let Some(segment) = iter.next() else {
        return new_cursor;
    };

    if is_atomic(options, &segment) {
        // Skip one atomic segment.
        new_cursor += segment.len();
    } else if is_word_like(&segment) {
        // Skip inside one word-like segment, preserving ASCII punctuation boundaries.
        let first_punctuation = segment
            .char_indices()
            .find(|(_, c)| is_punctuation_char(&c.to_string()))
            .map(|(index, _)| index);
        new_cursor += first_punctuation.unwrap_or(segment.len());
    } else {
        // Skip a run of punctuation.
        let mut current = Some(segment);
        while let Some(segment) = current {
            if is_atomic(options, &segment)
                || is_word_like(&segment)
                || is_whitespace_char(&segment)
            {
                break;
            }
            new_cursor += segment.len();
            current = iter.next();
        }
    }

    new_cursor
}
