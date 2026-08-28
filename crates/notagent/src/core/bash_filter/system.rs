//! The system adapters of the bash filter, for the handful of commands an
//! agent runs constantly.
//! Two execution models, per command: `ls`, `tree` and the two search engines
//! receive a parse-friendly invocation and are compacted afterwards, while
//! `find` and the read commands execute against the filesystem directly and
//! hand their raw data to the same buffered formatters.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use ignore::WalkBuilder;
use regex::Regex;

use super::ExecutedOutput;
use super::command::{ClassifiedCommand, Token, basename};
use super::native::NativeFilter;

const NOISE_DIRS: &[&str] = &[
    "node_modules",
    ".git",
    "target",
    "__pycache__",
    ".next",
    "dist",
    "build",
    ".cache",
    ".turbo",
    ".vercel",
    ".pytest_cache",
    ".mypy_cache",
    ".tox",
    ".venv",
    "venv",
    "env",
    "coverage",
    ".nyc_output",
    ".DS_Store",
    "Thumbs.db",
    ".idea",
    ".vscode",
    ".vs",
    "*.egg-info",
    ".eggs",
];

const SEARCH_VALUE_FLAGS_SHORT: &[u8] = b"ABCMTdfgjmt";
const SEARCH_VALUE_FLAGS_LONG: &[&str] = &[
    "--after-context",
    "--before-context",
    "--color",
    "--colors",
    "--context",
    "--context-separator",
    "--encoding",
    "--engine",
    "--field-context-separator",
    "--field-match-separator",
    "--file",
    "--glob",
    "--iglob",
    "--ignore-file",
    "--max-columns",
    "--max-count",
    "--max-depth",
    "--max-filesize",
    "--path-separator",
    "--pre",
    "--pre-glob",
    "--replace",
    "--sort",
    "--sortr",
    "--threads",
    "--type",
    "--type-add",
    "--type-clear",
    "--type-not",
];

