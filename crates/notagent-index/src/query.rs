//! Query interface for symbol search

use crate::index::SymbolIndex;
use crate::symbol::Symbol;
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use std::sync::Arc;

/// Search result with relevance score
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub symbol: Arc<Symbol>,
    pub score: f64,
}

impl SearchResult {
    pub fn new(symbol: Arc<Symbol>, score: f64) -> Self {
        Self { symbol, score }
    }
}

/// Query builder for symbol search
pub struct Query<'a> {
    index: &'a SymbolIndex,
    query: String,
    max_results: usize,
    min_score: f64,
}

impl<'a> Query<'a> {
    /// Create a new query
    pub fn new(index: &'a SymbolIndex, query: impl Into<String>) -> Self {
        Self {
            index,
            query: query.into(),
            max_results: 50,
            min_score: 0.3,
        }
    }

    /// Set maximum number of results
    pub fn limit(mut self, max: usize) -> Self {
        self.max_results = max;
        self
    }

    /// Set minimum similarity score (0.0 - 1.0)
    pub fn min_score(mut self, score: f64) -> Self {
        self.min_score = score;
        self
    }

    /// Execute the search
    pub fn search(&self) -> Vec<SearchResult> {
        let query_lower = self.query.to_lowercase();
        let symbols = self.index.all_symbols();

        // Create nucleo matcher and pattern for fuzzy matching
        let mut matcher = Matcher::new(Config::DEFAULT);
        let pattern = Pattern::parse(&query_lower, CaseMatching::Ignore, Normalization::Smart);

        let mut results: Vec<SearchResult> = symbols
            .into_iter()
            .filter_map(|sym| {
                let name_lower = sym.name.to_lowercase();

                // Exact match gets highest score
                if name_lower == query_lower {
                    return Some(SearchResult::new(sym, 1.0));
                }

                // Prefix match gets high score
                if name_lower.starts_with(&query_lower) {
                    let score = 0.95 * (query_lower.len() as f64 / name_lower.len() as f64);
                    return Some(SearchResult::new(sym, score.max(0.5)));
                }

                // Contains match
                if name_lower.contains(&query_lower) {
                    let score = 0.8 * (query_lower.len() as f64 / name_lower.len() as f64);
                    return Some(SearchResult::new(sym, score.max(0.4)));
                }

                // Fuzzy match using nucleo (non-contiguous, like fzf)
                let mut buf = Vec::new();
                let haystack = Utf32Str::new(&name_lower, &mut buf);
                if let Some(raw_score) = pattern.score(haystack, &mut matcher) {
                    // Nucleo scores are u32 (higher = better), normalize to 0.0-1.0
                    // Typical scores range from 0 to ~300 for good matches
                    let normalized = (raw_score as f64 / 300.0).min(0.7);
                    if normalized >= self.min_score {
                        return Some(SearchResult::new(sym, normalized));
                    }
                }

                None
            })
            .collect();

        // Sort by score descending
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Limit results
        results.truncate(self.max_results);

        results
    }
}

/// Extension trait for SymbolIndex to add search capabilities
impl SymbolIndex {
    /// Search for symbols matching a query
    pub fn search(&self, query: impl Into<String>) -> Query<'_> {
        Query::new(self, query)
    }

    /// Quick search with default settings
    pub fn find(&self, query: &str) -> Vec<SearchResult> {
        self.search(query).search()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_fuzzy_search() {
        let index = SymbolIndex::new();
        let source = r#"
fn hello_world() {}
fn hello_there() {}
fn goodbye() {}
pub struct HelloStruct {}
"#;
        index.index_file(Path::new("test.rs"), source).unwrap();

        // Search for "hello"
        let results = index.find("hello");
        assert!(!results.is_empty());

        // First results should be hello-related
        assert!(results[0].symbol.name.to_lowercase().contains("hello"));
    }

    #[test]
    fn test_exact_match_priority() {
        let index = SymbolIndex::new();
        let source = r#"
fn test() {}
fn test_something() {}
fn testing() {}
"#;
        index.index_file(Path::new("test.rs"), source).unwrap();

        let results = index.find("test");
        assert!(!results.is_empty());

        // Exact match should be first
        assert_eq!(results[0].symbol.name, "test");
        assert_eq!(results[0].score, 1.0);
    }

    #[test]
    fn test_non_contiguous_fuzzy_match() {
        let index = SymbolIndex::new();
        let source = r#"
fn read_file_tool() {}
fn write_file_tool() {}
fn execute_command() {}
pub struct ReadFileTool {}
"#;
        index.index_file(Path::new("test.rs"), source).unwrap();

        // Non-contiguous match: "readtool" should match via nucleo
        // Use a more realistic query and lower threshold for short patterns
        let results = index.search("readtool").min_score(0.1).search();
        assert!(
            !results.is_empty(),
            "readtool should match read-related symbols"
        );

        // Should find read_file_tool or ReadFileTool
        let found = results
            .iter()
            .any(|r| r.symbol.name.to_lowercase().contains("read"));
        assert!(found, "Expected to find read-related symbol");
    }
}
