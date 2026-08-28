/// Result of [`fuzzy_match`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FuzzyMatch {
    /// Whether the query matched.
    pub matches: bool,
    /// Match quality; lower is better.
    pub score: f64,
}

fn is_word_boundary_char(c: char) -> bool {
    c.is_whitespace() || matches!(c, '-' | '_' | '.' | '/' | ':')
}

fn match_query(normalized_query: &str, text_lower: &[char]) -> FuzzyMatch {
    let query: Vec<char> = normalized_query.chars().collect();
    if query.is_empty() {
        return FuzzyMatch {
            matches: true,
            score: 0.0,
        };
    }
    if query.len() > text_lower.len() {
        return FuzzyMatch {
            matches: false,
            score: 0.0,
        };
    }

    let mut query_index = 0;
    let mut score = 0.0;
    let mut last_match_index: i64 = -1;
    let mut consecutive_matches = 0.0;

    for (index, character) in text_lower.iter().enumerate() {
        if query_index >= query.len() {
            break;
        }
        if *character != query[query_index] {
            continue;
        }
        let is_word_boundary = index == 0 || is_word_boundary_char(text_lower[index - 1]);

        // Reward consecutive matches.
        if last_match_index == index as i64 - 1 {
            consecutive_matches += 1.0;
            score -= consecutive_matches * 5.0;
        } else {
            consecutive_matches = 0.0;
            // Penalize gaps.
            if last_match_index >= 0 {
                score += (index as f64 - last_match_index as f64 - 1.0) * 2.0;
            }
        }

        // Reward word boundary matches.
        if is_word_boundary {
            score -= 10.0;
        }

        // Slight penalty for later matches.
        score += index as f64 * 0.1;

        last_match_index = index as i64;
        query_index += 1;
    }

    if query_index < query.len() {
        return FuzzyMatch {
            matches: false,
            score: 0.0,
        };
    }

    if query.as_slice() == text_lower {
        score -= 100.0;
    }

    FuzzyMatch {
        matches: true,
        score,
    }
}

/// Fuzzy match `query` against `text`.
pub fn fuzzy_match(query: &str, text: &str) -> FuzzyMatch {
    let query_lower = query.to_lowercase();
    let text_lower: Vec<char> = text.to_lowercase().chars().collect();

    let primary_match = match_query(&query_lower, &text_lower);
    if primary_match.matches {
        return primary_match;
    }

    // Retry with swapped letter/digit halves (`abc12` <-> `12abc`).
    let swapped_query = swap_alpha_numeric(&query_lower);
    let Some(swapped_query) = swapped_query else {
        return primary_match;
    };
    let swapped_match = match_query(&swapped_query, &text_lower);
    if !swapped_match.matches {
        return primary_match;
    }
    FuzzyMatch {
        matches: true,
        score: swapped_match.score + 5.0,
    }
}

/// `^([a-z]+)([0-9]+)$` or `^([0-9]+)([a-z]+)$` with the two groups swapped.
fn swap_alpha_numeric(query: &str) -> Option<String> {
    let chars: Vec<char> = query.chars().collect();
    if chars.is_empty() {
        return None;
    }
    let letters_first = chars[0].is_ascii_lowercase();
    let split = chars.iter().position(|c| {
        if letters_first {
            c.is_ascii_digit()
        } else {
            c.is_ascii_lowercase()
        }
    })?;
    let (head, tail) = chars.split_at(split);
    if head.is_empty() || tail.is_empty() {
        return None;
    }
    let head_ok = head.iter().all(|c| {
        if letters_first {
            c.is_ascii_lowercase()
        } else {
            c.is_ascii_digit()
        }
    });
    let tail_ok = tail.iter().all(|c| {
        if letters_first {
            c.is_ascii_digit()
        } else {
            c.is_ascii_lowercase()
        }
    });
    if !head_ok || !tail_ok {
        return None;
    }
    Some(tail.iter().chain(head.iter()).collect())
}

/// Filter and sort items by match quality (best first).
/// Whitespace- and slash-separated tokens all have to match.
pub fn fuzzy_filter<T: Clone>(items: &[T], query: &str, get_text: impl Fn(&T) -> String) -> Vec<T> {
    if query.trim().is_empty() {
        return items.to_vec();
    }
    let tokens: Vec<&str> = query
        .trim()
        .split(|c: char| c.is_whitespace() || c == '/')
        .filter(|token| !token.is_empty())
        .collect();
    if tokens.is_empty() {
        return items.to_vec();
    }

    let mut results: Vec<(usize, T, f64)> = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let text = get_text(item);
        let mut total_score = 0.0;
        let mut all_match = true;
        for token in &tokens {
            let candidate = fuzzy_match(token, &text);
            if candidate.matches {
                total_score += candidate.score;
            } else {
                all_match = false;
                break;
            }
        }
        if all_match {
            results.push((index, item.clone(), total_score));
        }
    }

    // `Array#sort` is stable, so equal scores keep their original order.
    results.sort_by(|a, b| {
        a.2.partial_cmp(&b.2)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.cmp(&b.0))
    });
    results.into_iter().map(|(_, item, _)| item).collect()
}
