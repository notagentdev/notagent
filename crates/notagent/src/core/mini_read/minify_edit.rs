//! Applies edits expressed in minified space onto the original source.
//! Adapted from the Rust reference implementation
//! `notagent-main-rust/crates/notagent_services/src/tool_services/minify_edit.rs`
//! (see [`super::minify`] for why the reference is the source here).
//! Counterpart of the `read` tool: the agent matches and replaces
//! text in the minified rendering, and this module maps the match back onto
//! the original file via the byte-level source map, re-indents the
//! replacement using the file's own indentation widths, and splices it in.
//! Untouched bytes — including comments and formatting — stay byte-identical;
//! no external formatter is involved.

use std::ops::Range;
use std::path::Path;

use tree_sitter::Parser;

use super::minify::{self, MinifyResult};

/// Result of a successful minified edit.
#[derive(Debug)]
pub struct MinifiedEdit {
    /// The full new file content.
    pub content: String,
    /// Non-fatal observations the agent should see (removed comments, new
    /// syntax errors).
    pub warnings: Vec<String>,
}

/// Applies a single replace in minified space. Returns `None` when the
/// language is unsupported by the minifier; callers should then treat the
/// edit as a plain patch on the raw source (mirroring `read`'s raw
/// fallback).
pub fn apply_minified_edit(
    path: &Path,
    source: &str,
    old_string: &str,
    new_string: &str,
    replace_all: bool,
    keep_comments: bool,
) -> Option<Result<MinifiedEdit, String>> {
    let minified = minify::minify_with_map(path, source, keep_comments)?;
    let result = splice(
        path,
        source,
        &minified,
        old_string,
        new_string,
        replace_all,
        keep_comments,
    );

    // A comment-only search can never match the default (comment-stripped)
    // view. Rather than fail confusingly, retry the whole edit with comments
    // kept, so editing a comment works regardless of the keep_comments flag.
    if result.is_err() && !keep_comments && minify::fragment_has_only_comments(path, old_string) {
        let minified = minify::minify_with_map(path, source, true)?;
        let mut retried = splice(
            path,
            source,
            &minified,
            old_string,
            new_string,
            replace_all,
            true,
        );
        if let Ok(edit) = &mut retried {
            edit.warnings.insert(
                0,
                "Search text is only comments, which are hidden with keep_comments=false; matched with keep_comments enabled."
                    .to_string(),
            );
        }
        return Some(retried);
    }
    Some(result)
}

