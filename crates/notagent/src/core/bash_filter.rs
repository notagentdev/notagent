//! The bash filter: conservative, process-free compaction of shell output
//! before it enters the agent context.
//!
//! Port addition (user decision 2026-08-17, v0.1.20), off by default and
//! toggled with `/bash-filter on|off`. Provenance of the filter inventory is
//! recorded in the workspace `NOTICE`.
//!
//! Three relationships to the child process, chosen per command:
//!
//! - **Filter only.** The command runs as written and its output is compacted
//!   afterwards — every declarative filter and most native ones.
//! - **Rewrite, then filter.** The invocation is changed so the tool emits a
//!   parseable format, then the output is compacted. What the user and the
//!   transcript see is still the command as written.
//! - **Execution override.** `find` and the read commands are answered from
//!   the filesystem; no child process is spawned at all.
//!
//! Whatever the route, filtering never costs the caller information it would
//! otherwise have had: an unchanged result, an empty result where raw output
//! existed, a result that estimates larger than the raw, and a panic all send
//! the raw streams through untouched.

mod builtin_filters;
mod command;
mod native;
mod system;
mod toml_filter;

use std::panic::{AssertUnwindSafe, catch_unwind};

use command::{ClassifiedCommand, append_after, classify};
use native::{NativeFilter, detect};
use toml_filter::PipelineLossiness;

/// Raw output produced by a process-free system-command adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutedOutput {
    /// Captured standard output.
    pub stdout: String,
    /// Captured standard error.
    pub stderr: String,
    /// Process-compatible exit code.
    pub exit_code: Option<i32>,
}
impl ExecutedOutput {
    fn success(stdout: String) -> Self {
        Self {
            stdout,
            stderr: String::new(),
            exit_code: Some(0),
        }
    }
    fn failure(stderr: String) -> Self {
        Self {
            stdout: String::new(),
            stderr,
            exit_code: Some(1),
        }
    }
}
/// A safely classified shell invocation and its optional execution rewrite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedInvocation {
    original: String,
    execution: String,
    command: Option<ClassifiedCommand>,
    native_filter: Option<NativeFilter>,
    /// Whether a filter was found for this command and then turned down — a
    /// user-named output format, a benchmark run. Such a refusal outranks
    /// anything the output itself might suggest.
    declined: bool,
}
impl PreparedInvocation {
    /// Returns the original command supplied by the tool caller.
    #[must_use]
    pub fn original(&self) -> &str {
        &self.original
    }
    /// Returns the command that may safely be executed.
    #[must_use]
    pub fn execution(&self) -> &str {
        &self.execution
    }
    /// Returns whether preparation changed the execution invocation.
    #[must_use]
    pub fn rewritten(&self) -> bool {
        self.original != self.execution
    }
    /// Returns whether this invocation uses a filesystem-backed adapter instead of a child process.
    #[must_use]
    pub fn uses_execution_override(&self) -> bool {
        self.native_filter
            .is_some_and(NativeFilter::uses_execution_override)
    }
    /// Returns whether process output must remain buffered until system filtering completes.
    #[must_use]
    pub fn buffers_live_output(&self) -> bool {
        self.native_filter.is_some_and(NativeFilter::is_system)
    }
}
/// Buffered stdout/stderr returned after safe filtering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilteredOutput {
    /// Filtered stdout intended for the agent context.
    pub stdout: String,
    /// Filtered stderr intended for the agent context.
    pub stderr: String,
    /// Whether either output stream differs from the raw process output.
    pub changed: bool,
    /// Whether information was removed, truncated, or streams were combined.
    pub lossy: bool,
}
/// Conservatively classifies and, on Unix, rewrites supported single commands.
///
/// Unsupported, compound, redirected, substituted, or ambiguous commands are
/// returned unchanged and later pass through filtering unchanged.
///
/// # Arguments
///
/// * `command` - Original shell command supplied by the tool caller.
#[must_use]
pub fn prepare(command: &str) -> PreparedInvocation {
    let classified = classify(command);
    let compound = classified
        .as_ref()
        .is_some_and(|classified| classified.compound);
    let detected = classified.as_ref().and_then(detect);
    let eligible = detected.filter(|filter| {
        native_filter_eligible(classified.as_ref().expect("classified command"), *filter)
    });
    // `vitest --reporter=verbose` and its kind name a format on purpose. That
    // is a refusal, and content detection must not talk them out of it.
    let declined = detected.is_some() && eligible.is_none();
    let native_filter = eligible
        // A system filter either answers from the filesystem instead of the
        // process, or reformats output whose shape it dictated itself. Behind
        // a pipe it has neither: the stage that ran last owns the bytes, and
        // `wc -l` leaves nothing an `ls` adapter could recognize. Standing
        // down also keeps such a line streaming live.
        .filter(|filter| !(compound && filter.is_system()));
    let execution = if cfg!(windows) || compound {
        // Flags appended to the first stage change what the later ones read.
        command.to_string()
    } else {
        classified
            .as_ref()
            .zip(native_filter)
            .and_then(|(classified, filter)| {
                rewrite(classified, filter)
                    .map(|rewritten| format!("{rewritten}{}", classified.suffix))
            })
            .unwrap_or_else(|| command.to_string())
    };
    PreparedInvocation {
        original: command.to_string(),
        execution,
        command: classified,
        native_filter,
        declined,
    }
}

