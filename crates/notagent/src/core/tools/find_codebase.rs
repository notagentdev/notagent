//! `find_codebase` — local ranked codebase search over a BM25 index.
//!
//! New over the TS original (user decision 2026-08-16, v0.1.7): a 1:1
//! takeover of `cb_search` from ../notagent-main-rust — the tool service
//! (`notagent_services/src/tool_services/cb_search.rs`: lazy shared index
//! state, 30s refresh window, writer-lock fallback), the result XML shape
//! (`notagent_app/src/operation.rs`), the tool description
//! (`notagent_domain/src/tools/descriptions/cb_search.md`) and the collapsed
//! toolbox renderer (`notagent_tui/src/tool_renderers.rs`). The search itself
//! lives in the `notagent-index` crate, copied unchanged from the reference.
//! Renames relative to the reference: `cb_search` → `find_codebase`,
//! `fs_search` → `find_filesystem`, config key → `findCodebaseEnabled`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use notagent_agent::types::{
    AgentTool, AgentToolResult, AgentToolUpdateCallback, BoxFuture, ToolExecutionError,
};
use notagent_ai::types::{TextContent, TextOrImageContent};
use notagent_index::{CbSearchParams, CbSearchResult, IndexManager, cb_search, workspace_key};
use notagent_tui::tui::ComponentRef;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::core::tools::render_utils::{
    call_title, get_text_output, invalid_arg_text, shorten_path, str_arg,
};
use crate::core::tools::tool_definition::{
    ToolContext, ToolDefinition, ToolRenderContext, ToolRenderResult, ToolRenderResultOptions,
    render_text_call, render_text_result, wrap_tool_definition,
};
use crate::modes::interactive::theme::theme::{Theme, ThemeColor};

/// Minimum time between incremental re-scans of the workspace. Within this
/// window queries hit the existing index without touching the filesystem.
const REFRESH_INTERVAL: Duration = Duration::from_secs(30);

struct CbIndexState {
    manager: Option<IndexManager>,
    last_refresh: Option<Instant>,
}

static INDEX_STATES: LazyLock<Mutex<HashMap<PathBuf, Arc<Mutex<CbIndexState>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn shared_index_state(state_dir: &Path) -> Arc<Mutex<CbIndexState>> {
    let mut states = INDEX_STATES
        .lock()
        .expect("find_codebase state registry poisoned");
    states
        .entry(state_dir.to_path_buf())
        .or_insert_with(|| {
            Arc::new(Mutex::new(CbIndexState {
                manager: None,
                last_refresh: None,
            }))
        })
        .clone()
}

/// Progress of an explicit index build (`/index`), the reference's
/// `CbIndexProgress` reduced to what the status line shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexBuildPhase {
    Scanning,
    Indexing,
    Done,
    Error,
}

#[derive(Debug, Clone, Copy)]
pub struct IndexBuildProgress {
    pub phase: IndexBuildPhase,
    pub total_files: usize,
    pub indexed_files: usize,
}

/// Explicit full refresh with progress, shared with the `/index` command.
/// Mirrors `build_index_blocking` of the reference service.
pub fn rebuild_index_blocking(
    cwd: &str,
    state_base_dir: &Path,
    on_progress: impl Fn(IndexBuildProgress) + Send + Sync + 'static,
) -> Result<(), String> {
    let (cwd, state_dir) = resolve_index_paths(cwd, state_base_dir);
    let state = shared_index_state(&state_dir);
    let mut state = state.lock().expect("find_codebase index state poisoned");
    let (manager, _) = IndexManager::refresh_workspace(
        cwd,
        state_dir,
        Some(move |progress: notagent_index::IndexProgress| {
            on_progress(IndexBuildProgress {
                phase: match progress.phase {
                    notagent_index::IndexingPhase::Scanning => IndexBuildPhase::Scanning,
                    notagent_index::IndexingPhase::Indexing => IndexBuildPhase::Indexing,
                    notagent_index::IndexingPhase::Done => IndexBuildPhase::Done,
                    notagent_index::IndexingPhase::Error => IndexBuildPhase::Error,
                },
                total_files: progress.total_files,
                indexed_files: progress.indexed_files,
            });
        }),
    )
    .map_err(|error| format!("{error:#}"))?;
    state.manager = Some(manager);
    state.last_refresh = Some(Instant::now());
    Ok(())
}