const UNSUPPORTED_FIND_FLAGS: &[&str] = &[
    "-not", "!", "-or", "-o", "-and", "-a", "-exec", "-execdir", "-delete", "-print0", "-newer",
    "-perm", "-size", "-mtime", "-mmin", "-atime", "-amin", "-ctime", "-cmin", "-empty", "-link",
    "-regex", "-iregex",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SearchEngine {
    Grep,
    Rg,
}

impl SearchEngine {
    fn parse_flags(self) -> &'static [&'static str] {
        match self {
            Self::Grep => &["-n", "-H", "-I", "--null"],
            Self::Rg => &["-n", "--with-filename", "--null"],
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum ClusterResult {
    Boolean(Option<String>),
    ValueTaking {
        prefix: Option<String>,
        flag: char,
        inline: String,
    },
}

#[derive(Debug, Clone)]
struct SearchArgs {
    patterns: Vec<String>,
    paths: Vec<String>,
    flags: Vec<String>,
}

#[derive(Debug, Clone)]
struct FindArgs {
    pattern: String,
    path: String,
    max_results: usize,
    max_depth: Option<usize>,
    file_type: String,
    case_insensitive: bool,
}

impl Default for FindArgs {
    fn default() -> Self {
        Self {
            pattern: "*".to_string(),
            path: ".".to_string(),
            max_results: 50,
            max_depth: None,
            file_type: "f".to_string(),
            case_insensitive: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReadMode {
    Cat { line_numbers: bool },
    Head { max_lines: Option<usize> },
    Tail { tail_lines: usize },
}

pub(crate) fn eligible(command: &ClassifiedCommand, filter: NativeFilter) -> bool {
    match filter {
        NativeFilter::Find => !command_arguments(command).is_empty(),
        NativeFilter::Grep => search_args(command).is_some_and(|args| {
            !args.patterns.is_empty()
                && !has_search_format_flag(&args.flags)
                && !has_help_flag(command)
        }),
        NativeFilter::Rg => search_args(command).is_some_and(|args| {
            !args.patterns.is_empty()
                && !has_search_format_flag(&args.flags)
                && !has_help_flag(command)
        }),
        NativeFilter::Read => read_mode(command).is_some(),
        _ => true,
    }
}

pub(crate) fn rewrite(command: &ClassifiedCommand, filter: NativeFilter) -> Option<String> {
    match filter {
        NativeFilter::Ls => rewrite_ls(command),
        NativeFilter::Tree => rewrite_tree(command),
        NativeFilter::Grep => rewrite_search(command, SearchEngine::Grep),
        NativeFilter::Rg => rewrite_search(command, SearchEngine::Rg),
        NativeFilter::Find | NativeFilter::Read => None,
        _ => None,
    }
}

pub(crate) fn execute_override(
    filter: NativeFilter,
    command: &ClassifiedCommand,
    cwd: &Path,
) -> Option<ExecutedOutput> {
    match filter {
        NativeFilter::Find => Some(execute_find(command, cwd)),
        NativeFilter::Read => Some(execute_read(command, cwd)),
        _ => None,
    }
}

pub(crate) fn should_retry_original(
    filter: NativeFilter,
    stdout: &str,
    _exit_code: Option<i32>,
) -> bool {
    filter.is_search()
        && stdout.lines().any(|line| {
            let line = line.trim();
            !line.is_empty() && line != "--" && parse_match_line(line).is_none()
        })
}
pub(crate) fn allows_empty_output(filter: NativeFilter, command: &ClassifiedCommand) -> bool {
    matches!(filter, NativeFilter::Read)
        && matches!(read_mode(command), Some(ReadMode::Tail { tail_lines: 0 }))
}

pub(crate) fn apply(
    filter: NativeFilter,
    command: &ClassifiedCommand,
    cwd: &Path,
    stdout: &str,
    exit_code: Option<i32>,
) -> Option<String> {
    if exit_code.unwrap_or_default() != 0 {
        return None;
    }
    match filter {
        NativeFilter::Ls => compact_ls_for_command(command, stdout),
        NativeFilter::Tree => Some(filter_tree_output(stdout)),
        NativeFilter::Find => format_find(command, stdout),
        NativeFilter::Grep => format_search(command, cwd, stdout, SearchEngine::Grep),
        NativeFilter::Rg => format_search(command, cwd, stdout, SearchEngine::Rg),
        NativeFilter::Read => format_read(command, stdout),
        _ => None,
    }
}

fn command_arguments(command: &ClassifiedCommand) -> &[Token] {
    &command.tokens[command.effective_index + 1..]
}

fn argument_values(command: &ClassifiedCommand) -> Vec<String> {
    command_arguments(command)
        .iter()
        .map(|token| token.value.clone())
        .collect()
}

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    if value
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || "_+-./:=,@%".contains(character))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn execution_prefix(command: &ClassifiedCommand) -> &str {
    &command.original[..command.tokens[command.effective_index].start]
}

fn render_command(command: &ClassifiedCommand, executable: &str, args: &[String]) -> String {
    let mut rendered = execution_prefix(command).to_string();
    rendered.push_str(executable);
    for arg in args {
        rendered.push(' ');
        rendered.push_str(&shell_quote(arg));
    }
    rendered
}

fn rewrite_ls(command: &ClassifiedCommand) -> Option<String> {
    let values = argument_values(command);
    let flags = values
        .iter()
        .filter(|value| value.starts_with('-'))
        .collect::<Vec<_>>();
    let paths = values
        .iter()
        .filter(|value| !value.starts_with('-'))
        .cloned()
        .collect::<Vec<_>>();
    let mut args = vec!["-la".to_string()];
    for flag in flags {
        if flag.starts_with("--") {
            if flag != "--all" {
                args.push(flag.clone());
            }
        } else {
            let extra = flag
                .trim_start_matches('-')
                .chars()
                .filter(|character| !matches!(character, 'l' | 'a' | 'h'))
                .collect::<String>();
            if !extra.is_empty() {
                args.push(format!("-{extra}"));
            }
        }
    }
    if paths.is_empty() {
        args.push(".".to_string());
    } else {
        args.extend(paths);
    }
    Some(render_command(
        command,
        "env -u CLICOLOR_FORCE -u FORCE_COLOR LC_ALL=C ls",
        &args,
    ))
}

fn rewrite_tree(command: &ClassifiedCommand) -> Option<String> {
    let values = argument_values(command);
    let show_all = values.iter().any(|value| value == "-a" || value == "--all");
    let has_ignore = values
        .iter()
        .any(|value| value == "-I" || value.starts_with("--ignore="));
    let mut args = Vec::new();
    if !show_all && !has_ignore {
        args.push("-I".to_string());
        args.push(NOISE_DIRS.join("|"));
    }
    args.extend(values);
    Some(render_command(command, "tree", &args))
}

fn parse_cluster(rest: &str) -> ClusterResult {
    let bytes = rest.as_bytes();
    let mut prefix = String::new();
    let mut index = 0;
    while index < bytes.len() {
        let character = bytes[index];
        if character == b'e' || SEARCH_VALUE_FLAGS_SHORT.contains(&character) {
            return ClusterResult::ValueTaking {
                prefix: (!prefix.is_empty()).then_some(prefix),
                flag: character as char,
                inline: std::str::from_utf8(&bytes[index + 1..])
                    .unwrap_or("")
                    .to_string(),
            };
        }
        prefix.push(character as char);
        index += 1;
    }
    ClusterResult::Boolean((!prefix.is_empty()).then_some(prefix))
}

fn parse_search_args(values: &[String]) -> SearchArgs {
    let mut explicit_patterns = Vec::new();
    let mut positionals = Vec::new();
    let mut flags = Vec::new();
    let mut past_dashdash = false;
    let mut index = 0;
    while index < values.len() {
        let value = values[index].as_str();
        if past_dashdash {
            positionals.push(value.to_string());
            index += 1;
            continue;
        }
        if value == "--" {
            past_dashdash = true;
            index += 1;
            continue;
        }
        if value.starts_with("--") {
            if value == "--regexp" {
                if let Some(next) = values.get(index + 1) {
                    explicit_patterns.push(next.clone());
                    index += 2;
                } else {
                    index += 1;
                }
                continue;
            }
            if SEARCH_VALUE_FLAGS_LONG.contains(&value) {
                flags.push(value.to_string());
                if let Some(next) = values.get(index + 1) {
                    flags.push(next.clone());
                    index += 2;
                } else {
                    index += 1;
                }
                continue;
            }
            flags.push(value.to_string());
            index += 1;
            continue;
        }
        match value.strip_prefix('-') {
            Some(rest) if !rest.is_empty() => match parse_cluster(rest) {
                ClusterResult::Boolean(prefix) => {
                    if let Some(prefix) = prefix {
                        flags.push(format!("-{prefix}"));
                    }
                    index += 1;
                }
                ClusterResult::ValueTaking {
                    prefix,
                    flag,
                    inline,
                } => {
                    if let Some(prefix) = prefix {
                        flags.push(format!("-{prefix}"));
                    }
                    if flag == 'e' {
                        if !inline.is_empty() {
                            explicit_patterns.push(inline);
                            index += 1;
                        } else if let Some(next) = values.get(index + 1) {
                            explicit_patterns.push(next.clone());
                            index += 2;
                        } else {
                            flags.push("-e".to_string());
                            index += 1;
                        }
                    } else {
                        flags.push(format!("-{flag}"));
                        if !inline.is_empty() {
                            flags.push(inline);
                            index += 1;
                        } else if let Some(next) = values.get(index + 1) {
                            flags.push(next.clone());
                            index += 2;
                        } else {
                            index += 1;
                        }
                    }
                }
            },
            _ => {
                positionals.push(value.to_string());
                index += 1;
            }
        }
    }
    let (patterns, paths) = if explicit_patterns.is_empty() {
        (
            positionals.iter().take(1).cloned().collect(),
            positionals.into_iter().skip(1).collect(),
        )
    } else {
        (explicit_patterns, positionals)
    };
    SearchArgs {
        patterns,
        paths,
        flags,
    }
}

fn search_args(command: &ClassifiedCommand) -> Option<SearchArgs> {
    let args = parse_search_args(&argument_values(command));
    (!args.patterns.is_empty()).then_some(args)
}

fn has_help_flag(command: &ClassifiedCommand) -> bool {
    argument_values(command)
        .iter()
        .any(|value| matches!(value.as_str(), "--version" | "--help" | "-h"))
}

fn has_short_flag(flags: &[String], character: char) -> bool {
    flags.iter().any(|flag| {
        flag.starts_with('-') && !flag.starts_with("--") && flag[1..].contains(character)
    })
}

fn has_search_format_flag(flags: &[String]) -> bool {
    const LONG: &[&str] = &[
        "--count",
        "--count-matches",
        "--files-with-matches",
        "--files-without-match",
        "--only-matching",
        "--quiet",
        "--silent",
        "--byte-offset",
        "--column",
        "--vimgrep",
        "--null",
        "--null-data",
        "--json",
        "--passthru",
        "--files",
    ];
    flags.iter().any(|flag| {
        if flag.starts_with("--") {
            LONG.contains(&flag.split('=').next().unwrap_or(flag))
        } else if let Some(letters) = flag.strip_prefix('-').filter(|value| !value.is_empty()) {
            letters
                .chars()
                .any(|character| matches!(character, 'c' | 'l' | 'L' | 'o' | 'q' | 'b' | 'Z' | 'z'))
        } else {
            false
        }
    })
}

fn rewrite_search(command: &ClassifiedCommand, engine: SearchEngine) -> Option<String> {
    let args = search_args(command)?;
    if has_search_format_flag(&args.flags) || has_help_flag(command) {
        return None;
    }
    let mut rewritten = engine
        .parse_flags()
        .iter()
        .map(|value| value.to_string())
        .collect::<Vec<_>>();
    rewritten.extend(args.flags);
    for pattern in args.patterns {
        rewritten.push("-e".to_string());
        rewritten.push(pattern);
    }
    rewritten.push("--".to_string());
    rewritten.extend(args.paths);
    Some(render_command(
        command,
        match engine {
            SearchEngine::Grep => "GREP_OPTIONS= grep",
            SearchEngine::Rg => "NO_COLOR=1 rg",
        },
        &rewritten,
    ))
}

fn ls_date_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r"\s+(Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)\s+\d{1,2}\s+(?:\d{4}|\d{2}:\d{2})\s+",
        )
        .expect("ls date regex is valid")
    })
}

fn is_dotdir(line: &str) -> bool {
    line.trim().ends_with('.') || line.trim().ends_with("..")
}

fn parse_ls_line(line: &str) -> Option<(char, String, u64, String)> {
    if is_dotdir(line) {
        return None;
    }
    let date_match = ls_date_regex().find(line)?;
    let name = line[date_match.end()..].to_string();
    let before = line[..date_match.start()]
        .split_whitespace()
        .collect::<Vec<_>>();
    if before.len() < 4 {
        return None;
    }
    let permissions = before[0].to_string();
    let file_type = permissions.chars().next()?;
    let size = before
        .iter()
        .rev()
        .find_map(|part| part.parse::<u64>().ok())
        .unwrap_or(0);
    Some((file_type, permissions, size, name))
}

fn human_size(bytes: u64) -> String {
    if bytes >= 1_048_576 {
        format!("{:.1}M", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.1}K", bytes as f64 / 1024.0)
    } else {
        format!("{bytes}B")
    }
}

fn permissions_to_octal(permissions: &str) -> Option<String> {
    if permissions.len() < 10 || !permissions.is_ascii() {
        return None;
    }
    let bytes = permissions.as_bytes();
    let value = |read: bool, write: bool, execute: bool| {
        ((read as u32) << 2) | ((write as u32) << 1) | execute as u32
    };
    let owner = value(
        bytes[1] == b'r',
        bytes[2] == b'w',
        matches!(bytes[3], b'x' | b's'),
    );
    let group = value(
        bytes[4] == b'r',
        bytes[5] == b'w',
        matches!(bytes[6], b'x' | b's'),
    );
    let other = value(
        bytes[7] == b'r',
        bytes[8] == b'w',
        matches!(bytes[9], b'x' | b't'),
    );
    let special = value(
        matches!(bytes[3], b's' | b'S'),
        matches!(bytes[6], b's' | b'S'),
        matches!(bytes[9], b't' | b'T'),
    );
    if special > 0 {
        Some(format!("{special}{owner}{group}{other}"))
    } else {
        Some(format!("{owner}{group}{other}"))
    }
}

fn compact_ls_for_command(command: &ClassifiedCommand, raw: &str) -> Option<String> {
    let values = argument_values(command);
    let show_all = values.iter().any(|value| {
        (value.starts_with('-') && !value.starts_with("--") && value.contains('a'))
            || value == "--all"
    });
    let show_long = values.iter().any(|value| {
        value == "--full-time"
            || value == "--format=long"
            || value == "--format=verbose"
            || value.starts_with('-')
                && !value.starts_with("--")
                && value
                    .chars()
                    .any(|character| matches!(character, 'l' | 'g' | 'n' | 'o'))
    });
    compact_ls(raw, show_all, show_long)
}

fn compact_ls(raw: &str, show_all: bool, show_long: bool) -> Option<String> {
    let mut directories = Vec::new();
    let mut files = Vec::new();
    let mut lines_seen = 0;
    let mut parsed = 0;
    let mut dotdirs = 0;
    for line in raw.lines() {
        if line.starts_with("total ") || line.is_empty() {
            continue;
        }
        lines_seen += 1;
        let Some((file_type, permissions, size, name)) = parse_ls_line(line) else {
            if is_dotdir(line) {
                dotdirs += 1;
            }
            continue;
        };
        parsed += 1;
        if !show_all && NOISE_DIRS.iter().any(|noise| name == *noise) {
            continue;
        }
        let octal = show_long
            .then(|| permissions_to_octal(&permissions))
            .flatten();
        if file_type == 'd' {
            directories.push((name, octal));
        } else {
            files.push((name, human_size(size), octal));
        }
    }
    if directories.is_empty() && files.is_empty() {
        if lines_seen > 0 && parsed == 0 && dotdirs != lines_seen {
            return None;
        }
        return Some("(empty)\n".to_string());
    }
    let mut output = String::new();
    for (name, octal) in directories {
        if let Some(octal) = octal {
            output.push_str(&octal);
            output.push_str("  ");
        }
        output.push_str(&name);
        output.push_str("/\n");
    }
    for (name, size, octal) in files {
        if let Some(octal) = octal {
            output.push_str(&octal);
            output.push_str("  ");
        }
        output.push_str(&name);
        output.push_str("  ");
        output.push_str(&size);
        output.push('\n');
    }
    Some(output)
}

fn filter_tree_output(raw: &str) -> String {
    if raw.lines().next().is_none() {
        return "\n".to_string();
    }
    let mut lines = raw
        .lines()
        .filter(|line| !(line.contains("director") && line.contains("file")))
        .skip_while(|line| line.trim().is_empty())
        .collect::<Vec<_>>();
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    format!("{}\n", lines.join("\n"))
}

fn parse_find_args(values: &[String]) -> Result<FindArgs, String> {
    if values.is_empty() {
        return Ok(FindArgs::default());
    }
    if values
        .iter()
        .any(|value| UNSUPPORTED_FIND_FLAGS.contains(&value.as_str()))
    {
        return Err(
            "the bash filter does not support compound predicates or actions (e.g. -not, -exec) in find. Switch the filter off with /bash-filter off to run find directly."
                .to_string(),
        );
    }
    let native = values
        .iter()
        .any(|value| matches!(value.as_str(), "-name" | "-type" | "-maxdepth" | "-iname"));
    if native {
        parse_native_find_args(values)
    } else {
        parse_compact_find_args(values)
    }
}

fn take_next(values: &[String], index: &mut usize) -> Option<String> {
    *index += 1;
    values.get(*index).cloned()
}

fn parse_native_find_args(values: &[String]) -> Result<FindArgs, String> {
    let mut parsed = FindArgs::default();
    let mut index = 0;
    if !values[0].starts_with('-') {
        parsed.path = values[0].clone();
        index = 1;
    }
    while index < values.len() {
        match values[index].as_str() {
            "-name" => {
                if let Some(value) = take_next(values, &mut index) {
                    parsed.pattern = value;
                }
            }
            "-iname" => {
                if let Some(value) = take_next(values, &mut index) {
                    parsed.pattern = value;
                    parsed.case_insensitive = true;
                }
            }
            "-type" => {
                if let Some(value) = take_next(values, &mut index) {
                    parsed.file_type = value;
                }
            }
            "-maxdepth" => {
                if let Some(value) = take_next(values, &mut index) {
                    parsed.max_depth = Some(
                        value
                            .parse()
                            .map_err(|_| "invalid -maxdepth value".to_string())?,
                    );
                }
            }
            _ => {}
        }
        index += 1;
    }
    Ok(parsed)
}

fn parse_compact_find_args(values: &[String]) -> Result<FindArgs, String> {
    let mut parsed = FindArgs {
        pattern: values[0].clone(),
        ..FindArgs::default()
    };
    let mut index = 1;
    if index < values.len() && !values[index].starts_with('-') {
        parsed.path = values[index].clone();
        index += 1;
    }
    while index < values.len() {
        match values[index].as_str() {
            "-m" | "--max" => {
                if let Some(value) = take_next(values, &mut index) {
                    parsed.max_results = value
                        .parse()
                        .map_err(|_| "invalid --max value".to_string())?;
                }
            }
            "-t" | "--file-type" => {
                if let Some(value) = take_next(values, &mut index) {
                    parsed.file_type = value;
                }
            }
            _ => {}
        }
        index += 1;
    }
    Ok(parsed)
}

/// Matches a `find -name` glob: `*` spans any run of characters, `?` one.
/// Iterative with a single backtrack point, so the cost stays proportional to
/// the input. The obvious recursive form — try both branches at every `*` —
/// backtracks exponentially: a pattern of 14 `*a` pairs against a 40-character
/// name took over four minutes, and the pattern comes from the model.
/// Steps over `char`s, not bytes, so `?` means one character. On bytes,
/// `find . -name "?.rs"` missed `é.rs` while `??.rs` matched it.
fn glob_match(pattern: &str, name: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let name: Vec<char> = name.chars().collect();
    let (mut p, mut n) = (0usize, 0usize);
    // Where to resume if the current attempt dies: the last `*` seen, and the
    // position after the last character it had swallowed.
    let (mut star, mut after_star) = (None, 0usize);

    while n < name.len() {
        match pattern.get(p) {
            Some('*') => {
                star = Some(p);
                after_star = n;
                p += 1;
            }
            Some('?') => {
                p += 1;
                n += 1;
            }
            Some(c) if *c == name[n] => {
                p += 1;
                n += 1;
            }
            // Mismatch: let the last `*` swallow one more character and retry
            // from there. Without a `*` behind us there is nothing to retry.
            _ => match star {
                Some(star) => {
                    p = star + 1;
                    after_star += 1;
                    n = after_star;
                }
                None => return false,
            },
        }
    }

    // The name is used up; the pattern may only have trailing `*` left.
    while pattern.get(p) == Some(&'*') {
        p += 1;
    }
    p == pattern.len()
}

fn execute_find(command: &ClassifiedCommand, cwd: &Path) -> ExecutedOutput {
    let args = match parse_find_args(&argument_values(command)) {
        Ok(args) => args,
        Err(error) => return ExecutedOutput::failure(error),
    };
    let pattern = if args.pattern == "." {
        "*"
    } else {
        &args.pattern
    };
    let root = resolve_path(cwd, &args.path);
    let mut builder = WalkBuilder::new(&root);
    builder
        .hidden(!pattern.starts_with('.'))
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true);
    if let Some(depth) = args.max_depth {
        builder.max_depth(Some(depth));
    }
    let want_directories = args.file_type == "d";
    let mut paths = Vec::new();
    for entry in builder.build().flatten() {
        let file_type = entry.file_type();
        let is_directory = file_type.as_ref().is_some_and(std::fs::FileType::is_dir);
        if want_directories != is_directory {
            continue;
        }
        let Some(name) = entry.path().file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let matches = if args.case_insensitive {
            glob_match(&pattern.to_lowercase(), &name.to_lowercase())
        } else {
            glob_match(pattern, name)
        };
        if !matches {
            continue;
        }
        let display = entry
            .path()
            .strip_prefix(&root)
            .unwrap_or(entry.path())
            .to_string_lossy()
            .to_string();
        if !display.is_empty() {
            paths.push(display);
        }
    }
    paths.sort();
    ExecutedOutput::success(paths.join("\n"))
}

