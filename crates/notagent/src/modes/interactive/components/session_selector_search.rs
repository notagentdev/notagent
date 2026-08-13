//! 1:1 port of
//! `packages/coding-agent/src/modes/interactive/components/session-selector-search.ts` (194 LOC).

use notagent_tui::fuzzy::fuzzy_match;
use regex::Regex;

use crate::core::session_manager::SessionInfo;

/// `SortMode` — how the filtered sessions are ordered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortMode {
    /// Threaded view; sorted by relevance like [`SortMode::Relevance`].
    Threaded,
    /// Keeps the incoming order.
    Recent,
    /// Sorted by score, ties broken by `modified` descending.
    Relevance,
}

/// `NameFilter` — restricts the list to named sessions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NameFilter {
    /// Every session.
    #[default]
    All,
    /// Only sessions with a non-blank name.
    Named,
}

/// One token of a parsed token-mode query.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchToken {
    /// Fuzzy tokens match through [`fuzzy_match`], phrases through a substring search.
    pub kind: TokenKind,
    /// The token text.
    pub value: String,
}

/// Token kind of [`SearchToken`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    /// Bare token.
    Fuzzy,
    /// Quoted token.
    Phrase,
}

/// `ParsedSearchQuery`
pub struct ParsedSearchQuery {
    /// `mode` — `tokens` or `regex`.
    pub mode: QueryMode,
    /// Tokens of a token-mode query.
    pub tokens: Vec<SearchToken>,
    /// Compiled pattern of a regex-mode query.
    pub regex: Option<Regex>,
    /// If set, parsing failed and we should treat query as non-matching.
    pub error: Option<String>,
}

/// Query mode of [`ParsedSearchQuery`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueryMode {
    /// Whitespace/quote separated tokens.
    Tokens,
    /// `re:<pattern>`.
    Regex,
}

/// `MatchResult`
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MatchResult {
    /// Whether the session matched.
    pub matches: bool,
    /// Lower is better; only meaningful when matches === true
    pub score: f64,
}

fn normalize_whitespace_lower(text: &str) -> String {
    // `text.toLowerCase().replace(/\s+/g, " ").trim()`.
    let lowered = text.to_lowercase();
    let mut normalized = String::with_capacity(lowered.len());
    let mut in_whitespace = false;
    for character in lowered.chars() {
        if is_js_whitespace(character) {
            in_whitespace = true;
            continue;
        }
        if in_whitespace && !normalized.is_empty() {
            normalized.push(' ');
        }
        in_whitespace = false;
        normalized.push(character);
    }
    normalized
}

/// The character class of the JavaScript `\s` escape.
fn is_js_whitespace(character: char) -> bool {
    matches!(
        character,
        '\t' | '\n' | '\u{0b}' | '\u{0c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
                | '\u{feff}'
    )
}

/// `String.prototype.trim()` — trims the JavaScript `\s` class plus U+FEFF,
/// which differs slightly from Rust's `char::is_whitespace`.
fn js_trim(text: &str) -> &str {
    text.trim_matches(is_js_whitespace)
}

fn get_session_search_text(session: &SessionInfo) -> String {
    format!(
        "{} {} {} {}",
        session.id,
        session.name.as_deref().unwrap_or(""),
        session.all_messages_text,
        session.cwd
    )
}

/// Whether the session carries a non-blank name.
pub fn has_session_name(session: &SessionInfo) -> bool {
    session
        .name
        .as_ref()
        .is_some_and(|name| !js_trim(name).is_empty())
}

fn matches_name_filter(session: &SessionInfo, filter: NameFilter) -> bool {
    if filter == NameFilter::All {
        return true;
    }
    has_session_name(session)
}

