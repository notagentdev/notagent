use crate::modes::interactive::theme::theme::{
    BlockStyle, ThemeBg, ThemeColor, block_style, theme,
};

pub mod word_diff;

use crate::modes::interactive::theme::theme::ansi_rgb;

/// Linear blend from `base` toward `toward` by `t` (reference `blend_rgb`).
fn blend_rgb(base: (u8, u8, u8), toward: (u8, u8, u8), t: f32) -> (u8, u8, u8) {
    let mix = |a: u8, b: u8| -> u8 { (f32::from(a) + (f32::from(b) - f32::from(a)) * t) as u8 };
    (
        mix(base.0, toward.0),
        mix(base.1, toward.1),
        mix(base.2, toward.2),
    )
}

/// The badge style's diff-line background comes directly from the theme. The
/// changed tokens layer another 12% of the corresponding diff foreground over
/// that line colour. Non-truecolor themes keep the line background and use
/// inverse video for token emphasis.
fn diff_wash(added: bool, emphasis: bool) -> String {
    let theme = theme();
    let (bg_token, fg_token) = if added {
        (ThemeBg::ToolDiffAddedBg, ThemeColor::ToolDiffAdded)
    } else {
        (ThemeBg::ToolDiffRemovedBg, ThemeColor::ToolDiffRemoved)
    };
    let base = ansi_rgb(theme.get_bg_ansi(bg_token));
    let accent = ansi_rgb(theme.get_fg_ansi(fg_token));
    match (base, accent) {
        (Some(base), Some(accent)) => {
            let (r, g, b) = if emphasis {
                blend_rgb(base, accent, 0.12)
            } else {
                base
            };
            format!("\x1b[48;2;{r};{g};{b}m")
        }
        _ if emphasis => String::new(),
        _ => theme.get_bg_ansi(bg_token).to_string(),
    }
}

/// The open/close pair wrapped around changed tokens of a modified line: the
/// emphasis wash restoring to the line's own wash in the badge style, plain
/// inverse in the standard style (and as the non-truecolor fallback).
fn emphasis_style(added: bool) -> (String, String) {
    if block_style() == BlockStyle::Badge {
        let emphasis = diff_wash(added, true);
        if !emphasis.is_empty() {
            return (emphasis, diff_wash(added, false));
        }
    }
    ("\x1b[7m".to_string(), "\x1b[27m".to_string())
}

/// Half-open visible character ranges within one line.
type CharSpans = Vec<(usize, usize)>;

/// The visible character ranges of the changed tokens on each side of a
/// single-line modification, following the same leading-whitespace rule as
/// [`render_intra_line_diff`] (reference `intra_line_change_spans`).
/// The badge style needs positions rather than a rendered string, because it
/// marks the ranges *on top of* the syntax-highlighted line.
fn intra_line_change_spans(old_content: &str, new_content: &str) -> (CharSpans, CharSpans) {
    let word_diff = word_diff::diff_words(old_content, new_content);
    let mut removed_spans = Vec::new();
    let mut added_spans = Vec::new();
    let mut removed_pos = 0usize;
    let mut added_pos = 0usize;
    let mut is_first_removed = true;
    let mut is_first_added = true;

    for part in &word_diff {
        let chars = part.value.chars().count();
        if part.removed {
            let mut start = removed_pos;
            if is_first_removed {
                start += word_diff::leading_ws(&part.value).chars().count();
                is_first_removed = false;
            }
            let end = removed_pos + chars;
            if end > start {
                removed_spans.push((start, end));
            }
            removed_pos = end;
        } else if part.added {
            let mut start = added_pos;
            if is_first_added {
                start += word_diff::leading_ws(&part.value).chars().count();
                is_first_added = false;
            }
            let end = added_pos + chars;
            if end > start {
                added_spans.push((start, end));
            }
            added_pos = end;
        } else {
            removed_pos += chars;
            added_pos += chars;
        }
    }

    (removed_spans, added_spans)
}

