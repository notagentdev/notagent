//! High-level codebase search (`cb_search`) over the BM25 + symbol indexes.
//! BM25 chunk search runs first
//! with adaptive score thresholds and symbol/kind boosting, falling back to
//! fuzzy symbol search when BM25 yields nothing. The agent-tool wiring
//! (schema, catalog entry) lives in the tool layer, not here.

use serde::Serialize;

use crate::watcher::IndexManager;

/// Parameters for a codebase search.
#[derive(Debug, Clone)]
pub struct CbSearchParams {
    /// Search query: concrete keywords, symbol names, or short phrases.
    pub query: String,
    /// Optional path substring filter (relative to workspace).
    pub path: Option<String>,
    /// Maximum number of results.
    pub max_results: usize,
    /// Return only the best match per file.
    pub aggregate_by_file: bool,
}

impl CbSearchParams {
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            path: None,
            max_results: 20,
            aggregate_by_file: false,
        }
    }
}

/// Search result item.
#[derive(Debug, Clone, Serialize)]
pub struct CbSearchResult {
    pub name: String,
    pub kind: String,
    pub file: String,
    pub line: usize,
    pub score: f64,
    pub snippet: Option<String>,
}

/// Performs a ranked codebase search: BM25 chunk search first, fuzzy symbol
/// search as fallback.
pub fn cb_search(
    manager: &IndexManager,
    params: &CbSearchParams,
) -> anyhow::Result<Vec<CbSearchResult>> {
    // Perform BM25 search first (if available)
    if let Some(bm25_results) = manager.bm25_search(&params.query, params.max_results)?
        && !bm25_results.is_empty()
    {
        let min_score = adaptive_min_score(&params.query);
        let mut results = rank_bm25_results(&bm25_results, params, min_score);

        // Retry with a permissive threshold when the adaptive one filtered
        // everything out.
        if results.is_empty() && min_score > 0.05 {
            results = rank_bm25_results(&bm25_results, params, 0.05);
        }

        tracing::info!(
            "BM25 codebase search '{}': {} results",
            params.query,
            results.len()
        );
        return Ok(results);
    }

    // Fallback to symbol search
    let index = manager.index();
    let search_results = index
        .search(&params.query)
        .limit(params.max_results)
        .search();

    let results: Vec<CbSearchResult> = search_results
        .into_iter()
        .filter(|r| {
            if let Some(ref path_filter) = params.path {
                r.symbol.file_path.to_string_lossy().contains(path_filter)
            } else {
                true
            }
        })
        .map(|r| CbSearchResult {
            name: r.symbol.name.clone(),
            kind: r.symbol.kind.display_name().to_string(),
            file: r.symbol.file_path.to_string_lossy().to_string(),
            line: r.symbol.line_range.start + 1,
            score: r.score,
            snippet: None,
        })
        .collect();

    let results = if params.aggregate_by_file {
        aggregate_results_by_file(results)
    } else {
        results
    };

    tracing::info!(
        "Codebase search '{}': {} results",
        params.query,
        results.len()
    );

    Ok(results)
}

fn rank_bm25_results(
    bm25_results: &[crate::Bm25Hit],
    params: &CbSearchParams,
    min_score: f32,
) -> Vec<CbSearchResult> {
    let mut results: Vec<CbSearchResult> = bm25_results
        .iter()
        .filter(|r| r.score >= min_score)
        .filter(|r| {
            if let Some(ref path_filter) = params.path {
                r.path.contains(path_filter)
            } else {
                true
            }
        })
        .map(|r| {
            let mut score = r.score as f64;
            let q = params.query.to_lowercase();
            let symbol_lc = r.symbol.to_lowercase();
            if symbol_lc.contains(&q) {
                score += 0.2;
            }
            let kind_lc = r.kind.to_lowercase();
            if kind_lc.contains("function") || kind_lc.contains("method") {
                score += 0.1;
            } else if kind_lc.contains("class") || kind_lc.contains("struct") {
                score += 0.05;
            } else if kind_lc.contains("file") {
                score -= 0.05;
            }

            CbSearchResult {
                name: r.symbol.clone(),
                kind: r.kind.clone(),
                file: r.path.clone(),
                line: r.range_start,
                score,
                snippet: Some(truncate_snippet(&r.content, 240)),
            }
        })
        .collect();

    results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    if params.aggregate_by_file {
        results = aggregate_results_by_file(results);
    }

    results
}

fn truncate_snippet(content: &str, max_chars: usize) -> String {
    let trimmed = content.trim();
    if trimmed.len() <= max_chars {
        return trimmed.to_string();
    }
    let mut out = trimmed.chars().take(max_chars).collect::<String>();
    out.push('…');
    out
}

fn adaptive_min_score(query: &str) -> f32 {
    let token_count = query.split_whitespace().count();
    if token_count <= 1 {
        0.05
    } else if token_count == 2 {
        0.1
    } else {
        0.2
    }
}

fn aggregate_results_by_file(results: Vec<CbSearchResult>) -> Vec<CbSearchResult> {
    use std::collections::HashMap;

    let mut best_by_file: HashMap<String, CbSearchResult> = HashMap::new();
    for result in results {
        match best_by_file.get(&result.file) {
            Some(existing) if existing.score >= result.score => {}
            _ => {
                best_by_file.insert(result.file.clone(), result);
            }
        }
    }

    let mut out: Vec<CbSearchResult> = best_by_file.into_values().collect();
    out.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(file: &str, score: f64) -> CbSearchResult {
        CbSearchResult {
            name: "sym".to_string(),
            kind: "function".to_string(),
            file: file.to_string(),
            line: 1,
            score,
            snippet: None,
        }
    }

    #[test]
    fn adaptive_min_score_scales_with_token_count() {
        assert_eq!(adaptive_min_score("one"), 0.05);
        assert_eq!(adaptive_min_score("two words"), 0.1);
        assert_eq!(adaptive_min_score("three word query"), 0.2);
    }

    #[test]
    fn truncate_snippet_trims_and_appends_ellipsis() {
        assert_eq!(truncate_snippet("  short  ", 240), "short");
        let long = "x".repeat(300);
        let truncated = truncate_snippet(&long, 240);
        assert_eq!(truncated.chars().count(), 241);
        assert!(truncated.ends_with('…'));
    }

    #[test]
    fn aggregate_keeps_best_result_per_file() {
        let results = vec![
            result("a.rs", 0.5),
            result("a.rs", 0.9),
            result("b.rs", 0.7),
        ];
        let aggregated = aggregate_results_by_file(results);
        assert_eq!(aggregated.len(), 2);
        assert_eq!(aggregated[0].file, "a.rs");
        assert_eq!(aggregated[0].score, 0.9);
        assert_eq!(aggregated[1].file, "b.rs");
    }
}
