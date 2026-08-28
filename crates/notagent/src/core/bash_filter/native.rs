//! The native half of the bash filter: one post-processor per supported
//! command, each a pure function over the finished output.
//! Nothing here starts a process, streams, or rewrites an exit code. A
//! post-processor that cannot make sense of what it was given returns `None`,
//! and the raw output is what the caller keeps.

use std::collections::{BTreeMap, HashMap};
use std::sync::OnceLock;

use regex::Regex;
use serde::Deserialize;

use super::command::{ClassifiedCommand, basename};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeFilter {
    Ls,
    Tree,
    Find,
    Grep,
    Rg,
    Read,
    GitDiff,
    GitLog,
    CargoBuild,
    CargoCheck,
    CargoClippy,
    CargoTest,
    CargoInstall,
    CargoNextest,
    TypeScript,
    Npm,
    PnpmInstall,
    PnpmList,
    PnpmOutdated,
    Eslint,
    Prettier,
    NextBuild,
    Vitest,
    Jest,
    Playwright,
    GoTest,
    GoBuild,
    GoVet,
    Golangci,
    Pytest,
    RuffCheck,
    RuffFormat,
    Mypy,
    PipList,
    PipOutdated,
    UvRun,
}

impl NativeFilter {
    pub(crate) fn is_system(self) -> bool {
        matches!(
            self,
            Self::Ls | Self::Tree | Self::Find | Self::Grep | Self::Rg | Self::Read
        )
    }
    pub(crate) fn is_search(self) -> bool {
        matches!(self, Self::Grep | Self::Rg)
    }
    pub(crate) fn uses_execution_override(self) -> bool {
        matches!(self, Self::Find | Self::Read)
    }
}
pub(crate) fn detect(command: &ClassifiedCommand) -> Option<NativeFilter> {
    let tokens = &command.tokens;
    let index = command.effective_index;
    let executable = basename(&tokens.get(index)?.value);
    let args = || {
        tokens[index + 1..]
            .iter()
            .map(|token| token.value.as_str())
            .collect::<Vec<_>>()
    };
    match executable {
        "ls" => Some(NativeFilter::Ls),
        "tree" => Some(NativeFilter::Tree),
        "find" => Some(NativeFilter::Find),
        "grep" => Some(NativeFilter::Grep),
        "rg" => Some(NativeFilter::Rg),
        "cat" | "head" | "tail" => Some(NativeFilter::Read),
        "git" => match args().first().copied() {
            Some("diff") => Some(NativeFilter::GitDiff),
            Some("log")
                if args().iter().any(|arg| {
                    arg.starts_with("--pretty")
                        || arg.starts_with("--format")
                        || *arg == "--oneline"
                }) =>
            {
                Some(NativeFilter::GitLog)
            }
            _ => None,
        },
        "cargo" => match args().first().copied() {
            Some("build") => Some(NativeFilter::CargoBuild),
            Some("check") => Some(NativeFilter::CargoCheck),
            Some("clippy") => Some(NativeFilter::CargoClippy),
            Some("test") => Some(NativeFilter::CargoTest),
            Some("install") => Some(NativeFilter::CargoInstall),
            Some("nextest") => Some(NativeFilter::CargoNextest),
            _ => None,
        },
        "tsc" => Some(NativeFilter::TypeScript),
        "npm"
            if index == command.executable_index
                && matches!(
                    args().first().copied(),
                    Some("run" | "test" | "t" | "start" | "stop" | "restart")
                ) =>
        {
            Some(NativeFilter::Npm)
        }
        "pnpm" if index == command.executable_index => match args().first().copied() {
            Some("install" | "i") => Some(NativeFilter::PnpmInstall),
            Some("list" | "ls") => Some(NativeFilter::PnpmList),
            Some("outdated") => Some(NativeFilter::PnpmOutdated),
            _ => None,
        },
        "eslint" => Some(NativeFilter::Eslint),
        "prettier" => Some(NativeFilter::Prettier),
        "next" if args().first().copied() == Some("build") => Some(NativeFilter::NextBuild),
        "vitest" => Some(NativeFilter::Vitest),
        "jest" => Some(NativeFilter::Jest),
        "playwright" if args().first().copied() == Some("test") => Some(NativeFilter::Playwright),
        "go" => match args().first().copied() {
            Some("test") => Some(NativeFilter::GoTest),
            Some("build") => Some(NativeFilter::GoBuild),
            Some("vet") => Some(NativeFilter::GoVet),
            _ => None,
        },
        "golangci-lint"
            if args().first().copied().is_none_or(|arg| arg == "run")
                && has_compatible_golangci_json(&args()) =>
        {
            Some(NativeFilter::Golangci)
        }
        "pytest" => Some(NativeFilter::Pytest),
        "ruff" => match args().first().copied() {
            Some("format") => Some(NativeFilter::RuffFormat),
            Some("check") | None => Some(NativeFilter::RuffCheck),
            Some(arg) if arg.starts_with('-') => Some(NativeFilter::RuffCheck),
            _ => None,
        },
        "mypy" => Some(NativeFilter::Mypy),
        "pip" | "pip3" => match args().first().copied() {
            Some("list") if args().contains(&"--outdated") => Some(NativeFilter::PipOutdated),
            Some("list") => Some(NativeFilter::PipList),
            _ => None,
        },
        "uv" if args().first().copied() == Some("run") => Some(NativeFilter::UvRun),
        _ if index != command.executable_index
            && basename(&tokens[command.executable_index].value) == "npx" =>
        {
            Some(NativeFilter::Npm)
        }
        _ => None,
    }
}