fn format_find(command: &ClassifiedCommand, raw: &str) -> Option<String> {
    let args = parse_find_args(&argument_values(command)).ok()?;
    let files = raw.lines().map(str::to_string).collect::<Vec<_>>();
    if files.is_empty() {
        return Some(String::new());
    }
    let mut by_directory = BTreeMap::<String, Vec<String>>::new();
    for file in &files {
        let path = Path::new(file);
        let directory = path
            .parent()
            .map(|value| value.to_string_lossy().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| ".".to_string());
        let filename = path
            .file_name()
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_default();
        by_directory.entry(directory).or_default().push(filename);
    }
    let mut output = format!("{}F {}D:\n\n", files.len(), by_directory.len());
    let mut displayed = 0;
    for (directory, names) in &by_directory {
        if displayed >= args.max_results {
            break;
        }
        let display = if directory.len() > 50 {
            format!("...{}", &directory[directory.len() - 47..])
        } else {
            directory.clone()
        };
        let remaining = args.max_results - displayed;
        let visible = names.iter().take(remaining).cloned().collect::<Vec<_>>();
        output.push_str(&format!("{display}/ {}\n", visible.join(" ")));
        displayed += visible.len();
    }
    if displayed < files.len() {
        output.push_str(&format!("+{} more\n", files.len() - displayed));
    }
    let mut extensions = BTreeMap::<String, usize>::new();
    for file in &files {
        let extension = Path::new(file)
            .extension()
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_else(|| "none".to_string());
        *extensions.entry(extension).or_default() += 1;
    }
    if extensions.len() > 1 {
        let mut extensions = extensions.into_iter().collect::<Vec<_>>();
        extensions.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
        output.push('\n');
        output.push_str("ext: ");
        output.push_str(
            &extensions
                .into_iter()
                .take(5)
                .map(|(extension, count)| format!(".{extension}({count})"))
                .collect::<Vec<_>>()
                .join(" "),
        );
        output.push('\n');
    }
    Some(output)
}