/// Wraps the given visible-character ranges of an ANSI-styled line in `open`,
/// restoring `close` behind each range; escape sequences in the line are left
/// untouched (reference `emphasize_spans`).
fn emphasize_spans(ansi: &str, spans: &[(usize, usize)], open: &str, close: &str) -> String {
    if spans.is_empty() || open.is_empty() {
        return ansi.to_string();
    }
    let mut out = String::new();
    let mut visible = 0usize;
    let mut in_span = false;
    let mut span_iter = spans.iter().copied().peekable();
    let mut chars = ansi.chars();

    while let Some(character) = chars.next() {
        if character == '\x1b' {
            out.push(character);
            for escape in chars.by_ref() {
                out.push(escape);
                if escape.is_ascii_alphabetic() {
                    break;
                }
            }
            continue;
        }
        loop {
            match span_iter.peek().copied() {
                Some((start, _)) if !in_span && visible == start => {
                    out.push_str(open);
                    in_span = true;
                }
                Some((_, end)) if in_span && visible == end => {
                    out.push_str(close);
                    in_span = false;
                    span_iter.next();
                }
                _ => break,
            }
        }
        out.push(character);
        visible += 1;
    }
    if in_span {
        out.push_str(close);
    }
    out
}

/// One line of diff content through the syntax highlighter.
/// The highlighter closes every line with a full reset, which would drop the
/// wash painted behind it — the callers close themselves, so the reset is
/// stripped here (reference `render_diff`'s `highlight` closure).
fn highlight_content(content: &str, lang: Option<&str>) -> String {
    let text = replace_tabs(content);
    let Some(lang) = lang else {
        return text;
    };
    let highlighted = crate::modes::interactive::theme::theme::highlight_code(&text, Some(lang))
        .into_iter()
        .next()
        .unwrap_or_default();
    highlighted
        .strip_suffix("\x1b[0m")
        .map_or(highlighted.clone(), str::to_string)
}

/// Parsed shape of a diff line.
struct ParsedDiffLine<'a> {
    prefix: char,
    line_num: &'a str,
    content: &'a str,
}

/// Parse diff line to extract prefix, line number, and content.
/// Format: `"+123 content"` or `"-123 content"` or `" 123 content"` or `"     ..."`
/// so a line containing one never matches.
fn parse_diff_line(line: &str) -> Option<ParsedDiffLine<'_>> {
    let prefix = line.chars().next()?;
    if prefix != '+' && prefix != '-' && !word_diff::is_js_whitespace(prefix) {
        return None;
    }
    let rest = &line[prefix.len_utf8()..];
    let chars: Vec<(usize, char)> = rest.char_indices().collect();

    // `\s*` is greedy: start with the longest run of whitespace and shorten it
    // the way the regex engine backtracks.
    let mut max_whitespace = 0;
    while max_whitespace < chars.len() && word_diff::is_js_whitespace(chars[max_whitespace].1) {
        max_whitespace += 1;
    }

    for whitespace_len in (0..=max_whitespace).rev() {
        // `\d*` is greedy as well.
        let mut max_digits = 0;
        while whitespace_len + max_digits < chars.len()
            && chars[whitespace_len + max_digits].1.is_ascii_digit()
        {
            max_digits += 1;
        }
        for digits_len in (0..=max_digits).rev() {
            let separator_index = whitespace_len + digits_len;
            if separator_index >= chars.len() {
                continue;
            }
            let (separator_offset, separator) = chars[separator_index];
            if !word_diff::is_js_whitespace(separator) {
                continue;
            }
            let content = &rest[separator_offset + separator.len_utf8()..];
            // `.` never matches a line terminator and `$` anchors at the end,
            // so a content group containing one makes the match fail.
            if content.contains(['\n', '\r', '\u{2028}', '\u{2029}']) {
                continue;
            }
            return Some(ParsedDiffLine {
                prefix,
                line_num: &rest[..separator_offset],
                content,
            });
        }
    }
    None
}

/// Replace tabs with spaces for consistent rendering.
fn replace_tabs(text: &str) -> String {
    text.replace('\t', "   ")
}

