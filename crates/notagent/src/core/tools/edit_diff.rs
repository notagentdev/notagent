//! Port of `packages/coding-agent/src/core/tools/edit-diff.ts` (text matching and
//! replacement half).
//!
//! Deviation (class 1): JS string indices count UTF-16 units, Rust indices count
//! bytes. Every offset here is produced and consumed inside this module, so the
//! results are identical; only the numbers differ.

use unicode_normalization::UnicodeNormalization;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    Crlf,
}

impl LineEnding {
    pub fn as_str(self) -> &'static str {
        match self {
            LineEnding::Lf => "\n",
            LineEnding::Crlf => "\r\n",
        }
    }
}

pub fn detect_line_ending(content: &str) -> LineEnding {
    let crlf_index = content.find("\r\n");
    let Some(lf_index) = content.find('\n') else {
        return LineEnding::Lf;
    };
    match crlf_index {
        Some(crlf_index) if crlf_index < lf_index => LineEnding::Crlf,
        _ => LineEnding::Lf,
    }
}

pub fn normalize_to_lf(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

pub fn restore_line_endings(text: &str, ending: LineEnding) -> String {
    match ending {
        LineEnding::Crlf => text.replace('\n', "\r\n"),
        LineEnding::Lf => text.to_owned(),
    }
}

/// Normalize text for fuzzy matching: NFKC, strip trailing whitespace per line,
/// then fold smart quotes, dashes and special spaces to their ASCII forms.
pub fn normalize_for_fuzzy_match(text: &str) -> String {
    let normalized: String = text.nfkc().collect();
    let trimmed = normalized
        .split('\n')
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n");
    trimmed
        .chars()
        .map(|character| match character {
            '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' => '\'',
            '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' => '"',
            '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2015}'
            | '\u{2212}' => '-',
            '\u{00A0}' | '\u{202F}' | '\u{205F}' | '\u{3000}' => ' ',
            character if ('\u{2002}'..='\u{200A}').contains(&character) => ' ',
            character => character,
        })
        .collect()
}

/// Split into lines that keep their trailing newline.
fn split_lines_with_endings(content: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = Vec::new();
    let mut start = 0;
    for (index, byte) in content.bytes().enumerate() {
        if byte == b'\n' {
            lines.push(&content[start..=index]);
            start = index + 1;
        }
    }
    if start < content.len() {
        lines.push(&content[start..]);
    }
    lines
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LineSpan {
    start: usize,
    end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextReplacement {
    pub match_index: usize,
    pub match_length: usize,
    pub new_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchedEdit {
    pub edit_index: usize,
    pub replacement: TextReplacement,
}

fn get_line_spans(content: &str) -> Vec<LineSpan> {
    let mut offset = 0;
    split_lines_with_endings(content)
        .into_iter()
        .map(|line| {
            let span = LineSpan {
                start: offset,
                end: offset + line.len(),
            };
            offset = span.end;
            span
        })
        .collect()
}

const OUTSIDE_BASE_CONTENT: &str = "Replacement range is outside the base content.";

fn get_replacement_line_range(
    lines: &[LineSpan],
    replacement: &TextReplacement,
) -> Result<(usize, usize), String> {
    let replacement_start = replacement.match_index;
    let replacement_end = replacement.match_index + replacement.match_length;

    let Some(start_line) = lines
        .iter()
        .position(|line| replacement_start >= line.start && replacement_start < line.end)
    else {
        return Err(OUTSIDE_BASE_CONTENT.to_owned());
    };

    let mut end_line = start_line;
    while end_line < lines.len() && lines[end_line].end < replacement_end {
        end_line += 1;
    }
    if end_line >= lines.len() {
        return Err(OUTSIDE_BASE_CONTENT.to_owned());
    }
    Ok((start_line, end_line + 1))
}

fn apply_replacements(content: &str, replacements: &[&TextReplacement], offset: usize) -> String {
    let mut result = content.to_owned();
    for replacement in replacements.iter().rev() {
        let match_index = replacement.match_index - offset;
        result = format!(
            "{}{}{}",
            &result[..match_index],
            replacement.new_text,
            &result[match_index + replacement.match_length..]
        );
    }
    result
}

/// Apply replacements matched against `base_content` to `original_content` while
/// preserving the unchanged line blocks of the original.
pub fn apply_replacements_preserving_unchanged_lines(
    original_content: &str,
    base_content: &str,
    replacements: &[&TextReplacement],
) -> Result<String, String> {
    let original_lines = split_lines_with_endings(original_content);
    let base_lines = get_line_spans(base_content);
    if original_lines.len() != base_lines.len() {
        return Err(
            "Cannot preserve unchanged lines because the base content has a different line count."
                .to_owned(),
        );
    }

    struct Group<'a> {
        start_line: usize,
        end_line: usize,
        replacements: Vec<&'a TextReplacement>,
    }

    let mut sorted: Vec<&TextReplacement> = replacements.to_vec();
    sorted.sort_by_key(|replacement| replacement.match_index);

    let mut groups: Vec<Group<'_>> = Vec::new();
    for replacement in sorted {
        let (start_line, end_line) = get_replacement_line_range(&base_lines, replacement)?;
        match groups.last_mut() {
            Some(current) if start_line < current.end_line => {
                current.end_line = current.end_line.max(end_line);
                current.replacements.push(replacement);
            }
            _ => groups.push(Group {
                start_line,
                end_line,
                replacements: vec![replacement],
            }),
        }
    }

    let mut original_line_index = 0;
    let mut result = String::new();
    for group in groups {
        result.push_str(&original_lines[original_line_index..group.start_line].concat());
        let group_start_offset = base_lines[group.start_line].start;
        let group_end_offset = base_lines[group.end_line - 1].end;
        result.push_str(&apply_replacements(
            &base_content[group_start_offset..group_end_offset],
            &group.replacements,
            group_start_offset,
        ));
        original_line_index = group.end_line;
    }
    result.push_str(&original_lines[original_line_index..].concat());
    Ok(result)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuzzyMatchResult {
    pub found: bool,
    pub index: usize,
    pub match_length: usize,
    /// False for an exact match.
    pub used_fuzzy_match: bool,
    /// The content the replacement offsets refer to.
    pub content_for_replacement: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub old_text: String,
    pub new_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedEditsResult {
    pub base_content: String,
    pub new_content: String,
}

/// Find `old_text` in `content`, exact first, then fuzzy.
pub fn fuzzy_find_text(content: &str, old_text: &str) -> FuzzyMatchResult {
    if let Some(exact_index) = content.find(old_text) {
        return FuzzyMatchResult {
            found: true,
            index: exact_index,
            match_length: old_text.len(),
            used_fuzzy_match: false,
            content_for_replacement: content.to_owned(),
        };
    }

    let fuzzy_content = normalize_for_fuzzy_match(content);
    let fuzzy_old_text = normalize_for_fuzzy_match(old_text);
    match fuzzy_content.find(&fuzzy_old_text) {
        // Offsets are in normalized space; callers replace against that content.
        Some(fuzzy_index) => FuzzyMatchResult {
            found: true,
            index: fuzzy_index,
            match_length: fuzzy_old_text.len(),
            used_fuzzy_match: true,
            content_for_replacement: fuzzy_content,
        },
        None => FuzzyMatchResult {
            found: false,
            index: 0,
            match_length: 0,
            used_fuzzy_match: false,
            content_for_replacement: content.to_owned(),
        },
    }
}

/// Strip a UTF-8 BOM, returning the BOM (if any) and the remaining text.
pub fn strip_bom(content: &str) -> (&str, &str) {
    match content.strip_prefix('\u{FEFF}') {
        Some(text) => ("\u{FEFF}", text),
        None => ("", content),
    }
}

fn count_occurrences(content: &str, old_text: &str) -> usize {
    let fuzzy_content = normalize_for_fuzzy_match(content);
    let fuzzy_old_text = normalize_for_fuzzy_match(old_text);
    if fuzzy_old_text.is_empty() {
        // `split` on an empty needle yields one part per character in JS as well.
        return fuzzy_content.chars().count();
    }
    fuzzy_content.matches(&fuzzy_old_text).count()
}

fn not_found_error(path: &str, edit_index: usize, total_edits: usize) -> String {
    if total_edits == 1 {
        format!(
            "Could not find the exact text in {path}. The old text must match exactly including all whitespace and newlines."
        )
    } else {
        format!(
            "Could not find edits[{edit_index}] in {path}. The oldText must match exactly including all whitespace and newlines."
        )
    }
}

fn duplicate_error(
    path: &str,
    edit_index: usize,
    total_edits: usize,
    occurrences: usize,
) -> String {
    if total_edits == 1 {
        format!(
            "Found {occurrences} occurrences of the text in {path}. The text must be unique. Please provide more context to make it unique."
        )
    } else {
        format!(
            "Found {occurrences} occurrences of edits[{edit_index}] in {path}. Each oldText must be unique. Please provide more context to make it unique."
        )
    }
}

fn empty_old_text_error(path: &str, edit_index: usize, total_edits: usize) -> String {
    if total_edits == 1 {
        format!("oldText must not be empty in {path}.")
    } else {
        format!("edits[{edit_index}].oldText must not be empty in {path}.")
    }
}

fn no_change_error(path: &str, total_edits: usize) -> String {
    if total_edits == 1 {
        format!(
            "No changes made to {path}. The replacement produced identical content. This might indicate an issue with special characters or the text not existing as expected."
        )
    } else {
        format!("No changes made to {path}. The replacements produced identical content.")
    }
}

/// Apply one or more exact-text replacements to LF-normalized content.
///
/// All edits match against the same original content; replacements are applied
/// back to front so offsets stay stable. If any edit needs fuzzy matching, the
/// whole operation runs in fuzzy-normalized space and the touched lines are
/// overlaid onto the original so unchanged lines keep their bytes.
pub fn apply_edits_to_normalized_content(
    normalized_content: &str,
    edits: &[Edit],
    path: &str,
) -> Result<AppliedEditsResult, String> {
    let normalized_edits: Vec<Edit> = edits
        .iter()
        .map(|edit| Edit {
            old_text: normalize_to_lf(&edit.old_text),
            new_text: normalize_to_lf(&edit.new_text),
        })
        .collect();

    for (index, edit) in normalized_edits.iter().enumerate() {
        if edit.old_text.is_empty() {
            return Err(empty_old_text_error(path, index, normalized_edits.len()));
        }
    }

    let used_fuzzy_match = normalized_edits
        .iter()
        .any(|edit| fuzzy_find_text(normalized_content, &edit.old_text).used_fuzzy_match);
    let replacement_base_content = if used_fuzzy_match {
        normalize_for_fuzzy_match(normalized_content)
    } else {
        normalized_content.to_owned()
    };

    let mut matched_edits: Vec<MatchedEdit> = Vec::new();
    for (index, edit) in normalized_edits.iter().enumerate() {
        let match_result = fuzzy_find_text(&replacement_base_content, &edit.old_text);
        if !match_result.found {
            return Err(not_found_error(path, index, normalized_edits.len()));
        }
        let occurrences = count_occurrences(&replacement_base_content, &edit.old_text);
        if occurrences > 1 {
            return Err(duplicate_error(
                path,
                index,
                normalized_edits.len(),
                occurrences,
            ));
        }
        matched_edits.push(MatchedEdit {
            edit_index: index,
            replacement: TextReplacement {
                match_index: match_result.index,
                match_length: match_result.match_length,
                new_text: edit.new_text.clone(),
            },
        });
    }

    matched_edits.sort_by_key(|edit| edit.replacement.match_index);
    for window in matched_edits.windows(2) {
        let (previous, current) = (&window[0], &window[1]);
        if previous.replacement.match_index + previous.replacement.match_length
            > current.replacement.match_index
        {
            return Err(format!(
                "edits[{}] and edits[{}] overlap in {path}. Merge them into one edit or target disjoint regions.",
                previous.edit_index, current.edit_index
            ));
        }
    }

    let replacements: Vec<&TextReplacement> =
        matched_edits.iter().map(|edit| &edit.replacement).collect();
    let base_content = normalized_content.to_owned();
    let new_content = if used_fuzzy_match {
        apply_replacements_preserving_unchanged_lines(
            normalized_content,
            &replacement_base_content,
            &replacements,
        )?
    } else {
        apply_replacements(&replacement_base_content, &replacements, 0)
    };

    if base_content == new_content {
        return Err(no_change_error(path, normalized_edits.len()));
    }
    Ok(AppliedEditsResult {
        base_content,
        new_content,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(old_text: &str, new_text: &str) -> Edit {
        Edit {
            old_text: old_text.to_owned(),
            new_text: new_text.to_owned(),
        }
    }

    #[test]
    fn detects_and_restores_line_endings() {
        assert_eq!(detect_line_ending("a\r\nb"), LineEnding::Crlf);
        assert_eq!(detect_line_ending("a\nb\r\nc"), LineEnding::Lf);
        assert_eq!(detect_line_ending("no newline"), LineEnding::Lf);
        assert_eq!(normalize_to_lf("a\r\nb\rc"), "a\nb\nc");
        assert_eq!(restore_line_endings("a\nb", LineEnding::Crlf), "a\r\nb");
        assert_eq!(restore_line_endings("a\nb", LineEnding::Lf), "a\nb");
    }

    #[test]
    fn strips_the_byte_order_mark() {
        assert_eq!(strip_bom("\u{FEFF}text"), ("\u{FEFF}", "text"));
        assert_eq!(strip_bom("text"), ("", "text"));
    }

    #[test]
    fn normalizes_text_for_fuzzy_matching() {
        assert_eq!(
            normalize_for_fuzzy_match("trailing   \nspace\t"),
            "trailing\nspace"
        );
        assert_eq!(
            normalize_for_fuzzy_match("\u{2018}a\u{2019} \u{201C}b\u{201D}"),
            "'a' \"b\""
        );
        assert_eq!(
            normalize_for_fuzzy_match("a\u{2013}b\u{2014}c\u{2212}d"),
            "a-b-c-d"
        );
        assert_eq!(normalize_for_fuzzy_match("a\u{00A0}b\u{2009}c"), "a b c");
        // NFKC folds compatibility characters.
        assert_eq!(normalize_for_fuzzy_match("ﬁle"), "file");
    }

    #[test]
    fn finds_text_exactly_before_falling_back_to_fuzzy() {
        let result = fuzzy_find_text("hello world", "world");
        assert!(result.found && !result.used_fuzzy_match);
        assert_eq!(result.index, 6);
        assert_eq!(result.match_length, 5);

        let result = fuzzy_find_text("a \u{2018}quoted\u{2019} word", "'quoted'");
        assert!(result.found && result.used_fuzzy_match);
        assert_eq!(
            &result.content_for_replacement[result.index..result.index + result.match_length],
            "'quoted'"
        );

        let result = fuzzy_find_text("hello", "missing");
        assert!(!result.found);
    }

    #[test]
    fn applies_a_single_edit() {
        let result = apply_edits_to_normalized_content(
            "line one\nline two\nline three\n",
            &[edit("line two", "line 2")],
            "file.txt",
        )
        .expect("edit");
        assert_eq!(result.new_content, "line one\nline 2\nline three\n");
        assert_eq!(result.base_content, "line one\nline two\nline three\n");
    }

    #[test]
    fn applies_several_disjoint_edits_back_to_front() {
        let result = apply_edits_to_normalized_content(
            "a\nb\nc\nd\n",
            &[edit("a", "A"), edit("d", "D")],
            "file.txt",
        )
        .expect("edits");
        assert_eq!(result.new_content, "A\nb\nc\nD\n");
    }

    #[test]
    fn rejects_empty_missing_duplicate_and_overlapping_edits() {
        let error = apply_edits_to_normalized_content("a\n", &[edit("", "x")], "file.txt")
            .expect_err("empty");
        assert_eq!(error, "oldText must not be empty in file.txt.");

        let error =
            apply_edits_to_normalized_content("a\n", &[edit("", "x"), edit("a", "b")], "file.txt")
                .expect_err("empty");
        assert_eq!(error, "edits[0].oldText must not be empty in file.txt.");

        let error = apply_edits_to_normalized_content("a\n", &[edit("zzz", "x")], "file.txt")
            .expect_err("missing");
        assert_eq!(
            error,
            "Could not find the exact text in file.txt. The old text must match exactly including all whitespace and newlines."
        );

        let error =
            apply_edits_to_normalized_content("dup\ndup\n", &[edit("dup", "x")], "file.txt")
                .expect_err("duplicate");
        assert_eq!(
            error,
            "Found 2 occurrences of the text in file.txt. The text must be unique. Please provide more context to make it unique."
        );

        let error = apply_edits_to_normalized_content(
            "abcdef\n",
            &[edit("abcd", "x"), edit("cdef", "y")],
            "file.txt",
        )
        .expect_err("overlap");
        assert_eq!(
            error,
            "edits[0] and edits[1] overlap in file.txt. Merge them into one edit or target disjoint regions."
        );

        let error =
            apply_edits_to_normalized_content("same\n", &[edit("same", "same")], "file.txt")
                .expect_err("no change");
        assert!(error.starts_with("No changes made to file.txt."));
    }

    #[test]
    fn reports_multi_edit_errors_with_their_index() {
        let error = apply_edits_to_normalized_content(
            "a\ndup\ndup\n",
            &[edit("a", "A"), edit("dup", "x")],
            "file.txt",
        )
        .expect_err("duplicate");
        assert_eq!(
            error,
            "Found 2 occurrences of edits[1] in file.txt. Each oldText must be unique. Please provide more context to make it unique."
        );

        let error = apply_edits_to_normalized_content(
            "a\n",
            &[edit("a", "A"), edit("zzz", "x")],
            "file.txt",
        )
        .expect_err("missing");
        assert_eq!(
            error,
            "Could not find edits[1] in file.txt. The oldText must match exactly including all whitespace and newlines."
        );
    }

    #[test]
    fn a_fuzzy_edit_keeps_the_bytes_of_untouched_lines() {
        // Line 1 keeps its trailing spaces and smart quote; only line 2 is rewritten.
        let content =
            "keep \u{2018}this\u{2019}   \nchange \u{2018}that\u{2019}\nkeep\u{00A0}too\n";
        let result = apply_edits_to_normalized_content(
            content,
            &[edit("change 'that'", "changed")],
            "file.txt",
        )
        .expect("edit");
        assert_eq!(
            result.new_content,
            "keep \u{2018}this\u{2019}   \nchanged\nkeep\u{00A0}too\n"
        );
    }

    #[test]
    fn a_fuzzy_edit_spanning_lines_rewrites_only_those_lines() {
        let content = "a\u{00A0}1\nb\u{2013}2\nc\u{2019}3\nd 4\n";
        let result =
            apply_edits_to_normalized_content(content, &[edit("b-2\nc'3", "merged")], "file.txt")
                .expect("edit");
        assert_eq!(result.new_content, "a\u{00A0}1\nmerged\nd 4\n");
    }

    #[test]
    fn preserving_replacements_requires_the_same_line_count() {
        let replacement = TextReplacement {
            match_index: 0,
            match_length: 1,
            new_text: "X".to_owned(),
        };
        let error = apply_replacements_preserving_unchanged_lines("a\nb\n", "a\n", &[&replacement])
            .expect_err("line count");
        assert!(error.contains("different line count"), "{error}");
    }

    #[test]
    fn splits_lines_keeping_their_endings() {
        assert_eq!(split_lines_with_endings("a\nb\n"), vec!["a\n", "b\n"]);
        assert_eq!(split_lines_with_endings("a\nb"), vec!["a\n", "b"]);
        assert_eq!(split_lines_with_endings(""), Vec::<&str>::new());
        assert_eq!(split_lines_with_endings("\n"), vec!["\n"]);
    }
}
