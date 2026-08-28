use regex::{Captures, Regex};
use std::sync::LazyLock;

/// `"(?:\\.|[^"\\])*"|\/\/[^\n]*`
static COMMENTS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""(?:\\.|[^"\\])*"|//[^\n]*"#).expect("comment pattern"));

/// `"(?:\\.|[^"\\])*"|,(\s*[}\]])`
static TRAILING_COMMAS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""(?:\\.|[^"\\])*"|,(\s*[}\]])"#).expect("comma pattern"));

/// Strip `//` line comments and trailing commas from JSON, leaving string literals untouched.
pub fn strip_json_comments(input: &str) -> String {
    let without_comments = COMMENTS.replace_all(input, |captures: &Captures<'_>| {
        let matched = &captures[0];
        if matched.starts_with('"') {
            matched.to_owned()
        } else {
            String::new()
        }
    });
    TRAILING_COMMAS
        .replace_all(&without_comments, |captures: &Captures<'_>| {
            match captures.get(1) {
                // `,(\s*[}\]])` — the comma is dropped, the tail kept.
                Some(tail) => tail.as_str().to_owned(),
                None => captures[0].to_owned(),
            }
        })
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_line_comments_outside_strings() {
        assert_eq!(
            strip_json_comments("{\n  // note\n  \"a\": 1 // tail\n}"),
            "{\n  \n  \"a\": 1 \n}"
        );
    }

    #[test]
    fn keeps_comment_like_content_inside_strings() {
        assert_eq!(
            strip_json_comments(r#"{"url": "https://x.dev//y"}"#),
            r#"{"url": "https://x.dev//y"}"#
        );
    }

    #[test]
    fn strips_trailing_commas() {
        assert_eq!(strip_json_comments("{\"a\": [1, 2,],}"), "{\"a\": [1, 2]}");
    }

    #[test]
    fn keeps_escaped_quotes_intact() {
        assert_eq!(
            strip_json_comments(r#"{"a": "x\"// y"}"#),
            r#"{"a": "x\"// y"}"#
        );
    }
}
