//! Port of `packages/tui/test/autocomplete.test.ts` (542 LOC).
//!
//! The `fd`-based cases are skipped when `fd` is not installed, exactly as the
//! TS suite does with `{ skip: !isFdInstalled }`.

use std::fs;
use std::path::{Path, PathBuf};

use notagent_tui::autocomplete::{
    AutocompleteProvider, AutocompleteSuggestions, CombinedAutocompleteProvider, SuggestionOptions,
};

/// `resolveFdPath()` of the TS suite.
fn resolve_fd_path() -> Option<String> {
    let command = if cfg!(windows) { "where" } else { "which" };
    let output = std::process::Command::new(command)
        .arg("fd")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .find(|line| !line.is_empty())
        .map(|line| line.trim().to_string())
}

/// Unique temporary directory (`mkdtempSync`).
fn make_temp_dir(prefix: &str) -> PathBuf {
    let base = std::env::temp_dir();
    for attempt in 0..1000 {
        let candidate = base.join(format!("{prefix}{}-{attempt}", std::process::id()));
        if fs::create_dir(&candidate).is_ok() {
            return candidate;
        }
    }
    panic!("no free temporary directory");
}

/// `setupFolder()` of the TS suite.
fn setup_folder(base_dir: &Path, dirs: &[&str], files: &[(&str, &str)]) {
    for dir in dirs {
        fs::create_dir_all(base_dir.join(dir)).expect("create directory");
    }
    for (file_path, contents) in files {
        let full_path = base_dir.join(file_path);
        if let Some(parent) = full_path.parent() {
            fs::create_dir_all(parent).expect("create parent directory");
        }
        fs::write(&full_path, contents).expect("write file");
    }
}

async fn get_suggestions(
    provider: &CombinedAutocompleteProvider,
    lines: &[&str],
    cursor_line: usize,
    cursor_col: usize,
    force: bool,
) -> Option<AutocompleteSuggestions> {
    let lines: Vec<String> = lines.iter().map(|line| (*line).to_string()).collect();
    provider
        .get_suggestions(
            &lines,
            cursor_line,
            cursor_col,
            SuggestionOptions {
                force,
                ..SuggestionOptions::default()
            },
        )
        .await
}

fn values(result: &Option<AutocompleteSuggestions>) -> Vec<String> {
    result.as_ref().map_or_else(Vec::new, |result| {
        result.items.iter().map(|item| item.value.clone()).collect()
    })
}

// === extractPathPrefix ===

#[tokio::test]
async fn extracts_slash_from_hey_slash_when_forced() {
    let provider = CombinedAutocompleteProvider::new(Vec::new(), "/tmp", None);
    let result = get_suggestions(&provider, &["hey /"], 0, 5, true).await;
    assert!(result.is_some(), "should return suggestions for the root");
    assert_eq!(result.expect("suggestions").prefix, "/");
}

#[tokio::test]
async fn extracts_slash_a_when_forced() {
    let provider = CombinedAutocompleteProvider::new(Vec::new(), "/tmp", None);
    let result = get_suggestions(&provider, &["/A"], 0, 2, true).await;
    // May be `None` when nothing matches `/A`; only the prefix matters here.
    if let Some(result) = result {
        assert_eq!(result.prefix, "/A");
    }
}

#[tokio::test]
async fn does_not_trigger_for_slash_commands() {
    let provider = CombinedAutocompleteProvider::new(Vec::new(), "/tmp", None);
    let result = get_suggestions(&provider, &["/model"], 0, 6, true).await;
    assert!(result.is_none());
}

#[tokio::test]
async fn triggers_for_absolute_paths_after_a_slash_command_argument() {
    let provider = CombinedAutocompleteProvider::new(Vec::new(), "/tmp", None);
    let result = get_suggestions(&provider, &["/command /"], 0, 10, true).await;
    assert!(result.is_some());
    assert_eq!(result.expect("suggestions").prefix, "/");
}

// === dot-slash path completion ===