/// Compute word-level diff and render with inverse on changed parts.
/// Uses diffWords which groups whitespace with adjacent words for cleaner highlighting.
/// Strips leading whitespace from inverse to avoid highlighting indentation.
fn render_intra_line_diff(old_content: &str, new_content: &str) -> (String, String) {
    let word_diff = word_diff::diff_words(old_content, new_content);

    let mut removed_line = String::new();
    let mut added_line = String::new();
    let mut is_first_removed = true;
    let mut is_first_added = true;

    // The standard style marks changed tokens with inverse, exactly as the
    // (see `washed_diff_line`), so this stays untouched by it.
    let theme = theme();
    for part in &word_diff {
        if part.removed {
            let mut value = part.value.as_str();
            // Strip leading whitespace from the first removed part
            if is_first_removed {
                let leading_ws = word_diff::leading_ws(value);
                value = &value[leading_ws.len()..];
                removed_line.push_str(leading_ws);
                is_first_removed = false;
            }
            if !value.is_empty() {
                removed_line.push_str(&theme.inverse(value));
            }
        } else if part.added {
            let mut value = part.value.as_str();
            // Strip leading whitespace from the first added part
            if is_first_added {
                let leading_ws = word_diff::leading_ws(value);
                value = &value[leading_ws.len()..];
                added_line.push_str(leading_ws);
                is_first_added = false;
            }
            if !value.is_empty() {
                added_line.push_str(&theme.inverse(value));
            }
        } else {
            removed_line.push_str(&part.value);
            added_line.push_str(&part.value);
        }
    }

    (removed_line, added_line)
}

/// One washed diff row of the badge style: the line's wash, the signed line
/// number in the diff colour, then the syntax-highlighted content with the
/// changed tokens emphasised on top (reference `washed_diff_line`).
fn washed_diff_line(
    kind: char,
    line_num: &str,
    content: &str,
    spans: &[(usize, usize)],
    lang: Option<&str>,
) -> String {
    let theme = theme();
    let added = kind == '+';
    let background = diff_wash(added, false);
    let (open, close) = emphasis_style(added);
    // Behind each emphasised range the line's own wash is restored; with no
    // wash to restore (no truecolor) the inverse pair closes itself.
    let close = if close.starts_with("\x1b[48") || close == "\x1b[27m" {
        close
    } else {
        background.clone()
    };
    let code = emphasize_spans(&highlight_content(content, lang), spans, &open, &close);
    let prefix_color = if added {
        ThemeColor::ToolDiffAdded
    } else {
        ThemeColor::ToolDiffRemoved
    };
    // Keep the background active while `Text` appends its right-side padding.
    // The screen renderer closes the segment after the padded row; resetting
    // here would leave the wash ending at the last code character.
    format!(
        "{background}{} {code}{background}\x1b[39m",
        theme.fg(prefix_color, &format!("{kind}{line_num}"))
    )
}

/// A context row of the badge style: the line number dimmed, the content in
/// its own syntax colours (reference `context_line`).
fn context_line(line_num: &str, content: &str, lang: Option<&str>) -> String {
    format!(
        "\x1b[0m{} {}\x1b[0m",
        theme().fg(ThemeColor::ToolDiffContext, &format!(" {line_num}")),
        highlight_content(content, lang)
    )
}

/// One washed added row from already-highlighted content — the badge style's
/// diff look for lines that are new by construction (the `write` preview).
/// The caller keeps its own highlight cache, so no highlighting happens here.
pub fn washed_added_row(line_num: &str, highlighted_content: &str) -> String {
    let theme = theme();
    let background = diff_wash(true, false);
    format!(
        "{background}{} {highlighted_content}{background}\x1b[39m",
        theme.fg(ThemeColor::ToolDiffAdded, &format!("+{line_num}"))
    )
}

/// The summary line under a rendered diff: "Added N lines, removed M lines".
/// Counts the tool diff format (`+NN content` / `-NN content`); returns `None`
/// when the diff has no signed lines.
pub fn diff_info_line(diff_text: &str) -> Option<String> {
    let mut added = 0usize;
    let mut removed = 0usize;
    for line in diff_text.split('\n') {
        match line.chars().next() {
            Some('+') => added += 1,
            Some('-') => removed += 1,
            _ => {}
        }
    }
    info_line_text(added, removed)
}

