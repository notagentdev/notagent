//! The declarative half of the bash filter: one regex per command, then an
//! eight-stage line pipeline over its output.
//!
//! Built-in filters only. The definitions and their inline fixtures are data,
//! embedded by [`super::builtin_filters`]; nothing is read from the filesystem,
//! so a filter cannot be added or changed without a rebuild.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use regex::{Regex, RegexSet};
use serde::Deserialize;

/// The concatenated built-in document, built once from the embedded files.
fn builtin_toml() -> &'static str {
    static BUILTIN_TOML: OnceLock<String> = OnceLock::new();
    BUILTIN_TOML.get_or_init(super::builtin_filters::builtin_document)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MatchOutputRule {
    pattern: String,
    message: String,
    #[serde(default)]
    unless: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplaceRule {
    pattern: String,
    replacement: String,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TestDefinition {
    pub name: String,
    pub input: String,
    pub expected: String,
}

#[derive(Deserialize)]
struct FilterFile {
    schema_version: u32,
    #[serde(default)]
    filters: BTreeMap<String, FilterDefinition>,
    #[serde(default)]
    tests: BTreeMap<String, Vec<TestDefinition>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FilterDefinition {
    description: Option<String>,
    match_command: String,
    #[serde(default)]
    strip_ansi: bool,
    #[serde(default)]
    replace: Vec<ReplaceRule>,
    #[serde(default)]
    match_output: Vec<MatchOutputRule>,
    #[serde(default)]
    strip_lines_matching: Vec<String>,
    #[serde(default)]
    keep_lines_matching: Vec<String>,
    truncate_lines_at: Option<usize>,
    head_lines: Option<usize>,
    tail_lines: Option<usize>,
    max_lines: Option<usize>,
    on_empty: Option<String>,
    #[serde(default)]
    filter_stderr: bool,
}

#[derive(Debug)]
struct CompiledMatchOutputRule {
    pattern: Regex,
    message: String,
    unless: Option<Regex>,
}

#[derive(Debug)]
struct CompiledReplaceRule {
    pattern: Regex,
    replacement: String,
}

#[derive(Debug)]
enum LineFilter {
    None,
    Strip(RegexSet),
    Keep(RegexSet),
}

#[allow(dead_code)]
#[derive(Debug)]
pub(crate) struct CompiledFilter {
    pub name: String,
    #[allow(dead_code)]
    description: Option<String>,
    match_regex: Regex,
    strip_ansi: bool,
    replace: Vec<CompiledReplaceRule>,
    match_output: Vec<CompiledMatchOutputRule>,
    line_filter: LineFilter,
    truncate_lines_at: Option<usize>,
    head_lines: Option<usize>,
    tail_lines: Option<usize>,
    max_lines: Option<usize>,
    on_empty: Option<String>,
    pub filter_stderr: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PipelineLossiness {
    Lossless,
    Lossy,
}

fn compile_filter(name: String, definition: FilterDefinition) -> Result<CompiledFilter, String> {
    if !definition.strip_lines_matching.is_empty() && !definition.keep_lines_matching.is_empty() {
        return Err("strip_lines_matching and keep_lines_matching are mutually exclusive".into());
    }
    let match_regex = Regex::new(&definition.match_command)
        .map_err(|error| format!("invalid match_command regex: {error}"))?;
    let replace = definition
        .replace
        .into_iter()
        .map(|rule| {
            Regex::new(&rule.pattern)
                .map(|pattern| CompiledReplaceRule {
                    pattern,
                    replacement: rule.replacement,
                })
                .map_err(|error| format!("invalid replace regex: {error}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let match_output = definition
        .match_output
        .into_iter()
        .map(|rule| {
            let pattern = Regex::new(&rule.pattern)
                .map_err(|error| format!("invalid match_output regex: {error}"))?;
            let unless = rule
                .unless
                .as_deref()
                .map(Regex::new)
                .transpose()
                .map_err(|error| format!("invalid unless regex: {error}"))?;
            Ok(CompiledMatchOutputRule {
                pattern,
                message: rule.message,
                unless,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let line_filter = if !definition.strip_lines_matching.is_empty() {
        LineFilter::Strip(
            RegexSet::new(&definition.strip_lines_matching)
                .map_err(|error| format!("invalid strip regex: {error}"))?,
        )
    } else if !definition.keep_lines_matching.is_empty() {
        LineFilter::Keep(
            RegexSet::new(&definition.keep_lines_matching)
                .map_err(|error| format!("invalid keep regex: {error}"))?,
        )
    } else {
        LineFilter::None
    };
    Ok(CompiledFilter {
        name,
        description: definition.description,
        match_regex,
        strip_ansi: definition.strip_ansi,
        replace,
        match_output,
        line_filter,
        truncate_lines_at: definition.truncate_lines_at,
        head_lines: definition.head_lines,
        tail_lines: definition.tail_lines,
        max_lines: definition.max_lines,
        on_empty: definition.on_empty,
        filter_stderr: definition.filter_stderr,
    })
}

type ParsedFilters = (Vec<CompiledFilter>, BTreeMap<String, Vec<TestDefinition>>);
fn parse(content: &str) -> Result<ParsedFilters, String> {
    let file: FilterFile = toml::from_str(content).map_err(|error| error.to_string())?;
    if file.schema_version != 1 {
        return Err(format!(
            "unsupported schema_version {} (expected 1)",
            file.schema_version
        ));
    }
    let filters = file
        .filters
        .into_iter()
        .map(|(name, definition)| compile_filter(name, definition))
        .collect::<Result<Vec<_>, _>>()?;
    Ok((filters, file.tests))
}

/// The compiled built-in filters, in file order.
///
/// A document that fails to parse yields no filters rather than a panic: the
/// filter is an optimisation, and no filters means raw output, which is what
/// the caller would have had anyway. The inventory tests below are what makes
/// a broken document loud.
fn registry() -> &'static Vec<CompiledFilter> {
    static REGISTRY: OnceLock<Vec<CompiledFilter>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        parse(builtin_toml())
            .map(|parsed| parsed.0)
            .unwrap_or_default()
    })
}

pub(crate) fn find(command: &str) -> Option<&'static CompiledFilter> {
    registry()
        .iter()
        .find(|filter| filter.match_regex.is_match(command))
}

pub(crate) fn filter_count() -> usize {
    registry().len()
}

fn strip_ansi(input: &str) -> String {
    static ANSI: OnceLock<Regex> = OnceLock::new();
    ANSI.get_or_init(|| {
        Regex::new(r"\x1b(?:\[[0-?]*[ -/]*[@-~]|\][^\x07]*(?:\x07|\x1b\\))")
            .expect("ANSI regex is valid")
    })
    .replace_all(input, "")
    .into_owned()
}

fn truncate(input: &str, max_chars: usize) -> String {
    let length = input.chars().count();
    if length <= max_chars {
        return input.to_string();
    }
    if max_chars <= 3 {
        return ".".repeat(max_chars);
    }
    format!(
        "{}...",
        input.chars().take(max_chars - 3).collect::<String>()
    )
}

pub(crate) fn apply(filter: &CompiledFilter, output: &str) -> (String, PipelineLossiness) {
    let mut lines: Vec<String> = output.lines().map(str::to_string).collect();
    let mut lossy = false;

    if filter.strip_ansi {
        lines = lines.into_iter().map(|line| strip_ansi(&line)).collect();
    }
    if !filter.replace.is_empty() {
        lines = lines
            .into_iter()
            .map(|mut line| {
                for rule in &filter.replace {
                    line = rule
                        .pattern
                        .replace_all(&line, rule.replacement.as_str())
                        .into_owned();
                }
                line
            })
            .collect();
    }
    if !filter.match_output.is_empty() {
        let blob = lines.join("\n");
        for rule in &filter.match_output {
            if rule.pattern.is_match(&blob)
                && rule
                    .unless
                    .as_ref()
                    .is_none_or(|unless| !unless.is_match(&blob))
            {
                return (rule.message.clone(), PipelineLossiness::Lossy);
            }
        }
    }
    let before_line_filter = lines.len();
    match &filter.line_filter {
        LineFilter::Strip(patterns) => lines.retain(|line| !patterns.is_match(line)),
        LineFilter::Keep(patterns) => lines.retain(|line| patterns.is_match(line)),
        LineFilter::None => {}
    }
    lossy |= lines.len() != before_line_filter;

    if let Some(max_chars) = filter.truncate_lines_at {
        lines = lines
            .into_iter()
            .map(|line| {
                let compact = truncate(&line, max_chars);
                lossy |= compact != line;
                compact
            })
            .collect();
    }

    let total = lines.len();
    match (filter.head_lines, filter.tail_lines) {
        (Some(head), Some(tail)) if total > head + tail => {
            let mut compact = lines[..head].to_vec();
            compact.push(format!("... ({} lines omitted)", total - head - tail));
            compact.extend_from_slice(&lines[total - tail..]);
            lines = compact;
            lossy = true;
        }
        (Some(head), None) if total > head => {
            lines.truncate(head);
            lines.push(format!("... ({} lines omitted)", total - head));
            lossy = true;
        }
        (None, Some(tail)) if total > tail => {
            lines = lines[total - tail..].to_vec();
            lines.insert(0, format!("... ({} lines omitted)", total - tail));
            lossy = true;
        }
        _ => {}
    }
    if let Some(maximum) = filter.max_lines
        && lines.len() > maximum
    {
        let dropped = lines.len() - maximum;
        lines.truncate(maximum);
        lines.push(format!("... ({dropped} lines truncated)"));
        lossy = true;
    }
    let result = lines.join("\n");
    if result.trim().is_empty()
        && let Some(message) = &filter.on_empty
    {
        return (
            message.clone(),
            if lossy {
                PipelineLossiness::Lossy
            } else {
                PipelineLossiness::Lossless
            },
        );
    }
    (
        result,
        if lossy {
            PipelineLossiness::Lossy
        } else {
            PipelineLossiness::Lossless
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reference asserts this in a build script; this port has none, so the
    /// same three guarantees are tests: one file per filter, the expected
    /// count, and no name claimed twice.
    #[test]
    fn every_embedded_file_defines_exactly_one_uniquely_named_filter() {
        let files = super::super::builtin_filters::BUILTIN_FILTER_FILES;
        let (filters, _) = parse(builtin_toml()).unwrap();
        let mut names = std::collections::HashSet::new();
        let duplicates: Vec<&str> = filters
            .iter()
            .filter(|filter| !names.insert(filter.name.as_str()))
            .map(|filter| filter.name.as_str())
            .collect();
        let mut sorted: Vec<&str> = files.iter().map(|(name, _)| *name).collect();
        let embedded = sorted.clone();
        sorted.sort_unstable();

        let actual = (files.len(), filters.len(), duplicates, embedded == sorted);
        let expected = (63, 63, Vec::new(), true);

        assert_eq!(actual, expected);
    }

    #[test]
    fn pinned_inventory_is_complete() {
        let fixture = parse(builtin_toml()).unwrap();
        let actual_filters = fixture.0.len();
        let actual_tests = fixture.1.values().map(Vec::len).sum::<usize>();
        let actual_stderr_filters = fixture
            .0
            .iter()
            .filter(|filter| filter.filter_stderr)
            .count();
        let expected = (63, 154, 1);
        assert_eq!(
            (actual_filters, actual_tests, actual_stderr_filters),
            expected
        );
    }

    #[test]
    fn every_pinned_inline_fixture_passes() {
        let (filters, tests) = parse(builtin_toml()).unwrap();
        let fixture_count = tests.values().map(Vec::len).sum::<usize>();
        let mut actual = Vec::new();
        for filter in &filters {
            for fixture in tests.get(&filter.name).into_iter().flatten() {
                let output = apply(filter, &fixture.input).0;
                if output.trim_end_matches('\n') != fixture.expected.trim_end_matches('\n') {
                    actual.push(format!("{}: {}", filter.name, fixture.name));
                }
            }
        }
        let expected: Vec<String> = Vec::new();
        assert_eq!(fixture_count, 154);
        assert_eq!(actual, expected);
    }
    #[test]
    fn measures_pinned_inline_fixture_savings() {
        let (filters, tests) = parse(builtin_toml()).unwrap();
        let mut raw_tokens = 0usize;
        let mut reference_tokens = 0usize;
        let mut reduced_tokens = 0usize;
        let mut reducing = 0usize;
        let mut unchanged = 0usize;
        let mut never_worse_passthrough = 0usize;
        for filter in &filters {
            for fixture in tests.get(&filter.name).into_iter().flatten() {
                let raw = crate::core::bash_filter::estimate_tokens(&fixture.input);
                let reference =
                    crate::core::bash_filter::estimate_tokens(&apply(filter, &fixture.input).0);
                let reduced = if reference > raw { raw } else { reference };
                raw_tokens += raw;
                reference_tokens += reference;
                reduced_tokens += reduced;
                if reduced < raw {
                    reducing += 1;
                } else if reference > raw {
                    never_worse_passthrough += 1;
                } else {
                    unchanged += 1;
                }
            }
        }
        let savings_percent =
            (raw_tokens.saturating_sub(reduced_tokens) as f64 / raw_tokens as f64) * 100.0;
        let reference_savings_percent =
            (raw_tokens.saturating_sub(reference_tokens) as f64 / raw_tokens as f64) * 100.0;
        let reference_reduction = raw_tokens.saturating_sub(reference_tokens);
        let retention_percent = if reference_reduction == 0 {
            None
        } else {
            Some(
                raw_tokens.saturating_sub(reduced_tokens) as f64 / reference_reduction as f64
                    * 100.0,
            )
        };
        println!(
            "TOML fixtures: count={}, raw={}, reference={}, reduced={}, reference_savings={reference_savings_percent:.2}%, savings={savings_percent:.2}%, retention={retention_percent:.2?}%, reducing={}, unchanged={}, never_worse_passthrough={}",
            reducing + unchanged + never_worse_passthrough,
            raw_tokens,
            reference_tokens,
            reduced_tokens,
            reducing,
            unchanged,
            never_worse_passthrough,
        );
        let actual = (
            reducing + unchanged + never_worse_passthrough,
            raw_tokens,
            reference_tokens,
            reduced_tokens,
            reducing,
            unchanged,
            never_worse_passthrough,
        );
        // Raw is one token below the reference's 6206: the `brew-install`
        // fixture's sample output named a package this port does not name, and
        // the replacement is three bytes shorter. Every other number is the
        // reference's, and that is the point of pinning them.
        let expected = (154, 6205, 3238, 3199, 105, 37, 12);
        assert_eq!(actual, expected);
    }
    #[test]
    fn invalid_schema_and_regex_are_rejected() {
        let invalid_schema = "schema_version = 2\n";
        let invalid_regex = "schema_version = 1\n[filters.bad]\nmatch_command = '['\n";
        let actual = (
            parse(invalid_schema).is_err(),
            parse(invalid_regex).is_err(),
        );
        let expected = (true, true);
        assert_eq!(actual, expected);
    }
}