fn splice(
    path: &Path,
    source: &str,
    minified: &MinifyResult,
    old_string: &str,
    new_string: &str,
    replace_all: bool,
    keep_comments: bool,
) -> Result<MinifiedEdit, String> {
    if old_string.is_empty() {
        return Err("search text must not be empty".to_owned());
    }

    let normalized_old = minify::normalize_fragment_for_path(path, old_string, keep_comments);
    let search = select_search_text(&minified.text, old_string, normalized_old.as_deref());
    let matches = match search.matches {
        matches if !matches.is_empty() => matches,
        _ => {
            if let Some(normalized) = normalized_old.as_deref()
                && normalized != old_string
            {
                return Err(format!(
                    "Could not find match for search text in the minified view of the file. Tried the original search text and its normalized minified form: '{normalized}'. Re-read the file with read (using the same keep_comments setting) or provide a narrower search string."
                ));
            }
            return Err(format!(
                "Could not find match for search text: '{old_string}' in the minified view of the file. Re-read the file with read (using the same keep_comments setting) and try again."
            ));
        }
    };
    if matches.len() > 1 && !replace_all {
        return Err(format!(
            "Multiple matches found for search text: '{}'. Either provide a more specific search pattern or use replace_all to replace all occurrences.",
            search.text
        ));
    }

    let normalized_new = minify::normalize_fragment_for_path(path, new_string, keep_comments);
    let replacement_text = normalized_new.as_deref().unwrap_or(new_string);
    let style = minify::detect_indent_style(source);
    let language = minify::language_for_path(path);
    let target_line_ending = detect_line_ending(source);

    let mut content = source.to_string();
    let mut replaced_ranges: Vec<Range<usize>> = Vec::new();
    // Splice back-to-front so earlier source offsets stay valid.
    for matched in matches.iter().rev() {
        let at = matched.start;
        let src_range = source_range(&minified.src_map, matched.start, matched.end, source.len());
        // The first replacement line is only re-indented when the match
        // itself starts at a line boundary, i.e. the matched text covers the
        // line's indentation. Mid-line matches keep whatever precedes them.
        let at_line_start = at == 0 || minified.text.as_bytes()[at - 1] == b'\n';
        let replacement = if at_line_start {
            // The match covers the matched line's indentation, so the
            // replacement must reproduce it. Anchor every re-indentable line at
            // the matched line's depth (the fix for the column-0 corruption);
            // baseline subtraction keeps it correct whether the search text was
            // copied with or without leading indentation.
            let aligned =
                redepth_replacement(replacement_text, &minified.text, at, language.as_ref());
            minify::expand_indentation(&aligned, &minified.indent_widths, style, language.as_ref())
        } else {
            // Mid-line match: the file's own prefix before the match supplies
            // the first line's position, so the replacement's own depths are
            // used as-is — only unindented continuation lines of a normalized
            // replacement inherit the matched depth.
            let aligned = if (search.used_normalized
                || normalized_new
                    .as_deref()
                    .is_some_and(|normalized| normalized != new_string))
                && !minified.text.contains(replacement_text)
            {
                inherit_depth_for_unindented_following_lines(
                    replacement_text,
                    &minified.text,
                    at,
                    search.used_normalized,
                )
            } else {
                replacement_text.to_string()
            };
            let expanded = minify::expand_indentation(
                &aligned,
                &minified.indent_widths,
                style,
                language.as_ref(),
            );
            keep_first_line_verbatim(&aligned, &expanded)
        };
        // Minifying the replacement drops blank lines; restore them so
        // deliberate spacing survives a multi-line edit. The original blank
        // structure is preferred (the model edits from a blank-less view and
        // rarely re-types blanks), falling back to blanks the caller put in
        // `new_string`.
        let replacement =
            reinsert_blank_lines(&source[src_range.clone()], new_string, &replacement)
                .unwrap_or(replacement);
        let replacement = normalize_line_endings(&replacement, target_line_ending);
        content.replace_range(src_range.clone(), &replacement);
        replaced_ranges.push(src_range);
    }

    let mut warnings = Vec::new();
    if search.used_normalized {
        warnings.push(
            "Used normalized minified search text after the original search text did not match."
                .to_string(),
        );
    }
    if normalized_new
        .as_deref()
        .is_some_and(|normalized| normalized != new_string)
    {
        warnings.push(
            "Normalized replacement text into minified form before applying indentation expansion."
                .to_string(),
        );
    }
    let removed_comments = minified
        .comments
        .iter()
        .filter(|c| {
            replaced_ranges
                .iter()
                .any(|r| c.start >= r.start && c.end <= r.end)
        })
        .count();
    if removed_comments > 0 {
        warnings.push(format!(
            "{removed_comments} comment(s) located inside the replaced range were removed (they were not visible in the minified view). Re-add them if they are still relevant."
        ));
    }
    if let Some(language) = language
        && let Some(warning) = syntax_regression(source, &content, &language)
    {
        warnings.push(warning);
    }

    Ok(MinifiedEdit { content, warnings })
}

struct SearchText<'a> {
    text: &'a str,
    matches: Vec<Range<usize>>,
    used_normalized: bool,
}

