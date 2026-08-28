use notagent_tui::{visible_width, wrap_text_with_ansi};

#[test]
fn treats_partial_flag_grapheme_as_full_width_to_avoid_streaming_render_drift() {
    let partial_flag = "🇨";
    let list_line = "      - 🇨";

    assert_eq!(visible_width(partial_flag), 2);
    assert_eq!(visible_width(list_line), 10);
}

#[test]
fn wraps_intermediate_partial_flag_list_line_before_overflow() {
    let wrapped = wrap_text_with_ansi("      - 🇨", 9);

    assert_eq!(wrapped.len(), 2);
    assert_eq!(visible_width(wrapped.first().map_or("", |l| l.as_str())), 7);
    assert_eq!(visible_width(wrapped.get(1).map_or("", |l| l.as_str())), 2);
}

#[test]
fn treats_all_regional_indicator_singleton_graphemes_as_width_2() {
    for cp in 0x1f1e6u32..=0x1f1ff {
        let regional_indicator = char::from_u32(cp).expect("valid code point").to_string();
        assert_eq!(
            visible_width(&regional_indicator),
            2,
            "Expected {regional_indicator} (U+{cp:X}) to be width 2"
        );
    }
}

#[test]
fn keeps_full_flag_pairs_at_width_2() {
    for flag in ["🇯🇵", "🇺🇸", "🇬🇧", "🇨🇳", "🇩🇪", "🇫🇷"] {
        assert_eq!(visible_width(flag), 2, "Expected {flag} to be width 2");
    }
}

#[test]
fn keeps_common_streaming_emoji_intermediates_at_stable_width() {
    for sample in ["👍", "👍🏻", "✅", "⚡", "⚡️", "👨", "👨‍💻", "🏳️‍🌈"]
    {
        assert_eq!(visible_width(sample), 2, "Expected {sample} to be width 2");
    }
}