/// The workspace root and its index state directory. The index state lives
/// under the agent dir (the reference uses `<base_path>/cb_index/<key>`), so
/// the workspace itself is never touched.
fn resolve_index_paths(cwd: &str, state_base_dir: &Path) -> (PathBuf, PathBuf) {
    let cwd = PathBuf::from(cwd);
    let cwd = std::fs::canonicalize(&cwd).unwrap_or(cwd);
    let state_dir = state_base_dir.join("cb_index").join(workspace_key(&cwd));
    (cwd, state_dir)
}

/// Init, refresh, and search — verbatim logic of the reference's
/// `search_blocking`: refresh at most once per [`REFRESH_INTERVAL`]; when a
/// foreign process holds the Tantivy writer lock, serve the existing index
/// instead of failing.
fn search_blocking(
    state: &Mutex<CbIndexState>,
    cwd: PathBuf,
    state_dir: PathBuf,
    params: &CbSearchParams,
) -> Result<Vec<CbSearchResult>, String> {
    let mut state = state.lock().expect("find_codebase index state poisoned");
    let needs_refresh = state.manager.is_none()
        || state
            .last_refresh
            .is_none_or(|at| at.elapsed() >= REFRESH_INTERVAL);
    if needs_refresh {
        match IndexManager::refresh_workspace::<fn(notagent_index::IndexProgress)>(
            cwd.clone(),
            state_dir.clone(),
            None,
        ) {
            Ok((manager, _)) => {
                state.manager = Some(manager);
                state.last_refresh = Some(Instant::now());
            }
            Err(error) if notagent_index::is_writer_lock_busy(&error) => {
                if state.manager.is_none() {
                    let mut manager =
                        IndexManager::new(cwd, state_dir).map_err(|error| format!("{error:#}"))?;
                    manager.init_bm25().map_err(|error| format!("{error:#}"))?;
                    state.manager = Some(manager);
                }
                state.last_refresh = Some(Instant::now());
            }
            Err(error) => return Err(format!("{error:#}")),
        }
    }
    let manager = state.manager.as_ref().expect("manager initialized above");
    cb_search(manager, params).map_err(|error| format!("{error:#}"))
}

// ============================================================================
// Result XML — the shape of the reference's `ToolOperation::CbSearch` output
// ============================================================================