#[tokio::test]
async fn preserves_dot_slash_prefix_when_completing_paths() {
    let base_dir = make_temp_dir("notagent-autocomplete-dotslash-file");
    setup_folder(
        &base_dir,
        &[],
        &[("update.sh", "#!/bin/bash"), ("utils.ts", "export {};")],
    );

    let provider =
        CombinedAutocompleteProvider::new(Vec::new(), base_dir.to_string_lossy().as_ref(), None);
    let result = get_suggestions(&provider, &["./up"], 0, 4, true).await;
    assert!(result.is_some());
    assert!(
        values(&result).contains(&"./update.sh".to_string()),
        "values: {:?}",
        values(&result)
    );

    fs::remove_dir_all(&base_dir).ok();
}

#[tokio::test]
async fn preserves_dot_slash_prefix_for_directory_completions() {
    let base_dir = make_temp_dir("notagent-autocomplete-dotslash-dir");
    setup_folder(&base_dir, &["src"], &[("src/index.ts", "export {};")]);

    let provider =
        CombinedAutocompleteProvider::new(Vec::new(), base_dir.to_string_lossy().as_ref(), None);
    let result = get_suggestions(&provider, &["./sr"], 0, 4, true).await;
    assert!(result.is_some());
    assert!(
        values(&result).contains(&"./src/".to_string()),
        "values: {:?}",
        values(&result)
    );

    fs::remove_dir_all(&base_dir).ok();
}

// === quoted path completion ===

#[tokio::test]
async fn quotes_paths_with_spaces_for_direct_completion() {
    let base_dir = make_temp_dir("notagent-autocomplete-quoted-space");
    setup_folder(
        &base_dir,
        &["my folder"],
        &[("my folder/test.txt", "content")],
    );

    let provider =
        CombinedAutocompleteProvider::new(Vec::new(), base_dir.to_string_lossy().as_ref(), None);
    let result = get_suggestions(&provider, &["my"], 0, 2, true).await;
    assert!(result.is_some());
    assert!(
        values(&result).contains(&"\"my folder/\"".to_string()),
        "values: {:?}",
        values(&result)
    );

    fs::remove_dir_all(&base_dir).ok();
}

#[tokio::test]
async fn continues_completion_inside_quoted_paths() {
    let base_dir = make_temp_dir("notagent-autocomplete-quoted-continue");
    setup_folder(
        &base_dir,
        &[],
        &[
            ("my folder/test.txt", "content"),
            ("my folder/other.txt", "content"),
        ],
    );

    let provider =
        CombinedAutocompleteProvider::new(Vec::new(), base_dir.to_string_lossy().as_ref(), None);
    let line = "\"my folder/\"";
    let result = get_suggestions(&provider, &[line], 0, line.len() - 1, true).await;
    assert!(result.is_some());
    let values = values(&result);
    assert!(
        values.contains(&"\"my folder/test.txt\"".to_string()),
        "{values:?}"
    );
    assert!(
        values.contains(&"\"my folder/other.txt\"".to_string()),
        "{values:?}"
    );

    fs::remove_dir_all(&base_dir).ok();
}

#[tokio::test]
async fn applies_quoted_completion_without_duplicating_the_closing_quote() {
    let base_dir = make_temp_dir("notagent-autocomplete-quoted-apply");
    setup_folder(&base_dir, &[], &[("my folder/test.txt", "content")]);

    let provider =
        CombinedAutocompleteProvider::new(Vec::new(), base_dir.to_string_lossy().as_ref(), None);
    let line = "\"my folder/te\"";
    let cursor_col = line.len() - 1;
    let result = get_suggestions(&provider, &[line], 0, cursor_col, true).await;
    let result = result.expect("suggestions for the quoted path");
    let item = result
        .items
        .iter()
        .find(|item| item.value == "\"my folder/test.txt\"")
        .expect("test.txt suggestion");

    let applied =
        provider.apply_completion(&[line.to_string()], 0, cursor_col, item, &result.prefix);
    assert_eq!(applied.lines[0], "\"my folder/test.txt\"");

    fs::remove_dir_all(&base_dir).ok();
}

// === fd @ file suggestions ===

/// Root, base and outside directory of the `fd` cases.
struct FdFixture {
    root_dir: PathBuf,
    base_dir: PathBuf,
    outside_dir: PathBuf,
    fd_path: String,
}

impl Drop for FdFixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root_dir).ok();
    }
}