fn parse_match_line(line: &str) -> Option<(String, usize, bool, &str)> {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    let regex = REGEX.get_or_init(|| {
        Regex::new(r"^([^\x00]+)\x00(\d+)([:-])(.*)$").expect("search output regex is valid")
    });
    let captures = regex.captures(line)?;
    Some((
        captures.get(1)?.as_str().to_string(),
        captures.get(2)?.as_str().parse().ok()?,
        captures.get(3)?.as_str() == ":",
        captures.get(4)?.as_str(),
    ))
}

fn format_search(
    command: &ClassifiedCommand,
    cwd: &Path,
    raw: &str,
    _engine: SearchEngine,
) -> Option<String> {
    if raw.lines().any(|line| {
        let line = line.trim();
        !line.is_empty() && line != "--" && parse_match_line(line).is_none()
    }) {
        return None;
    }
    let args = search_args(command)?;
    let mut by_file = BTreeMap::<String, Vec<(usize, bool, String)>>::new();
    for line in raw.lines() {
        let Some((file, line_number, is_match, content)) = parse_match_line(line) else {
            continue;
        };
        by_file.entry(file).or_default().push((
            line_number,
            is_match,
            truncate_search_line(content, 80, &args.patterns.join("|")),
        ));
    }
    if by_file.is_empty() {
        return Some(String::new());
    }
    let show_file = by_file.len() > 1
        || args.paths.len() > 1
        || args
            .paths
            .iter()
            .any(|path| resolve_path(cwd, path).is_dir())
        || has_short_flag(&args.flags, 'H')
        || has_short_flag(&args.flags, 'r')
        || has_short_flag(&args.flags, 'R')
        || args
            .flags
            .iter()
            .any(|flag| flag == "--with-filename" || flag == "--recursive");
    let has_context = has_short_flag(&args.flags, 'A')
        || has_short_flag(&args.flags, 'B')
        || has_short_flag(&args.flags, 'C')
        || args.flags.iter().any(|flag| {
            flag == "--after-context"
                || flag == "--before-context"
                || flag == "--context"
                || flag.starts_with("--after-context=")
                || flag.starts_with("--before-context=")
                || flag.starts_with("--context=")
        });
    let show_line = !has_short_flag(&args.flags, 'N')
        && !args.flags.iter().any(|flag| flag == "--no-line-number");
    let mut plain = String::new();
    for line in raw.lines() {
        let Some((file, line_number, is_match, content)) = parse_match_line(line) else {
            if line == "--" {
                plain.push_str("--\n");
            }
            continue;
        };
        let separator = if is_match { ':' } else { '-' };
        if show_file {
            plain.push_str(&file);
            plain.push(separator);
        }
        if show_line {
            plain.push_str(&line_number.to_string());
            plain.push(separator);
        }
        plain.push_str(content);
        plain.push('\n');
    }
    let total_matches = by_file
        .values()
        .flatten()
        .filter(|(_, is_match, _)| *is_match)
        .count();
    let mut body = String::new();
    let mut shown = 0;
    let mut skipped_files = 0;
    for (file, entries) in &by_file {
        if shown >= 200 {
            skipped_files += 1;
            continue;
        }
        let mut previous = 0;
        let mut file_shown = 0;
        for (line, is_match, content) in entries.iter().take(25) {
            if shown >= 200 {
                break;
            }
            if has_context && previous > 0 && *line > previous + 1 {
                body.push_str("--\n");
            }
            previous = *line;
            let separator = if *is_match { ':' } else { '-' };
            if show_file {
                body.push_str(&compact_path(file));
                body.push(separator);
            }
            if show_line {
                body.push_str(&line.to_string());
                body.push(separator);
            }
            body.push_str(content);
            body.push('\n');
            shown += 1;
            file_shown += 1;
        }
        if file_shown < entries.len() {
            body.push_str(&format!(
                "  +{} more in {}\n",
                entries.len() - file_shown,
                compact_path(file)
            ));
        }
    }
    if skipped_files > 0 {
        body.push_str(&format!("+{skipped_files} more files\n"));
    }
    let capped = shown < total_matches || skipped_files > 0;
    let grouped = format!(
        "{} matches in {} files:\n\n{body}",
        total_matches,
        by_file.len()
    );
    let output = if capped && grouped.len() < plain.len() {
        grouped
    } else {
        plain
    };
    (!output.is_empty()).then_some(output)
}