/// Names a filter from what the output looks like, for the runs whose command
/// says nothing: a script, a `make` target, a wrapper, anything that reached
/// here unclassified.
/// Only filters that read a stream are eligible. The system adapters answer
/// from the filesystem or from flags this output never carried, so they have
/// no place here, so the search and find adapters have no counterpart below.
/// # Arguments
/// * `stdout` - Captured standard output of the finished command.
pub(crate) fn detect_by_content(stdout: &str) -> Option<NativeFilter> {
    // A byte window, cut back to a character boundary: the markers all sit in
    // the opening lines, and a megabyte of log costs nothing to skip.
    let mut end = stdout.len().min(1024);
    while end > 0 && !stdout.is_char_boundary(end) {
        end -= 1;
    }
    let head = &stdout[..end];
    let trimmed = head.trim_start();

    if head.contains("test result:") && head.contains("passed;") {
        return Some(NativeFilter::CargoTest);
    }
    if head.contains("=== test session starts") {
        return Some(NativeFilter::Pytest);
    }
    if trimmed.starts_with('{') && head.contains("\"Action\"") {
        return Some(NativeFilter::GoTest);
    }
    if head.contains("\"numTotalTests\"") || head.contains("\"testResults\"") {
        return Some(NativeFilter::Vitest);
    }
    if head.contains(": error:") && head.contains(".py:") {
        return Some(NativeFilter::Mypy);
    }
    None
}

fn has_compatible_golangci_json(args: &[&str]) -> bool {
    args.windows(2)
        .any(|pair| pair == ["--out-format", "json"] || pair == ["--output.json.path", "stdout"])
        || args
            .iter()
            .any(|arg| *arg == "--out-format=json" || *arg == "--output.json.path=stdout")
}

pub(crate) fn apply(
    filter: NativeFilter,
    stdout: &str,
    stderr: &str,
    exit_code: Option<i32>,
) -> Option<(String, String, bool)> {
    let combined = format!("{stdout}{stderr}");
    let stdout_only = matches!(
        filter,
        NativeFilter::GitDiff
            | NativeFilter::GitLog
            | NativeFilter::PnpmList
            | NativeFilter::PnpmOutdated
            | NativeFilter::Eslint
            | NativeFilter::Prettier
            | NativeFilter::Vitest
            | NativeFilter::Jest
            | NativeFilter::Playwright
            | NativeFilter::GoTest
            | NativeFilter::Golangci
            | NativeFilter::Pytest
            | NativeFilter::RuffCheck
            | NativeFilter::RuffFormat
            | NativeFilter::PipList
            | NativeFilter::PipOutdated
    );
    let result = match filter {
        NativeFilter::Ls
        | NativeFilter::Tree
        | NativeFilter::Find
        | NativeFilter::Grep
        | NativeFilter::Rg
        | NativeFilter::Read => return None,
        NativeFilter::GitDiff => compact_diff(stdout, 500),
        NativeFilter::GitLog => filter_log(stdout),
        NativeFilter::CargoBuild => filter_cargo_build(&combined, "build", exit_code),
        NativeFilter::CargoCheck => filter_cargo_build(&combined, "check", exit_code),
        NativeFilter::CargoClippy => filter_clippy(&combined, exit_code),
        NativeFilter::CargoTest => filter_cargo_test(&combined),
        NativeFilter::CargoInstall => filter_cargo_install(&combined),
        NativeFilter::CargoNextest => filter_nextest(&combined),
        NativeFilter::TypeScript => filter_tsc(&combined),
        NativeFilter::Npm => filter_npm(&combined, exit_code),
        NativeFilter::PnpmInstall => filter_pnpm_install(&combined, exit_code),
        NativeFilter::PnpmList => filter_pnpm_list(stdout),
        NativeFilter::PnpmOutdated => filter_pnpm_outdated(stdout),
        NativeFilter::Eslint => filter_eslint(stdout),
        NativeFilter::Prettier => filter_prettier(stdout),
        NativeFilter::NextBuild => filter_next(&combined),
        NativeFilter::Vitest | NativeFilter::Jest => filter_js_test(stdout, filter),
        NativeFilter::Playwright => filter_playwright(stdout),
        NativeFilter::GoTest => filter_go_test(stdout, exit_code),
        NativeFilter::GoBuild => filter_go_build(&combined, exit_code),
        NativeFilter::GoVet => filter_go_vet(&combined, exit_code),
        NativeFilter::Golangci => filter_golangci(stdout),
        NativeFilter::Pytest => filter_pytest(stdout, exit_code),
        NativeFilter::RuffCheck => filter_ruff(stdout),
        NativeFilter::RuffFormat => filter_ruff_format(stdout),
        NativeFilter::Mypy => filter_mypy(&combined),
        NativeFilter::PipList => filter_pip(stdout, false),
        NativeFilter::PipOutdated => filter_pip(stdout, true),
        NativeFilter::UvRun => filter_uv_run(&combined, exit_code),
    }?;

    Some((
        result,
        if stdout_only {
            stderr.to_string()
        } else {
            String::new()
        },
        true,
    ))
}

fn truncate(input: &str, width: usize) -> String {
    if input.chars().count() <= width {
        input.to_string()
    } else {
        format!(
            "{}...",
            input
                .chars()
                .take(width.saturating_sub(3))
                .collect::<String>()
        )
    }
}

fn compact_diff(diff: &str, max_lines: usize) -> Option<String> {
    if !diff.lines().any(|line| line.starts_with("diff --git")) {
        return None;
    }
    let mut result = Vec::new();
    let mut current = String::new();
    let mut added = 0;
    let mut removed = 0;
    let mut in_hunk = false;
    let mut shown = 0;
    for line in diff.lines() {
        if line.starts_with("diff --git") {
            if !current.is_empty() && (added > 0 || removed > 0) {
                result.push(format!("  +{added} -{removed}"));
            }
            current = line.split(" b/").nth(1).unwrap_or("unknown").to_string();
            result.push(format!("\n{current}"));
            added = 0;
            removed = 0;
            in_hunk = false;
            shown = 0;
        } else if line.starts_with("@@") {
            in_hunk = true;
            shown = 0;
            result.push(format!("  {line}"));
        } else if in_hunk {
            let changed = (line.starts_with('+') && !line.starts_with("+++"))
                || (line.starts_with('-') && !line.starts_with("---"));
            if line.starts_with('+') && !line.starts_with("+++") {
                added += 1;
            }
            if line.starts_with('-') && !line.starts_with("---") {
                removed += 1;
            }
            if changed && shown < 100 {
                result.push(format!("  {line}"));
                shown += 1;
            }
        }
        if result.len() >= max_lines {
            result.push("... (more changes truncated)".into());
            break;
        }
    }
    if !current.is_empty() && (added > 0 || removed > 0) {
        result.push(format!("  +{added} -{removed}"));
    }
    Some(result.join("\n").trim_start().to_string())
}