fn fd_fixture(name: &str) -> Option<FdFixture> {
    let fd_path = resolve_fd_path()?;
    let root_dir = make_temp_dir(&format!("notagent-autocomplete-root-{name}-"));
    let base_dir = root_dir.join("cwd");
    let outside_dir = root_dir.join("outside");
    fs::create_dir_all(&base_dir).expect("create base directory");
    fs::create_dir_all(&outside_dir).expect("create outside directory");
    Some(FdFixture {
        root_dir,
        base_dir,
        outside_dir,
        fd_path,
    })
}

fn fd_provider(fixture: &FdFixture) -> CombinedAutocompleteProvider {
    CombinedAutocompleteProvider::new(
        Vec::new(),
        fixture.base_dir.to_string_lossy().as_ref(),
        Some(fixture.fd_path.clone()),
    )
}

#[tokio::test]
async fn returns_all_files_and_folders_for_an_empty_at_query() {
    let Some(fixture) = fd_fixture("empty") else {
        return;
    };
    setup_folder(&fixture.base_dir, &["src"], &[("README.md", "readme")]);

    let provider = fd_provider(&fixture);
    let result = get_suggestions(&provider, &["@"], 0, 1, false).await;
    let mut actual = values(&result);
    actual.sort();
    assert_eq!(actual, ["@README.md", "@src/"]);
}

#[tokio::test]
async fn matches_a_file_with_an_extension_in_the_query() {
    let Some(fixture) = fd_fixture("extension") else {
        return;
    };
    setup_folder(&fixture.base_dir, &[], &[("file.txt", "content")]);

    let provider = fd_provider(&fixture);
    let line = "@file.txt";
    let result = get_suggestions(&provider, &[line], 0, line.len(), false).await;
    assert!(values(&result).contains(&"@file.txt".to_string()));
}

#[tokio::test]
async fn filters_are_case_insensitive() {
    let Some(fixture) = fd_fixture("case") else {
        return;
    };
    setup_folder(&fixture.base_dir, &["src"], &[("README.md", "readme")]);

    let provider = fd_provider(&fixture);
    let result = get_suggestions(&provider, &["@re"], 0, 3, false).await;
    assert_eq!(values(&result), ["@README.md"]);
}

#[tokio::test]
async fn ranks_directories_before_files() {
    let Some(fixture) = fd_fixture("rank") else {
        return;
    };
    setup_folder(&fixture.base_dir, &["src"], &[("src.txt", "text")]);

    let provider = fd_provider(&fixture);
    let result = get_suggestions(&provider, &["@src"], 0, 4, false).await;
    let values = values(&result);
    assert_eq!(values.first().map(String::as_str), Some("@src/"));
    assert!(values.contains(&"@src.txt".to_string()));
}

#[tokio::test]
async fn returns_nested_file_paths() {
    let Some(fixture) = fd_fixture("nested") else {
        return;
    };
    setup_folder(&fixture.base_dir, &[], &[("src/index.ts", "export {};\n")]);

    let provider = fd_provider(&fixture);
    let result = get_suggestions(&provider, &["@index"], 0, 6, false).await;
    assert!(values(&result).contains(&"@src/index.ts".to_string()));
}

#[tokio::test]
async fn matches_deeply_nested_paths() {
    let Some(fixture) = fd_fixture("deep") else {
        return;
    };
    setup_folder(
        &fixture.base_dir,
        &[],
        &[
            ("packages/tui/src/autocomplete.ts", "export {};"),
            ("packages/ai/src/autocomplete.ts", "export {};"),
        ],
    );

    let provider = fd_provider(&fixture);
    let line = "@tui/src/auto";
    let result = get_suggestions(&provider, &[line], 0, line.len(), false).await;
    let values = values(&result);
    assert!(values.contains(&"@packages/tui/src/autocomplete.ts".to_string()));
    assert!(!values.contains(&"@packages/ai/src/autocomplete.ts".to_string()));
}

#[tokio::test]
async fn matches_a_directory_in_the_middle_of_a_path() {
    let Some(fixture) = fd_fixture("middle") else {
        return;
    };
    setup_folder(
        &fixture.base_dir,
        &[],
        &[
            ("src/components/Button.tsx", "export {};"),
            ("src/utils/helpers.ts", "export {};"),
        ],
    );

    let provider = fd_provider(&fixture);
    let line = "@components/";
    let result = get_suggestions(&provider, &[line], 0, line.len(), false).await;
    let values = values(&result);
    assert!(values.contains(&"@src/components/Button.tsx".to_string()));
    assert!(!values.contains(&"@src/utils/helpers.ts".to_string()));
}