fn truncate_search_line(line: &str, max: usize, pattern: &str) -> String {
    let line = line.trim();
    if line.len() <= max {
        return line.to_string();
    }
    let lowercase = line.to_lowercase();
    let pattern = pattern.to_lowercase();
    let characters = line.chars().collect::<Vec<_>>();
    if let Some(byte_position) = lowercase.find(&pattern) {
        let character_position = lowercase[..byte_position].chars().count();
        let start = character_position.saturating_sub(max / 3);
        let end = (start + max).min(characters.len());
        let start = if end == characters.len() {
            end.saturating_sub(max)
        } else {
            start
        };
        let slice = characters[start..end].iter().collect::<String>();
        match (start > 0, end < characters.len()) {
            (true, true) => format!("...{slice}..."),
            (true, false) => format!("...{slice}"),
            (false, true) => format!("{slice}..."),
            (false, false) => slice,
        }
    } else {
        format!(
            "{}...",
            characters.into_iter().take(max - 3).collect::<String>()
        )
    }
}

fn compact_path(path: &str) -> String {
    if path.len() <= 50 {
        return path.to_string();
    }
    let parts = path.split('/').collect::<Vec<_>>();
    if parts.len() <= 3 {
        return path.to_string();
    }
    format!(
        "{}/.../{}/{}",
        parts[0],
        parts[parts.len() - 2],
        parts[parts.len() - 1]
    )
}

