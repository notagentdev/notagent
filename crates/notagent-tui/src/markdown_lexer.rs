//! Markdown lexer producing the token stream of `marked` 18.
//!
//! Port decision for plan task 11: instead of adapting a CommonMark parser, the
//! lexer reproduces the marked token stream for exactly the token types the
//! renderer consumes. `tools/gen-markdown-oracle.mjs` dumps the marked stream
//! for every source of the ported test suite; `tests/markdown_oracle.rs` asserts
//! equality against it, so the tokenizer is verified against the original.

/// A lexed token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    /// Token type, spelled as in marked (`paragraph`, `list_item`, …).
    pub kind: String,
    /// Source text the token was produced from.
    pub raw: String,
    /// Text content, where the token carries one.
    pub text: Option<String>,
    /// Heading level.
    pub depth: Option<usize>,
    /// Code fence language.
    pub lang: Option<String>,
    /// Link target.
    pub href: Option<String>,
    /// Whether a list is ordered.
    pub ordered: Option<bool>,
    /// Start number of an ordered list.
    pub start: Option<i64>,
    /// Whether a list is loose.
    pub loose: Option<bool>,
    /// Whether a list item is a task.
    pub task: Option<bool>,
    /// Whether a task item is checked.
    pub checked: Option<bool>,
    /// Whether a LaTeX token is still incomplete.
    pub pending: Option<bool>,
    /// Child tokens.
    pub tokens: Option<Vec<Token>>,
    /// List items.
    pub items: Option<Vec<Token>>,
    /// Table header cells.
    pub header: Option<Vec<TableCell>>,
    /// Table body rows.
    pub rows: Option<Vec<Vec<TableCell>>>,
}

/// One table cell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableCell {
    /// Raw cell text.
    pub text: String,
    /// Inline tokens of the cell.
    pub tokens: Vec<Token>,
}

impl Token {
    /// A token with only a type and raw text.
    fn new(kind: &str, raw: impl Into<String>) -> Self {
        Self {
            kind: kind.to_string(),
            raw: raw.into(),
            text: None,
            depth: None,
            lang: None,
            href: None,
            ordered: None,
            start: None,
            loose: None,
            task: None,
            checked: None,
            pending: None,
            tokens: None,
            items: None,
            header: None,
            rows: None,
        }
    }

    fn with_text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }

    fn with_tokens(mut self, tokens: Vec<Token>) -> Self {
        self.tokens = Some(tokens);
        self
    }
}

/// Lex `source` into the marked block token stream.
pub fn lex(source: &str) -> Vec<Token> {
    let normalized = source.replace("\r\n", "\n").replace('\r', "\n");
    block_tokens(&normalized, true)
}

/// Whether `line` is blank.
fn is_blank(line: &str) -> bool {
    line.trim_matches([' ', '\t']).is_empty()
}

/// Leading indentation of `line` in columns (tabs count as 4).
fn indent_width(line: &str) -> usize {
    let mut width = 0;
    for character in line.chars() {
        match character {
            ' ' => width += 1,
            '\t' => width += 4 - width % 4,
            _ => break,
        }
    }
    width
}

/// `/^ {0,3}(#{1,6})(?=\s|$)/`.
fn parse_heading(line: &str) -> Option<(usize, String)> {
    if indent_width(line) > 3 {
        return None;
    }
    let trimmed = line.trim_start_matches(' ');
    let hashes = trimmed.chars().take_while(|c| *c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = &trimmed[hashes..];
    if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
        return None;
    }
    let mut text = rest.trim().to_string();
    // A closing sequence of hashes is stripped.
    if text.ends_with('#') {
        let without = text.trim_end_matches('#');
        if without.is_empty() || without.ends_with(' ') {
            text = without.trim_end().to_string();
        }
    }
    Some((hashes, text))
}

/// `/^ {0,3}((?:-[\t ]*){3,}|(?:_[ \t]*){3,}|(?:\*[ \t]*){3,})(?:\n+|$)/`.
fn is_hr(line: &str) -> bool {
    if indent_width(line) > 3 {
        return false;
    }
    let trimmed = line.trim_start_matches(' ');
    let Some(marker) = trimmed.chars().next() else {
        return false;
    };
    if !matches!(marker, '-' | '_' | '*') {
        return false;
    }
    let mut count = 0;
    for character in trimmed.chars() {
        if character == marker {
            count += 1;
        } else if character != ' ' && character != '\t' {
            return false;
        }
    }
    count >= 3
}

/// A fence opener: `(indent, marker, count, language)`.
fn parse_fence(line: &str) -> Option<(usize, char, usize, String)> {
    let indent = indent_width(line);
    if indent > 3 {
        return None;
    }
    let trimmed = line.trim_start_matches(' ');
    let marker = trimmed.chars().next()?;
    if marker != '`' && marker != '~' {
        return None;
    }
    let count = trimmed.chars().take_while(|c| *c == marker).count();
    if count < 3 {
        return None;
    }
    let info = trimmed[count..].trim();
    if marker == '`' && info.contains('`') {
        return None;
    }
    Some((indent, marker, count, info.to_string()))
}