#[tokio::test]
async fn scopes_the_fuzzy_search_to_relative_directories() {
    let Some(fixture) = fd_fixture("scoped") else {
        return;
    };
    setup_folder(
        &fixture.outside_dir,
        &[],
        &[
            ("nested/alpha.ts", "export {};"),
            ("nested/deeper/also-alpha.ts", "export {};"),
            ("nested/deeper/zzz.ts", "export {};"),
        ],
    );

    let provider = fd_provider(&fixture);
    let line = "@../outside/a";
    let result = get_suggestions(&provider, &[line], 0, line.len(), false).await;
    let values = values(&result);
    assert!(values.contains(&"@../outside/nested/alpha.ts".to_string()));
    assert!(values.contains(&"@../outside/nested/deeper/also-alpha.ts".to_string()));
    assert!(!values.contains(&"@../outside/nested/deeper/zzz.ts".to_string()));
}

#[tokio::test]
async fn quotes_paths_with_spaces_for_at_suggestions() {
    let Some(fixture) = fd_fixture("quote") else {
        return;
    };
    setup_folder(
        &fixture.base_dir,
        &["my folder"],
        &[("my folder/test.txt", "content")],
    );

    let provider = fd_provider(&fixture);
    let result = get_suggestions(&provider, &["@my"], 0, 3, false).await;
    assert!(values(&result).contains(&"@\"my folder/\"".to_string()));
}

#[tokio::test]
async fn includes_hidden_paths_but_excludes_dot_git() {
    let Some(fixture) = fd_fixture("hidden") else {
        return;
    };
    setup_folder(
        &fixture.base_dir,
        &[".notagent", ".github", ".git"],
        &[
            (".notagent/config.json", "{}"),
            (".github/workflows/ci.yml", "name: ci"),
            (".git/config", "[core]"),
        ],
    );

    let provider = fd_provider(&fixture);
    let result = get_suggestions(&provider, &["@"], 0, 1, false).await;
    let values = values(&result);
    assert!(values.contains(&"@.notagent/".to_string()));
    assert!(values.contains(&"@.github/".to_string()));
    assert!(
        !values
            .iter()
            .any(|value| value == "@.git" || value.starts_with("@.git/"))
    );
}

#[tokio::test]
async fn follows_symlinked_directories_for_the_fuzzy_search() {
    let Some(fixture) = fd_fixture("symlink-dir") else {
        return;
    };
    setup_folder(&fixture.base_dir, &[], &[("dir/some_file.txt", "real")]);
    setup_folder(&fixture.outside_dir, &[], &[("some_file.txt", "symlinked")]);
    #[cfg(unix)]
    std::os::unix::fs::symlink("../outside", fixture.base_dir.join("symlinked_dir"))
        .expect("create symlink");

    let provider = fd_provider(&fixture);
    let result = get_suggestions(&provider, &["@some"], 0, 5, false).await;
    let values = values(&result);
    assert!(values.contains(&"@dir/some_file.txt".to_string()));
    #[cfg(unix)]
    assert!(values.contains(&"@symlinked_dir/some_file.txt".to_string()));
}

#[tokio::test]
async fn returns_symlinked_directories_when_matching_their_name() {
    let Some(fixture) = fd_fixture("symlink-name") else {
        return;
    };
    setup_folder(
        &fixture.outside_dir,
        &[],
        &[("nested/file.txt", "symlinked")],
    );
    #[cfg(unix)]
    std::os::unix::fs::symlink("../outside", fixture.base_dir.join("symlinked_dir"))
        .expect("create symlink");

    let provider = fd_provider(&fixture);
    let result = get_suggestions(&provider, &["@symlinked"], 0, 10, false).await;
    #[cfg(unix)]
    assert!(values(&result).contains(&"@symlinked_dir/".to_string()));
    #[cfg(not(unix))]
    let _ = result;
}

#[tokio::test]
async fn returns_symlinked_files_without_type_l() {
    let Some(fixture) = fd_fixture("symlink-file") else {
        return;
    };
    setup_folder(&fixture.base_dir, &[], &[("original.txt", "content")]);
    #[cfg(unix)]
    std::os::unix::fs::symlink("original.txt", fixture.base_dir.join("link.txt"))
        .expect("create symlink");

    let provider = fd_provider(&fixture);
    let result = get_suggestions(&provider, &["@link"], 0, 5, false).await;
    #[cfg(unix)]
    assert!(values(&result).contains(&"@link.txt".to_string()));
    #[cfg(not(unix))]
    let _ = result;
}