fn read_mode(command: &ClassifiedCommand) -> Option<ReadMode> {
    let executable = basename(&command.tokens[command.effective_index].value);
    let values = argument_values(command);
    match executable {
        "cat" => {
            let line_numbers = values.first().is_some_and(|value| value == "-n");
            let files = if line_numbers {
                &values[1..]
            } else {
                &values[..]
            };
            (!files.is_empty()
                && files.iter().all(|value| !value.starts_with('-'))
                && !values
                    .iter()
                    .any(|value| value.starts_with('-') && value != "-n"))
            .then_some(ReadMode::Cat { line_numbers })
        }
        "head" => parse_head_mode(&values),
        "tail" => parse_tail_mode(&values),
        _ => None,
    }
}

fn parse_head_mode(values: &[String]) -> Option<ReadMode> {
    match values {
        [file] if !file.starts_with('-') => Some(ReadMode::Head { max_lines: None }),
        [flag, _file] => {
            let count = flag
                .strip_prefix('-')
                .filter(|value| value.chars().all(|character| character.is_ascii_digit()))
                .or_else(|| flag.strip_prefix("--lines="))?
                .parse()
                .ok()?;
            Some(ReadMode::Head {
                max_lines: Some(count),
            })
        }
        _ => None,
    }
}

fn parse_tail_mode(values: &[String]) -> Option<ReadMode> {
    let (count, has_file) = match values {
        [flag, _file] => (
            flag.strip_prefix('-')
                .filter(|value| value.chars().all(|character| character.is_ascii_digit()))
                .or_else(|| flag.strip_prefix("--lines=")),
            true,
        ),
        [flag, count, _file] if flag == "-n" || flag == "--lines" => (Some(count.as_str()), true),
        _ => (None, false),
    };
    has_file.then_some(())?;
    Some(ReadMode::Tail {
        tail_lines: count?.parse().ok()?,
    })
}

fn read_files(command: &ClassifiedCommand) -> Vec<String> {
    let values = argument_values(command);
    match read_mode(command) {
        Some(ReadMode::Cat { line_numbers: true }) => values.into_iter().skip(1).collect(),
        Some(ReadMode::Cat {
            line_numbers: false,
        }) => values,
        Some(ReadMode::Head { .. }) => values.into_iter().rev().take(1).collect(),
        Some(ReadMode::Tail { .. }) => values.into_iter().rev().take(1).collect(),
        None => Vec::new(),
    }
}

fn execute_read(command: &ClassifiedCommand, cwd: &Path) -> ExecutedOutput {
    let files = read_files(command);
    let mut stdout = String::new();
    let mut stderr = String::new();
    let mut failed = false;
    for file in files {
        if file == "-" {
            continue;
        }
        let path = resolve_path(cwd, &file);
        match std::fs::read_to_string(&path) {
            Ok(content) => {
                if matches!(
                    read_mode(command),
                    Some(ReadMode::Cat { line_numbers: true })
                ) {
                    stdout.push_str(&format_with_line_numbers(&content));
                } else {
                    stdout.push_str(&content);
                }
            }
            Err(error) => {
                failed = true;
                stderr.push_str(&format!("cat: {file}: {error}\n"));
            }
        }
    }
    ExecutedOutput {
        stdout,
        stderr,
        exit_code: Some(i32::from(failed)),
    }
}

fn format_read(command: &ClassifiedCommand, raw: &str) -> Option<String> {
    match read_mode(command)? {
        ReadMode::Cat { .. } => Some(raw.to_string()),
        ReadMode::Head { max_lines } => Some(max_lines.map_or_else(
            || raw.to_string(),
            |max_lines| smart_truncate(raw, max_lines),
        )),
        ReadMode::Tail { tail_lines } => Some(tail_lines_of(raw, tail_lines)),
    }
}

fn format_with_line_numbers(content: &str) -> String {
    let lines = content.lines().collect::<Vec<_>>();
    let width = lines.len().to_string().len();
    let mut output = String::new();
    for (index, line) in lines.iter().enumerate() {
        output.push_str(&format!("{:>width$} │ {line}\n", index + 1, width = width));
    }
    output
}

fn signature_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"^(pub\s+)?(async\s+)?(fn|def|function|func|class|struct|enum|trait|interface|type)\s+\w+")
            .expect("signature regex is valid")
    })
}
fn import_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"^(use |import |from |require\(|#include)").expect("import regex is valid")
    })
}
fn smart_truncate(content: &str, max_lines: usize) -> String {
    let lines = content.lines().collect::<Vec<_>>();
    if lines.len() <= max_lines {
        return content.to_string();
    }
    let important = |line: &str| {
        let line = line.trim();
        signature_regex().is_match(line)
            || import_regex().is_match(line)
            || line.starts_with("pub ")
            || line.starts_with("export ")
            || matches!(line, "}" | "{")
    };
    let mut output = Vec::with_capacity(max_lines + 1);
    let mut kept = 0;
    for line in &lines {
        if important(line) || kept < max_lines / 2 {
            output.push((*line).to_string());
            kept += 1;
        }
        if kept >= max_lines.saturating_sub(1) {
            break;
        }
    }
    output.push(format!("[{} more lines]", lines.len() - kept));
    output.join("\n")
}

fn tail_lines_of(content: &str, count: usize) -> String {
    if count == 0 {
        return String::new();
    }
    let lines = content.lines().collect::<Vec<_>>();
    let start = lines.len().saturating_sub(count);
    let mut output = lines[start..].join("\n");
    if content.ends_with('\n') {
        output.push('\n');
    }
    output
}