/// A list marker: `(marker_width, ordered, start, marker_text)`.
fn parse_list_marker(line: &str) -> Option<(usize, bool, i64, String)> {
    let indent = indent_width(line);
    if indent > 3 {
        return None;
    }
    let trimmed = line.trim_start_matches(' ');
    let bytes = trimmed.as_bytes();
    if bytes.is_empty() {
        return None;
    }

    if matches!(bytes[0], b'-' | b'+' | b'*') {
        let rest = &trimmed[1..];
        if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
            return None;
        }
        if is_hr(line) {
            return None;
        }
        return Some((indent + 1, false, 1, trimmed[..1].to_string()));
    }

    let digits: String = trimmed.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() || digits.len() > 9 {
        return None;
    }
    let delimiter = trimmed[digits.len()..].chars().next()?;
    if delimiter != '.' && delimiter != ')' {
        return None;
    }
    let rest = &trimmed[digits.len() + 1..];
    if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
        return None;
    }
    Some((
        indent + digits.len() + 1,
        true,
        digits.parse().unwrap_or(1),
        trimmed[..digits.len() + 1].to_string(),
    ))
}

/// Whether `line` opens a GFM table (its next line is the delimiter row).
fn is_table_delimiter(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }
    let body = trimmed.trim_start_matches('|').trim_end_matches('|');
    if body.is_empty() {
        return false;
    }
    body.split('|').all(|cell| {
        let cell = cell.trim();
        !cell.is_empty()
            && cell.chars().all(|character| matches!(character, '-' | ':'))
            && cell.contains('-')
    })
}

/// Split a table row into its cells.
fn split_table_row(line: &str) -> Vec<String> {
    let trimmed = line.trim();
    let body = trimmed
        .strip_prefix('|')
        .unwrap_or(trimmed)
        .strip_suffix('|')
        .unwrap_or_else(|| trimmed.strip_prefix('|').unwrap_or(trimmed));
    let mut cells: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut escaped = false;
    for character in body.chars() {
        if escaped {
            if character != '|' {
                current.push('\\');
            }
            current.push(character);
            escaped = false;
            continue;
        }
        match character {
            '\\' => escaped = true,
            '|' => cells.push(std::mem::take(&mut current)),
            _ => current.push(character),
        }
    }
    if escaped {
        current.push('\\');
    }
    cells.push(current);
    cells
        .into_iter()
        .map(|cell| cell.trim().to_string())
        .collect()
}

/// Byte spans of every line, including its trailing newline.
fn line_spans(source: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start = 0;
    for (index, character) in source.char_indices() {
        if character == '\n' {
            spans.push((start, index + 1));
            start = index + 1;
        }
    }
    spans.push((start, source.len()));
    spans
}

/// Raw text of the lines `start..end`.
///
/// marked hands the newline that terminates a block to the following `space`
/// token whenever a blank line follows, so it is dropped here in that case.
fn raw_span(
    source: &str,
    spans: &[(usize, usize)],
    lines: &[&str],
    start: usize,
    end: usize,
) -> String {
    if start >= end {
        return String::new();
    }
    let mut raw_end = spans[end - 1].1;
    // The empty element after a trailing newline is not a blank line.
    let followed_by_blank_line = end < lines.len()
        && is_blank(lines[end])
        && !(end == lines.len() - 1 && lines[end].is_empty());
    if followed_by_blank_line && source[..raw_end].ends_with('\n') {
        raw_end -= 1;
    }
    source[spans[start].0..raw_end].to_string()
}