fn filter_log(output: &str) -> Option<String> {
    let lines = output.lines().collect::<Vec<_>>();
    if lines.is_empty() {
        return None;
    }
    Some(
        lines
            .into_iter()
            .take(50)
            .map(|line| truncate(line, 120))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

fn filter_cargo_build(output: &str, label: &str, exit_code: Option<i32>) -> Option<String> {
    let mut diagnostics = Vec::new();
    let mut compiled = 0;
    let mut finished = None;
    for line in output.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("Compiling ") || trimmed.starts_with("Checking ") {
            compiled += 1;
        }
        if trimmed.starts_with("Finished ") {
            finished = Some(trimmed.to_string());
        }
        if trimmed.starts_with("error[")
            || trimmed.starts_with("error:")
            || trimmed.starts_with("warning:")
            || trimmed.starts_with("--> ")
        {
            diagnostics.push(line.to_string());
        }
        if trimmed.starts_with('{')
            && let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed)
            && value.get("reason").and_then(|value| value.as_str()) == Some("compiler-message")
            && let Some(rendered) = value
                .pointer("/message/rendered")
                .and_then(|value| value.as_str())
        {
            diagnostics.push(rendered.trim().to_string());
        }
    }
    if diagnostics.is_empty() && exit_code.unwrap_or_default() == 0 {
        return Some(format!(
            "cargo {label} ({compiled} crates compiled){}",
            finished.map(|line| format!("\n{line}")).unwrap_or_default()
        ));
    }
    if diagnostics.is_empty() {
        return None;
    }
    Some(format!(
        "cargo {label}: {} diagnostic lines\n{}",
        diagnostics.len(),
        diagnostics
            .into_iter()
            .take(40)
            .collect::<Vec<_>>()
            .join("\n")
    ))
}

fn filter_cargo_test(output: &str) -> Option<String> {
    let summaries = output
        .lines()
        .filter(|line| line.starts_with("test result:"))
        .collect::<Vec<_>>();
    let failures = output
        .split("failures:")
        .nth(1)
        .and_then(|section| section.split("test result:").next())
        .unwrap_or("")
        .trim();
    if summaries.is_empty() {
        return filter_cargo_build(output, "test", Some(1));
    }
    if !failures.is_empty() {
        return Some(format!(
            "FAILURES:\n{}\n{}",
            truncate(failures, 3000),
            summaries.join("\n")
        ));
    }
    static SUMMARY: OnceLock<Regex> = OnceLock::new();
    let summary = SUMMARY.get_or_init(|| Regex::new(r"^test result: ok\.\s+(\d+) passed;\s+\d+ failed;\s+(\d+) ignored;\s+\d+ measured;\s+(\d+) filtered out(?:;\s+finished in ([\d.]+)s)?$").expect("cargo summary regex is valid"));
    let mut passed = 0;
    let mut ignored = 0;
    let mut filtered_out = 0;
    let mut duration = 0.0;
    let mut has_duration = true;
    for line in &summaries {
        let captures = summary.captures(line)?;
        passed += captures[1].parse::<usize>().ok()?;
        ignored += captures[2].parse::<usize>().ok()?;
        filtered_out += captures[3].parse::<usize>().ok()?;
        if let Some(value) = captures.get(4) {
            duration += value.as_str().parse::<f64>().ok()?;
        } else {
            has_duration = false;
        }
    }
    let mut counts = vec![format!("{passed} passed")];
    if ignored > 0 {
        counts.push(format!("{ignored} ignored"));
    }
    if filtered_out > 0 {
        counts.push(format!("{filtered_out} filtered out"));
    }
    let suites = if summaries.len() == 1 {
        "1 suite".to_string()
    } else {
        format!("{} suites", summaries.len())
    };
    Some(if has_duration {
        format!(
            "cargo test: {} ({suites}, {duration:.2}s)",
            counts.join(", ")
        )
    } else {
        format!("cargo test: {} ({suites})", counts.join(", "))
    })
}

fn filter_clippy(output: &str, exit_code: Option<i32>) -> Option<String> {
    let issues = output
        .lines()
        .filter(|line| {
            let line = line.trim_start();
            line.starts_with("error:")
                || line.starts_with("error[")
                || line.starts_with("warning:")
                || line.starts_with("--> ")
        })
        .collect::<Vec<_>>();
    if issues.is_empty() {
        (exit_code.unwrap_or_default() == 0).then(|| "cargo clippy: No issues found".into())
    } else {
        Some(format!(
            "cargo clippy: {} issue lines\n{}",
            issues.len(),
            issues.into_iter().take(60).collect::<Vec<_>>().join("\n")
        ))
    }
}

fn filter_cargo_install(output: &str) -> Option<String> {
    let errors = output
        .lines()
        .filter(|line| line.trim_start().starts_with("error"))
        .collect::<Vec<_>>();
    if !errors.is_empty() {
        return Some(format!(
            "cargo install: {} errors\n{}",
            errors.len(),
            errors.join("\n")
        ));
    }
    let installed = output.lines().find(|line| {
        line.trim_start().starts_with("Installed")
            || line.trim_start().starts_with("Ignored package")
    })?;
    Some(format!("cargo install: {}", installed.trim()))
}