fn escape_xml_attr(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn cdata(value: &str) -> String {
    // A literal "]]>" inside CDATA is split across two sections.
    format!("<![CDATA[{}]]>", value.replace("]]>", "]]]]><![CDATA[>"))
}

fn results_to_xml(query: &str, path: Option<&str>, results: &[CbSearchResult]) -> String {
    let mut root = format!(
        "<find_codebase_results query=\"{}\" count=\"{}\"",
        escape_xml_attr(query),
        results.len()
    );
    if let Some(path) = path {
        root.push_str(&format!(" path=\"{}\"", escape_xml_attr(path)));
    }
    root.push('>');
    if results.is_empty() {
        root.push_str(
            "No results found for query. Try more specific keywords or symbol-name fragments, or fall back to find_filesystem for exact matches.",
        );
    } else {
        for result in results {
            root.push_str(&format!(
                "\n  <result name=\"{}\" kind=\"{}\" file=\"{}\" line=\"{}\" score=\"{:.3}\"",
                escape_xml_attr(&result.name),
                escape_xml_attr(&result.kind),
                escape_xml_attr(&result.file),
                result.line,
                result.score,
            ));
            match &result.snippet {
                Some(snippet) => {
                    root.push('>');
                    root.push_str(&cdata(snippet));
                    root.push_str("</result>");
                }
                None => root.push_str("/>"),
            }
        }
        root.push('\n');
    }
    root.push_str("</find_codebase_results>");
    root
}

// ============================================================================
// Rendering — the reference's collapsed toolbox: one call line, results only
// when expanded
// ============================================================================

fn format_find_codebase_call(args: &Value, theme: &Theme) -> String {
    let query = str_arg(args.get("query"));
    let search_path = str_arg(args.get("path"))
        .filter(|path| !path.is_empty())
        .map(|path| shorten_path(Some(&path)));

    let query_display = match query {
        None => invalid_arg_text(theme),
        Some(query) => theme.fg(ThemeColor::Accent, &format!("/{query}/")),
    };

    let mut text = format!("{}{}", call_title(theme, "find_codebase"), query_display,);
    if let Some(path) = search_path {
        text.push_str(&theme.fg(ThemeColor::ToolOutput, &format!(" in {path}")));
    }
    if let Some(limit) = args.get("max_results").and_then(Value::as_u64) {
        text.push_str(&theme.fg(ThemeColor::ToolOutput, &format!(" limit {limit}")));
    }
    text
}

/// Parses the `<find_codebase_results>` XML into readable
/// `file:line  name (kind)` lines so the expanded view never shows raw XML.
/// Returns an empty vec when there are no `<result>` elements.
fn parse_find_codebase_results(xml: &str, theme: &Theme) -> Vec<String> {
    static RESULT_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    static ATTR_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let result_re = RESULT_RE.get_or_init(|| regex::Regex::new(r"<result\b([^>]*)>").unwrap());
    let attr_re = ATTR_RE.get_or_init(|| regex::Regex::new(r#"(\w+)="([^"]*)""#).unwrap());

    result_re
        .captures_iter(xml)
        .filter_map(|caps| {
            let attrs: HashMap<String, String> = attr_re
                .captures_iter(&caps[1])
                .map(|attr| (attr[1].to_string(), attr[2].to_string()))
                .collect();
            let file = attrs.get("file")?;
            let loc = match attrs.get("line") {
                Some(line) if !line.is_empty() => format!("{file}:{line}"),
                _ => file.clone(),
            };
            let mut line = theme.fg(ThemeColor::ToolOutput, &format!("  {loc}"));
            if let Some(name) = attrs.get("name").filter(|name| !name.is_empty()) {
                let suffix = match attrs.get("kind").filter(|kind| !kind.is_empty()) {
                    Some(kind) => format!("  {name} ({kind})"),
                    None => format!("  {name}"),
                };
                line.push_str(&theme.fg(ThemeColor::Muted, &suffix));
            }
            Some(line)
        })
        .collect()
}

/// Strips markup for the "No results found" text and unparseable payloads.
fn xml_to_plain_text(xml: &str) -> String {
    static TAG_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let tag_re = TAG_RE.get_or_init(|| regex::Regex::new(r"<[^>]*>").unwrap());
    tag_re.replace_all(xml, "").trim().to_string()
}

fn format_find_codebase_result(
    result: ToolRenderResult<'_>,
    options: ToolRenderResultOptions,
    theme: &Theme,
    show_images: bool,
) -> String {
    if !options.expanded {
        return String::new();
    }
    let output = get_text_output(Some(result.content), show_images)
        .trim()
        .to_string();
    if output.is_empty() {
        return String::new();
    }
    let parsed = parse_find_codebase_results(&output, theme);
    let body = if parsed.is_empty() {
        theme.fg(ThemeColor::ToolOutput, &xml_to_plain_text(&output))
    } else {
        parsed.join("\n")
    };
    format!("\n{body}")
}

// ============================================================================
// The tool
// ============================================================================

#[derive(Clone, Default)]
pub struct FindCodebaseToolOptions {
    /// Whether the codebase index is enabled; absent means enabled (the
    /// reference defaults `cb_search_enabled` to true). The interactive
    /// session wires this to the `findCodebaseEnabled` setting.
    pub enabled: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
    /// Where the index state lives; defaults to the agent dir. The index is
    /// stored under `<base>/cb_index/<workspace-key>/`, never in the
    /// workspace itself.
    pub state_base_dir: Option<PathBuf>,
}

pub struct FindCodebaseToolDefinition {
    cwd: String,
    enabled: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
    state_base_dir: PathBuf,
    description: String,
    parameters: Value,
}

fn find_codebase_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "query": { "type": "string", "description": "Search query: concrete keywords, symbol names, or short phrases taken from code (e.g. \"calculate_total\", \"IndexManager\", \"workspace layout\"). This is lexical BM25 matching over indexed code chunks, NOT semantic search — it does not infer meaning beyond tokens." },
            "path": { "type": "string", "description": "Optional subdirectory to search (relative to the workspace). Results are limited to paths containing this value." },
            "max_results": { "type": "number", "description": "Maximum number of results (default: 20)" },
            "aggregate_by_file": { "type": "boolean", "description": "Return only the best match per file (default: false)" },
        },
        "required": ["query"],
    })
}

