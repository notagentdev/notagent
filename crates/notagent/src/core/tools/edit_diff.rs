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
            "Could not find edits[{edit_index}] in {path}. The old_string must match exactly including all whitespace and newlines."
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
            "Found {occurrences} occurrences of edits[{edit_index}] in {path}. Each old_string must be unique. Please provide more context to make it unique."
        )
    }
}

fn empty_old_text_error(path: &str, edit_index: usize, total_edits: usize) -> String {
    if total_edits == 1 {
        format!("old_string must not be empty in {path}.")
    } else {
        format!("edits[{edit_index}].old_string must not be empty in {path}.")
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

// =============================================================================
// Diff rendering
// =============================================================================

/// One run of consecutive lines with the same diff tag, mirroring the parts that
/// `Diff.diffLines` returns.
struct DiffPart {
    lines: Vec<String>,
    added: bool,
    removed: bool,
}

/// Tech substitution (class 3): the `diff` npm package becomes the `similar`
/// crate; both compute a line-level LCS diff.
fn diff_parts(old_content: &str, new_content: &str) -> Vec<DiffPart> {
    use similar::{ChangeTag, TextDiff};

    let diff = TextDiff::from_lines(old_content, new_content);
    let mut parts: Vec<DiffPart> = Vec::new();
    for change in diff.iter_all_changes() {
        let (added, removed) = match change.tag() {
            ChangeTag::Insert => (true, false),
            ChangeTag::Delete => (false, true),
            ChangeTag::Equal => (false, false),
        };
        let line = change
            .value()
            .strip_suffix('\n')
            .unwrap_or(change.value())
            .to_owned();
        match parts.last_mut() {
            Some(part) if part.added == added && part.removed == removed => part.lines.push(line),
            _ => parts.push(DiffPart {
                lines: vec![line],
                added,
                removed,
            }),
        }
    }
    parts
}

/// Generate a standard unified patch.
pub fn generate_unified_patch(
    path: &str,
    old_content: &str,
    new_content: &str,
    context_lines: usize,
) -> String {
    use similar::TextDiff;

    let diff = TextDiff::from_lines(old_content, new_content);
    let mut patch = diff
        .unified_diff()
        .context_radius(context_lines)
        .header(path, path)
        .to_string();
    if !patch.is_empty() && !patch.ends_with('\n') {
        patch.push('\n');
    }
    patch
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffString {
    pub diff: String,
    /// The first changed line, counted in the new file.
    pub first_changed_line: Option<usize>,
}

/// Generate a display-oriented diff with line numbers and bounded context.
pub fn generate_diff_string(
    old_content: &str,
    new_content: &str,
    context_lines: usize,
) -> DiffString {
    let parts = diff_parts(old_content, new_content);
    let mut output: Vec<String> = Vec::new();

    let old_lines = old_content.split('\n').count();
    let new_lines = new_content.split('\n').count();
    let line_number_width = old_lines.max(new_lines).to_string().len();

    let mut old_line_number = 1usize;
    let mut new_line_number = 1usize;
    let mut last_was_change = false;
    let mut first_changed_line: Option<usize> = None;

    for index in 0..parts.len() {
        let part = &parts[index];
        if part.added || part.removed {
            if first_changed_line.is_none() {
                first_changed_line = Some(new_line_number);
            }
            for line in &part.lines {
                if part.added {
                    output.push(format!("+{new_line_number:>line_number_width$} {line}"));
                    new_line_number += 1;
                } else {
                    output.push(format!("-{old_line_number:>line_number_width$} {line}"));
                    old_line_number += 1;
                }
            }
            last_was_change = true;
            continue;
        }

        // Context lines are only shown around changes.
        let next_part_is_change = parts
            .get(index + 1)
            .is_some_and(|next| next.added || next.removed);
        let has_leading_change = last_was_change;
        let has_trailing_change = next_part_is_change;
        let raw = &part.lines;

        let push_context =
            |line: &str, old: &mut usize, new: &mut usize, output: &mut Vec<String>| {
                output.push(format!(" {:>line_number_width$} {line}", *old));
                *old += 1;
                *new += 1;
            };

        if has_leading_change && has_trailing_change {
            if raw.len() <= context_lines * 2 {
                for line in raw {
                    push_context(
                        line,
                        &mut old_line_number,
                        &mut new_line_number,
                        &mut output,
                    );
                }
            } else {
                let skipped = raw.len() - context_lines * 2;
                for line in &raw[..context_lines] {
                    push_context(
                        line,
                        &mut old_line_number,
                        &mut new_line_number,
                        &mut output,
                    );
                }
                output.push(format!(" {:>line_number_width$} ...", ""));
                old_line_number += skipped;
                new_line_number += skipped;
                for line in &raw[raw.len() - context_lines..] {
                    push_context(
                        line,
                        &mut old_line_number,
                        &mut new_line_number,
                        &mut output,
                    );
                }
            }
        } else if has_leading_change {
            let shown = raw.len().min(context_lines);
            let skipped = raw.len() - shown;
            for line in &raw[..shown] {
                push_context(
                    line,
                    &mut old_line_number,
                    &mut new_line_number,
                    &mut output,
                );
            }
            if skipped > 0 {
                output.push(format!(" {:>line_number_width$} ...", ""));
                old_line_number += skipped;
                new_line_number += skipped;
            }
        } else if has_trailing_change {
            let skipped = raw.len().saturating_sub(context_lines);
            if skipped > 0 {
                output.push(format!(" {:>line_number_width$} ...", ""));
                old_line_number += skipped;
                new_line_number += skipped;
            }
            for line in &raw[skipped..] {
                push_context(
                    line,
                    &mut old_line_number,
                    &mut new_line_number,
                    &mut output,
                );
            }
        } else {
            // Context far away from any change is skipped entirely.
            old_line_number += raw.len();
            new_line_number += raw.len();
        }
        last_was_change = false;
    }

    DiffString {
        diff: output.join("\n"),
        first_changed_line,
    }
}

/// The diff of one or more edits, without applying them.
///
/// The TUI shows it as a preview while the model is still streaming the call,
/// which is why it reads the file itself instead of going through the tool's
/// operations: `edit-diff.ts` imports `fs/promises` directly for the same
/// reason. A failure is not an error of the render path — it is the preview,
/// and it is shown in place of the diff (`EditDiffError` in TypeScript).
///
/// Deviation (class 1): the wording of a filesystem failure is Rust's rather
/// than Node's; the `Could not edit file: …` sentence around it is verbatim.
pub async fn compute_edits_diff(
    path: &str,
    edits: &[Edit],
    cwd: &str,
) -> Result<DiffString, String> {
    let absolute_path = crate::core::tools::path_utils::resolve_to_cwd(path, cwd);

    if let Err(error) = tokio::fs::metadata(&absolute_path).await {
        return Err(format!("Could not edit file: {path}. {error}."));
    }

    let raw_content = tokio::fs::read(&absolute_path)
        .await
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .map_err(|error| error.to_string())?;

    // The model never includes an invisible BOM in old_string.
    let (_bom, content) = strip_bom(&raw_content);
    let normalized_content = normalize_to_lf(content);
    let applied = apply_edits_to_normalized_content(&normalized_content, edits, path)?;
    Ok(generate_diff_string(
        &applied.base_content,
        &applied.new_content,
        4,
    ))
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
        assert_eq!(error, "old_string must not be empty in file.txt.");

        let error =
            apply_edits_to_normalized_content("a\n", &[edit("", "x"), edit("a", "b")], "file.txt")
                .expect_err("empty");
        assert_eq!(error, "edits[0].old_string must not be empty in file.txt.");

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
            "Found 2 occurrences of edits[1] in file.txt. Each old_string must be unique. Please provide more context to make it unique."
        );

        let error = apply_edits_to_normalized_content(
            "a\n",
            &[edit("a", "A"), edit("zzz", "x")],
            "file.txt",
        )
        .expect_err("missing");
        assert_eq!(
            error,
            "Could not find edits[1] in file.txt. The old_string must match exactly including all whitespace and newlines."
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
    fn renders_a_display_diff_with_line_numbers() {
        let old_content = "one\ntwo\nthree\n";
        let new_content = "one\nTWO\nthree\n";
        let result = generate_diff_string(old_content, new_content, 4);
        assert_eq!(result.first_changed_line, Some(2));
        assert_eq!(result.diff, " 1 one\n-2 two\n+2 TWO\n 3 three");
    }

    #[test]
    fn the_display_diff_elides_far_away_context() {
        let old_content: String = (1..=30).map(|index| format!("line {index}\n")).collect();
        let new_content = old_content.replace("line 15\n", "changed 15\n");
        let result = generate_diff_string(&old_content, &new_content, 2);
        let lines: Vec<&str> = result.diff.lines().collect();
        assert_eq!(result.first_changed_line, Some(15));
        // Leading context is elided, then two lines of context on each side.
        assert_eq!(lines[0], "    ...");
        assert_eq!(lines[1], " 13 line 13");
        assert_eq!(lines[2], " 14 line 14");
        assert_eq!(lines[3], "-15 line 15");
        assert_eq!(lines[4], "+15 changed 15");
        assert_eq!(lines[5], " 16 line 16");
        assert_eq!(lines[6], " 17 line 17");
        assert_eq!(lines[7], "    ...");
        assert_eq!(lines.len(), 8);
    }

    #[test]
    fn the_display_diff_keeps_short_context_between_two_changes() {
        let old_content = "a\nb\nc\nd\ne\n";
        let new_content = "A\nb\nc\nd\nE\n";
        let result = generate_diff_string(old_content, new_content, 4);
        let lines: Vec<&str> = result.diff.lines().collect();
        assert_eq!(lines[0], "-1 a");
        assert_eq!(lines[1], "+1 A");
        assert_eq!(lines[2], " 2 b");
        assert_eq!(lines[3], " 3 c");
        assert_eq!(lines[4], " 4 d");
        assert_eq!(lines[5], "-5 e");
        assert_eq!(lines[6], "+5 E");
        assert_eq!(lines.len(), 7);
    }

    #[test]
    fn generates_a_unified_patch_that_applies_cleanly() {
        let old_content = "Hello, world!";
        let new_content = "Hello, testing!";
        let patch = generate_unified_patch("file.txt", old_content, new_content, 4);
        assert!(patch.contains("--- file.txt"), "{patch}");
        assert!(patch.contains("+++ file.txt"), "{patch}");
        assert!(patch.contains("@@"), "{patch}");
        assert!(patch.contains("-Hello, world!"), "{patch}");
        assert!(patch.contains("+Hello, testing!"), "{patch}");
        assert_eq!(
            apply_patch(old_content, &patch),
            Some(new_content.to_owned())
        );
    }

    #[test]
    fn a_multi_hunk_patch_applies_cleanly() {
        let old_content: String = (1..=40).map(|index| format!("line {index}\n")).collect();
        let new_content = old_content
            .replace("line 5\n", "five\n")
            .replace("line 35\n", "thirty-five\n");
        let patch = generate_unified_patch("file.txt", &old_content, &new_content, 4);
        assert_eq!(patch.matches("@@").count(), 4, "two hunks");
        assert_eq!(apply_patch(&old_content, &patch), Some(new_content));
    }

    /// Minimal unified-patch applier, matching what the TS suite checks with
    /// `applyPatch` from the `diff` package.
    fn apply_patch(original: &str, patch: &str) -> Option<String> {
        let original_lines: Vec<&str> = original.split('\n').collect();
        let mut result: Vec<String> = Vec::new();
        let mut cursor = 0usize;
        let mut lines = patch.lines().peekable();
        while let Some(line) = lines.next() {
            if !line.starts_with("@@") {
                continue;
            }
            let old_start: usize = line
                .split(['-', ',', ' '])
                .find(|part| {
                    part.chars().all(|character| character.is_ascii_digit()) && !part.is_empty()
                })?
                .parse()
                .ok()?;
            let hunk_start = old_start.saturating_sub(1);
            while cursor < hunk_start {
                result.push((*original_lines.get(cursor)?).to_owned());
                cursor += 1;
            }
            while let Some(hunk_line) = lines.peek() {
                if hunk_line.starts_with("@@") {
                    break;
                }
                let hunk_line = lines.next().expect("peeked");
                match hunk_line.chars().next() {
                    Some(' ') => {
                        result.push((*original_lines.get(cursor)?).to_owned());
                        cursor += 1;
                    }
                    Some('-') => cursor += 1,
                    Some('+') => result.push(hunk_line[1..].to_owned()),
                    Some('\\') => {}
                    _ => {}
                }
            }
        }
        while cursor < original_lines.len() {
            result.push((*original_lines.get(cursor)?).to_owned());
            cursor += 1;
        }
        Some(result.join("\n"))
    }

    #[test]
    fn splits_lines_keeping_their_endings() {
        assert_eq!(split_lines_with_endings("a\nb\n"), vec!["a\n", "b\n"]);
        assert_eq!(split_lines_with_endings("a\nb"), vec!["a\n", "b"]);
        assert_eq!(split_lines_with_endings(""), Vec::<&str>::new());
        assert_eq!(split_lines_with_endings("\n"), vec!["\n"]);
    }
}