fn filter_nextest(output: &str) -> Option<String> {
    let summary = output
        .lines()
        .find(|line| line.trim().starts_with("Summary"))?
        .trim();
    let failures = output
        .lines()
        .filter(|line| line.trim().starts_with("FAIL"))
        .take(20)
        .collect::<Vec<_>>();
    Some(if failures.is_empty() {
        format!("cargo nextest: {summary}")
    } else {
        format!("{}\ncargo nextest: {summary}", failures.join("\n"))
    })
}

fn filter_tsc(output: &str) -> Option<String> {
    static ERROR: OnceLock<Regex> = OnceLock::new();
    let regex = ERROR.get_or_init(|| {
        Regex::new(r"^(.+?)\((\d+),\d+\): error (TS\d+): (.+)$")
            .expect("TypeScript error regex is valid")
    });
    let errors = output
        .lines()
        .filter_map(|line| regex.captures(line))
        .map(|captures| {
            (
                captures[1].to_string(),
                captures[2].to_string(),
                captures[3].to_string(),
                truncate(&captures[4], 120),
            )
        })
        .collect::<Vec<_>>();
    if errors.is_empty() {
        return output
            .contains("Found 0 errors")
            .then(|| "TypeScript: No errors found".into());
    }
    let mut files = BTreeMap::<String, Vec<(String, String, String)>>::new();
    for (file, line, code, message) in errors {
        files.entry(file).or_default().push((line, code, message));
    }
    let total = files.values().map(Vec::len).sum::<usize>();
    let mut result = format!("TypeScript: {total} errors in {} files", files.len());
    for (file, errors) in files {
        result.push_str(&format!("\n\n{file} ({} errors)", errors.len()));
        for (line, code, message) in errors {
            result.push_str(&format!("\n  L{line}: {code} {message}"));
        }
    }
    Some(result)
}

fn filter_npm(output: &str, exit_code: Option<i32>) -> Option<String> {
    let lines = output
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            !(trimmed.is_empty()
                || trimmed.starts_with("npm WARN")
                || trimmed.starts_with("npm notice")
                || line.starts_with('>') && line.contains('@'))
        })
        .collect::<Vec<_>>();
    if lines.is_empty() {
        (exit_code.unwrap_or_default() == 0).then(|| "ok".into())
    } else {
        Some(lines.join("\n"))
    }
}

fn filter_pnpm_install(output: &str, exit_code: Option<i32>) -> Option<String> {
    let lines = output
        .lines()
        .filter(|line| {
            let line = line.trim();
            line.contains("ERR")
                || line.to_ascii_lowercase().contains("error")
                || line.contains("packages in")
                || line.contains("dependencies")
                || line.starts_with('+')
                || line.starts_with('-')
        })
        .collect::<Vec<_>>();
    if lines.is_empty() {
        (exit_code.unwrap_or_default() == 0).then(|| "ok".into())
    } else {
        Some(lines.join("\n"))
    }
}

#[derive(Deserialize)]
struct PnpmNode {
    name: Option<String>,
    version: Option<String>,
    #[serde(default)]
    dependencies: BTreeMap<String, PnpmNode>,
    #[serde(rename = "devDependencies", default)]
    dev_dependencies: BTreeMap<String, PnpmNode>,
}
fn filter_pnpm_list(output: &str) -> Option<String> {
    let roots: Vec<PnpmNode> = serde_json::from_str(output).ok()?;
    let mut items = Vec::new();
    fn collect(nodes: &BTreeMap<String, PnpmNode>, items: &mut Vec<String>) {
        for (name, node) in nodes {
            if let Some(version) = &node.version {
                items.push(format!("{name} {version}"));
            }
            collect(&node.dependencies, items);
            collect(&node.dev_dependencies, items);
        }
    }
    for root in &roots {
        if let (Some(name), Some(version)) = (&root.name, &root.version) {
            items.push(format!("{name} {version}"));
        }
        collect(&root.dependencies, &mut items);
        collect(&root.dev_dependencies, &mut items);
    }
    Some(format!(
        "{} packages\n{}",
        items.len(),
        items.into_iter().take(100).collect::<Vec<_>>().join("\n")
    ))
}

#[derive(Deserialize)]
struct Outdated {
    current: String,
    latest: String,
}
fn filter_pnpm_outdated(output: &str) -> Option<String> {
    let packages: BTreeMap<String, Outdated> = serde_json::from_str(output).ok()?;
    if packages.is_empty() {
        return Some("All packages up-to-date".into());
    }
    Some(format!(
        "{} outdated packages\n{}",
        packages.len(),
        packages
            .into_iter()
            .take(50)
            .map(|(name, package)| format!("{name}: {} -> {}", package.current, package.latest))
            .collect::<Vec<_>>()
            .join("\n")
    ))
}