fn select_search_text<'a>(
    minified_text: &str,
    original: &'a str,
    normalized: Option<&'a str>,
) -> SearchText<'a> {
    let original_matches = exact_matches(minified_text, original);
    if !original_matches.is_empty() {
        return SearchText {
            text: original,
            matches: original_matches,
            used_normalized: false,
        };
    }
    if let Some(normalized) = normalized
        && !normalized.is_empty()
        && normalized != original
    {
        let normalized_matches = exact_matches(minified_text, normalized);
        if !normalized_matches.is_empty() {
            return SearchText {
                text: normalized,
                matches: normalized_matches,
                used_normalized: true,
            };
        }
    }
    // Last resort: indentation-tolerant, line-aligned matching. The view
    // indents one space per nesting depth; search text the model retyped or
    // flattened (e.g. copied a comment block at column 0) still matches here.
    // Only whole-line, content-exact matches are accepted, so this never
    // produces a spurious mid-line hit.
    let needle = normalized
        .filter(|candidate| !candidate.is_empty())
        .unwrap_or(original);
    let tolerant = whitespace_insensitive_matches(minified_text, needle);
    if !tolerant.is_empty() {
        return SearchText {
            text: needle,
            matches: tolerant,
            used_normalized: true,
        };
    }
    SearchText {
        text: original,
        matches: Vec::new(),
        used_normalized: false,
    }
}

fn exact_matches(haystack: &str, needle: &str) -> Vec<Range<usize>> {
    haystack
        .match_indices(needle)
        .map(|(i, _)| i..i + needle.len())
        .collect()
}

/// Finds whole-line occurrences of `needle` in `haystack`, ignoring each
/// line's leading spaces. Both sides are compared line-by-line with leading
/// spaces stripped; the returned ranges index the real `haystack` (from the
/// first matched line's start, including its indentation, to the last matched
/// line's content end). Used only as a fallback when exact matching fails, so
/// the model's flattened/retyped search text still resolves to a precise range.
fn whitespace_insensitive_matches(haystack: &str, needle: &str) -> Vec<Range<usize>> {
    let needle_lines: Vec<&str> = needle
        .split('\n')
        .map(|line| line.trim_start_matches(' '))
        .collect();
    if needle_lines.iter().all(|line| line.is_empty()) {
        return Vec::new();
    }

    // (start including indentation, content end) for every haystack line.
    let mut line_spans: Vec<(usize, usize)> = Vec::new();
    let mut start = 0usize;
    for (i, byte) in haystack.bytes().enumerate() {
        if byte == b'\n' {
            line_spans.push((start, i));
            start = i + 1;
        }
    }
    line_spans.push((start, haystack.len()));

    let n = needle_lines.len();
    let mut matches = Vec::new();
    if n == 0 || n > line_spans.len() {
        return matches;
    }
    for i in 0..=(line_spans.len() - n) {
        let aligned = (0..n).all(|k| {
            let (s, e) = line_spans[i + k];
            haystack[s..e].trim_start_matches(' ') == needle_lines[k]
        });
        if aligned {
            matches.push(line_spans[i].0..line_spans[i + n - 1].1);
        }
    }
    matches
}