/// Parse the search box content into [`ParsedSearchQuery`].
pub fn parse_search_query(query: &str) -> ParsedSearchQuery {
    let trimmed = js_trim(query);
    if trimmed.is_empty() {
        return ParsedSearchQuery {
            mode: QueryMode::Tokens,
            tokens: Vec::new(),
            regex: None,
            error: None,
        };
    }

    // Regex mode: re:<pattern>
    if let Some(rest) = trimmed.strip_prefix("re:") {
        let pattern = js_trim(rest);
        if pattern.is_empty() {
            return ParsedSearchQuery {
                mode: QueryMode::Regex,
                tokens: Vec::new(),
                regex: None,
                error: Some("Empty regex".to_string()),
            };
        }
        // `new RegExp(pattern, "i")` — the `regex` crate is the master plan's
        // substitution for the JS engine (deviation class 3). Patterns it
        // rejects take the same path as a JS `SyntaxError`.
        return match Regex::new(&format!("(?i){pattern}")) {
            Ok(regex) => ParsedSearchQuery {
                mode: QueryMode::Regex,
                tokens: Vec::new(),
                regex: Some(regex),
                error: None,
            },
            Err(err) => ParsedSearchQuery {
                mode: QueryMode::Regex,
                tokens: Vec::new(),
                regex: None,
                error: Some(err.to_string()),
            },
        };
    }

    // Token mode with quote support.
    // Example: foo "node cve" bar
    let mut tokens: Vec<SearchToken> = Vec::new();
    let mut buf = String::new();
    let mut in_quote = false;
    let mut had_unclosed_quote = false;

    let mut flush = |kind: TokenKind, buf: &mut String| {
        let value = js_trim(buf).to_string();
        buf.clear();
        if value.is_empty() {
            return;
        }
        tokens.push(SearchToken { kind, value });
    };

    for character in trimmed.chars() {
        if character == '"' {
            if in_quote {
                flush(TokenKind::Phrase, &mut buf);
                in_quote = false;
            } else {
                flush(TokenKind::Fuzzy, &mut buf);
                in_quote = true;
            }
            continue;
        }

        if !in_quote && is_js_whitespace(character) {
            flush(TokenKind::Fuzzy, &mut buf);
            continue;
        }

        buf.push(character);
    }

    if in_quote {
        had_unclosed_quote = true;
    }

    // If quotes were unbalanced, fall back to plain whitespace tokenization.
    if had_unclosed_quote {
        return ParsedSearchQuery {
            mode: QueryMode::Tokens,
            tokens: trimmed
                .split(is_js_whitespace)
                .map(js_trim)
                .filter(|token| !token.is_empty())
                .map(|token| SearchToken {
                    kind: TokenKind::Fuzzy,
                    value: token.to_string(),
                })
                .collect(),
            regex: None,
            error: None,
        };
    }

    flush(
        if in_quote {
            TokenKind::Phrase
        } else {
            TokenKind::Fuzzy
        },
        &mut buf,
    );

    ParsedSearchQuery {
        mode: QueryMode::Tokens,
        tokens,
        regex: None,
        error: None,
    }
}

/// Match one session against a parsed query.
pub fn match_session(session: &SessionInfo, parsed: &ParsedSearchQuery) -> MatchResult {
    let text = get_session_search_text(session);

    if parsed.mode == QueryMode::Regex {
        let Some(regex) = parsed.regex.as_ref() else {
            return MatchResult {
                matches: false,
                score: 0.0,
            };
        };
        let Some(found) = regex.find(&text) else {
            return MatchResult {
                matches: false,
                score: 0.0,
            };
        };
        return MatchResult {
            matches: true,
            score: utf16_index(&text, found.start()) as f64 * 0.1,
        };
    }

    if parsed.tokens.is_empty() {
        return MatchResult {
            matches: true,
            score: 0.0,
        };
    }

    let mut total_score = 0.0;
    let mut normalized_text: Option<String> = None;

    for token in &parsed.tokens {
        if token.kind == TokenKind::Phrase {
            let normalized =
                normalized_text.get_or_insert_with(|| normalize_whitespace_lower(&text));
            let phrase = normalize_whitespace_lower(&token.value);
            if phrase.is_empty() {
                continue;
            }
            let Some(index) = normalized.find(&phrase) else {
                return MatchResult {
                    matches: false,
                    score: 0.0,
                };
            };
            total_score += utf16_index(normalized, index) as f64 * 0.1;
            continue;
        }

        let matched = fuzzy_match(&token.value, &text);
        if !matched.matches {
            return MatchResult {
                matches: false,
                score: 0.0,
            };
        }
        total_score += matched.score;
    }

    MatchResult {
        matches: true,
        score: total_score,
    }
}

/// JS string indices count UTF-16 code units; the score is derived from them.
fn utf16_index(text: &str, byte_offset: usize) -> usize {
    text[..byte_offset].chars().map(char::len_utf16).sum()
}

/// Apply the name filter, the search query and the sort mode.
pub fn filter_and_sort_sessions(
    sessions: &[SessionInfo],
    query: &str,
    sort_mode: SortMode,
    name_filter: NameFilter,
) -> Vec<SessionInfo> {
    let name_filtered: Vec<SessionInfo> = if name_filter == NameFilter::All {
        sessions.to_vec()
    } else {
        sessions
            .iter()
            .filter(|session| matches_name_filter(session, name_filter))
            .cloned()
            .collect()
    };
    let trimmed = js_trim(query);
    if trimmed.is_empty() {
        return name_filtered;
    }

    let parsed = parse_search_query(query);
    if parsed.error.is_some() {
        return Vec::new();
    }

    // Recent mode: filter only, keep incoming order.
    if sort_mode == SortMode::Recent {
        let mut filtered: Vec<SessionInfo> = Vec::new();
        for session in name_filtered {
            if match_session(&session, &parsed).matches {
                filtered.push(session);
            }
        }
        return filtered;
    }

    // Relevance mode: sort by score, tie-break by modified desc.
    let mut scored: Vec<(SessionInfo, f64)> = Vec::new();
    for session in name_filtered {
        let result = match_session(&session, &parsed);
        if !result.matches {
            continue;
        }
        scored.push((session, result.score));
    }

    scored.sort_by(|a, b| {
        if a.1 != b.1 {
            return a.1.total_cmp(&b.1);
        }
        b.0.modified.cmp(&a.0.modified)
    });

    scored.into_iter().map(|entry| entry.0).collect()
}