/// Tokenize a block-level source.
fn block_tokens(source: &str, top: bool) -> Vec<Token> {
    let lines: Vec<&str> = source.split('\n').collect();
    let spans = line_spans(source);
    let mut tokens: Vec<Token> = Vec::new();
    let mut index = 0;
    // Byte offset just past the previous token's raw text.
    let mut previous_end = 0usize;

    while index < lines.len() {
        let line = lines[index];

        // The final empty element after a trailing newline is not a token.
        if index == lines.len() - 1 && line.is_empty() {
            break;
        }

        if is_blank(line) {
            let start = index;
            while index < lines.len() && is_blank(lines[index]) {
                index += 1;
            }
            // A trailing newline belongs to the preceding token, not a space token.
            let space_start = if tokens.is_empty() {
                spans[start].0
            } else {
                previous_end
            };
            let raw = source[space_start..spans[index - 1].1].to_string();
            previous_end = spans[index - 1].1;
            if raw.is_empty() {
                continue;
            }
            if let Some(last) = tokens.last_mut()
                && last.kind == "space"
            {
                last.raw.push_str(&raw);
            } else {
                tokens.push(Token::new("space", raw));
            }
            continue;
        }

        if let Some(mut token) = latex_block(&lines, &mut index) {
            // marked folds the blank lines after a block extension into its raw.
            if token.raw.ends_with('\n') {
                while index < lines.len() && is_blank(lines[index]) {
                    if index == lines.len() - 1 && lines[index].is_empty() {
                        break;
                    }
                    token.raw.push('\n');
                    index += 1;
                }
            }
            previous_end = spans[index.min(lines.len()) - 1].1;
            tokens.push(token);
            continue;
        }

        if let Some((_, marker, count, language)) = parse_fence(line) {
            let start = index;
            index += 1;
            let mut body: Vec<&str> = Vec::new();
            let mut closed = false;
            while index < lines.len() {
                let candidate = lines[index];
                let trimmed = candidate.trim_start_matches(' ');
                if indent_width(candidate) <= 3
                    && trimmed.starts_with(marker)
                    && trimmed.chars().take_while(|c| *c == marker).count() >= count
                    && trimmed
                        .trim_end()
                        .chars()
                        .all(|character| character == marker)
                {
                    index += 1;
                    closed = true;
                    break;
                }
                body.push(candidate);
                index += 1;
            }
            let indent = indent_width(line);
            let text = body
                .iter()
                .map(|body_line| strip_indent(body_line, indent))
                .collect::<Vec<_>>()
                .join("\n");
            let raw = raw_span(source, &spans, &lines, start, index);
            previous_end = spans[start].0 + raw.len();
            let _ = closed;
            let mut token = Token::new("code", raw).with_text(text);
            token.lang = (!language.is_empty()).then_some(language);
            tokens.push(token);
            continue;
        }

        if let Some((depth, text)) = parse_heading(line) {
            let raw = raw_span(source, &spans, &lines, index, index + 1);
            previous_end = spans[index].0 + raw.len();
            index += 1;
            let mut token = Token::new("heading", raw).with_text(&text);
            token.depth = Some(depth);
            token.tokens = Some(inline_tokens(&text));
            tokens.push(token);
            continue;
        }

        if is_hr(line) {
            let raw = raw_span(source, &spans, &lines, index, index + 1);
            previous_end = spans[index].0 + raw.len();
            index += 1;
            tokens.push(Token::new("hr", raw));
            continue;
        }

        if line.trim_start().starts_with('>') && indent_width(line) <= 3 {
            let start = index;
            let mut body: Vec<String> = Vec::new();
            while index < lines.len() {
                let candidate = lines[index];
                if candidate.trim_start().starts_with('>') && indent_width(candidate) <= 3 {
                    let trimmed = candidate.trim_start_matches(' ');
                    let without_marker = &trimmed[1..];
                    body.push(
                        without_marker
                            .strip_prefix(' ')
                            .unwrap_or(without_marker)
                            .to_string(),
                    );
                    index += 1;
                    continue;
                }
                // Lazy continuation lines belong to the quote's paragraph.
                if !is_blank(candidate)
                    && !body.last().is_some_and(|line| is_blank(line))
                    && parse_list_marker(candidate).is_none()
                    && parse_fence(candidate).is_none()
                    && parse_heading(candidate).is_none()
                    && !is_hr(candidate)
                {
                    body.push(candidate.to_string());
                    index += 1;
                    continue;
                }
                break;
            }
            let raw = raw_span(source, &spans, &lines, start, index);
            previous_end = spans[start].0 + raw.len();
            let inner_source = if raw.ends_with('\n') {
                format!("{}\n", body.join("\n"))
            } else {
                body.join("\n")
            };
            let token = Token::new("blockquote", raw)
                .with_text(body.join("\n"))
                .with_tokens(block_tokens(&inner_source, false));
            tokens.push(token);
            continue;
        }

        if parse_list_marker(line).is_some() {
            let list_start = index;
            let token = parse_list(source, &lines, &spans, &mut index);
            previous_end = spans[list_start].0 + token.raw.len();
            tokens.push(token);
            continue;
        }

        if index + 1 < lines.len()
            && line.contains('|')
            && is_table_delimiter(lines[index + 1])
            && split_table_row(line).len() == split_table_row(lines[index + 1]).len()
        {
            let table_start = index;
            let token = parse_table(source, &lines, &spans, &mut index);
            previous_end = spans[table_start].0 + token.raw.len();
            tokens.push(token);
            continue;
        }

        if top && is_html_block_start(line) {
            let start = index;
            while index < lines.len() && !is_blank(lines[index]) {
                index += 1;
            }
            let raw = raw_span(source, &spans, &lines, start, index);
            previous_end = spans[start].0 + raw.len();
            let text = raw.clone();
            tokens.push(Token::new("html", raw).with_text(text));
            continue;
        }

        // Paragraph, including its setext underline.
        let start = index;
        let mut paragraph: Vec<&str> = Vec::new();
        while index < lines.len() {
            let candidate = lines[index];
            if is_blank(candidate) {
                break;
            }
            if !paragraph.is_empty()
                && (parse_fence(candidate).is_some()
                    || parse_heading(candidate).is_some()
                    || is_hr(candidate)
                    || candidate.trim_start().starts_with('>')
                    || parse_list_marker(candidate).is_some()
                    || (top && is_html_block_start(candidate)))
            {
                break;
            }
            paragraph.push(candidate);
            index += 1;
        }
        let raw = raw_span(source, &spans, &lines, start, index);
        previous_end = spans[start].0 + raw.len();
        let text = paragraph
            .iter()
            .map(|line| line.trim())
            .collect::<Vec<_>>()
            .join("\n");
        let token = Token::new("paragraph", raw)
            .with_text(&text)
            .with_tokens(inline_tokens(&text));
        tokens.push(token);
    }

    tokens
}

