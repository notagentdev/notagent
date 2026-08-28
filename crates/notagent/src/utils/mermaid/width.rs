use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthChar;

const VS16: char = '\u{fe0f}';

fn is_regional_indicator(character: char) -> bool {
    ('\u{1f1e6}'..='\u{1f1ff}').contains(&character)
}

/// Width of one code point; the table covers the whole code point space.
fn code_point_width(character: char) -> usize {
    UnicodeWidthChar::width(character).unwrap_or(1)
}

/// Columns occupied by one grapheme cluster.
/// The widest code point wins, so a base plus its combining marks measures as
/// the base. Two adjustments: a variation selector requesting emoji
/// presentation forces two columns, as does a regional indicator pair (a flag).
/// Zero is a real answer — a soft hyphen or zero-width space occupies nothing.
pub fn cluster_width(cluster: &str) -> usize {
    let mut width = 0;
    let mut vs16 = false;
    let mut regional = 0;
    for character in cluster.chars() {
        if character == VS16 {
            vs16 = true;
        }
        if is_regional_indicator(character) {
            regional += 1;
        }
        let character_width = code_point_width(character);
        if character_width > width {
            width = character_width;
        }
    }
    if vs16 || regional >= 2 { 2 } else { width }
}

/// Iterate grapheme clusters, so no loop can split one.
pub fn clusters(value: &str) -> impl Iterator<Item = &str> {
    value.graphemes(true)
}

/// Iterate clusters paired with their display width.
pub fn measured(value: &str) -> impl Iterator<Item = (&str, usize)> {
    clusters(value).map(|cluster| (cluster, cluster_width(cluster)))
}

/// Display columns of a string.
pub fn string_width(value: &str) -> usize {
    clusters(value).map(cluster_width).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measures_clusters_not_code_points() {
        assert_eq!(string_width("abc"), 3);
        // CJK is two columns, a combining mark none of its own.
        assert_eq!(string_width("日本"), 4);
        assert_eq!(string_width("e\u{0301}"), 1);
        // A ZWJ family is one cluster of two columns, not four emoji.
        assert_eq!(string_width("👨‍👩‍👧"), 2);
        // A flag is a regional indicator pair.
        assert_eq!(string_width("🇩🇪"), 2);
        // Emoji presentation selector forces two columns.
        assert_eq!(string_width("\u{2764}\u{fe0f}"), 2);
        assert_eq!(string_width("\u{00ad}"), 0);
    }
}
