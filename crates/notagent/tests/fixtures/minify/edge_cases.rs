//! Edge-case bench for the minified file-editing tools.
//! This file is deliberately packed with constructs that stress
//! `read_minified`, `patch_minified` and `multi_patch_minified`: doc/line/
//! block/inline comments, comments at every nesting depth, multi-line and raw
//! string literals, strings that look like code, macros, match arms,
//! attributes, generics and deeply nested closures.
//! It is valid Rust and compiles as a library:
//!     rustc --crate-type lib --edition 2021 edge_cases.rs
//! Drive edits with PROMPT.md, then re-run the compile and diff to confirm the
//! tools kept the 4-space indentation, preserved comments and left string
//! literals byte-identical.

use std::collections::HashMap;

/// Severity levels for log records. (doc comment — EDIT TARGET A)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Informational, low priority.
    Info,
    /// Something went wrong.
    Error,
}

/// A parsed configuration.
#[derive(Debug, Clone, Default)]
pub struct Config {
    /// Human-readable name.
    pub name: String,
    /// Retry budget; `0` disables retries. (doc comment — EDIT TARGET B)
    pub retries: u32,
    pub tags: Vec<String>,
}

impl Config {
    /// Creates a config with default retries.
    pub fn new(name: &str) -> Self {
        // build the struct with defaults (top-level fn comment)
        Self {
            name: name.to_string(),
            retries: 3, // trailing inline comment on a field — EDIT TARGET C
            tags: Vec::new(),
        }
    }

    /// Adds a tag and returns self for chaining.
    pub fn with_tag(mut self, tag: &str) -> Self {
        self.tags.push(tag.to_string());
        self
    }
}

/// Nested closures + method chains + comments at several depths.
pub fn normalize(input: &str) -> Result<String, String> {
    let cleaned = input
        .trim()
        .strip_prefix("data:")
        .map(|rest| {
            // comment at depth 3, inside a closure inside a method chain
            // second line of the same nested comment block — EDIT TARGET D
            rest.replace(' ', "_")
        })
        .unwrap_or_else(|| {
            // fallback branch: no prefix present
            String::new()
        });

    if cleaned.is_empty() {
        // guard against empty results — EDIT TARGET E (comment-only)
        return Err("empty input".to_string());
    }

    Ok(cleaned)
}

/// match arms with per-arm comments.
pub fn label(severity: Severity) -> &'static str {
    match severity {
        // informational records are quiet
        Severity::Info => "info",
        Severity::Error => "error", // loud — EDIT TARGET F
    }
}

/// Multi-line and raw string literals must stay byte-identical through edits.
pub fn banner() -> String {
    let raw = r#"
        line one of a raw string  // this is NOT a comment, do not touch
        line two with { braces } and ; semicolons inside the string
    "#;
    let normal = "first line\n    second line indented inside the string\nthird line";
    format!("{raw}\n{normal}")
}

/// Strings that look like code/comments, plus macros.
pub fn tricky_strings() -> Vec<String> {
    // the strings below contain // and {} and ; and must survive verbatim
    vec![
        "// looks like a comment but is a string".to_string(),
        "fn fake() { return 0; }".to_string(),
        format!("interpolated value is {}", 1 + 1),
    ]
}

/// Deeply nested control flow with comments at each level.
pub fn deep(matrix: &[Vec<i32>]) -> i32 {
    let mut total = 0;
    for row in matrix {
        // iterate each row of the matrix
        for &value in row {
            if value > 0 {
                // only positive values contribute — EDIT TARGET G
                total += value;
            } else {
                // skip non-positive values
                continue;
            }
        }
    }
    total
}

/* A single-line block comment above a function. */
pub fn block_comments() -> u32 {
    /*
     * A multi-line block comment.
     * Second line of the block comment.
     */
    let x = 41; // inline comment after code
    x + 1
}

/// Generics, a where-clause and an inner comment.
pub fn collect_pairs<K, V>(items: Vec<(K, V)>) -> HashMap<K, V>
where
    K: std::hash::Hash + Eq,
{
    // build the map from the provided pairs
    items.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke() {
        // unicode in a comment: café — façade — 日本語 — EDIT TARGET H
        let cfg = Config::new("café").with_tag("ünïcödé");
        assert_eq!(cfg.retries, 3);
        assert_eq!(label(Severity::Info), "info");
        assert_eq!(deep(&[vec![1, -2, 3]]), 4);
    }
}