/// Remove up to `width` columns of leading indentation.
fn strip_indent(line: &str, width: usize) -> String {
    let mut removed = 0;
    let mut characters = line.char_indices();
    let mut cut = 0;
    while removed < width {
        let Some((offset, character)) = characters.next() else {
            break;
        };
        match character {
            ' ' => removed += 1,
            '\t' => removed += 4 - removed % 4,
            _ => break,
        }
        cut = offset + character.len_utf8();
    }
    line[cut..].to_string()
}

/// `/^<(?:[a-z][\w-]*|!--)/i` at the start of a block.
fn is_html_block_start(line: &str) -> bool {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('<') {
        return false;
    }
    let rest = &trimmed[1..];
    rest.starts_with("!--")
        || rest.starts_with('/')
        || rest.starts_with(|character: char| character.is_ascii_alphabetic())
}

/// Parse a list starting at `*index`.
fn parse_list(source: &str, lines: &[&str], spans: &[(usize, usize)], index: &mut usize) -> Token {
    let start = *index;
    let (_, ordered, list_start, first_marker) =
        parse_list_marker(lines[start]).expect("list marker present");
    let marker_character = first_marker.chars().next_back().expect("marker character");

    let mut items: Vec<Token> = Vec::new();
    let mut loose = false;

    while *index < lines.len() {
        let line = lines[*index];
        let Some((marker_width, item_ordered, _, marker)) = parse_list_marker(line) else {
            break;
        };
        if item_ordered != ordered || !marker.ends_with(marker_character) {
            break;
        }

        let item_start = *index;
        let content_indent = {
            let rest = &line.trim_start_matches(' ')[marker.len()..];
            let spaces = rest.len() - rest.trim_start_matches([' ', '\t']).len();
            marker_width
                + if rest.trim().is_empty() {
                    1
                } else {
                    spaces.max(1)
                }
        };
        let mut body: Vec<String> = vec![{
            let trimmed = line.trim_start_matches(' ');
            let rest = &trimmed[marker.len()..];
            rest.strip_prefix(' ').unwrap_or(rest).to_string()
        }];
        *index += 1;

        let mut trailing_blank = false;
        while *index < lines.len() {
            let candidate = lines[*index];
            if is_blank(candidate) {
                // A blank line only continues the item when indented content follows.
                let next = lines.get(*index + 1).copied();
                let continues = next.is_some_and(|next| {
                    if is_blank(next) {
                        return false;
                    }
                    if indent_width(next) >= content_indent {
                        return true;
                    }
                    // Only a marker of the same list continues it.
                    parse_list_marker(next).is_some_and(|(_, next_ordered, _, next_marker)| {
                        next_ordered == ordered && next_marker.ends_with(marker_character)
                    })
                });
                if !continues {
                    break;
                }
                if next.is_some_and(|next| indent_width(next) < content_indent) {
                    // The blank line separates two items: the list is loose and
                    // the item text keeps the terminating newline.
                    loose = true;
                    trailing_blank = true;
                    body.push(String::new());
                    *index += 1;
                    break;
                }
                loose = true;
                body.push(String::new());
                *index += 1;
                continue;
            }
            if indent_width(candidate) >= content_indent {
                body.push(strip_indent(candidate, content_indent));
                *index += 1;
                continue;
            }
            if parse_list_marker(candidate).is_some() {
                break;
            }
            // Lazy continuation of the item's paragraph.
            if !body.last().is_some_and(String::is_empty) {
                body.push(candidate.trim_start().to_string());
                *index += 1;
                continue;
            }
            break;
        }
        let _ = trailing_blank;

        let raw = raw_span(source, spans, lines, item_start, *index);

        let mut text = body.join("\n");
        let mut task = false;
        let mut checked = false;
        let mut checkbox_raw = String::new();
        if let Some(rest) = text.strip_prefix("[ ] ") {
            task = true;
            checkbox_raw = "[ ] ".to_string();
            text = rest.to_string();
        } else if let Some(rest) = text.strip_prefix("[x] ") {
            task = true;
            checked = true;
            checkbox_raw = "[x] ".to_string();
            text = rest.to_string();
        } else if let Some(rest) = text.strip_prefix("[X] ") {
            task = true;
            checked = true;
            checkbox_raw = "[X] ".to_string();
            text = rest.to_string();
        }

        let item_source = text.clone();
        let mut item_tokens = block_tokens(&item_source, false);
        // A tight item renders its paragraph as a `text` token.
        if !loose {
            for token in &mut item_tokens {
                if token.kind == "paragraph" {
                    token.kind = "text".to_string();
                }
            }
        }
        if task {
            let mut checkbox = Token::new("checkbox", checkbox_raw);
            checkbox.checked = Some(checked);
            item_tokens.insert(0, checkbox);
        }

        let mut item = Token::new("list_item", raw).with_text(&text);
        item.loose = Some(false);
        item.task = Some(task);
        if task {
            item.checked = Some(checked);
        }
        item.tokens = Some(item_tokens);
        items.push(item);
    }

    let raw = raw_span(source, spans, lines, start, *index);

    // marked drops the trailing newline of the final item.
    if let Some(last) = items.last_mut()
        && let Some(trimmed) = last.raw.strip_suffix('\n')
    {
        last.raw = trimmed.to_string();
    }

    // A list is loose when any item contains a blank-line-separated block.
    if items.iter().any(|item| {
        item.tokens
            .as_ref()
            .is_some_and(|tokens| tokens.iter().filter(|t| t.kind != "space").count() > 1)
            && item.raw.contains("\n\n")
    }) {
        loose = true;
    }

    if loose {
        for item in &mut items {
            item.loose = Some(true);
            if let Some(tokens) = item.tokens.as_mut() {
                for token in tokens {
                    if token.kind == "text" {
                        token.kind = "paragraph".to_string();
                    }
                }
            }
        }
    }

    let mut token = Token::new("list", raw);
    token.ordered = Some(ordered);
    token.start = ordered.then_some(list_start);
    token.loose = Some(loose);
    token.items = Some(items);
    token
}

