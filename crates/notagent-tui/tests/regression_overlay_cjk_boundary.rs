//! Port von `packages/tui/test/regression-overlay-cjk-boundary.test.ts` (46 LOC).
//!
//! Die beiden `compositeTuiLine`-Fälle der TS-Suite folgen mit Task 5
//! (`src/tui.ts`), sobald `composite_tui_line` portiert ist.

use notagent_tui::{extract_segments, visible_width};

#[test]
fn excludes_a_wide_grapheme_from_before_when_overlay_starts_inside_it() {
    let segments = extract_segments("abcd让EFGH", 5, 9, 11, true);

    assert_eq!(segments.before, "abcd");
    assert_eq!(segments.before_width, 4);
    assert_eq!(visible_width(&segments.before), segments.before_width);
    assert_eq!(segments.after, "H");
    assert_eq!(segments.after_width, 1);
}

#[test]
fn keeps_ascii_before_segment_behavior_at_the_same_boundary() {
    let segments = extract_segments("abcdG EFGH", 5, 9, 11, true);

    assert_eq!(segments.before, "abcdG");
    assert_eq!(segments.before_width, 5);
    assert_eq!(visible_width(&segments.before), segments.before_width);
}