fn native_filter_eligible(command: &ClassifiedCommand, filter: NativeFilter) -> bool {
    if filter.is_system() {
        return system::eligible(command, filter);
    }
    let values = command.tokens[command.effective_index + 1..]
        .iter()
        .map(|token| token.value.as_str())
        .collect::<Vec<_>>();
    let contains = |needle: &str| {
        values
            .iter()
            .any(|value| *value == needle || value.starts_with(&format!("{needle}=")))
    };
    match filter {
        NativeFilter::GoTest => !values.iter().any(|value| value.starts_with("-bench")),
        NativeFilter::RuffCheck => format_is_absent_or(&values, &["--output-format"], &["json"]),
        NativeFilter::PipList | NativeFilter::PipOutdated => {
            format_is_absent_or(&values, &["--format"], &["json"])
        }
        NativeFilter::PnpmList => {
            !contains("--json")
                || values
                    .iter()
                    .any(|value| *value == "--json" || *value == "--json=true")
        }
        NativeFilter::PnpmOutdated => format_is_absent_or(&values, &["--format"], &["json"]),
        NativeFilter::Eslint => format_is_absent_or(&values, &["--format", "-f"], &["json"]),
        NativeFilter::Vitest => {
            !contains("--reporter")
                && !values
                    .iter()
                    .any(|value| *value == "--watch" || *value == "--ui")
        }
        NativeFilter::Jest => {
            !contains("--reporter") && !values.iter().any(|value| value.starts_with("--watch"))
        }
        NativeFilter::Playwright => !contains("--reporter"),
        _ => true,
    }
}
fn format_is_absent_or(values: &[&str], names: &[&str], expected: &[&str]) -> bool {
    values.iter().enumerate().all(|(index, value)| {
        names.iter().all(|name| {
            if value == name {
                values
                    .get(index + 1)
                    .is_some_and(|next| expected.contains(next))
            } else if let Some(actual) = value.strip_prefix(&format!("{name}=")) {
                expected.contains(&actual)
            } else {
                true
            }
        })
    })
}
fn rewrite(command: &ClassifiedCommand, filter: NativeFilter) -> Option<String> {
    if filter.is_system() {
        return system::rewrite(command, filter);
    }
    let tokens = &command.tokens;
    let index = command.effective_index;
    let executable = tokens.get(index)?;
    let arguments = &tokens[index + 1..];
    let values = arguments
        .iter()
        .map(|token| token.value.as_str())
        .collect::<Vec<_>>();
    let contains = |needle: &str| {
        values
            .iter()
            .any(|value| *value == needle || value.starts_with(&format!("{needle}=")))
    };
    match filter {
        NativeFilter::GoTest
            if !contains("-json") && !values.iter().any(|value| value.starts_with("-bench")) =>
        {
            arguments
                .first()
                .map(|token| append_after(&command.original, token, "-json"))
        }
        NativeFilter::Pytest => {
            let mut flags = Vec::new();
            if !contains("--tb") {
                flags.push("--tb=short");
            }
            if !values
                .iter()
                .any(|value| *value == "-q" || *value == "--quiet")
            {
                flags.push("-q");
            }
            if !values
                .iter()
                .any(|value| value.starts_with("-r") && !value.starts_with("--"))
            {
                flags.push("-rxX");
            }
            (!flags.is_empty())
                .then(|| append_after(&command.original, executable, &flags.join(" ")))
        }

        NativeFilter::RuffCheck if !contains("--output-format") => {
            if values.first().copied() == Some("check") {
                arguments
                    .first()
                    .map(|token| append_after(&command.original, token, "--output-format=json"))
            } else {
                Some(append_after(
                    &command.original,
                    executable,
                    "check --output-format=json",
                ))
            }
        }

        NativeFilter::PipList | NativeFilter::PipOutdated if !contains("--format") => arguments
            .first()
            .map(|token| append_after(&command.original, token, "--format=json")),

        NativeFilter::PnpmList if !contains("--json") => arguments
            .first()
            .map(|subcommand| append_after(&command.original, subcommand, "--json")),
        NativeFilter::PnpmOutdated if !contains("--format") => arguments
            .first()
            .map(|subcommand| append_after(&command.original, subcommand, "--format json")),

        NativeFilter::Eslint if !contains("--format") && !contains("-f") => {
            Some(append_after(&command.original, executable, "-f json"))
        }
        NativeFilter::Vitest
            if !contains("--reporter")
                && !values
                    .iter()
                    .any(|value| *value == "--watch" || *value == "--ui") =>
        {
            if values.first().copied() == Some("run") {
                arguments.first().map(|subcommand| {
                    append_after(&command.original, subcommand, "--reporter=json")
                })
            } else {
                Some(append_after(
                    &command.original,
                    executable,
                    "run --reporter=json",
                ))
            }
        }
        NativeFilter::Jest
            if !contains("--reporter")
                && !contains("--json")
                && !values.iter().any(|value| value.starts_with("--watch")) =>
        {
            Some(append_after(
                &command.original,
                executable,
                "--no-watch --json",
            ))
        }
        NativeFilter::Playwright if !contains("--reporter") => arguments
            .first()
            .map(|token| append_after(&command.original, token, "--reporter=json")),
        _ => None,
    }
}