/// Parse a GFM table starting at `*index`.
fn parse_table(source: &str, lines: &[&str], spans: &[(usize, usize)], index: &mut usize) -> Token {
    let start = *index;
    let header_cells = split_table_row(lines[start]);
    *index += 2;

    let mut rows: Vec<Vec<TableCell>> = Vec::new();
    while *index < lines.len() {
        let line = lines[*index];
        if is_blank(line) || !line.contains('|') {
            break;
        }
        let mut cells: Vec<TableCell> = split_table_row(line)
            .into_iter()
            .take(header_cells.len())
            .map(|text| TableCell {
                tokens: inline_tokens(&text),
                text,
            })
            .collect();
        while cells.len() < header_cells.len() {
            cells.push(TableCell {
                text: String::new(),
                tokens: Vec::new(),
            });
        }
        rows.push(cells);
        *index += 1;
    }

    let raw = raw_span(source, spans, lines, start, *index);

    let mut token = Token::new("table", raw);
    token.header = Some(
        header_cells
            .into_iter()
            .map(|text| TableCell {
                tokens: inline_tokens(&text),
                text,
            })
            .collect(),
    );
    token.rows = Some(rows);
    token
}

// === LaTeX extension ===

/// Whether the index sits behind an odd number of backslashes.
fn is_escaped(source: &str, index: usize) -> bool {
    let mut backslashes = 0;
    let bytes = source.as_bytes();
    let mut position = index;
    while position > 0 && bytes[position - 1] == b'\\' {
        backslashes += 1;
        position -= 1;
    }
    backslashes % 2 == 1
}

fn find_closing_delimiter(source: &str, closing: &str, start: usize) -> Option<usize> {
    let mut index = source[start..].find(closing).map(|offset| start + offset);
    while let Some(found) = index {
        if !is_escaped(source, found) {
            return Some(found);
        }
        index = source[found + closing.len()..]
            .find(closing)
            .map(|offset| found + closing.len() + offset);
    }
    None
}

/// `looksLikePendingDollarMath`.
fn looks_like_pending_dollar_math(source: &str) -> bool {
    for (index, character) in source.char_indices() {
        if character == '\\'
            && source[index + 1..].starts_with(|next: char| next.is_ascii_alphabetic())
        {
            return true;
        }
        if "_^=+*/<>()[]|±≤≥≠≈∈→⇒∞∫∑√-".contains(character) {
            return true;
        }
    }
    false
}