#[tokio::test]
async fn returns_the_same_at_suggestions_when_the_cwd_contains_the_query() {
    let Some(fixture) = fd_fixture("cwd-query") else {
        return;
    };
    let normal_base_dir = fixture.root_dir.join("cwd-normal");
    let query_in_path_base_dir = fixture.root_dir.join("cwd-plan-repro");
    fs::create_dir_all(&normal_base_dir).expect("create directory");
    fs::create_dir_all(&query_in_path_base_dir).expect("create directory");

    let dirs = ["packages/coding-agent/examples/extensions/plan-mode"];
    let files = [
        (
            "packages/coding-agent/examples/extensions/plan-mode/README.md",
            "readme",
        ),
        ("packages/tui/docs/plan.md", "plan"),
    ];
    setup_folder(&normal_base_dir, &dirs, &files);
    setup_folder(&query_in_path_base_dir, &dirs, &files);

    let normal_provider = CombinedAutocompleteProvider::new(
        Vec::new(),
        normal_base_dir.to_string_lossy().as_ref(),
        Some(fixture.fd_path.clone()),
    );
    let query_in_path_provider = CombinedAutocompleteProvider::new(
        Vec::new(),
        query_in_path_base_dir.to_string_lossy().as_ref(),
        Some(fixture.fd_path.clone()),
    );

    let normal_result = get_suggestions(&normal_provider, &["@plan"], 0, 5, false).await;
    let query_in_path_result =
        get_suggestions(&query_in_path_provider, &["@plan"], 0, 5, false).await;

    let normalize = |result: &Option<AutocompleteSuggestions>| {
        let mut entries: Vec<String> = result.as_ref().map_or_else(Vec::new, |result| {
            result
                .items
                .iter()
                .map(|item| {
                    format!(
                        "{} :: {}",
                        item.label,
                        item.description.clone().unwrap_or_default()
                    )
                })
                .collect()
        });
        entries.sort();
        entries
    };

    assert_eq!(normalize(&query_in_path_result), normalize(&normal_result));
    let normalized = normalize(&normal_result);
    assert!(normalized.contains(
        &"plan-mode/ :: packages/coding-agent/examples/extensions/plan-mode".to_string()
    ));
    assert!(normalized.contains(&"plan.md :: packages/tui/docs/plan.md".to_string()));
}

#[tokio::test]
async fn continues_autocomplete_inside_quoted_at_paths() {
    let Some(fixture) = fd_fixture("quoted-at") else {
        return;
    };
    setup_folder(
        &fixture.base_dir,
        &[],
        &[
            ("my folder/test.txt", "content"),
            ("my folder/other.txt", "content"),
        ],
    );

    let provider = fd_provider(&fixture);
    let line = "@\"my folder/\"";
    let result = get_suggestions(&provider, &[line], 0, line.len() - 1, false).await;
    assert!(result.is_some());
    let values = values(&result);
    assert!(
        values.contains(&"@\"my folder/test.txt\"".to_string()),
        "{values:?}"
    );
    assert!(
        values.contains(&"@\"my folder/other.txt\"".to_string()),
        "{values:?}"
    );
}

#[tokio::test]
async fn applies_quoted_at_completion_without_duplicating_the_closing_quote() {
    let Some(fixture) = fd_fixture("quoted-at-apply") else {
        return;
    };
    setup_folder(&fixture.base_dir, &[], &[("my folder/test.txt", "content")]);

    let provider = fd_provider(&fixture);
    let line = "@\"my folder/te\"";
    let cursor_col = line.len() - 1;
    let result = get_suggestions(&provider, &[line], 0, cursor_col, false).await;
    let result = result.expect("suggestions for the quoted @ path");
    let item = result
        .items
        .iter()
        .find(|item| item.value == "@\"my folder/test.txt\"")
        .expect("test.txt suggestion");

    let applied =
        provider.apply_completion(&[line.to_string()], 0, cursor_col, item, &result.prefix);
    assert_eq!(applied.lines[0], "@\"my folder/test.txt\" ");
}
