//! Port of `grok-mermaid/src/labels.ts` (326 LOC).

use super::width::{measured, string_width};

/// Node labels wrap to at most this many display columns per line ...
pub const WRAP_WIDTH: usize = 24;
/// ... and at most this many lines; overflow is truncated with an ellipsis.
pub const MAX_LINES: usize = 4;
/// Edge labels are truncated to this many columns.
pub const MAX_LABEL: usize = 28;

/// Identifier-boundary characters preferred as break points when a single word
/// is too wide to fit, so it is not sliced mid-segment.
pub const LABEL_BREAK_CHARS: [char; 4] = ['_', '-', '.', '/'];

/// ASCII-only case folding, matching Rust's `to_ascii_lowercase`.
pub fn ascii_lower(value: &str) -> String {
    value.to_ascii_lowercase()
}

pub fn ascii_upper(value: &str) -> String {
    value.to_ascii_uppercase()
}

/// C0 and C1 controls, less the `\t\n\r` the parsers and `src_lines` read.
fn is_control(character: char) -> bool {
    matches!(character, '\0'..='\u{08}' | '\u{0b}' | '\u{0c}' | '\u{0e}'..='\u{1f}' | '\u{7f}'..='\u{9f}')
}

/// Applied by every public entry point that takes untrusted source.
pub fn strip_controls(src: &str) -> String {
    src.chars().filter(|c| !is_control(*c)).collect()
}

/// Split source into lines the way Rust's `str::lines()` does.
pub fn src_lines(src: &str) -> Vec<String> {
    src.lines().map(str::to_string).collect()
}

/// Matches Rust's `char::is_alphanumeric`.
pub fn is_alphanumeric(character: char) -> bool {
    character.is_alphanumeric()
}

/// Characters allowed in a bare node/state/class identifier.
pub fn is_id_char(character: char) -> bool {
    is_alphanumeric(character) || character == '_'
}

const ENTITY_LOOKAHEAD: usize = 10;

fn named_entity(body: &str) -> Option<char> {
    match body {
        "lt" => Some('<'),
        "gt" => Some('>'),
        "amp" => Some('&'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        _ => None,
    }
}

fn decode_entity_body(body: &str) -> Option<char> {
    if let Some(named) = named_entity(body) {
        return Some(named);
    }
    let num = body.strip_prefix('#')?;
    let hex = num.starts_with('x') || num.starts_with('X');
    let digits = if hex { &num[1..] } else { num };
    if digits.is_empty() {
        return None;
    }
    let valid = if hex {
        digits.chars().all(|c| c.is_ascii_hexdigit())
    } else {
        digits.chars().all(|c| c.is_ascii_digit())
    };
    if !valid {
        return None;
    }
    let code = u32::from_str_radix(digits, if hex { 16 } else { 10 }).ok()?;
    // Surrogates and out-of-range values are not characters at all.
    if code > 0x10ffff || (0xd800..=0xdfff).contains(&code) {
        return None;
    }
    // Reject control chars: NUL collides with the CONT sentinel and ESC would
    // inject ANSI into scrollback.
    if code < 0x20 || (0x7f..=0x9f).contains(&code) {
        return None;
    }
    char::from_u32(code)
}

/// Decode HTML entities in label text.
pub fn decode_html_entities(value: &str) -> String {
    if !value.contains('&') {
        return value.to_string();
    }
    let chars: Vec<char> = value.chars().collect();
    let mut out = String::new();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] != '&' {
            out.push(chars[index]);
            index += 1;
            continue;
        }
        // Scan a bounded window including the terminating `;`, so a stray `&`
        // or an over-long run stays literal.
        let high = (index + 1 + ENTITY_LOOKAHEAD).min(chars.len());
        let semi = chars[index + 1..high]
            .iter()
            .position(|character| *character == ';')
            .map(|offset| index + 1 + offset);
        let decoded = semi.and_then(|semi| {
            let body: String = chars[index + 1..semi].iter().collect();
            decode_entity_body(&body)
        });
        match (decoded, semi) {
            // Resume past the `;`. The single pass never re-scans emitted text.
            (Some(decoded), Some(semi)) => {
                out.push(decoded);
                index = semi + 1;
            }
            _ => {
                out.push('&');
                index += 1;
            }
        }
    }
    out
}

/// Strip markdown emphasis from a `` `backtick` `` label string.
pub fn strip_markdown(value: &str) -> String {
    let no_code: String = value.chars().filter(|c| *c != '`').collect();
    let no_strong = no_code.replace("**", "").replace("__", "");
    let chars: Vec<char> = no_strong.chars().collect();
    let mut out = String::new();
    for index in 0..chars.len() {
        let character = chars[index];
        // Keep `*`/`_` only when they sit inside a word, so snake_case survives.
        let in_word = index > 0
            && is_alphanumeric(chars[index - 1])
            && chars
                .get(index + 1)
                .is_some_and(|next| is_alphanumeric(*next));
        if (character == '*' || character == '_') && !in_word {
            continue;
        }
        out.push(character);
    }
    out.trim().to_string()
}

