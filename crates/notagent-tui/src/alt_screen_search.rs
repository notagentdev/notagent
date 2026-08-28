use crate::components::input::Input;
use crate::tui::{Component, Focusable, Line};
use crate::utils::{graphemes, strip_terminal_sequences, truncate_to_width_opts, visible_width};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SearchSourceSpan {
    row: usize,
    start_col: usize,
    end_col: usize,
}

/// One contiguous run of a match inside a single row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AltScreenSearchSegment {
    /// Row of the segment.
    pub row: usize,
    /// First column (inclusive).
    pub start_col: usize,
    /// Last column (exclusive).
    pub end_col: usize,
}

/// A match, possibly spanning several rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AltScreenSearchMatch {
    /// Segments the match covers.
    pub segments: Vec<AltScreenSearchSegment>,
}

#[derive(Default)]
struct SearchCorpus {
    /// per character here, which keeps corpus and source aligned).
    text: Vec<char>,
    source: Vec<Option<SearchSourceSpan>>,
}

impl SearchCorpus {
    fn append(&mut self, text: &str, span: Option<SearchSourceSpan>) {
        for character in text.chars() {
            self.text.push(character);
            self.source.push(span);
        }
    }
}

fn build_search_corpus(lines: &[Line]) -> SearchCorpus {
    let mut corpus = SearchCorpus::default();
    let mut pending_separator = false;

    for (row, line) in lines.iter().enumerate() {
        let stripped = strip_terminal_sequences(line);
        let mut column = 0;
        for grapheme in graphemes(&stripped) {
            let width = visible_width(grapheme);
            if !grapheme.is_empty() && grapheme.chars().all(char::is_whitespace) {
                if !corpus.text.is_empty() {
                    pending_separator = true;
                }
                column += width;
                continue;
            }
            if pending_separator {
                corpus.append(" ", None);
                pending_separator = false;
            }
            corpus.append(
                grapheme,
                Some(SearchSourceSpan {
                    row,
                    start_col: column,
                    end_col: column + width,
                }),
            );
            column += width;
        }
        if !corpus.text.is_empty() {
            pending_separator = true;
        }
    }

    corpus
}

fn normalize_query(query: &str) -> String {
    let mut normalized = String::new();
    let mut in_space = false;
    for character in query.chars() {
        if character.is_whitespace() {
            if !in_space {
                normalized.push(' ');
                in_space = true;
            }
        } else {
            in_space = false;
            normalized.push(character);
        }
    }
    normalized.trim().to_string()
}

/// Find all case-insensitive matches of `query` in the rendered `lines`.
pub fn find_alt_screen_search_matches(lines: &[Line], query: &str) -> Vec<AltScreenSearchMatch> {
    let normalized_query = normalize_query(query);
    if normalized_query.is_empty() {
        return Vec::new();
    }

    let corpus = build_search_corpus(lines);
    let needle: Vec<char> = normalized_query
        .chars()
        .flat_map(char::to_lowercase)
        .collect();
    let haystack: Vec<char> = corpus
        .text
        .iter()
        .flat_map(|character| character.to_lowercase())
        .collect();
    // Lowercasing can change the character count; fall back to a per-character
    // comparison so corpus indices stay aligned.
    let haystack = if haystack.len() == corpus.text.len() {
        haystack
    } else {
        corpus.text.clone()
    };

    let mut matches = Vec::new();
    if needle.is_empty() || needle.len() > haystack.len() {
        return matches;
    }
    let mut start = 0;
    while start + needle.len() <= haystack.len() {
        let window = &haystack[start..start + needle.len()];
        let hit = window
            .iter()
            .zip(needle.iter())
            .all(|(left, right)| left.eq_ignore_ascii_case(right) || *left == *right);
        if !hit {
            start += 1;
            continue;
        }
        let end = start + needle.len();
        let mut segments: Vec<AltScreenSearchSegment> = Vec::new();
        for index in start..end {
            let Some(span) = corpus.source.get(index).copied().flatten() else {
                continue;
            };
            match segments.last_mut() {
                Some(previous)
                    if previous.row == span.row && span.start_col <= previous.end_col =>
                {
                    previous.end_col = previous.end_col.max(span.end_col);
                }
                _ => segments.push(AltScreenSearchSegment {
                    row: span.row,
                    start_col: span.start_col,
                    end_col: span.end_col,
                }),
            }
        }
        if !segments.is_empty() {
            matches.push(AltScreenSearchMatch { segments });
        }
        start += 1;
    }

    matches
}

/// Stable key of a match, used to keep the selection across re-renders.
pub fn get_alt_screen_search_match_key(search_match: &AltScreenSearchMatch) -> String {
    let (Some(first), Some(last)) = (search_match.segments.first(), search_match.segments.last())
    else {
        return String::new();
    };
    format!(
        "{}:{}:{}:{}",
        first.row, first.start_col, last.row, last.end_col
    )
}

/// Overlay component of the transcript search.
pub struct AltScreenSearchComponent {
    input: Input,
    query_changed: bool,
    result_count: usize,
    result_index: i64,
    focused: bool,
}

impl Default for AltScreenSearchComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl AltScreenSearchComponent {
    /// New search component with an empty query.
    pub fn new() -> Self {
        Self {
            input: Input::new(),
            query_changed: false,
            result_count: 0,
            result_index: -1,
            focused: false,
        }
    }

    /// Current query.
    pub fn query(&self) -> &str {
        self.input.get_value()
    }

    /// Whether the query changed since the last check.
    /// instead because the callback would need `&mut` access to the renderer.
    pub fn take_query_changed(&mut self) -> bool {
        std::mem::take(&mut self.query_changed)
    }

    /// Store the current result position.
    pub fn set_result(&mut self, index: i64, count: usize) {
        self.result_index = index;
        self.result_count = count;
    }
}

impl Component for AltScreenSearchComponent {
    fn render(&mut self, width: usize) -> Vec<Line> {
        let safe_width = width.max(1);
        let label = " Find transcript";
        let query = self.input.get_value().to_string();
        let status = if query.is_empty() {
            String::new()
        } else if self.result_count == 0 {
            "No matches ".to_string()
        } else {
            format!("{}/{} ", self.result_index + 1, self.result_count)
        };
        let label_width = visible_width(label);
        let status_width = visible_width(&status);
        let gap = " ".repeat(safe_width.saturating_sub(label_width + status_width).max(1));
        let title = truncate_to_width_opts(&format!("{label}{gap}{status}"), safe_width, "", false);
        let padding = " ".repeat(safe_width.saturating_sub(visible_width(&title)));
        let mut lines = vec![Line::from(format!("\x1b[7m{title}{padding}\x1b[27m"))];
        lines.extend(self.input.render(safe_width));
        lines
    }

    fn handle_input(&mut self, data: &str) {
        let previous = self.input.get_value().to_string();
        self.input.handle_input(data);
        if self.input.get_value() != previous {
            self.query_changed = true;
        }
    }

    fn invalidate(&mut self) {
        self.input.invalidate();
    }

    fn as_focusable(&mut self) -> Option<&mut dyn Focusable> {
        Some(self)
    }
}

impl Focusable for AltScreenSearchComponent {
    fn focused(&self) -> bool {
        self.focused
    }

    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
        if let Some(focusable) = self.input.as_focusable() {
            focusable.set_focused(focused);
        }
    }
}