/// `tokenizeBlockLatex`, applied at the start of a block.
fn latex_block(lines: &[&str], index: &mut usize) -> Option<Token> {
    let start = *index;
    let source = lines[start..].join("\n");
    let line = lines[start];
    if indent_width(line) > 3 {
        return None;
    }
    let trimmed = line.trim_start_matches(' ');
    let opening = if trimmed.starts_with("$$") {
        "$$"
    } else if trimmed.starts_with("\\[") {
        "\\["
    } else {
        return None;
    };
    let closing = if opening == "$$" { "$$" } else { "\\]" };

    let leading = line.len() - trimmed.len();
    let body_start = leading + opening.len();
    let after_opening = &source[body_start..];
    let content_start = body_start + after_opening.len()
        - after_opening
            .trim_start_matches([' ', '\t'])
            .trim_start_matches('\n')
            .len();

    match find_closing_delimiter(&source, closing, content_start) {
        Some(end) => {
            let text = source[content_start..end].trim().to_string();
            if text.is_empty() {
                return None;
            }
            let mut raw_end = end + closing.len();
            let tail = &source[raw_end..];
            let trailing = tail.len() - tail.trim_start_matches([' ', '\t']).len();
            raw_end += trailing;
            if source[raw_end..].starts_with('\n') {
                raw_end += 1;
            }
            let raw = source[..raw_end].to_string();
            *index += raw.matches('\n').count().max(1);
            if !raw.ends_with('\n') {
                *index = lines.len();
            }
            Some(Token::new("latexBlock", raw).with_text(text))
        }
        None => {
            let text = source[content_start..].to_string();
            if opening == "$$" && !looks_like_pending_dollar_math(&text) {
                return None;
            }
            if text.is_empty() && opening == "$$" {
                return None;
            }
            *index = lines.len();
            let mut token = Token::new("latexBlock", source.clone()).with_text(text);
            token.pending = Some(true);
            Some(token)
        }
    }
}

/// `tokenizeInlineLatex`.
fn latex_inline(source: &str) -> Option<Token> {
    let (opening, closing) = if source.starts_with("$$") {
        ("$$", "$$")
    } else if source.starts_with("\\(") {
        ("\\(", "\\)")
    } else if source.starts_with("\\[") {
        ("\\[", "\\]")
    } else if source.starts_with('$')
        && !source[1..].starts_with(|character: char| character.is_whitespace())
    {
        ("$", "$")
    } else {
        return None;
    };

    let closing_index = find_closing_delimiter(source, closing, opening.len());
    if let Some(closing_index) = closing_index
        && opening == "$"
    {
        let inner = &source[opening.len()..closing_index];
        let tail = &source[closing_index + 1..];
        let ends_with_space = inner.ends_with(char::is_whitespace);
        let digit_follows = tail.starts_with(|character: char| character.is_ascii_digit());
        let identifier_like = is_upper_identifier(inner)
            && tail
                .starts_with(|character: char| character.is_ascii_alphabetic() || character == '_');
        if ends_with_space || digit_follows || identifier_like || inner.contains('`') {
            return None;
        }
    }

    let Some(closing_index) = closing_index else {
        let pending_source = &source[opening.len()..];
        if opening.starts_with('\\') || looks_like_pending_dollar_math(pending_source) {
            let mut token = Token::new("latex", source).with_text(pending_source);
            token.pending = Some(true);
            return Some(token);
        }
        return None;
    };

    let text = &source[opening.len()..closing_index];
    if text.is_empty() || text.contains('\n') {
        return None;
    }
    Some(Token::new("latex", &source[..closing_index + closing.len()]).with_text(text))
}

/// `/^[A-Z_][A-Z0-9_]*(?:[^A-Za-z0-9_\s])?$/`.
fn is_upper_identifier(value: &str) -> bool {
    let mut characters = value.chars().peekable();
    let Some(first) = characters.next() else {
        return false;
    };
    if !(first.is_ascii_uppercase() || first == '_') {
        return false;
    }
    let rest: Vec<char> = characters.collect();
    let (body, tail) = match rest.last() {
        Some(last) if !(last.is_ascii_alphanumeric() || *last == '_' || last.is_whitespace()) => {
            (&rest[..rest.len() - 1], true)
        }
        _ => (&rest[..], false),
    };
    let _ = tail;
    body.iter().all(|character| {
        character.is_ascii_uppercase() || character.is_ascii_digit() || *character == '_'
    })
}

// === Inline tokenizer ===