#[derive(Deserialize)]
struct EslintResult {
    #[serde(rename = "filePath")]
    file_path: String,
    messages: Vec<EslintMessage>,
    #[serde(rename = "errorCount")]
    error_count: usize,
    #[serde(rename = "warningCount")]
    warning_count: usize,
}
#[derive(Deserialize)]
struct EslintMessage {
    #[serde(rename = "ruleId")]
    rule_id: Option<String>,
}
fn filter_eslint(output: &str) -> Option<String> {
    let results: Vec<EslintResult> = serde_json::from_str(output).ok()?;
    let errors = results
        .iter()
        .map(|result| result.error_count)
        .sum::<usize>();
    let warnings = results
        .iter()
        .map(|result| result.warning_count)
        .sum::<usize>();
    if errors + warnings == 0 {
        return Some("ESLint: No issues found".into());
    }
    let files = results
        .iter()
        .filter(|result| !result.messages.is_empty())
        .map(|result| {
            format!(
                "{} ({} issues: {})",
                result.file_path,
                result.messages.len(),
                result
                    .messages
                    .iter()
                    .filter_map(|message| message.rule_id.as_deref())
                    .take(5)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
        .take(20)
        .collect::<Vec<_>>();
    Some(format!(
        "ESLint: {errors} errors, {warnings} warnings in {} files\n{}",
        files.len(),
        files.join("\n")
    ))
}

fn filter_prettier(output: &str) -> Option<String> {
    if output.contains("All matched files use Prettier") {
        return Some("Prettier: All files formatted correctly".into());
    }
    let files = output
        .lines()
        .filter(|line| {
            [
                ".ts", ".tsx", ".js", ".jsx", ".json", ".md", ".css", ".scss",
            ]
            .iter()
            .any(|suffix| line.trim().ends_with(suffix))
        })
        .collect::<Vec<_>>();
    (!files.is_empty()).then(|| {
        format!(
            "Prettier: {} files need formatting\n{}",
            files.len(),
            files.into_iter().take(20).collect::<Vec<_>>().join("\n")
        )
    })
}

fn filter_next(output: &str) -> Option<String> {
    let routes = output
        .lines()
        .filter(|line| line.trim_start().starts_with(['○', '●', '◐', 'λ']))
        .count();
    let errors = output
        .lines()
        .filter(|line| line.to_ascii_lowercase().contains("error"))
        .count();
    let warnings = output
        .lines()
        .filter(|line| line.to_ascii_lowercase().contains("warning"))
        .count();
    (routes > 0 || output.contains("Compiled") || errors > 0)
        .then(|| format!("Next.js Build\n{routes} routes\nErrors: {errors} | Warnings: {warnings}"))
}

#[derive(Deserialize)]
struct JsTestOutput {
    #[serde(rename = "numTotalTests")]
    _total: usize,
    #[serde(rename = "numPassedTests")]
    passed: usize,
    #[serde(rename = "numFailedTests")]
    failed: usize,
    #[serde(rename = "numPendingTests", default)]
    skipped: usize,
}
fn filter_js_test(output: &str, _filter: NativeFilter) -> Option<String> {
    let parsed: JsTestOutput = serde_json::from_str(output).ok()?;
    let mut summary = format!("PASS ({}) FAIL ({})", parsed.passed, parsed.failed);
    if parsed.skipped > 0 {
        summary.push_str(&format!(" skipped ({})", parsed.skipped));
    }
    Some(summary)
}

#[derive(Deserialize)]
struct PlaywrightOutput {
    stats: PlaywrightStats,
}
#[derive(Deserialize)]
struct PlaywrightStats {
    expected: usize,
    unexpected: usize,
    skipped: usize,
    duration: f64,
}
fn filter_playwright(output: &str) -> Option<String> {
    let parsed: PlaywrightOutput = serde_json::from_str(output).ok()?;
    Some(format!(
        "Playwright: {} passed, {} failed, {} skipped ({:.2}s)",
        parsed.stats.expected,
        parsed.stats.unexpected,
        parsed.stats.skipped,
        parsed.stats.duration / 1000.0
    ))
}

#[derive(Deserialize)]
struct GoEvent {
    #[serde(rename = "Action")]
    action: String,
    #[serde(rename = "Package")]
    package: Option<String>,
    #[serde(rename = "Test")]
    test: Option<String>,
    #[serde(rename = "Output")]
    output: Option<String>,
}
fn filter_go_test(output: &str, exit_code: Option<i32>) -> Option<String> {
    let mut passed = 0;
    let mut failed = 0;
    let mut skipped = 0;
    let mut packages = std::collections::HashSet::new();
    let mut failures = Vec::new();
    let mut parsed = 0;
    for line in output.lines() {
        let Ok(event) = serde_json::from_str::<GoEvent>(line) else {
            continue;
        };
        parsed += 1;
        if let Some(package) = event.package {
            packages.insert(package);
        }
        match (event.action.as_str(), event.test) {
            ("pass", Some(_)) => passed += 1,
            ("fail", Some(test)) => {
                failed += 1;
                failures.push(format!(
                    "[FAIL] {test} {}",
                    event.output.unwrap_or_default().trim()
                ));
            }
            ("skip", Some(_)) => skipped += 1,
            _ => {}
        }
    }
    if parsed == 0 || exit_code.unwrap_or_default() != 0 && failed == 0 {
        return None;
    }
    if failed == 0 && skipped == 0 {
        return Some(format!(
            "Go test: {passed} passed in {} packages",
            packages.len()
        ));
    }
    Some(format!(
        "Go test: {passed} passed, {failed} failed{} in {} packages{}",
        if skipped > 0 {
            format!(", {skipped} skipped")
        } else {
            String::new()
        },
        packages.len(),
        if failures.is_empty() {
            String::new()
        } else {
            format!(
                "\n{}",
                failures.into_iter().take(20).collect::<Vec<_>>().join("\n")
            )
        }
    ))
}

fn filter_go_build(output: &str, exit_code: Option<i32>) -> Option<String> {
    let errors = output
        .lines()
        .filter(|line| {
            line.contains(".go:") || line.starts_with("go: error") || line.starts_with("undefined:")
        })
        .collect::<Vec<_>>();
    if errors.is_empty() {
        (exit_code.unwrap_or_default() == 0).then(|| "Go build: Success".into())
    } else {
        Some(format!(
            "Go build: {} errors\n{}",
            errors.len(),
            errors.into_iter().take(30).collect::<Vec<_>>().join("\n")
        ))
    }
}
fn filter_go_vet(output: &str, exit_code: Option<i32>) -> Option<String> {
    let issues = output
        .lines()
        .filter(|line| line.contains(".go:") && !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>();
    if issues.is_empty() {
        (exit_code.unwrap_or_default() == 0).then(|| "Go vet: No issues found".into())
    } else {
        Some(format!(
            "Go vet: {} issues\n{}",
            issues.len(),
            issues.join("\n")
        ))
    }
}

#[derive(Deserialize)]
struct GolangciOutput {
    #[serde(rename = "Issues", alias = "issues", default)]
    issues: Vec<GolangciIssue>,
}
#[derive(Deserialize)]
struct GolangciIssue {
    #[serde(rename = "FromLinter", alias = "fromLinter")]
    linter: String,
    #[serde(rename = "Pos", alias = "pos")]
    pos: GolangciPosition,
}
#[derive(Deserialize)]
struct GolangciPosition {
    #[serde(rename = "Filename", alias = "filename")]
    filename: String,
}
fn filter_golangci(output: &str) -> Option<String> {
    let parsed: GolangciOutput = serde_json::from_str(output).ok()?;
    if parsed.issues.is_empty() {
        return Some("golangci-lint: No issues found".into());
    }
    let mut counts = HashMap::new();
    for issue in &parsed.issues {
        *counts
            .entry((&issue.linter, &issue.pos.filename))
            .or_insert(0usize) += 1;
    }
    Some(format!(
        "golangci-lint: {} issues\n{}",
        parsed.issues.len(),
        counts
            .into_iter()
            .take(30)
            .map(|((linter, file), count)| format!("{file}: {linter} ({count}x)"))
            .collect::<Vec<_>>()
            .join("\n")
    ))
}

fn summary_counts(output: &str) -> Option<(usize, usize, usize)> {
    fn count(output: &str, label: &str) -> usize {
        let pattern = Regex::new(&format!(r"(\d+)\s+{label}\b")).expect("summary regex is valid");
        pattern
            .captures_iter(output)
            .last()
            .and_then(|captures| captures[1].parse().ok())
            .unwrap_or(0)
    }
    let passed = count(output, "passed");
    let failed = count(output, "failed");
    let skipped = count(output, "skipped");
    (passed + failed + skipped > 0).then_some((passed, failed, skipped))
}
fn filter_pytest(output: &str, exit_code: Option<i32>) -> Option<String> {
    let (passed, failed, skipped) = summary_counts(output).unwrap_or((0, 0, 0));
    if passed + failed + skipped == 0 {
        return output
            .contains("collected 0 items")
            .then(|| "Pytest: No tests collected".into());
    }
    if exit_code.unwrap_or_default() != 0 && failed == 0 {
        return None;
    }
    if failed == 0 && skipped == 0 {
        return Some(format!("Pytest: {passed} passed"));
    }
    let failures = output
        .lines()
        .filter(|line| line.starts_with("FAILED ") || line.trim_start().starts_with('E'))
        .take(30)
        .collect::<Vec<_>>();
    let mut counts = format!("Pytest: {passed} passed, {failed} failed");
    if skipped > 0 {
        counts.push_str(&format!(", {skipped} skipped"));
    }
    Some(format!(
        "{counts}{}",
        if failures.is_empty() {
            String::new()
        } else {
            format!("\n{}", failures.join("\n"))
        }
    ))
}

#[derive(Deserialize)]
struct RuffDiagnostic {
    code: String,
    message: String,
    filename: String,
    location: RuffLocation,
}
#[derive(Deserialize)]
struct RuffLocation {
    row: usize,
    column: usize,
}
fn filter_ruff(output: &str) -> Option<String> {
    let diagnostics: Vec<RuffDiagnostic> = serde_json::from_str(output).ok()?;
    if diagnostics.is_empty() {
        return Some("Ruff: No issues found".into());
    }
    Some(format!(
        "Ruff: {} issues\n{}",
        diagnostics.len(),
        diagnostics
            .into_iter()
            .take(50)
            .map(|d| format!(
                "{}:{}:{} {} {}",
                d.filename,
                d.location.row,
                d.location.column,
                d.code,
                truncate(&d.message, 100)
            ))
            .collect::<Vec<_>>()
            .join("\n")
    ))
}
fn filter_ruff_format(output: &str) -> Option<String> {
    if output.contains("left unchanged") && !output.to_ascii_lowercase().contains("would reformat")
    {
        return Some("Ruff format: All files formatted correctly".into());
    }
    let files = output
        .lines()
        .filter_map(|line| {
            line.to_ascii_lowercase()
                .contains("would reformat:")
                .then(|| line.split_once(':').map(|(_, file)| file.trim()))
                .flatten()
        })
        .collect::<Vec<_>>();
    (!files.is_empty()).then(|| {
        format!(
            "Ruff format: {} files need formatting\n{}",
            files.len(),
            files.join("\n")
        )
    })
}

fn filter_mypy(output: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let regex = RE.get_or_init(|| {
        Regex::new(r"^(.+?):(\d+)(?::\d+)?: (error|warning): (.+?)(?:\s+\[(.+)\])?$").unwrap()
    });
    let errors = output
        .lines()
        .filter_map(|line| regex.captures(line))
        .map(|caps| {
            format!(
                "{}:{} [{}] {}",
                &caps[1],
                &caps[2],
                caps.get(5).map(|m| m.as_str()).unwrap_or(""),
                truncate(&caps[4], 120)
            )
        })
        .collect::<Vec<_>>();
    if errors.is_empty() {
        output
            .contains("no issues found")
            .then(|| "mypy: No issues found".into())
    } else {
        Some(format!(
            "mypy: {} errors\n{}",
            errors.len(),
            errors.join("\n")
        ))
    }
}

#[derive(Deserialize)]
struct PipPackage {
    name: String,
    version: String,
    latest_version: Option<String>,
}
fn filter_pip(output: &str, outdated: bool) -> Option<String> {
    let packages: Vec<PipPackage> = serde_json::from_str(output).ok()?;
    if packages.is_empty() {
        return Some(
            if outdated {
                "pip outdated: All packages up to date"
            } else {
                "pip list: No packages installed"
            }
            .into(),
        );
    }
    Some(format!(
        "pip {}: {} packages\n{}",
        if outdated { "outdated" } else { "list" },
        packages.len(),
        packages
            .into_iter()
            .take(100)
            .map(|package| if outdated {
                format!(
                    "{} ({} -> {})",
                    package.name,
                    package.version,
                    package.latest_version.as_deref().unwrap_or("unknown")
                )
            } else {
                format!("{} ({})", package.name, package.version)
            })
            .collect::<Vec<_>>()
            .join("\n")
    ))
}

fn filter_uv_run(output: &str, exit_code: Option<i32>) -> Option<String> {
    let errors = output
        .lines()
        .filter(|line| {
            let lower = line.to_ascii_lowercase();
            lower.contains("error")
                || lower.contains("failed")
                || lower.contains("exception")
                || lower.contains("panic")
                || line.starts_with("Traceback")
                || line.trim_start().starts_with("File \"")
        })
        .take(40)
        .collect::<Vec<_>>();
    if errors.is_empty() {
        (exit_code.unwrap_or_default() == 0).then(|| "ok".into())
    } else {
        Some(errors.join("\n"))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn filters_representative_pinned_ecosystem_fixtures() {
        let fixtures = [
            (
                NativeFilter::CargoTest,
                "running 2 tests\ntest one ... ok\ntest two ... ok\ntest result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out",
                "cargo test: 2 passed (1 suite)",
            ),
            (
                NativeFilter::Vitest,
                "{\"numTotalTests\":13,\"numPassedTests\":13,\"numFailedTests\":0,\"numPendingTests\":0,\"testResults\":[],\"startTime\":1000}",
                "PASS (13) FAIL (0)",
            ),
            (
                NativeFilter::TypeScript,
                "src/main.ts(12,5): error TS2322: Type 'string' is not assignable to type 'number'.",
                "TypeScript: 1 errors in 1 files\n\nsrc/main.ts (1 errors)\n  L12: TS2322 Type 'string' is not assignable to type 'number'.",
            ),
            (
                NativeFilter::GoTest,
                "{\"Action\":\"run\",\"Package\":\"example.com/foo\",\"Test\":\"TestBar\"}\n{\"Action\":\"pass\",\"Package\":\"example.com/foo\",\"Test\":\"TestBar\"}\n{\"Action\":\"pass\",\"Package\":\"example.com/foo\"}",
                "Go test: 1 passed in 1 packages",
            ),
            (
                NativeFilter::Pytest,
                "=== test session starts ===\ncollected 5 items\ntests/test_foo.py ..... [100%]\n=== 5 passed in 0.50s ===",
                "Pytest: 5 passed",
            ),
        ];
        let actual = fixtures
            .iter()
            .map(|(filter, input, _)| apply(*filter, input, "", Some(0)).unwrap().0)
            .collect::<Vec<_>>();
        let expected = fixtures
            .iter()
            .map(|(_, _, expected)| expected.to_string())
            .collect::<Vec<_>>();
        assert_eq!(actual, expected);
    }
    #[test]
    fn detects_supported_native_command_matrix() {
        let fixture = [
            ("ls", Some(NativeFilter::Ls)),
            ("tree .", Some(NativeFilter::Tree)),
            ("find . -name *.rs", Some(NativeFilter::Find)),
            ("grep TODO src", Some(NativeFilter::Grep)),
            ("rg TODO src", Some(NativeFilter::Rg)),
            ("cat file.rs", Some(NativeFilter::Read)),
            ("head -20 file.rs", Some(NativeFilter::Read)),
            ("tail -n 20 file.rs", Some(NativeFilter::Read)),
            ("git diff", Some(NativeFilter::GitDiff)),
            ("git log --oneline", Some(NativeFilter::GitLog)),
            ("git log", None),
            ("git status", None),
            ("cargo build", Some(NativeFilter::CargoBuild)),
            ("cargo check", Some(NativeFilter::CargoCheck)),
            ("cargo clippy", Some(NativeFilter::CargoClippy)),
            ("cargo test", Some(NativeFilter::CargoTest)),
            ("cargo install demo", Some(NativeFilter::CargoInstall)),
            ("cargo nextest run", Some(NativeFilter::CargoNextest)),
            ("tsc --noEmit", Some(NativeFilter::TypeScript)),
            ("npm run build", Some(NativeFilter::Npm)),
            ("npm test", Some(NativeFilter::Npm)),
            ("npm install", None),
            ("npx cowsay hello", Some(NativeFilter::Npm)),
            ("pnpm install", Some(NativeFilter::PnpmInstall)),
            ("pnpm list", Some(NativeFilter::PnpmList)),
            ("pnpm outdated", Some(NativeFilter::PnpmOutdated)),
            ("eslint src", Some(NativeFilter::Eslint)),
            ("prettier --check .", Some(NativeFilter::Prettier)),
            ("next build", Some(NativeFilter::NextBuild)),
            ("vitest run", Some(NativeFilter::Vitest)),
            ("jest", Some(NativeFilter::Jest)),
            ("playwright test", Some(NativeFilter::Playwright)),
            ("go test ./...", Some(NativeFilter::GoTest)),
            ("go build ./...", Some(NativeFilter::GoBuild)),
            ("go vet ./...", Some(NativeFilter::GoVet)),
            ("golangci-lint run", None),
            (
                "golangci-lint run --out-format=json",
                Some(NativeFilter::Golangci),
            ),
            ("pytest", Some(NativeFilter::Pytest)),
            ("ruff check", Some(NativeFilter::RuffCheck)),
            ("ruff format", Some(NativeFilter::RuffFormat)),
            ("mypy src", Some(NativeFilter::Mypy)),
            ("pip list", Some(NativeFilter::PipList)),
            ("pip list --outdated", Some(NativeFilter::PipOutdated)),
            ("pip outdated", None),
            ("uv run pytest", Some(NativeFilter::UvRun)),
        ];
        let actual = fixture
            .iter()
            .map(|(command, _)| {
                detect(&crate::core::bash_filter::command::classify(command).unwrap())
            })
            .collect::<Vec<_>>();
        let expected = fixture
            .iter()
            .map(|(_, filter)| *filter)
            .collect::<Vec<_>>();
        assert_eq!(actual, expected);
    }
    #[test]
    fn every_native_dispatcher_has_a_success_fixture() {
        let fixtures = [
            (
                NativeFilter::GitDiff,
                "diff --git a/a b/a\n@@ -1 +1 @@\n-old\n+new",
                Some(0),
            ),
            (
                NativeFilter::GitLog,
                "abc123 first commit\ndef456 second commit",
                Some(0),
            ),
            (
                NativeFilter::CargoBuild,
                "Compiling demo\nFinished `dev` profile",
                Some(0),
            ),
            (
                NativeFilter::CargoCheck,
                "Checking demo\nFinished `dev` profile",
                Some(0),
            ),
            (NativeFilter::CargoClippy, "Finished `dev` profile", Some(0)),
            (
                NativeFilter::CargoTest,
                "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out",
                Some(0),
            ),
            (
                NativeFilter::CargoInstall,
                "Installed package `demo v1.0.0`",
                Some(0),
            ),
            (
                NativeFilter::CargoNextest,
                "Summary [ 0.01s] 1 test run: 1 passed",
                Some(0),
            ),
            (
                NativeFilter::TypeScript,
                "src/main.ts(1,1): error TS2322: mismatch",
                Some(1),
            ),
            (
                NativeFilter::Npm,
                "> demo@1.0.0 build\nnpm WARN old\nbuild complete",
                Some(0),
            ),
            (
                NativeFilter::PnpmInstall,
                "dependencies:\n+ serde 1.0",
                Some(0),
            ),
            (
                NativeFilter::PnpmList,
                "[{\"name\":\"demo\",\"version\":\"1.0.0\"}]",
                Some(0),
            ),
            (
                NativeFilter::PnpmOutdated,
                "{\"demo\":{\"current\":\"1.0.0\",\"latest\":\"2.0.0\"}}",
                Some(1),
            ),
            (
                NativeFilter::Eslint,
                "[{\"filePath\":\"src/a.ts\",\"messages\":[{\"ruleId\":\"eqeqeq\"}],\"errorCount\":1,\"warningCount\":0}]",
                Some(1),
            ),
            (
                NativeFilter::Prettier,
                "All matched files use Prettier code style!",
                Some(0),
            ),
            (
                NativeFilter::NextBuild,
                "Compiled successfully\n○ /",
                Some(0),
            ),
            (
                NativeFilter::Vitest,
                "{\"numTotalTests\":1,\"numPassedTests\":1,\"numFailedTests\":0,\"numPendingTests\":0}",
                Some(0),
            ),
            (
                NativeFilter::Jest,
                "{\"numTotalTests\":1,\"numPassedTests\":1,\"numFailedTests\":0,\"numPendingTests\":0}",
                Some(0),
            ),
            (
                NativeFilter::Playwright,
                "{\"stats\":{\"expected\":1,\"unexpected\":0,\"skipped\":0,\"duration\":1000.0}}",
                Some(0),
            ),
            (
                NativeFilter::GoTest,
                "{\"Action\":\"pass\",\"Package\":\"example.com/demo\",\"Test\":\"TestOne\"}",
                Some(0),
            ),
            (NativeFilter::GoBuild, "", Some(0)),
            (NativeFilter::GoVet, "", Some(0)),
            (NativeFilter::Golangci, "{\"Issues\":[]}", Some(0)),
            (NativeFilter::Pytest, "1 passed in 0.01s", Some(0)),
            (NativeFilter::RuffCheck, "[]", Some(0)),
            (NativeFilter::RuffFormat, "1 file left unchanged", Some(0)),
            (
                NativeFilter::Mypy,
                "Success: no issues found in 1 source file",
                Some(0),
            ),
            (
                NativeFilter::PipList,
                "[{\"name\":\"pip\",\"version\":\"24.0\"}]",
                Some(0),
            ),
            (
                NativeFilter::PipOutdated,
                "[{\"name\":\"pip\",\"version\":\"24.0\",\"latest_version\":\"25.0\"}]",
                Some(0),
            ),
            (NativeFilter::UvRun, "completed", Some(0)),
        ];
        let actual = fixtures
            .iter()
            .map(|(filter, input, exit_code)| {
                (*filter, apply(*filter, input, "", *exit_code).is_some())
            })
            .collect::<Vec<_>>();
        let expected = fixtures
            .iter()
            .map(|(filter, _, _)| (*filter, true))
            .collect::<Vec<_>>();
        assert_eq!(actual, expected);
    }
    #[test]
    fn failed_commands_without_parseable_diagnostics_pass_through() {
        let fixtures = [
            NativeFilter::CargoClippy,
            NativeFilter::Npm,
            NativeFilter::PnpmInstall,
            NativeFilter::GoTest,
            NativeFilter::GoBuild,
            NativeFilter::GoVet,
            NativeFilter::Pytest,
            NativeFilter::UvRun,
        ];
        let actual = fixtures
            .into_iter()
            .map(|filter| {
                apply(filter, "unrecognized failure", "", Some(2)).is_none_or(
                    |(stdout, stderr, _)| stdout == "unrecognized failure" && stderr.is_empty(),
                )
            })
            .collect::<Vec<_>>();
        let expected = vec![true; fixtures.len()];
        assert_eq!(actual, expected);
    }
    #[test]
    fn json_parsers_reject_unstructured_output() {
        let fixture = [
            NativeFilter::PnpmList,
            NativeFilter::PnpmOutdated,
            NativeFilter::Eslint,
            NativeFilter::Vitest,
            NativeFilter::Jest,
            NativeFilter::Playwright,
            NativeFilter::GoTest,
            NativeFilter::Golangci,
            NativeFilter::RuffCheck,
            NativeFilter::PipList,
            NativeFilter::PipOutdated,
        ];
        let actual = fixture
            .into_iter()
            .map(|filter| apply(filter, "not structured output", "", Some(1)).is_none())
            .collect::<Vec<_>>();
        let expected = vec![true; fixture.len()];
        assert_eq!(actual, expected);
    }
    #[test]
    fn combined_filters_preserve_upstream_stream_concatenation() {
        let actual = apply(
            NativeFilter::CargoBuild,
            "Compiling one\n",
            "error: failed",
            Some(1),
        )
        .unwrap();
        let expected = "cargo build: 1 diagnostic lines\nerror: failed";
        assert_eq!(actual.0, expected);
        assert_eq!(actual.1, "");
    }
}