fn resolve_path(cwd: &Path, value: &str) -> PathBuf {
    let path = Path::new(value);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::bash_filter::command::classify;

    #[test]
    fn glob_matches_the_shapes_find_is_given() {
        for (pattern, name, expected) in [
            ("*.rs", "main.rs", true),
            ("*.rs", "main.rss", false),
            ("main.*", "main.rs", true),
            ("*", "anything", true),
            ("*", "", true),
            ("", "", true),
            ("", "x", false),
            ("a*b*c", "azzbzzc", true),
            ("a*b*c", "azzbzz", false),
            ("**", "ab", true),
            ("?.rs", "a.rs", true),
            ("?.rs", "ab.rs", false),
            ("a?c", "abc", true),
            // `?` is one character, not one byte.
            ("?.rs", "é.rs", true),
            ("??.rs", "é.rs", false),
            ("*é*", "aébc", true),
        ] {
            assert_eq!(
                glob_match(pattern, name),
                expected,
                "{pattern:?} against {name:?}"
            );
        }
    }

    #[test]
    fn a_pathological_glob_finishes_promptly() {
        // The recursive form needed minutes for this; anything near a second
        // means the exponential backtracking is back.
        let pattern = format!("{}b", "*a".repeat(24));
        let name = "a".repeat(64);
        let started = std::time::Instant::now();
        assert!(!glob_match(&pattern, &name));
        assert!(
            started.elapsed() < std::time::Duration::from_millis(200),
            "took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn rewrites_ls_tree_and_search_to_their_parse_friendly_form() {
        let fixtures = [
            (
                "ls src",
                "env -u CLICOLOR_FORCE -u FORCE_COLOR LC_ALL=C ls -la src",
            ),
            (
                "tree .",
                "tree -I 'node_modules|.git|target|__pycache__|.next|dist|build|.cache|.turbo|.vercel|.pytest_cache|.mypy_cache|.tox|.venv|venv|env|coverage|.nyc_output|.DS_Store|Thumbs.db|.idea|.vscode|.vs|*.egg-info|.eggs' .",
            ),
            (
                "rg TODO src",
                "NO_COLOR=1 rg -n --with-filename --null -e TODO -- src",
            ),
            (
                "grep -i TODO src",
                "GREP_OPTIONS= grep -n -H -I --null -i -e TODO -- src",
            ),
        ];
        let actual = fixtures
            .iter()
            .map(|(command, _)| {
                let command = classify(command).unwrap();
                let filter = match basename(&command.tokens[command.effective_index].value) {
                    "ls" => NativeFilter::Ls,
                    "tree" => NativeFilter::Tree,
                    "rg" => NativeFilter::Rg,
                    _ => NativeFilter::Grep,
                };
                rewrite(&command, filter).unwrap()
            })
            .collect::<Vec<_>>();
        let expected = fixtures
            .iter()
            .map(|(_, expected)| expected.to_string())
            .collect::<Vec<_>>();
        assert_eq!(actual, expected);
    }

    #[test]
    fn formats_pinned_ls_tree_find_and_search_fixtures() {
        let ls = classify("ls").unwrap();
        let tree = classify("tree").unwrap();
        let find = classify("find . -name *.rs").unwrap();
        let rg = classify("rg TODO src").unwrap();
        let actual = [
            apply(
                NativeFilter::Ls,
                &ls,
                Path::new("."),
                "total 8\ndrwxr-xr-x 2 user staff 64 Jan  1 12:00 src\n-rw-r--r-- 1 user staff 1234 Jan  1 12:00 Cargo.toml\n",
                Some(0),
            )
            .unwrap(),
            apply(
                NativeFilter::Tree,
                &tree,
                Path::new("."),
                ".\n└── src\n\n1 directory, 0 files\n",
                Some(0),
            )
            .unwrap(),
            apply(
                NativeFilter::Find,
                &find,
                Path::new("."),
                "src/lib.rs\nsrc/main.rs",
                Some(0),
            )
            .unwrap(),
            apply(
                NativeFilter::Rg,
                &rg,
                Path::new("."),
                "src/lib.rs\x0010:TODO first\nsrc/main.rs\x0020:TODO second\n",
                Some(0),
            )
            .unwrap(),
        ];
        let expected = [
            "src/\nCargo.toml  1.2K\n".to_string(),
            ".\n└── src\n".to_string(),
            "2F 1D:\n\nsrc/ lib.rs main.rs\n".to_string(),
            "src/lib.rs:10:TODO first\nsrc/main.rs:20:TODO second\n".to_string(),
        ];
        assert_eq!(actual, expected);
    }

    #[test]
    fn read_modes_match_the_rewrite_contract() {
        let fixtures = [
            (
                "cat file.rs",
                Some(ReadMode::Cat {
                    line_numbers: false,
                }),
            ),
            ("cat -n file.rs", Some(ReadMode::Cat { line_numbers: true })),
            ("cat -A file.rs", None),
            ("head file.rs", Some(ReadMode::Head { max_lines: None })),
            (
                "head -50 file.rs",
                Some(ReadMode::Head {
                    max_lines: Some(50),
                }),
            ),
            ("head -3 a b", None),
            (
                "tail -n 20 file.rs",
                Some(ReadMode::Tail { tail_lines: 20 }),
            ),
            ("tail file.rs", None),
        ];
        let actual = fixtures
            .iter()
            .map(|(command, _)| read_mode(&classify(command).unwrap()))
            .collect::<Vec<_>>();
        let expected = fixtures.iter().map(|(_, mode)| *mode).collect::<Vec<_>>();
        assert_eq!(actual, expected);
    }

    #[test]
    fn system_dispatcher_covers_all_frequent_commands_and_safe_passthroughs() {
        let fixtures = [
            ("ls", Some(NativeFilter::Ls)),
            ("tree .", Some(NativeFilter::Tree)),
            ("find . -name *.rs", Some(NativeFilter::Find)),
            ("grep TODO src", Some(NativeFilter::Grep)),
            ("rg TODO src", Some(NativeFilter::Rg)),
            ("cat file.rs", Some(NativeFilter::Read)),
            ("head file.rs", Some(NativeFilter::Read)),
            ("head -20 file.rs", Some(NativeFilter::Read)),
            ("tail -n 20 file.rs", Some(NativeFilter::Read)),
            ("cat -A file.rs", None),
            ("head -3 a b", None),
            ("tail file.rs", None),
            ("rg --json TODO src", None),
            ("grep -c TODO src", None),
        ];
        let actual = fixtures
            .iter()
            .map(|(command, _)| {
                let command = classify(command).unwrap();
                crate::core::bash_filter::native::detect(&command)
                    .filter(|filter| eligible(&command, *filter))
            })
            .collect::<Vec<_>>();
        let expected = fixtures
            .iter()
            .map(|(_, filter)| *filter)
            .collect::<Vec<_>>();
        assert_eq!(actual, expected);
    }

    #[test]
    fn filesystem_adapters_match_read_and_find_failure_contracts() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("sample.txt");
        std::fs::write(&file, "alpha\nbeta\ngamma\n").unwrap();
        let cat = classify("cat -n sample.txt").unwrap();
        let head = classify("head -2 sample.txt").unwrap();
        let tail = classify("tail -n 2 sample.txt").unwrap();
        let invalid_find = classify("find . -name *.rs -exec echo {} ;")
            .unwrap_or_else(|| classify("find . -name *.rs -exec").unwrap());
        let actual = (
            execute_read(&cat, directory.path()),
            execute_read(&head, directory.path()),
            execute_read(&tail, directory.path()),
            execute_find(&invalid_find, directory.path()),
        );
        assert_eq!(actual.0.stdout, "1 │ alpha\n2 │ beta\n3 │ gamma\n");
        assert_eq!(
            format_read(&head, &actual.1.stdout).unwrap(),
            "alpha\n[2 more lines]"
        );
        assert_eq!(
            format_read(&tail, &actual.2.stdout).unwrap(),
            "beta\ngamma\n"
        );
        assert_eq!(actual.3.exit_code, Some(1));
        assert!(actual.3.stderr.contains("compound predicates"));
    }

    #[test]
    fn uncapped_search_preserves_the_faithful_native_shape() {
        let command = classify("rg TODO src/lib.rs").unwrap();
        let raw = "src/lib.rs\x0010:TODO first\nsrc/lib.rs\x0020:TODO second\n";
        let actual = apply(NativeFilter::Rg, &command, Path::new("."), raw, Some(0)).unwrap();
        let expected = "10:TODO first\n20:TODO second\n";
        assert_eq!(actual, expected);
    }

    #[test]
    fn capped_search_uses_the_grouped_shape() {
        let command = classify("rg TODO src").unwrap();
        let content = "x".repeat(120);
        let raw = (1..=30)
            .map(|line| format!("src/lib.rs\x00{line}:TODO {content}"))
            .collect::<Vec<_>>()
            .join("\n");
        let actual = apply(NativeFilter::Rg, &command, Path::new("."), &raw, Some(0)).unwrap();
        assert!(actual.starts_with("30 matches in 1 files:\n\n"));
        assert!(actual.contains("+5 more in src/lib.rs"));
        assert!(
            crate::core::bash_filter::estimate_tokens(&actual)
                < crate::core::bash_filter::estimate_tokens(&raw)
        );
    }

    #[test]
    fn plain_head_reads_the_full_file_like_the_pinned_rewrite() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("sample.txt"), "one\ntwo\nthree\n").unwrap();
        let command = classify("head sample.txt").unwrap();
        let actual = execute_read(&command, directory.path());
        let filtered = format_read(&command, &actual.stdout).unwrap();
        let expected = "one\ntwo\nthree\n";
        assert_eq!(filtered, expected);
    }

    #[test]
    fn missing_find_root_matches_pinned_empty_result_behavior() {
        let directory = tempfile::tempdir().unwrap();
        let command = classify("find missing -name *.rs").unwrap();
        let actual = execute_find(&command, directory.path());
        let expected = ExecutedOutput::success(String::new());
        assert_eq!(actual, expected);
    }

    #[test]
    fn zero_tail_preserves_intentional_empty_output() {
        let command = classify("tail -0 sample.txt").unwrap();
        let raw = "one\ntwo\n";
        let actual = format_read(&command, raw).unwrap();
        assert!(actual.is_empty());
        assert!(allows_empty_output(NativeFilter::Read, &command));
    }

    #[test]
    fn search_truncation_uses_the_pinned_utf8_byte_threshold() {
        let line = "界".repeat(30);
        let actual = truncate_search_line(&line, 80, "TODO");
        assert!(actual.ends_with("..."));
        assert_ne!(actual, line);
    }

    #[test]
    fn reports_measured_system_fixture_savings() {
        let fixtures = [
            (
                "ls",
                "ls",
                "total 16\ndrwxr-xr-x 2 user staff 64 Jan  1 12:00 src\ndrwxr-xr-x 2 user staff 64 Jan  1 12:00 tests\n-rw-r--r-- 1 user staff 1234 Jan  1 12:00 Cargo.toml\n-rw-r--r-- 1 user staff 5678 Jan  1 12:00 README.md\n",
                NativeFilter::Ls,
            ),
            (
                "tree",
                "tree .",
                ".\n├── src\n│   ├── lib.rs\n│   └── main.rs\n└── tests\n    └── integration.rs\n\n3 directories, 3 files\n",
                NativeFilter::Tree,
            ),
            (
                "find",
                "find . -name *.rs",
                "src/lib.rs\nsrc/main.rs\ntests/integration.rs\ncrates/demo/src/lib.rs\ncrates/demo/src/main.rs",
                NativeFilter::Find,
            ),
            (
                "rg",
                "rg TODO src",
                "src/lib.rs\x0010:TODO first item with a long explanatory message that should be compacted for agent context\nsrc/lib.rs\x0020:TODO second item with another long explanatory message that should be compacted for agent context\nsrc/main.rs\x0030:TODO third item with a long explanatory message that should be compacted for agent context\n",
                NativeFilter::Rg,
            ),
        ];
        let actual = fixtures
            .iter()
            .map(|(name, command, raw, filter)| {
                let command = classify(command).unwrap();
                let reduced = apply(*filter, &command, Path::new("."), raw, Some(0)).unwrap();
                let raw_tokens = crate::core::bash_filter::estimate_tokens(raw);
                let reduced_tokens = crate::core::bash_filter::estimate_tokens(&reduced);
                let savings = ((raw_tokens - reduced_tokens) as f64 / raw_tokens as f64) * 100.0;
                println!(
                    "{name}: raw={raw_tokens}, reduced={reduced_tokens}, savings={savings:.2}%"
                );
                (*name, raw_tokens, reduced_tokens)
            })
            .collect::<Vec<_>>();
        let expected = vec![
            ("ls", 51, 12),
            ("tree", 33, 27),
            ("find", 23, 21),
            ("rg", 81, 81),
        ];
        assert_eq!(actual, expected);
    }
}