/// Mid-line match helper: when a normalized replacement's continuation lines
/// arrive unindented but the matched line is nested, prefix them with the
/// matched line's depth so they keep nesting under the (verbatim) first line.
fn inherit_depth_for_unindented_following_lines(
    replacement: &str,
    minified_text: &str,
    match_start: usize,
    allow_brace_block: bool,
) -> String {
    let line_start = minified_text[..match_start]
        .rfind('\n')
        .map_or(0, |index| index + 1);
    let match_depth = minified_text[line_start..match_start]
        .bytes()
        .take_while(|&byte| byte == b' ')
        .count();
    if match_depth == 0 || !replacement_needs_depth_inheritance(replacement, allow_brace_block) {
        return replacement.to_string();
    }
    let continuation_min_depth = replacement
        .split('\n')
        .skip(1)
        .filter(|line| !line.is_empty())
        .map(|line| line.bytes().take_while(|&byte| byte == b' ').count())
        .min()
        .unwrap_or(usize::MAX);
    if continuation_min_depth == usize::MAX || continuation_min_depth > match_depth {
        return replacement.to_string();
    }
    let indent = " ".repeat(match_depth);
    replacement
        .split('\n')
        .enumerate()
        .map(|(index, line)| {
            let line_depth = line.bytes().take_while(|&byte| byte == b' ').count();
            if index == 0
                || line.is_empty()
                || (allow_brace_block
                    && line_depth == continuation_min_depth
                    && line.trim_start().starts_with('}'))
            {
                line.to_string()
            } else {
                format!("{indent}{line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn replacement_needs_depth_inheritance(replacement: &str, allow_brace_block: bool) -> bool {
    let Some(first_line) = replacement.lines().find(|line| !line.trim().is_empty()) else {
        return false;
    };
    let trimmed = first_line.trim_end();
    (allow_brace_block && trimmed.ends_with('{'))
        || trimmed.ends_with(':')
        || trimmed.trim_start().starts_with("//")
        || trimmed.trim_start().starts_with('#')
}

/// Re-indents a minified-space replacement so its first re-indentable line
/// sits exactly at the matched line's depth and every following line keeps its
/// own nesting relative to that first line. Anchoring on the matched line's
/// depth (ground truth from the view) is what stops a line-start match from
/// collapsing to column 0 — the indentation-corruption bug. The relative
/// structure comes from the replacement's own (already rank-collapsed)
/// indentation, so it can never over-indent. Lines inside multiline literals
/// and blank lines are left untouched.
fn redepth_replacement(
    replacement: &str,
    minified_text: &str,
    match_start: usize,
    language: Option<&tree_sitter::Language>,
) -> String {
    let line_start = minified_text[..match_start]
        .rfind('\n')
        .map_or(0, |index| index + 1);
    // The matched line's minified depth (its leading spaces), regardless of
    // whether the search text itself included that indentation.
    let base_depth = minified_text[line_start..]
        .bytes()
        .take_while(|&byte| byte == b' ')
        .count();

    let interior = language
        .map(|lang| minify::literal_interior_lines(replacement, lang))
        .unwrap_or_default();

    let lines: Vec<&str> = replacement.split('\n').collect();
    let is_plain = |i: usize, line: &str| {
        !line.trim().is_empty() && !interior.get(i).copied().unwrap_or(false)
    };
    let own_depth = |line: &str| line.bytes().take_while(|&b| b == b' ').count();

    // Baseline = own depth of the first re-indentable line, so its delta is 0
    // and it lands exactly at base_depth; following lines shift by the same
    // amount, preserving their relative nesting.
    let baseline = lines
        .iter()
        .enumerate()
        .find(|(i, line)| is_plain(*i, line))
        .map(|(_, line)| own_depth(line))
        .unwrap_or(0);

    lines
        .iter()
        .enumerate()
        .map(|(i, line)| {
            if !is_plain(i, line) {
                return (*line).to_string();
            }
            let rel = own_depth(line).saturating_sub(baseline);
            format!(
                "{}{}",
                " ".repeat(base_depth + rel),
                line.trim_start_matches(' ')
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Maps a match range in minified space onto the source. The start maps
/// directly; the end is the first source offset after the matched minified
/// range. Synthetic output bytes often point at the beginning of skipped source
/// whitespace, comments, or blank lines, so the end uses the next mapped output
/// byte when one exists. That preserves comments and separators after the
/// match while still replacing skipped source content inside multiline matches.
fn source_range(src_map: &[usize], start: usize, end: usize, source_len: usize) -> Range<usize> {
    let src_start = src_map[start];
    let src_end = src_map
        .get(end)
        .copied()
        .unwrap_or_else(|| src_map[end - 1] + 1)
        .min(source_len);
    src_start..src_end.max(src_start)
}

fn detect_line_ending(source: &str) -> &'static str {
    if source.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

fn normalize_line_endings(content: &str, target: &str) -> String {
    content.replace("\r\n", "\n").replace('\n', target)
}

/// Restores blank lines lost when the replacement was minified (the minified
/// view drops blank lines). `expanded` is the blank-less replacement; `expanded`
/// has one line per non-blank source line.
/// Blank lines are keyed by the number of non-blank lines that precede them.
/// The original replaced text's blank structure is tried first (the model edits
/// from a blank-less view, so it reflects intent better than the model's typed
/// `new_string`), then `new_string`'s own blanks. A source is only used when its
/// non-blank line count equals `expanded`'s line count, so blanks are restored
/// at exact positions and real content is never shifted — structural rewrites
/// that change the line count simply keep no blanks (returns `None`).
fn reinsert_blank_lines(original: &str, new_string: &str, expanded: &str) -> Option<String> {
    let expanded_lines: Vec<&str> = expanded.split('\n').collect();

    // (non-blank count, blank positions keyed by #non-blank-lines before each).
    let blank_layout = |text: &str| -> (usize, Vec<usize>) {
        let mut non_blank = 0usize;
        let mut positions = Vec::new();
        for line in text.split('\n') {
            if line.trim().is_empty() {
                positions.push(non_blank);
            } else {
                non_blank += 1;
            }
        }
        (non_blank, positions)
    };

    let pick = |text: &str| -> Option<Vec<usize>> {
        let (non_blank, positions) = blank_layout(text);
        (!positions.is_empty() && non_blank == expanded_lines.len()).then_some(positions)
    };

    let blanks = pick(original).or_else(|| pick(new_string))?;

    let mut out: Vec<String> = Vec::with_capacity(expanded_lines.len() + blanks.len());
    let mut next_blank = 0usize;
    for (index, line) in expanded_lines.iter().enumerate() {
        while next_blank < blanks.len() && blanks[next_blank] == index {
            out.push(String::new());
            next_blank += 1;
        }
        out.push((*line).to_string());
    }
    while next_blank < blanks.len() {
        out.push(String::new());
        next_blank += 1;
    }
    Some(out.join("\n"))
}

/// Replaces the first line of `expanded` with the first line of the original
/// replacement, leaving every following line re-indented.
fn keep_first_line_verbatim(original: &str, expanded: &str) -> String {
    let first = original.split('\n').next().unwrap_or_default();
    match expanded.split_once('\n') {
        Some((_, rest)) => format!("{first}\n{rest}"),
        None => first.to_string(),
    }
}

/// Reports a warning when the edited content has tree-sitter syntax errors
/// that the original content did not have.
fn syntax_regression(
    before: &str,
    after: &str,
    language: &tree_sitter::Language,
) -> Option<String> {
    let mut parser = Parser::new();
    parser.set_language(language).ok()?;
    let had_errors = parser.parse(before, None)?.root_node().has_error();
    let has_errors = parser.parse(after, None)?.root_node().has_error();
    (!had_errors && has_errors).then(|| {
        "The edit introduced syntax errors (tree-sitter no longer parses the file cleanly). Review the change.".to_string()
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::apply_minified_edit;

    fn edit(
        name: &str,
        source: &str,
        old: &str,
        new: &str,
        replace_all: bool,
    ) -> Result<super::MinifiedEdit, String> {
        apply_minified_edit(Path::new(name), source, old, new, replace_all, false)
            .expect("language should be supported")
    }

    fn edit_keep_comments(
        name: &str,
        source: &str,
        old: &str,
        new: &str,
        replace_all: bool,
    ) -> Result<super::MinifiedEdit, String> {
        apply_minified_edit(Path::new(name), source, old, new, replace_all, true)
            .expect("language should be supported")
    }

    #[test]
    fn test_single_line_edit_keeps_rest_of_file_byte_identical() {
        let fixture = "/// Doc comment.\nfn main() {\n    // keep me\n    let x = 1;\n}\n";
        let actual = edit("main.rs", fixture, "let x = 1;", "let x = 2;", false).unwrap();
        let expected = "/// Doc comment.\nfn main() {\n    // keep me\n    let x = 2;\n}\n";
        assert_eq!(actual.content, expected);
        assert!(actual.warnings.is_empty());
    }

    #[test]
    fn test_mid_line_match_keeps_surrounding_bytes() {
        let fixture = "fn main() {\n    let value = compute(1, 2); // trailing\n}\n";
        let actual = edit("main.rs", fixture, "compute(1, 2)", "compute(3, 4)", false).unwrap();
        let expected = "fn main() {\n    let value = compute(3, 4); // trailing\n}\n";
        assert_eq!(actual.content, expected);
    }

    #[test]
    fn test_full_line_edit_preserves_hidden_trailing_comment() {
        let fixture = "fn main() {\n    let value = compute(1, 2); // trailing\n}\n";
        let actual = edit(
            "main.rs",
            fixture,
            "let value = compute(1, 2);",
            "let value = compute(3, 4);",
            false,
        )
        .unwrap();
        let expected = "fn main() {\n    let value = compute(3, 4); // trailing\n}\n";
        assert_eq!(actual.content, expected);
    }

    #[test]
    fn test_full_line_edit_preserves_crlf_line_endings() {
        let fixture = "fn main() {\r\n    let value = 1;\r\n}\r\n";
        let actual = edit(
            "main.rs",
            fixture,
            "let value = 1;",
            "let value = 2;",
            false,
        )
        .unwrap();
        let expected = "fn main() {\r\n    let value = 2;\r\n}\r\n";
        assert_eq!(actual.content, expected);
    }

    #[test]
    fn test_raw_indented_old_string_is_normalized_before_matching() {
        let fixture = "fn main() {\n    if a {\n        b();\n        c();\n    }\n}\n";
        let actual = edit(
            "main.rs",
            fixture,
            "if a {\n        b();\n\n        c();\n    }",
            "if a {\n b();\n c();\n d();\n }",
            false,
        )
        .unwrap();
        let expected =
            "fn main() {\n    if a {\n        b();\n        c();\n        d();\n    }\n}\n";
        assert_eq!(actual.content, expected);
        assert!(
            actual
                .warnings
                .iter()
                .any(|warning| warning.contains("normalized minified search text"))
        );
    }

    #[test]
    fn test_raw_indented_new_string_is_normalized_before_expansion() {
        let fixture = "fn main() {\n    if a {\n        b();\n    }\n}\n";
        let actual = edit(
            "main.rs",
            fixture,
            "if a {\n  b();\n }",
            "if a {\n        b();\n\n        c();\n    }",
            false,
        )
        .unwrap();
        // The blank line the caller put in new_string is preserved; the raw
        // 8-space indentation is still normalized to the file's 4-space style.
        let expected = "fn main() {\n    if a {\n        b();\n\n        c();\n    }\n}\n";
        assert_eq!(actual.content, expected);
        assert!(
            actual
                .warnings
                .iter()
                .any(|warning| warning.contains("Normalized replacement text"))
        );
    }

    #[test]
    fn test_normalized_edit_preserves_string_literal_whitespace() {
        let fixture = "fn main() {\n    let value = \"a    b\";\n}\n";
        let actual = edit(
            "main.rs",
            fixture,
            "let value = \"a    b\";",
            "let value = \"c    d\";",
            false,
        )
        .unwrap();
        let expected = "fn main() {\n    let value = \"c    d\";\n}\n";
        assert_eq!(actual.content, expected);
    }

    #[test]
    fn test_keep_comments_allows_comment_in_normalized_search() {
        let fixture = "fn main() {\n    // keep\n    let value = 1;\n}\n";
        let actual = edit_keep_comments(
            "main.rs",
            fixture,
            "// keep\n        let value = 1;",
            "// keep\nlet value = 2;",
            false,
        )
        .unwrap();
        let expected = "fn main() {\n    // keep\n    let value = 2;\n}\n";
        assert_eq!(actual.content, expected);
    }

    #[test]
    fn test_without_keep_comments_drops_comment_from_normalized_search() {
        let fixture = "fn main() {\n    // hidden\n    let value = 1;\n}\n";
        let actual = edit(
            "main.rs",
            fixture,
            "// hidden\nlet value = 1;",
            "let value = 2;",
            false,
        )
        .unwrap();
        let expected = "fn main() {\n    // hidden\n    let value = 2;\n}\n";
        assert_eq!(actual.content, expected);
        assert!(
            actual
                .warnings
                .iter()
                .any(|warning| warning.contains("normalized minified search text"))
        );
    }

    #[test]
    fn test_comment_only_edit_auto_retries_with_keep_comments() {
        // keep_comments defaults to false (comments hidden in the view), yet a
        // comment-only search must still succeed by auto-enabling keep_comments.
        let fixture = "fn h() {\n    // OLD: stale note\n    do_it();\n}\n";
        let actual = edit(
            "main.rs",
            fixture,
            "// OLD: stale note",
            "// NEW: fresh note",
            false,
        )
        .unwrap();
        let expected = "fn h() {\n    // NEW: fresh note\n    do_it();\n}\n";
        assert_eq!(actual.content, expected);
        assert!(
            actual
                .warnings
                .iter()
                .any(|warning| warning.contains("keep_comments enabled"))
        );
    }

    #[test]
    fn test_flat_copied_multiline_search_matches_via_tolerance() {
        // Search text retyped/flattened to column 0: exact matching fails, the
        // indentation-tolerant fallback resolves it and write-back re-indents.
        let fixture = "fn f() {\n    // note\n    let y = compute();\n}\n";
        let actual = edit_keep_comments(
            "main.rs",
            fixture,
            "// note\nlet y = compute();",
            "// note\nlet y = compute2();",
            false,
        )
        .unwrap();
        let expected = "fn f() {\n    // note\n    let y = compute2();\n}\n";
        assert_eq!(actual.content, expected);
    }

    #[test]
    fn test_python_comment_only_edit_auto_retries() {
        // Indentation-significant language, comment-only search, keep_comments
        // defaulted off: auto-retry keeps the comment correctly indented.
        let fixture = "def f(a):\n    # old\n    return a\n";
        let actual = edit("main.py", fixture, "# old", "# updated", false).unwrap();
        let expected = "def f(a):\n    # updated\n    return a\n";
        assert_eq!(actual.content, expected);
    }

    #[test]
    fn test_tabs_file_comment_edit_preserves_tab_indentation() {
        let path = Path::new("main.rs");
        let fixture = "fn f() {\n\t// old note\n\tlet x = 1;\n}\n";
        let view = super::super::minify::minify_for_path(path, fixture, true).unwrap();
        let old = view
            .lines()
            .find(|line| line.contains("// old note"))
            .unwrap();
        let new = old.replacen("// old note", "// fresh note", 1);
        let actual = apply_minified_edit(path, fixture, old, &new, false, true)
            .expect("language should be supported")
            .unwrap();
        let expected = "fn f() {\n\t// fresh note\n\tlet x = 1;\n}\n";
        assert_eq!(actual.content, expected);
    }

    #[test]
    fn test_line_start_comment_edit_keeps_file_indentation() {
        // Regression for the indentation-corruption bug: editing a deeply
        // nested comment whose search text was copied verbatim from the
        // keep_comments view (so the match starts at the line beginning) must
        // keep the file's indentation, not collapse the line to column 0.
        let path = Path::new("main.rs");
        let fixture = "fn outer() {\n    foo.map_err(|e| {\n                // old note\n                let x = 1;\n                x\n    });\n}\n";
        let view = super::super::minify::minify_for_path(path, fixture, true).unwrap();
        let old = view
            .lines()
            .find(|line| line.contains("// old note"))
            .unwrap();
        let new = old.replacen("// old note", "// brand new note", 1);
        let actual = apply_minified_edit(path, fixture, old, &new, false, true)
            .expect("language should be supported")
            .unwrap();
        let expected = "fn outer() {\n    foo.map_err(|e| {\n                // brand new note\n                let x = 1;\n                x\n    });\n}\n";
        assert_eq!(actual.content, expected);
    }

    #[test]
    fn test_multiline_replacement_is_reindented_to_file_style() {
        let fixture = "fn main() {\n    if a {\n        b();\n    }\n}\n";
        let actual = edit(
            "main.rs",
            fixture,
            "if a {\n  b();\n }",
            "if a {\n  b();\n  c();\n }",
            false,
        )
        .unwrap();
        let expected = "fn main() {\n    if a {\n        b();\n        c();\n    }\n}\n";
        assert_eq!(actual.content, expected);
    }

    #[test]
    fn test_replacement_spanning_hidden_comment_warns() {
        let fixture = "fn main() {\n    let x = 1;\n    // hidden\n    let y = 2;\n}\n";
        let actual = edit(
            "main.rs",
            fixture,
            "let x = 1;\n let y = 2;",
            "let x = 1;\n let y = 3;",
            false,
        )
        .unwrap();
        let expected = "fn main() {\n    let x = 1;\n    let y = 3;\n}\n";
        assert_eq!(actual.content, expected);
        assert_eq!(actual.warnings.len(), 1);
        assert!(actual.warnings[0].contains("1 comment(s)"));
    }

    #[test]
    fn test_replace_all() {
        let fixture = "fn main() {\n    foo(1);\n    bar();\n    foo(1);\n}\n";
        let actual = edit("main.rs", fixture, "foo(1)", "foo(2)", true).unwrap();
        let expected = "fn main() {\n    foo(2);\n    bar();\n    foo(2);\n}\n";
        assert_eq!(actual.content, expected);
    }

    #[test]
    fn test_multiple_matches_without_replace_all_errors() {
        let fixture = "fn main() {\n    foo(1);\n    foo(1);\n}\n";
        let actual = edit("main.rs", fixture, "foo(1)", "foo(2)", false);
        assert!(actual.unwrap_err().contains("Multiple matches"));
    }

    #[test]
    fn test_normalized_multiple_matches_without_replace_all_errors() {
        let fixture = "fn main() {\n    if a {\n        foo(1);\n    }\n    if a {\n        foo(1);\n    }\n}\n";
        let actual = edit(
            "main.rs",
            fixture,
            "if a {\n        foo(1);\n    }",
            "if a {\n foo(2);\n }",
            false,
        );
        assert!(actual.unwrap_err().contains("Multiple matches"));
    }

    #[test]
    fn test_no_match_errors_with_reread_hint() {
        let fixture = "fn main() {}\n";
        let actual = edit("main.rs", fixture, "does_not_exist", "x", false);
        assert!(actual.unwrap_err().contains("read"));
    }

    #[test]
    fn test_unsupported_language_returns_none() {
        let actual = apply_minified_edit(
            Path::new("notes.txt"),
            "hello\n",
            "hello",
            "bye",
            false,
            false,
        );
        assert!(actual.is_none());
    }

    #[test]
    fn test_syntax_regression_warns() {
        let fixture = "fn main() {\n    let x = 1;\n}\n";
        let actual = edit("main.rs", fixture, "let x = 1;", "let x = ((1;", false).unwrap();
        assert!(actual.warnings.iter().any(|w| w.contains("syntax errors")));
    }

    #[test]
    fn test_python_indentation_sensitive_edit() {
        let fixture = "def f(a):\n    if a:\n        return 1\n    return 2\n";
        let actual = edit(
            "x.py",
            fixture,
            "if a:\n  return 1",
            "if a:\n  return 10",
            false,
        )
        .unwrap();
        let expected = "def f(a):\n    if a:\n        return 10\n    return 2\n";
        assert_eq!(actual.content, expected);
    }

    #[test]
    fn test_tab_indented_file_gets_tab_replacement() {
        let fixture = "package main\n\nfunc main() {\n\tx := 1\n}\n";
        let actual = edit("main.go", fixture, "x := 1", "x := 2", false).unwrap();
        let expected = "package main\n\nfunc main() {\n\tx := 2\n}\n";
        assert_eq!(actual.content, expected);
    }
}