/// Executes a prepared filesystem-backed system command without spawning a shell.
///
/// Returns `None` for commands whose prepared invocation must be executed by the
/// normal shell process path.
///
/// # Arguments
///
/// * `prepared` - Prepared invocation returned by [`prepare`].
/// * `cwd` - Working directory used to resolve relative paths.
#[must_use]
pub fn execute_override(
    prepared: &PreparedInvocation,
    cwd: &std::path::Path,
) -> Option<ExecutedOutput> {
    let command = prepared.command.as_ref()?;
    system::execute_override(prepared.native_filter?, command, cwd)
}
/// Returns whether an unparseable rewritten search must be rerun verbatim.
#[must_use]
pub fn should_retry_original(
    prepared: &PreparedInvocation,
    stdout: &str,
    exit_code: Option<i32>,
) -> bool {
    let Some(filter) = prepared.native_filter else {
        return false;
    };
    prepared.rewritten() && system::should_retry_original(filter, stdout, exit_code)
}
/// Applies filtering with an explicit working directory.
#[must_use]
pub fn filter_with_cwd(
    prepared: &PreparedInvocation,
    cwd: &std::path::Path,
    stdout: &str,
    stderr: &str,
    exit_code: Option<i32>,
) -> FilteredOutput {
    filter_inner(prepared, cwd, stdout, stderr, exit_code)
}
/// Applies the prepared native or built-in TOML filter to completed output.
///
/// Parser failures, panics, empty filtered results with non-empty raw data, or
/// results larger under the `ceil(UTF-8 bytes / 4)` estimate fall back to the
/// raw streams. Exit codes and command strings are not modified.
#[must_use]
pub fn filter(
    prepared: &PreparedInvocation,
    stdout: &str,
    stderr: &str,
    exit_code: Option<i32>,
) -> FilteredOutput {
    filter_inner(
        prepared,
        std::path::Path::new("."),
        stdout,
        stderr,
        exit_code,
    )
}
fn filter_inner(
    prepared: &PreparedInvocation,
    cwd: &std::path::Path,
    stdout: &str,
    stderr: &str,
    exit_code: Option<i32>,
) -> FilteredOutput {
    let raw = FilteredOutput {
        stdout: stdout.to_string(),
        stderr: stderr.to_string(),
        changed: false,
        lossy: false,
    };
    let filtered = catch_unwind(AssertUnwindSafe(|| {
        if let Some(command) = prepared.command.as_ref() {
            if let Some(native_filter) = prepared.native_filter {
                if native_filter.is_system() {
                    let stdout = system::apply(native_filter, command, cwd, stdout, exit_code)?;
                    return Some((stdout, stderr.to_string(), true));
                }
                return native::apply(native_filter, stdout, stderr, exit_code);
            }
            let toml = toml_filter::find(&command.original).or_else(|| {
                (command.normalized != command.original)
                    .then(|| toml_filter::find(&command.normalized))
                    .flatten()
            });
            if let Some(filter) = toml {
                if filter.filter_stderr {
                    let combined = format!("{stdout}{stderr}");
                    let (filtered, lossiness) = toml_filter::apply(filter, &combined);
                    return Some((
                        filtered,
                        String::new(),
                        lossiness == PipelineLossiness::Lossy || !stderr.is_empty(),
                    ));
                }
                let (filtered, lossiness) = toml_filter::apply(filter, stdout);
                return Some((
                    filtered,
                    stderr.to_string(),
                    lossiness == PipelineLossiness::Lossy,
                ));
            }
        }
        None
    }))
    .ok()
    .flatten()
    // A filter that left the output as it found it has not claimed it.
    .filter(|(filtered_stdout, filtered_stderr, _)| {
        filtered_stdout != stdout || filtered_stderr != stderr
    })
    // Nothing about the command named a filter, or the one it named was a
    // wrapper that does not know this output: a script, a `make` target, a
    // `just` recipe, a line too tangled to classify. The output still says
    // what produced it, so let it name its own filter.
    .or_else(|| {
        if prepared.declined {
            return None;
        }
        catch_unwind(AssertUnwindSafe(|| {
            native::apply(
                native::detect_by_content(stdout)?,
                stdout,
                stderr,
                exit_code,
            )
        }))
        .ok()
        .flatten()
    });
    let Some((filtered_stdout, filtered_stderr, lossy)) = filtered else {
        return raw;
    };
    let changed = filtered_stdout != stdout || filtered_stderr != stderr;
    if !changed {
        return raw;
    }
    let lossy = lossy
        || estimate_tokens(&filtered_stdout) + estimate_tokens(&filtered_stderr)
            < estimate_tokens(stdout) + estimate_tokens(stderr);
    if filtered_stdout.is_empty()
        && filtered_stderr.is_empty()
        && (!stdout.is_empty() || !stderr.is_empty())
        && !prepared
            .native_filter
            .zip(prepared.command.as_ref())
            .is_some_and(|(filter, command)| system::allows_empty_output(filter, command))
    {
        return raw;
    }
    if estimate_tokens(&filtered_stdout) + estimate_tokens(&filtered_stderr)
        > estimate_tokens(stdout) + estimate_tokens(stderr)
    {
        return raw;
    }
    FilteredOutput {
        stdout: filtered_stdout,
        stderr: filtered_stderr,
        changed,
        lossy,
    }
}