/// Inline formatting tags that carry no meaning in a terminal.
const HTML_FORMAT_TAGS: [&str; 25] = [
    "b", "strong", "i", "em", "u", "s", "strike", "del", "ins", "mark", "small", "big", "sub",
    "sup", "code", "kbd", "samp", "var", "tt", "span", "font", "q", "abbr", "cite", "pre",
];

struct HtmlTag {
    name: String,
    end: usize,
}

/// Read a tag starting at `start`, returning its name and the index after `>`.
fn html_tag_at(chars: &[char], start: usize) -> Option<HtmlTag> {
    let mut index = start + 1;
    if chars.get(index) == Some(&'/') {
        index += 1;
    }
    let name_start = index;
    while index < chars.len() && chars[index].is_ascii_alphanumeric() {
        index += 1;
    }
    if index == name_start {
        return None;
    }
    let name: String = chars[name_start..index].iter().collect();
    while index < chars.len() && chars[index] != '>' {
        if chars[index] == '<' {
            return None;
        }
        index += 1;
    }
    (chars.get(index) == Some(&'>')).then(|| HtmlTag {
        name,
        end: index + 1,
    })
}

pub fn strip_html_tags(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    let mut out = String::new();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '<'
            && let Some(tag) = html_tag_at(&chars, index)
        {
            let lower = tag.name.to_lowercase();
            if lower == "br" {
                out.push(' ');
                index = tag.end;
                continue;
            }
            if HTML_FORMAT_TAGS.contains(&lower.as_str()) {
                index = tag.end;
                continue;
            }
        }
        out.push(chars[index]);
        index += 1;
    }
    out
}

/// Strip one matching pair of wrapping delimiters, if present.
fn unwrap_pair(value: &str, open: char, close: char) -> Option<String> {
    let chars: Vec<char> = value.chars().collect();
    (chars.len() >= 2 && chars[0] == open && chars[chars.len() - 1] == close)
        .then(|| chars[1..chars.len() - 1].iter().collect())
}

/// Normalise raw label text: strip markup, unquote, and decode entities.
pub fn clean_label(raw: &str) -> String {
    let trimmed = strip_html_tags(raw.trim()).trim().to_string();
    let unquoted = unwrap_pair(&trimmed, '"', '"')
        .or_else(|| unwrap_pair(&trimmed, '\'', '\''))
        .unwrap_or(trimmed)
        .trim()
        .to_string();
    match unwrap_pair(&unquoted, '`', '`') {
        Some(markdown) => decode_html_entities(&strip_markdown(markdown.trim())),
        None => decode_html_entities(&unquoted),
    }
}

/// Index of the last identifier-boundary character, or `None`.
fn last_break(value: &str) -> Option<usize> {
    LABEL_BREAK_CHARS
        .iter()
        .filter_map(|character| value.rfind(*character))
        .max()
}

/// Wrap a label to `width` columns over at most `max_lines` lines, truncating
/// the last line with an ellipsis if it overflows.
///
/// A word too wide to fit is broken after the last identifier boundary
/// (`_-./`) that fits, falling back to a per-character break when it has none.
pub fn wrap_label(label: &str, width: usize, max_lines: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_width = 0;

    for word in label.split_whitespace() {
        let word_width = string_width(word);
        if word_width > width {
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
            }
            let mut chunk = String::new();
            let mut chunk_width = 0;
            for (character, character_width) in measured(word) {
                if chunk_width + character_width > width && !chunk.is_empty() {
                    match last_break(&chunk) {
                        Some(position) => {
                            let carry = chunk[position + 1..].to_string();
                            lines.push(chunk[..position + 1].to_string());
                            chunk_width = string_width(&carry);
                            chunk = carry;
                        }
                        None => {
                            lines.push(std::mem::take(&mut chunk));
                            chunk_width = 0;
                        }
                    }
                }
                chunk.push_str(character);
                chunk_width += character_width;
            }
            current = chunk;
            current_width = chunk_width;
        } else if current.is_empty() {
            current = word.to_string();
            current_width = word_width;
        } else if current_width + 1 + word_width <= width {
            current.push(' ');
            current.push_str(word);
            current_width += 1 + word_width;
        } else {
            lines.push(std::mem::replace(&mut current, word.to_string()));
            current_width = word_width;
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }

    if lines.len() > max_lines {
        lines.truncate(max_lines);
        let target = width.saturating_sub(1).max(1);
        let mut shortened = String::new();
        let mut shortened_width = 0;
        let last = lines[lines.len() - 1].clone();
        for (character, character_width) in measured(&last) {
            if shortened_width + character_width > target {
                break;
            }
            shortened.push_str(character);
            shortened_width += character_width;
        }
        let index = lines.len() - 1;
        lines[index] = format!("{shortened}…");
    }
    lines
}

/// Truncate to `inner` columns, leaving room for the ellipsis.
pub fn fit_label(label: &str, inner: usize) -> String {
    if string_width(label) <= inner {
        return label.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for (character, character_width) in measured(label) {
        if used + character_width + 1 > inner {
            break;
        }
        out.push_str(character);
        used += character_width;
    }
    format!("{out}…")
}
