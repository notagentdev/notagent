use anyhow::Result;
use notagent_index::IndexManager;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tempfile::TempDir;

fn copy_dir_all(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_all(&from, &to)?;
        } else {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

fn fixture_workspace() -> Result<TempDir> {
    let tmp = TempDir::new()?;
    let fixture_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("mixed-language-workspace");
    copy_dir_all(&fixture_root, tmp.path())?;
    Ok(tmp)
}

#[test]
fn test_bm25_mixed_language_relevance() -> Result<()> {
    let workspace = fixture_workspace()?;
    let state_dir = TempDir::new()?;
    let mut manager = IndexManager::new(
        workspace.path().to_path_buf(),
        state_dir.path().to_path_buf(),
    )?;
    manager.init_bm25()?;

    let start_index = Instant::now();
    manager.index_workspace::<fn(notagent_index::IndexProgress)>(None)?;
    let index_duration = start_index.elapsed();
    assert!(
        index_duration < Duration::from_secs(60),
        "indexing too slow: {index_duration:?}"
    );

    let start_query = Instant::now();
    let bm25_results = manager
        .bm25_search("calculate total", 10)?
        .unwrap_or_default();
    let query_duration = start_query.elapsed();
    assert!(
        query_duration < Duration::from_secs(5),
        "query too slow: {query_duration:?}"
    );

    assert!(
        bm25_results.iter().any(|r| r.path.ends_with("main.ts")),
        "expected TypeScript file in BM25 results"
    );

    let symbol_results = manager.index().search("compute_checksum").limit(5).search();
    assert!(!symbol_results.is_empty(), "expected symbol search results");

    Ok(())
}

#[test]
fn test_bm25_worst_case_repo() -> Result<()> {
    let workspace = TempDir::new()?;
    let src_root = workspace.path().join("src");
    fs::create_dir_all(&src_root)?;

    for i in 0..200 {
        let path = src_root.join(format!("util_{i}.ts"));
        let content = format!(
            "export function utilHelper{0}() {{ return \"helper\" + \"{0}\"; }}\n",
            i
        );
        fs::write(path, content)?;
    }

    let state_dir = TempDir::new()?;
    let mut manager = IndexManager::new(
        workspace.path().to_path_buf(),
        state_dir.path().to_path_buf(),
    )?;
    manager.init_bm25()?;

    let start_index = Instant::now();
    manager.index_workspace::<fn(notagent_index::IndexProgress)>(None)?;
    let index_duration = start_index.elapsed();
    assert!(
        index_duration < Duration::from_secs(60),
        "indexing too slow: {index_duration:?}"
    );

    let start_query = Instant::now();
    let bm25_results = manager.bm25_search("util helper", 10)?.unwrap_or_default();
    let query_duration = start_query.elapsed();
    assert!(
        query_duration < Duration::from_secs(5),
        "query too slow: {query_duration:?}"
    );

    assert!(
        !bm25_results.is_empty(),
        "expected BM25 results for worst-case repo"
    );

    Ok(())
}

#[test]
fn test_cb_search_ranks_bm25_chunks() -> Result<()> {
    let workspace = fixture_workspace()?;
    let state_dir = TempDir::new()?;
    let mut manager = IndexManager::new(
        workspace.path().to_path_buf(),
        state_dir.path().to_path_buf(),
    )?;
    manager.init_bm25()?;
    manager.index_workspace::<fn(notagent_index::IndexProgress)>(None)?;

    let params = notagent_index::CbSearchParams::new("calculate total");
    let results = notagent_index::cb_search(&manager, &params)?;

    assert!(!results.is_empty(), "expected cb_search results");
    assert!(
        results.iter().any(|r| r.file.ends_with("main.ts")),
        "expected TypeScript file in cb_search results"
    );
    assert!(
        results.windows(2).all(|w| w[0].score >= w[1].score),
        "results must be sorted by score descending"
    );
    assert!(results[0].snippet.is_some(), "BM25 results carry a snippet");

    let aggregated = notagent_index::cb_search(
        &manager,
        &notagent_index::CbSearchParams {
            aggregate_by_file: true,
            ..params.clone()
        },
    )?;
    let mut files: Vec<&str> = aggregated.iter().map(|r| r.file.as_str()).collect();
    files.sort();
    files.dedup();
    assert_eq!(
        files.len(),
        aggregated.len(),
        "one result per file expected"
    );

    Ok(())
}

#[test]
fn test_index_workspace_respects_gitignore() -> Result<()> {
    let workspace = TempDir::new()?;
    let src_root = workspace.path().join("src");
    let ignored_root = workspace.path().join("node_modules");
    fs::create_dir_all(&src_root)?;
    fs::create_dir_all(&ignored_root)?;
    fs::write(workspace.path().join(".gitignore"), "node_modules/\n")?;
    fs::write(src_root.join("lib.rs"), "pub fn wanted_symbol() {}\n")?;
    fs::write(
        ignored_root.join("vendored.js"),
        "function unwantedVendoredHelper() { return 1; }\n",
    )?;

    let state_dir = TempDir::new()?;
    let mut manager = IndexManager::new(
        workspace.path().to_path_buf(),
        state_dir.path().to_path_buf(),
    )?;
    manager.init_bm25()?;
    manager.index_workspace::<fn(notagent_index::IndexProgress)>(None)?;

    let hits = manager
        .bm25_search("unwanted vendored helper", 10)?
        .unwrap_or_default();
    assert!(hits.is_empty(), "gitignored files must not be indexed");

    let wanted = manager
        .bm25_search("wanted symbol", 10)?
        .unwrap_or_default();
    assert!(!wanted.is_empty(), "tracked files must be indexed");

    Ok(())
}