/// Tokenize inline markdown.
pub fn inline_tokens(source: &str) -> Vec<Token> {
    let mut tokens: Vec<Token> = Vec::new();
    let mut text = String::new();
    let mut index = 0;

    while index < source.len() {
        let rest = &source[index..];

        if let Some(token) = latex_inline(rest) {
            push_text(&mut tokens, &mut text);
            index += token.raw.len();
            tokens.push(token);
            continue;
        }

        let character = rest.chars().next().expect("char boundary");

        if character == '\\'
            && let Some(next) = rest[1..].chars().next()
            && "\\!\"#$%&'()*+,-./:;<=>?@[]^_`{|}~".contains(next)
        {
            push_text(&mut tokens, &mut text);
            tokens.push(
                Token::new("escape", &rest[..1 + next.len_utf8()]).with_text(next.to_string()),
            );
            index += 1 + next.len_utf8();
            continue;
        }

        if character == '`' {
            let ticks = rest.chars().take_while(|c| *c == '`').count();
            if let Some(end) = find_code_span_end(rest, ticks) {
                push_text(&mut tokens, &mut text);
                let raw = &rest[..end + ticks];
                let mut content = &rest[ticks..end];
                // A single leading and trailing space is stripped.
                if content.len() > 2 && content.starts_with(' ') && content.ends_with(' ') {
                    content = &content[1..content.len() - 1];
                }
                tokens.push(Token::new("codespan", raw).with_text(content.replace('\n', " ")));
                index += raw.len();
                continue;
            }
        }

        if (character == '*' || (character == '_' && !is_link_boundary_char(source, index)))
            && let Some((token, length)) = emphasis(rest, character)
        {
            push_text(&mut tokens, &mut text);
            tokens.push(token);
            index += length;
            continue;
        }

        if character == '~'
            && let Some((token, length)) = strict_strikethrough(rest)
        {
            push_text(&mut tokens, &mut text);
            tokens.push(token);
            index += length;
            continue;
        }

        if character == '['
            && let Some((token, length)) = inline_link(rest)
        {
            push_text(&mut tokens, &mut text);
            tokens.push(token);
            index += length;
            continue;
        }

        // GFM autolinks fire at a token boundary.
        if (index == 0 || !is_link_boundary_char(source, index))
            && let Some((token, length)) = gfm_autolink(rest)
        {
            push_text(&mut tokens, &mut text);
            tokens.push(token);
            index += length;
            continue;
        }

        if character == '<'
            && let Some((token, length)) = autolink_or_html(rest)
        {
            push_text(&mut tokens, &mut text);
            tokens.push(token);
            index += length;
            continue;
        }

        if character == '\n' {
            // Two trailing spaces before a newline are a hard break.
            if text.ends_with("  ") {
                let trimmed = text.trim_end().to_string();
                text = trimmed;
                push_text(&mut tokens, &mut text);
                tokens.push(Token::new("br", "\n"));
                index += 1;
                continue;
            }
            text.push('\n');
            index += 1;
            continue;
        }

        text.push(character);
        index += character.len_utf8();
    }

    push_text(&mut tokens, &mut text);
    tokens
}

fn push_text(tokens: &mut Vec<Token>, text: &mut String) {
    if text.is_empty() {
        return;
    }
    let value = std::mem::take(text);
    tokens.push(Token::new("text", value.clone()).with_text(value));
}

/// Byte offset of the closing run of `ticks` backticks.
fn find_code_span_end(source: &str, ticks: usize) -> Option<usize> {
    let mut index = ticks;
    while index < source.len() {
        if source[index..].starts_with('`') {
            let run = source[index..].chars().take_while(|c| *c == '`').count();
            if run == ticks {
                return Some(index);
            }
            index += run;
            continue;
        }
        index += source[index..].chars().next().expect("char").len_utf8();
    }
    None
}

/// `strong`/`em` starting at the delimiter run.
fn emphasis(source: &str, marker: char) -> Option<(Token, usize)> {
    let run = source.chars().take_while(|c| *c == marker).count();
    if run == 0 {
        return None;
    }
    let (delimiter, kind) = if run >= 2 {
        (marker.to_string().repeat(2), "strong")
    } else {
        (marker.to_string(), "em")
    };
    let content_start = delimiter.len();
    if source[content_start..].starts_with(char::is_whitespace) {
        return None;
    }

    let mut search = content_start;
    while let Some(offset) = source[search..].find(&delimiter) {
        let end = search + offset;
        if is_escaped(source, end) {
            search = end + delimiter.len();
            continue;
        }
        let content = &source[content_start..end];
        if content.is_empty() || content.ends_with(char::is_whitespace) {
            search = end + delimiter.len();
            continue;
        }
        // `_` never closes inside a word.
        if marker == '_'
            && source[end + delimiter.len()..]
                .starts_with(|character: char| character.is_alphanumeric() || character == '_')
        {
            search = end + delimiter.len();
            continue;
        }
        let raw = &source[..end + delimiter.len()];
        let token = Token::new(kind, raw)
            .with_text(content)
            .with_tokens(inline_tokens(content));
        return Some((token, raw.len()));
    }
    None
}

/// `STRICT_STRIKETHROUGH_REGEX`.
fn strict_strikethrough(source: &str) -> Option<(Token, usize)> {
    if !source.starts_with("~~") {
        return None;
    }
    let content_start = 2;
    let first = source[content_start..].chars().next()?;
    if first.is_whitespace() || first == '~' {
        return None;
    }

    let mut search = content_start;
    while let Some(offset) = source[search..].find("~~") {
        let end = search + offset;
        if is_escaped(source, end) {
            search = end + 2;
            continue;
        }
        let content = &source[content_start..end];
        let last = content.chars().next_back();
        if content.is_empty()
            || last.is_some_and(|character| {
                character.is_whitespace() || character == '~' || character == '\\'
            })
        {
            search = end + 2;
            continue;
        }
        // `(?=[^~]|$)`: the closing run must not be followed by another tilde.
        if source[end + 2..].starts_with('~') {
            search = end + 2;
            continue;
        }
        let raw = &source[..end + 2];
        let token = Token::new("del", raw)
            .with_text(content)
            .with_tokens(inline_tokens(content));
        return Some((token, raw.len()));
    }
    None
}