/// The "Added N lines, removed M lines" text from raw counts.
pub fn info_line_text(added: usize, removed: usize) -> Option<String> {
    fn lines(count: usize) -> String {
        if count == 1 {
            "1 line".to_string()
        } else {
            format!("{count} lines")
        }
    }
    let text = match (added, removed) {
        (0, 0) => return None,
        (added, 0) => format!("Added {}", lines(added)),
        (0, removed) => format!("Removed {}", lines(removed)),
        (added, removed) => format!("Added {}, removed {}", lines(added), lines(removed)),
    };
    Some(theme().fg(ThemeColor::Muted, &text))
}

/// Options of [`render_diff`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RenderDiffOptions {
    /// File path (unused, kept for API compatibility)
    pub file_path: Option<String>,
}

/// Render a diff string with colored lines and intra-line change highlighting.
/// - Context lines: dim/gray
/// - Removed lines: red, with inverse on changed tokens
/// - Added lines: green, with inverse on changed tokens
///
/// Badge-style diffs wash added and removed lines with themed backgrounds,
/// retain syntax colors, and emphasize changed tokens within modified lines.
pub fn render_diff(diff_text: &str, options: &RenderDiffOptions) -> String {
    let lines: Vec<&str> = diff_text.split('\n').collect();
    let mut result: Vec<String> = Vec::new();
    let theme = theme();
    let badge_style = block_style() == BlockStyle::Badge;
    // Only the badge style paints the code, so the language is only resolved
    // there; the standard style has no syntax colours to place.
    let lang = badge_style
        .then(|| {
            options
                .file_path
                .as_deref()
                .and_then(crate::modes::interactive::theme::theme::get_language_from_path)
        })
        .flatten();
    let lang = lang.as_deref();

    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let Some(parsed) = parse_diff_line(line) else {
            let line = theme.fg(ThemeColor::ToolDiffContext, line);
            result.push(if badge_style {
                format!("\x1b[0m{line}\x1b[0m")
            } else {
                line
            });
            i += 1;
            continue;
        };

        if parsed.prefix == '-' {
            // Collect consecutive removed lines
            let mut removed_lines: Vec<(String, String)> = Vec::new();
            while i < lines.len() {
                let Some(parsed) = parse_diff_line(lines[i]) else {
                    break;
                };
                if parsed.prefix != '-' {
                    break;
                }
                removed_lines.push((parsed.line_num.to_string(), parsed.content.to_string()));
                i += 1;
            }

            // Collect consecutive added lines
            let mut added_lines: Vec<(String, String)> = Vec::new();
            while i < lines.len() {
                let Some(parsed) = parse_diff_line(lines[i]) else {
                    break;
                };
                if parsed.prefix != '+' {
                    break;
                }
                added_lines.push((parsed.line_num.to_string(), parsed.content.to_string()));
                i += 1;
            }

            // Only do intra-line diffing when there's exactly one removed and one added line
            // (indicating a single line modification). Otherwise, show lines as-is.
            let single_modification = removed_lines.len() == 1 && added_lines.len() == 1;
            if badge_style {
                let (removed_spans, added_spans) = if single_modification {
                    intra_line_change_spans(
                        &replace_tabs(&removed_lines[0].1),
                        &replace_tabs(&added_lines[0].1),
                    )
                } else {
                    (Vec::new(), Vec::new())
                };
                for removed in &removed_lines {
                    result.push(washed_diff_line(
                        '-',
                        &removed.0,
                        &removed.1,
                        &removed_spans,
                        lang,
                    ));
                }
                for added in &added_lines {
                    result.push(washed_diff_line(
                        '+',
                        &added.0,
                        &added.1,
                        &added_spans,
                        lang,
                    ));
                }
            } else if single_modification {
                let removed = &removed_lines[0];
                let added = &added_lines[0];

                let (removed_line, added_line) =
                    render_intra_line_diff(&replace_tabs(&removed.1), &replace_tabs(&added.1));

                result.push(theme.fg(
                    ThemeColor::ToolDiffRemoved,
                    &format!("-{} {removed_line}", removed.0),
                ));
                result.push(theme.fg(
                    ThemeColor::ToolDiffAdded,
                    &format!("+{} {added_line}", added.0),
                ));
            } else {
                // Show all removed lines first, then all added lines
                for removed in &removed_lines {
                    result.push(theme.fg(
                        ThemeColor::ToolDiffRemoved,
                        &format!("-{} {}", removed.0, replace_tabs(&removed.1)),
                    ));
                }
                for added in &added_lines {
                    result.push(theme.fg(
                        ThemeColor::ToolDiffAdded,
                        &format!("+{} {}", added.0, replace_tabs(&added.1)),
                    ));
                }
            }
        } else if parsed.prefix == '+' {
            // Standalone added line
            if badge_style {
                result.push(washed_diff_line(
                    '+',
                    parsed.line_num,
                    parsed.content,
                    &[],
                    lang,
                ));
            } else {
                result.push(theme.fg(
                    ThemeColor::ToolDiffAdded,
                    &format!("+{} {}", parsed.line_num, replace_tabs(parsed.content)),
                ));
            }
            i += 1;
        } else {
            // Context line
            if badge_style {
                result.push(context_line(parsed.line_num, parsed.content, lang));
            } else {
                result.push(theme.fg(
                    ThemeColor::ToolDiffContext,
                    &format!(" {} {}", parsed.line_num, replace_tabs(parsed.content)),
                ));
            }
            i += 1;
        }
    }

    result.join("\n")
}