/// Estimates tokens with the reproducible `ceil(UTF-8 bytes / 4)` metric the
/// filter inventory was measured against.
#[must_use]
pub fn estimate_tokens(value: &str) -> usize {
    value.len().div_ceil(4)
}

/// A retention comparison against a reference output.
///
/// How much of the reduction a reference implementation achieved on the same
/// input this filter still achieves. Used by the fixture measurements.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetentionMeasurement {
    /// Raw-output token estimate.
    pub raw_tokens: usize,
    /// Reference token estimate for the same fixture.
    pub reference_tokens: usize,
    /// Reduced Notagent token estimate.
    pub reduced_tokens: usize,
    /// Percentage of the reference reduction retained, when the reference
    /// reduced anything at all.
    pub retention_percent: Option<u32>,
}
/// Measures retained reduction using the pinned byte-based token estimate.
#[must_use]
pub fn measure_retention(raw: &str, reference: &str, reduced: &str) -> RetentionMeasurement {
    let raw_tokens = estimate_tokens(raw);
    let reference_tokens = estimate_tokens(reference);
    let reduced_tokens = estimate_tokens(reduced);
    let full_reduction = raw_tokens.saturating_sub(reference_tokens);
    let reduced_reduction = raw_tokens.saturating_sub(reduced_tokens);
    let retention_percent = (full_reduction > 0)
        .then(|| ((reduced_reduction as f64 / full_reduction as f64) * 100.0).round() as u32);
    RetentionMeasurement {
        raw_tokens,
        reference_tokens,
        reduced_tokens,
        retention_percent,
    }
}
/// Returns the number of pinned built-in TOML filters.
#[must_use]
pub fn builtin_filter_count() -> usize {
    toml_filter::filter_count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_equivalent_passthrough_for_unsafe_command() {
        let fixture = prepare("cargo test && echo done");
        let actual = filter(&fixture, "raw stdout", "raw stderr", Some(1));
        let expected = FilteredOutput {
            stdout: "raw stdout".into(),
            stderr: "raw stderr".into(),
            changed: false,
            lossy: false,
        };
        assert_eq!(actual, expected);
    }

    #[test]
    fn a_redirect_no_longer_costs_the_whole_filter() {
        // Step 1: setup: `2>&1` moves a descriptor, it does not move the
        // output out of reach — the rewrite has to survive it and carry it
        let fixture = ["go test ./... 2>&1", "ls -la src 2>&1", "cargo test 2>&1"];

        // Step 2: actual
        let actual = fixture
            .into_iter()
            .map(|command| {
                let prepared = prepare(command);
                (
                    prepared.execution().to_string(),
                    prepared.native_filter.is_some(),
                )
            })
            .collect::<Vec<_>>();

        // Step 3: expected: the redirect stays at the end of the rewrite
        let expected = vec![
            ("go test -json ./... 2>&1".to_string(), true),
            (
                "env -u CLICOLOR_FORCE -u FORCE_COLOR LC_ALL=C ls -la src 2>&1".to_string(),
                true,
            ),
            ("cargo test 2>&1".to_string(), true),
        ];

        // Assertions
        assert_eq!(actual, expected);
    }

    #[test]
    fn a_pipeline_is_filtered_but_never_rewritten() {
        // Step 1: setup: the same cargo run, once piped into a stage that
        // leaves its shape alone and once into one that destroys it
        let raw = "running 2 tests\ntest one ... ok\ntest two ... ok\ntest result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out";
        let piped = prepare("cargo test 2>&1 | tail -20");
        let counted = prepare("ls -la src | wc -l");

        // Step 2: actual
        let actual = (
            piped.execution().to_string(),
            filter(&piped, raw, "", Some(0)).stdout,
            counted.execution().to_string(),
            counted.native_filter.is_some(),
            counted.buffers_live_output(),
        );

        // Step 3: expected: the pipeline runs exactly as written and its
        // output is still compacted, while the counted one stands down
        // entirely rather than hand `wc`'s number to an `ls` adapter
        let expected = (
            "cargo test 2>&1 | tail -20".to_string(),
            "cargo test: 2 passed (1 suite)".to_string(),
            "ls -la src | wc -l".to_string(),
            false,
            false,
        );

        // Assertions
        assert_eq!(actual, expected);
    }

    #[test]
    fn output_names_its_own_filter_when_the_command_says_nothing() {
        // Step 1: setup: the same cargo run reached through things no
        // classifier can see into — a wrapper script, a make target, a
        // subshell that stops classification dead
        let raw = "running 2 tests\ntest one ... ok\ntest two ... ok\ntest result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out";
        let fixture = [
            "./scripts/ci.sh",
            "just verify",
            "(cd crates && cargo test)",
        ];

        // Step 2: actual
        let actual = fixture
            .into_iter()
            .map(|command| filter(&prepare(command), raw, "", Some(0)).stdout)
            .collect::<Vec<_>>();

        // Step 3: expected
        let expected = vec!["cargo test: 2 passed (1 suite)".to_string(); fixture.len()];

        // Assertions
        assert_eq!(actual, expected);
    }

    #[test]
    fn a_named_output_format_outranks_what_the_output_looks_like() {
        // Step 1: setup: vitest asked for a verbose reporter, so its filter
        // was turned down — and then it printed JSON anyway
        let raw = "{\n  \"numTotalTests\": 13,\n  \"numPassedTests\": 13,\n  \"numFailedTests\": 0,\n  \"numPendingTests\": 0,\n  \"testResults\": [],\n  \"startTime\": 1000\n}";

        // Step 2: actual
        let actual = (
            filter(&prepare("vitest --reporter=verbose"), raw, "", Some(0)).changed,
            filter(&prepare("./scripts/test.sh"), raw, "", Some(0)).stdout,
        );

        // Step 3: expected: the refusal holds, the unclassified run does not
        let expected = (false, "PASS (13) FAIL (0)".to_string());

        // Assertions
        assert_eq!(actual, expected);
    }

    #[test]
    fn prepares_go_test_rewrite_at_subcommand() {
        let fixture = prepare("go test ./...");
        let actual = (fixture.original(), fixture.execution());
        let expected = ("go test ./...", "go test -json ./...");
        assert_eq!(actual, expected);
    }
    #[test]
    fn prepares_rewrites_without_changing_original() {
        let fixture = prepare("pytest tests/unit");
        let actual = (fixture.original(), fixture.execution(), fixture.rewritten());
        let expected = (
            "pytest tests/unit",
            "pytest --tb=short -q -rxX tests/unit",
            true,
        );
        assert_eq!(actual, expected);
    }

    #[test]
    fn respects_user_reporters_and_benchmarks() {
        let fixture = [
            "vitest --reporter=verbose",
            "playwright test --reporter=line",
            "eslint -f stylish src",
            "ruff check --output-format=concise",
            "pip list --format=columns",
            "go test -bench=.",
        ];
        let actual = fixture
            .into_iter()
            .map(|command| {
                let prepared = prepare(command);
                (prepared.rewritten(), prepared.native_filter.is_some())
            })
            .collect::<Vec<_>>();
        let expected = vec![(false, false); fixture.len()];
        assert_eq!(actual, expected);
    }
    #[test]
    fn prepares_rewrites_at_supported_command_boundaries() {
        let fixture = [
            "pnpm list packages",
            "pnpm outdated packages",
            "vitest run tests/unit.ts",
            "vitest tests/unit.ts",
            "npx eslint src",
            "pnpm exec eslint src",
            "playwright test tests/example.ts",
        ];
        let actual = fixture
            .into_iter()
            .map(|command| prepare(command).execution().to_string())
            .collect::<Vec<_>>();
        let expected = vec![
            "pnpm list --json packages",
            "pnpm outdated --format json packages",
            "vitest run --reporter=json tests/unit.ts",
            "vitest run --reporter=json tests/unit.ts",
            "npx eslint -f json src",
            "pnpm exec eslint -f json src",
            "playwright test --reporter=json tests/example.ts",
        ];
        assert_eq!(actual, expected);
    }
    #[test]
    fn system_preparation_exposes_proxy_override_and_retry_contracts() {
        let ls = prepare("ls src");
        let find = prepare("find . -name *.rs");
        let read = prepare("cat src/lib.rs");
        let search = prepare("rg TODO src");
        let actual = (
            ls.execution().to_string(),
            ls.buffers_live_output(),
            find.uses_execution_override(),
            read.uses_execution_override(),
            search.execution().to_string(),
            search.buffers_live_output(),
            should_retry_original(&search, "unparseable search output", Some(2)),
        );
        let expected = (
            "env -u CLICOLOR_FORCE -u FORCE_COLOR LC_ALL=C ls -la src".to_string(),
            true,
            true,
            true,
            "NO_COLOR=1 rg -n --with-filename --null -e TODO -- src".to_string(),
            true,
            true,
        );
        assert_eq!(actual, expected);
    }

    #[test]
    fn unsafe_system_invocations_remain_unmodified() {
        let fixture = [
            "ls | wc -l",
            "find . -name *.rs -exec echo {} ;",
            "rg --json TODO src",
            "grep -c TODO src",
            "cat -A file.rs",
            "head -3 a b",
            "tail file.rs",
        ];
        let actual = fixture
            .iter()
            .map(|command| {
                let prepared = prepare(command);
                (
                    prepared.execution().to_string(),
                    prepared.uses_execution_override(),
                    prepared.buffers_live_output(),
                )
            })
            .collect::<Vec<_>>();
        let expected = fixture
            .iter()
            .map(|command| (command.to_string(), false, false))
            .collect::<Vec<_>>();
        assert_eq!(actual, expected);
    }

    #[test]
    fn native_dispatch_precedes_toml_and_filters_cargo_test() {
        let fixture = prepare("cargo test");
        let raw = "running 2 tests\ntest one ... ok\ntest two ... ok\ntest result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out";
        let actual = filter(&fixture, raw, "", Some(0));
        let expected = "cargo test: 2 passed (1 suite)";
        assert_eq!(actual.stdout, expected);
        assert!(actual.changed);
    }

    #[test]
    fn normalized_view_matches_built_in_filter() {
        let fixture = prepare("A=1 /usr/local/bin/make all");
        let raw =
            "make[1]: Entering directory '/tmp'\ngcc main.c\nmake[1]: Leaving directory '/tmp'";
        let actual = filter(&fixture, raw, "", Some(0));
        let expected = "gcc main.c";
        assert_eq!(actual.stdout, expected);
    }

    #[test]
    fn intentional_zero_tail_is_not_replaced_by_raw_output() {
        let prepared = prepare("tail -0 sample.txt");
        let actual = filter(&prepared, "one\ntwo\n", "", Some(0));
        let expected = FilteredOutput {
            stdout: String::new(),
            stderr: String::new(),
            changed: true,
            lossy: true,
        };
        assert_eq!(actual, expected);
    }

    #[test]
    fn never_worse_uses_byte_estimate() {
        let prepared = prepare("prettier --check .");
        let actual = filter(&prepared, "x", "", Some(0));
        let expected = FilteredOutput {
            stdout: "x".into(),
            stderr: String::new(),
            changed: false,
            lossy: false,
        };
        assert_eq!(actual, expected);
    }

    #[test]
    fn multi_line_and_shell_constructs_pass_through() {
        let fixture = [
            "cargo test\necho done",
            "cargo test || echo failed",
            "cargo test 2>errors.log",
            "cargo test < input.txt",
        ];
        let actual = fixture
            .into_iter()
            .map(|command| {
                let prepared = prepare(command);
                let output = filter(&prepared, "raw stdout", "raw stderr", Some(1));
                (prepared.execution().to_string(), output.changed)
            })
            .collect::<Vec<_>>();
        let expected = fixture
            .into_iter()
            .map(|command| (command.to_string(), false))
            .collect::<Vec<_>>();
        assert_eq!(actual, expected);
    }
    #[test]
    fn marks_token_reducing_text_changes_as_lossy() {
        let fixture = prepare("make all");
        let raw =
            "make[1]: Entering directory '/tmp'\ngcc main.c\nmake[1]: Leaving directory '/tmp'";
        let actual = filter(&fixture, raw, "", Some(0));
        assert!(actual.changed);
        assert!(actual.lossy);
    }

    #[test]
    fn retention_measurement_uses_pinned_formula() {
        let fixture = ("x".repeat(400), "x".repeat(80), "x".repeat(100));
        let actual = measure_retention(&fixture.0, &fixture.1, &fixture.2);
        let expected = RetentionMeasurement {
            raw_tokens: 100,
            reference_tokens: 20,
            reduced_tokens: 25,
            retention_percent: Some(94),
        };
        assert_eq!(actual, expected);
    }
    #[test]
    fn retention_measurement_marks_non_reducing_reference_not_applicable() {
        let fixture = ("x".repeat(80), "x".repeat(80), "x".repeat(40));
        let actual = measure_retention(&fixture.0, &fixture.1, &fixture.2);
        let expected = RetentionMeasurement {
            raw_tokens: 20,
            reference_tokens: 20,
            reduced_tokens: 10,
            retention_percent: None,
        };
        assert_eq!(actual, expected);
    }

    #[test]
    fn measures_pinned_ecosystem_fixture_retention() {
        let fixtures = [
            (
                "Rust",
                "cargo test",
                "running 2 tests\ntest one ... ok\ntest two ... ok\ntest result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out",
                "cargo test: 2 passed (1 suite)",
            ),
            (
                "JavaScript/TypeScript",
                "vitest run",
                "{\n  \"numTotalTests\": 13,\n  \"numPassedTests\": 13,\n  \"numFailedTests\": 0,\n  \"numPendingTests\": 0,\n  \"testResults\": [],\n  \"startTime\": 1000\n}",
                "PASS (13) FAIL (0)",
            ),
            (
                "Go",
                "go test ./...",
                "{\"Action\":\"run\",\"Package\":\"example.com/foo\",\"Test\":\"TestBar\"}\n{\"Action\":\"output\",\"Package\":\"example.com/foo\",\"Test\":\"TestBar\",\"Output\":\"=== RUN   TestBar\\n\"}\n{\"Action\":\"pass\",\"Package\":\"example.com/foo\",\"Test\":\"TestBar\"}\n{\"Action\":\"pass\",\"Package\":\"example.com/foo\"}",
                "Go test: 1 passed in 1 packages",
            ),
            (
                "Python",
                "pytest tests",
                "=== test session starts ===\nplatform darwin -- Python 3.11.0\ncollected 5 items\n\ntests/test_foo.py ..... [100%]\n\n=== 5 passed in 0.50s ===",
                "Pytest: 5 passed",
            ),
        ];
        let actual = fixtures
            .into_iter()
            .map(|(ecosystem, command, raw, reference)| {
                let prepared = prepare(command);
                let reduced = filter(&prepared, raw, "", Some(0));
                let measurement = measure_retention(raw, reference, &reduced.stdout);
                let savings_percent = if measurement.raw_tokens == 0 {
                    None
                } else {
                    Some(
                        (measurement.raw_tokens.saturating_sub(measurement.reduced_tokens) as f64
                            / measurement.raw_tokens as f64)
                            * 100.0,
                    )
                };
                println!(
                    "{ecosystem}: raw={}, reference={}, reduced={}, savings={savings_percent:?}, retention={:?}, passthrough_reason=none",
                    measurement.raw_tokens,
                    measurement.reference_tokens,
                    measurement.reduced_tokens,
                    measurement.retention_percent,
                );
                (ecosystem, reduced.stdout, measurement.retention_percent)
            })
            .collect::<Vec<_>>();
        let expected = vec![
            (
                "Rust",
                "cargo test: 2 passed (1 suite)".to_string(),
                Some(100),
            ),
            (
                "JavaScript/TypeScript",
                "PASS (13) FAIL (0)".to_string(),
                Some(100),
            ),
            (
                "Go",
                "Go test: 1 passed in 1 packages".to_string(),
                Some(100),
            ),
            ("Python", "Pytest: 5 passed".to_string(), Some(100)),
        ];
        assert_eq!(actual, expected);
    }

    #[test]
    fn inventory_count_is_publicly_stable() {
        let actual = builtin_filter_count();
        let expected = 63;
        assert_eq!(actual, expected);
    }
}