pub fn create_find_codebase_tool_definition(
    cwd: &str,
    options: Option<FindCodebaseToolOptions>,
) -> FindCodebaseToolDefinition {
    let options = options.unwrap_or_default();
    FindCodebaseToolDefinition {
        cwd: cwd.to_owned(),
        enabled: options.enabled,
        state_base_dir: options
            .state_base_dir
            .unwrap_or_else(crate::config::get_agent_dir),
        // The reference description (`cb_search.md`) with its template
        // variables resolved: {{env.cwd}} → cwd, fs_search → find_filesystem.
        description: format!(
            "Ranked lexical code search over a local index of {cwd}. Searches BM25-indexed code chunks (functions, structs, classes) plus a symbol index, and returns scored results with file, line, symbol and a snippet. The index is built locally on first use, respects .gitignore, and never leaves this machine.\n\n\
            **WHEN TO USE find_codebase:**\n\
            - Finding where a concept or feature lives without knowing the exact identifier (\"workspace lock handling\", \"retry backoff\")\n\
            - Locating a symbol when you only remember parts of its name — the tokenizer splits camelCase/snake_case, so \"workspace lock\" matches `WorkspaceLockManager`\n\
            - Getting a ranked shortlist of candidate files before reading them\n\
            - Exploring an unfamiliar area of the codebase with a handful of keywords\n\n\
            **WHEN NOT TO USE (use find_filesystem or grep instead):**\n\
            - Exact strings, regex patterns, TODOs, or all occurrences of an identifier\n\
            - Searching specific file paths or non-code files\n\
            - When you know the exact text to search for\n\n\
            Use concrete keywords, symbol names, or short phrases from code (e.g. \"calculate_total\", \"IndexManager\", \"workspace layout\"). This is NOT semantic search; it does not infer meaning beyond tokens. Use `path` to limit results to a directory, `aggregate_by_file` to return only the best match per file. Only searches within {cwd}."
        ),
        parameters: find_codebase_schema(),
    }
}

pub fn create_find_codebase_tool(
    cwd: &str,
    options: Option<FindCodebaseToolOptions>,
) -> Arc<dyn AgentTool> {
    wrap_tool_definition(
        Arc::new(create_find_codebase_tool_definition(cwd, options)),
        None,
    )
}

impl ToolDefinition for FindCodebaseToolDefinition {
    fn name(&self) -> &str {
        "find_codebase"
    }