/// `[text](href)`.
fn inline_link(source: &str) -> Option<(Token, usize)> {
    let mut depth = 0;
    let mut text_end = None;
    for (index, character) in source.char_indices() {
        match character {
            '[' if !is_escaped(source, index) => depth += 1,
            ']' if !is_escaped(source, index) => {
                depth -= 1;
                if depth == 0 {
                    text_end = Some(index);
                    break;
                }
            }
            _ => {}
        }
    }
    let text_end = text_end?;
    if !source[text_end + 1..].starts_with('(') {
        return None;
    }
    let href_start = text_end + 2;
    let href_end = href_start + source[href_start..].find(')')?;
    let text = &source[1..text_end];
    let href = source[href_start..href_end].trim();
    let href = href.strip_prefix('<').unwrap_or(href);
    let href = href.strip_suffix('>').unwrap_or(href);

    let raw = &source[..href_end + 1];
    let mut token = Token::new("link", raw)
        .with_text(text)
        .with_tokens(inline_tokens(text));
    token.href = Some(href.to_string());
    Some((token, raw.len()))
}

/// Whether the character before `index` prevents a GFM autolink.
fn is_link_boundary_char(source: &str, index: usize) -> bool {
    source[..index]
        .chars()
        .next_back()
        .is_some_and(|character| {
            character.is_alphanumeric() || character == '_' || character == '.'
        })
}

/// GFM autolink: a bare URL or e-mail address.
fn gfm_autolink(source: &str) -> Option<(Token, usize)> {
    let scheme_length = ["https://", "http://", "ftp://", "www."]
        .into_iter()
        .find(|scheme| source.starts_with(scheme))
        .map(str::len);

    if let Some(scheme_length) = scheme_length {
        let mut end = scheme_length;
        for (offset, character) in source[scheme_length..].char_indices() {
            if character.is_whitespace() || character == '<' {
                break;
            }
            end = scheme_length + offset + character.len_utf8();
        }
        // Trailing punctuation is not part of the link.
        while end > scheme_length {
            let last = source[..end].chars().next_back()?;
            if "?!.,:;*_~'\"".contains(last) {
                end -= last.len_utf8();
                continue;
            }
            if last == ')' {
                let opening = source[..end].matches('(').count();
                let closing = source[..end].matches(')').count();
                if closing > opening {
                    end -= 1;
                    continue;
                }
            }
            break;
        }
        if end <= scheme_length {
            return None;
        }
        let raw = &source[..end];
        let href = if raw.starts_with("www.") {
            format!("http://{raw}")
        } else {
            raw.to_string()
        };
        let mut token = Token::new("link", raw)
            .with_text(raw)
            .with_tokens(vec![Token::new("text", raw).with_text(raw)]);
        token.href = Some(href);
        return Some((token, raw.len()));
    }

    // `/^[A-Za-z0-9._+-]+(@)[a-zA-Z0-9-_]+(?:\.[a-zA-Z0-9-_]*[a-zA-Z0-9])+(?![-_])/`
    let local: String = source
        .chars()
        .take_while(|character| character.is_ascii_alphanumeric() || "._+-".contains(*character))
        .collect();
    if local.is_empty() || !source[local.len()..].starts_with('@') {
        return None;
    }
    let domain_source = &source[local.len() + 1..];
    let domain: String = domain_source
        .chars()
        .take_while(|character| character.is_ascii_alphanumeric() || "-_.".contains(*character))
        .collect();
    let domain = domain.trim_end_matches(['.', '-', '_']);
    if !domain.contains('.') || domain.is_empty() {
        return None;
    }
    let raw = &source[..local.len() + 1 + domain.len()];
    let mut token = Token::new("link", raw)
        .with_text(raw)
        .with_tokens(vec![Token::new("text", raw).with_text(raw)]);
    token.href = Some(format!("mailto:{raw}"));
    Some((token, raw.len()))
}

/// `<https://…>`, `<foo@bar>` or inline HTML.
fn autolink_or_html(source: &str) -> Option<(Token, usize)> {
    let end = source.find('>')?;
    let inner = &source[1..end];
    if inner.is_empty() || inner.contains(char::is_whitespace) {
        if inner.is_empty() {
            return None;
        }
        // Inline HTML tags keep their raw text.
        let raw = &source[..=end];
        return Some((Token::new("html", raw).with_text(raw), raw.len()));
    }

    let raw = &source[..=end];
    if inner.contains("://") {
        let mut token = Token::new("link", raw)
            .with_text(inner)
            .with_tokens(vec![Token::new("text", inner).with_text(inner)]);
        token.href = Some(inner.to_string());
        return Some((token, raw.len()));
    }
    if inner.contains('@') && !inner.starts_with('/') {
        let mut token = Token::new("link", raw)
            .with_text(inner)
            .with_tokens(vec![Token::new("text", inner).with_text(inner)]);
        token.href = Some(format!("mailto:{inner}"));
        return Some((token, raw.len()));
    }
    if inner.starts_with(|character: char| character.is_ascii_alphabetic())
        || inner.starts_with('/')
        || inner.starts_with('!')
    {
        return Some((Token::new("html", raw).with_text(raw), raw.len()));
    }
    None
}