#[cfg(test)]
mod tests {
    use std::sync::MutexGuard;

    use notagent_tui::components::text::Text;
    use notagent_tui::tui::Component;
    use notagent_tui::utils::visible_width;

    use super::*;
    use crate::modes::interactive::theme::theme::{init_theme, set_block_style, test_lock};
    use crate::utils::ansi::strip_ansi;

    /// The theme and the block style are process globals; every test that
    /// touches them shares one lock.
    fn theme_lock(style: BlockStyle) -> MutexGuard<'static, ()> {
        let guard = test_lock();
        init_theme(Some("dark"), false);
        set_block_style(style);
        guard
    }

    const RUST_DIFF: &str = "  10 fn main() {\n- 11     let x = 1;\n+ 11     let x = 2;\n  12 }";

    fn rust_options() -> RenderDiffOptions {
        RenderDiffOptions {
            file_path: Some("src/main.rs".to_string()),
        }
    }

    /// The opening sequence the `rust` grammar paints a keyword with — the
    /// highlighted `fn` without the word itself.
    fn keyword_ansi() -> String {
        let highlighted =
            crate::modes::interactive::theme::theme::highlight_code("fn", Some("rust"))
                .into_iter()
                .next()
                .unwrap_or_default();
        highlighted
            .split("fn")
            .next()
            .unwrap_or_default()
            .to_string()
    }

    #[test]
    fn the_badge_style_keeps_the_code_in_its_own_syntax_colours() {
        let _guard = theme_lock(BlockStyle::Badge);
        let rendered = render_diff(RUST_DIFF, &rust_options());
        let keyword = keyword_ansi();
        assert!(!keyword.is_empty(), "the fixture needs a highlighter");
        // `fn` on the context row and `let` on both changed rows.
        assert_eq!(
            rendered.matches(&keyword).count(),
            3,
            "context and changed rows are highlighted: {rendered:?}"
        );
        // The text itself survives the colouring.
        assert!(strip_ansi(&rendered).contains("let x = 2;"), "{rendered:?}");
    }

    #[test]
    fn the_badge_style_washes_the_changed_rows_and_leaves_context_bare() {
        let _guard = theme_lock(BlockStyle::Badge);
        let rendered = render_diff(RUST_DIFF, &rust_options());
        let rows: Vec<&str> = rendered.lines().collect();

        assert_eq!(rows.len(), 4, "{rendered:?}");
        assert!(
            !rows[0].contains("\x1b[48;"),
            "context row is unwashed: {:?}",
            rows[0]
        );
        assert!(
            rows[1].starts_with(&diff_wash(false, false)),
            "{:?}",
            rows[1]
        );
        assert!(
            rows[2].starts_with(&diff_wash(true, false)),
            "{:?}",
            rows[2]
        );
        assert!(
            !rows[3].contains("\x1b[48;"),
            "context row is unwashed: {:?}",
            rows[3]
        );
    }

    #[test]
    fn the_dark_diff_palette_matches_the_requested_theme_colors() {
        let _guard = theme_lock(BlockStyle::Badge);

        assert_eq!(
            ansi_rgb(theme().get_fg_ansi(ThemeColor::ToolDiffAdded)),
            Some((15, 188, 122))
        );
        assert_eq!(
            ansi_rgb(theme().get_fg_ansi(ThemeColor::ToolDiffRemoved)),
            Some((205, 49, 49))
        );
        assert_eq!(ansi_rgb(&diff_wash(true, false)), Some((34, 58, 43)));
        assert_eq!(ansi_rgb(&diff_wash(false, false)), Some((74, 34, 29)));
    }

    /// The changed token of a single-line modification gets the stronger wash
    /// on top of the syntax colours, and the line's own wash behind it again.
    #[test]
    fn the_badge_style_emphasises_only_the_changed_token() {
        let _guard = theme_lock(BlockStyle::Badge);
        let rendered = render_diff(RUST_DIFF, &rust_options());
        let added = rendered.lines().nth(2).expect("added row");

        assert!(added.contains(&diff_wash(true, true)), "{added:?}");
        // The emphasis covers `2` and nothing else: it opens once and hands
        // back to the line wash before the semicolon.
        assert_eq!(
            added.matches(&diff_wash(true, true)).count(),
            1,
            "{added:?}"
        );
        assert!(strip_ansi(added).contains("let x = 2;"), "{added:?}");
    }

    /// The changed token takes another 12% of the foreground accent from the
    /// themed line background.
    #[test]
    fn the_changed_token_uses_a_stronger_wash_than_the_rest_of_the_row() {
        let _guard = theme_lock(BlockStyle::Badge);
        let added_line = ansi_rgb(&diff_wash(true, false)).expect("truecolor wash");
        let added_token = ansi_rgb(&diff_wash(true, true)).expect("truecolor wash");
        let removed_line = ansi_rgb(&diff_wash(false, false)).expect("truecolor wash");
        let removed_token = ansi_rgb(&diff_wash(false, true)).expect("truecolor wash");
        let added_accent =
            ansi_rgb(theme().get_fg_ansi(ThemeColor::ToolDiffAdded)).expect("truecolor accent");
        let removed_accent =
            ansi_rgb(theme().get_fg_ansi(ThemeColor::ToolDiffRemoved)).expect("truecolor accent");
        assert_eq!(added_token, blend_rgb(added_line, added_accent, 0.12));
        assert_eq!(removed_token, blend_rgb(removed_line, removed_accent, 0.12));
        let distance = |left: (u8, u8, u8), right: (u8, u8, u8)| {
            u16::from(left.0.abs_diff(right.0))
                + u16::from(left.1.abs_diff(right.1))
                + u16::from(left.2.abs_diff(right.2))
        };

        assert!(distance(added_token, added_accent) < distance(added_line, added_accent));
        assert!(distance(removed_token, removed_accent) < distance(removed_line, removed_accent));
    }

    #[test]
    fn the_changed_row_keeps_its_background_through_the_full_render_width() {
        let _guard = theme_lock(BlockStyle::Badge);
        let rendered = render_diff(RUST_DIFF, &rust_options());
        let mut text = Text::new(rendered, 0, 0);
        let rows = text.render(48);
        let removed = rows.get(1).expect("removed row");
        let background = diff_wash(false, false);

        assert_eq!(visible_width(removed), 48, "row is padded to the viewport");
        let final_background = removed.rfind(&background).expect("final row wash");
        let padding = &removed[final_background + background.len()..];
        assert!(
            padding.starts_with("\x1b[39m"),
            "only the foreground is reset before the padding: {removed:?}"
        );
        assert!(
            padding["\x1b[39m".len()..]
                .chars()
                .all(|character| character == ' '),
            "the active row wash covers every trailing cell: {removed:?}"
        );
    }

    #[test]
    fn the_standard_style_paints_neither_wash_nor_syntax() {
        let _guard = theme_lock(BlockStyle::Standard);
        let rendered = render_diff(RUST_DIFF, &rust_options());

        assert!(!rendered.contains("\x1b[48;"), "no wash: {rendered:?}");
        assert!(
            !rendered.contains(&keyword_ansi()),
            "no syntax colours: {rendered:?}"
        );
        assert!(rendered.contains("\x1b[7m"), "inverse marks the change");
    }
}