    fn label(&self) -> &str {
        "find_codebase"
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    fn render_call(
        &self,
        args: &Value,
        theme: &Theme,
        context: &ToolRenderContext,
    ) -> Option<ComponentRef> {
        Some(render_text_call(
            context,
            &format_find_codebase_call(args, theme),
        ))
    }

    fn render_result(
        &self,
        result: ToolRenderResult<'_>,
        options: ToolRenderResultOptions,
        theme: &Theme,
        context: &ToolRenderContext,
    ) -> Option<ComponentRef> {
        Some(render_text_result(
            context,
            &format_find_codebase_result(result, options, theme, context.show_images),
        ))
    }

    fn execute<'a>(
        &'a self,
        _tool_call_id: &'a str,
        params: Value,
        signal: Option<CancellationToken>,
        _on_update: Option<AgentToolUpdateCallback>,
        _context: Option<ToolContext>,
    ) -> BoxFuture<'a, Result<AgentToolResult, ToolExecutionError>> {
        Box::pin(async move {
            if matches!(&signal, Some(signal) if signal.is_cancelled()) {
                return Err(ToolExecutionError::new("Operation aborted"));
            }
            if !self.enabled.as_ref().is_none_or(|enabled| enabled()) {
                return Err(ToolExecutionError::new(
                    "The codebase index is disabled (settings.json: findCodebaseEnabled, or /index on). Use the find_filesystem tool instead.",
                ));
            }
            let Some(query) = params.get("query").and_then(Value::as_str) else {
                return Err(ToolExecutionError::new("query is required"));
            };
            let search_params = CbSearchParams {
                query: query.to_owned(),
                path: params
                    .get("path")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                max_results: params
                    .get("max_results")
                    .and_then(Value::as_u64)
                    .map(|limit| limit as usize)
                    .unwrap_or(20),
                aggregate_by_file: params
                    .get("aggregate_by_file")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            };

            let (cwd, state_dir) = resolve_index_paths(&self.cwd, &self.state_base_dir);
            let state = shared_index_state(&state_dir);
            // Index init, refresh, and search are CPU/disk heavy (rayon
            // parse, tantivy commit) — keep them off the async executor.
            let params_for_task = search_params.clone();
            let results = tokio::task::spawn_blocking(move || {
                search_blocking(&state, cwd, state_dir, &params_for_task)
            })
            .await
            .map_err(|error| {
                ToolExecutionError::new(format!("find_codebase task failed: {error}"))
            })?
            .map_err(ToolExecutionError::new)?;

            let xml = results_to_xml(query, search_params.path.as_deref(), &results);
            Ok(AgentToolResult {
                content: vec![TextOrImageContent::Text(TextContent::new(xml))],
                details: None,
                usage: None,
                added_tool_names: None,
                terminate: None,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> Arc<Theme> {
        crate::modes::interactive::theme::theme::init_theme(None, false);
        crate::modes::interactive::theme::theme::theme()
    }

    fn result(name: &str, file: &str, line: usize, snippet: Option<&str>) -> CbSearchResult {
        CbSearchResult {
            name: name.to_owned(),
            kind: "function".to_owned(),
            file: file.to_owned(),
            line,
            score: 1.5,
            snippet: snippet.map(str::to_owned),
        }
    }

    #[test]
    fn renders_results_as_the_reference_xml() {
        let xml = results_to_xml(
            "workspace lock",
            None,
            &[result("lock_it", "src/lock.rs", 3, Some("fn lock_it() {}"))],
        );
        assert!(
            xml.starts_with("<find_codebase_results query=\"workspace lock\" count=\"1\">"),
            "{xml}"
        );
        assert!(
            xml.contains(
                "<result name=\"lock_it\" kind=\"function\" file=\"src/lock.rs\" line=\"3\" score=\"1.500\"><![CDATA[fn lock_it() {}]]></result>"
            ),
            "{xml}"
        );
    }

    #[test]
    fn escapes_attributes_and_cdata_terminators() {
        let xml = results_to_xml(
            "a\"b<c>",
            Some("dir&sub"),
            &[result("x", "f.rs", 1, Some("a ]]> b"))],
        );
        assert!(xml.contains("query=\"a&quot;b&lt;c&gt;\""), "{xml}");
        assert!(xml.contains("path=\"dir&amp;sub\""), "{xml}");
        assert!(xml.contains("<![CDATA[a ]]]]><![CDATA[> b]]>"), "{xml}");
    }

    #[test]
    fn empty_results_point_to_find_filesystem() {
        let xml = results_to_xml("nothing", None, &[]);
        assert!(xml.contains("fall back to find_filesystem"), "{xml}");
        assert!(!xml.contains("<result"), "{xml}");
    }

    #[test]
    fn the_expanded_view_parses_xml_into_readable_rows() {
        let theme = theme();
        let xml = results_to_xml("q", None, &[result("handle", "src/a.rs", 12, None)]);
        let rows = parse_find_codebase_results(&xml, &theme);
        assert_eq!(rows.len(), 1);
        let row = crate::utils::ansi::strip_ansi(&rows[0]);
        assert_eq!(row, "  src/a.rs:12  handle (function)");
    }

    #[tokio::test]
    async fn disabled_index_errors_with_the_find_filesystem_hint() {
        let tool = create_find_codebase_tool_definition(
            ".",
            Some(FindCodebaseToolOptions {
                enabled: Some(Arc::new(|| false)),
                state_base_dir: None,
            }),
        );
        let error = tool
            .execute("call-1", json!({ "query": "anything" }), None, None, None)
            .await
            .expect_err("disabled index must error");
        assert!(error.to_string().contains("find_filesystem"), "{error}");
    }

    #[tokio::test]
    async fn indexes_the_workspace_and_ranks_the_matching_file_first() {
        let workspace = tempfile::tempdir().expect("workspace");
        let base = tempfile::tempdir().expect("base");
        std::fs::create_dir_all(workspace.path().join("src")).expect("mkdir");
        std::fs::write(
            workspace.path().join("src/lock.rs"),
            "pub struct WorkspaceLockManager;\nimpl WorkspaceLockManager {\n    pub fn handle_file_changes(&self) {}\n}\n",
        )
        .expect("write");
        std::fs::write(
            workspace.path().join("src/other.rs"),
            "pub fn unrelated_helper() {}\n",
        )
        .expect("write");

        let tool = create_find_codebase_tool_definition(
            &workspace.path().to_string_lossy(),
            Some(FindCodebaseToolOptions {
                enabled: None,
                state_base_dir: Some(base.path().to_path_buf()),
            }),
        );
        let result = tool
            .execute(
                "call-1",
                json!({ "query": "workspace lock" }),
                None,
                None,
                None,
            )
            .await
            .expect("search succeeds");
        let TextOrImageContent::Text(text) = &result.content[0] else {
            panic!("text result");
        };
        let first = text
            .text
            .lines()
            .find(|line| line.contains("<result"))
            .expect("has results");
        assert!(
            first.contains("lock.rs"),
            "best hit is the lock file: {first}"
        );

        // The index state is persisted under the base dir, not the workspace.
        assert!(base.path().join("cb_index").exists());
        assert!(!workspace.path().join("cb_index").exists());

        // A second query within the refresh window reuses the index.
        let again = tool
            .execute(
                "call-2",
                json!({ "query": "unrelated helper" }),
                None,
                None,
                None,
            )
            .await
            .expect("second search succeeds");
        let TextOrImageContent::Text(text) = &again.content[0] else {
            panic!("text result");
        };
        assert!(text.text.contains("other.rs"), "{}", text.text);
    }

    #[tokio::test]
    async fn rebuild_streams_progress_until_done() {
        let workspace = tempfile::tempdir().expect("workspace");
        let base = tempfile::tempdir().expect("base");
        std::fs::create_dir_all(workspace.path().join("src")).expect("mkdir");
        std::fs::write(
            workspace.path().join("src/lib.rs"),
            "pub fn indexed_symbol() {}\n",
        )
        .expect("write");

        let phases: Arc<Mutex<Vec<IndexBuildPhase>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&phases);
        let cwd = workspace.path().to_string_lossy().into_owned();
        let base_dir = base.path().to_path_buf();
        tokio::task::spawn_blocking(move || {
            rebuild_index_blocking(&cwd, &base_dir, move |progress| {
                sink.lock().expect("phases").push(progress.phase);
            })
        })
        .await
        .expect("join")
        .expect("build succeeds");

        let phases = phases.lock().expect("phases").clone();
        assert_eq!(phases.first(), Some(&IndexBuildPhase::Scanning));
        assert_eq!(phases.last(), Some(&IndexBuildPhase::Done));
    }
}
