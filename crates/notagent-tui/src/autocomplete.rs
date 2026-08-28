use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;

use crate::fuzzy::fuzzy_filter;
use crate::node_path::{basename, dirname, homedir, join};

const PATH_DELIMITERS: [char; 5] = [' ', '\t', '"', '\'', '='];

fn is_path_delimiter(character: char) -> bool {
    PATH_DELIMITERS.contains(&character)
}

fn to_display_path(value: &str) -> String {
    value.replace('\\', "/")
}

fn escape_regex(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    for character in value.chars() {
        if ".*+?^${}()|[]\\".contains(character) {
            result.push('\\');
        }
        result.push(character);
    }
    result
}

/// Build the `fd` pattern for a query that contains path separators.
fn build_fd_path_query(query: &str) -> String {
    let normalized = to_display_path(query);
    if !normalized.contains('/') {
        return normalized;
    }

    let has_trailing_separator = normalized.ends_with('/');
    let trimmed = normalized.trim_matches('/');
    if trimmed.is_empty() {
        return normalized;
    }

    let separator_pattern = "[\\\\/]";
    let segments: Vec<String> = trimmed
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(escape_regex)
        .collect();
    if segments.is_empty() {
        return normalized;
    }

    let mut pattern = segments.join(separator_pattern);
    if has_trailing_separator {
        pattern.push_str(separator_pattern);
    }
    pattern
}

/// Byte index of the last path delimiter, or `None`.
fn find_last_delimiter(text: &str) -> Option<usize> {
    text.char_indices()
        .rev()
        .find(|(_, character)| is_path_delimiter(*character))
        .map(|(index, _)| index)
}

/// Byte index where an unclosed double quote starts.
fn find_unclosed_quote_start(text: &str) -> Option<usize> {
    let mut in_quotes = false;
    let mut quote_start = 0;
    for (index, character) in text.char_indices() {
        if character == '"' {
            in_quotes = !in_quotes;
            if in_quotes {
                quote_start = index;
            }
        }
    }
    in_quotes.then_some(quote_start)
}

fn is_token_start(text: &str, index: usize) -> bool {
    index == 0
        || text[..index]
            .chars()
            .next_back()
            .is_some_and(is_path_delimiter)
}

fn extract_quoted_prefix(text: &str) -> Option<&str> {
    let quote_start = find_unclosed_quote_start(text)?;

    if quote_start > 0 && text[..quote_start].ends_with('@') {
        let at_index = quote_start - 1;
        if !is_token_start(text, at_index) {
            return None;
        }
        return Some(&text[at_index..]);
    }

    if !is_token_start(text, quote_start) {
        return None;
    }

    Some(&text[quote_start..])
}

/// Prefix split into its raw path plus the `@` and quote flags.
struct ParsedPathPrefix {
    raw_prefix: String,
    is_at_prefix: bool,
    is_quoted_prefix: bool,
}

fn parse_path_prefix(prefix: &str) -> ParsedPathPrefix {
    if let Some(rest) = prefix.strip_prefix("@\"") {
        return ParsedPathPrefix {
            raw_prefix: rest.to_string(),
            is_at_prefix: true,
            is_quoted_prefix: true,
        };
    }
    if let Some(rest) = prefix.strip_prefix('"') {
        return ParsedPathPrefix {
            raw_prefix: rest.to_string(),
            is_at_prefix: false,
            is_quoted_prefix: true,
        };
    }
    if let Some(rest) = prefix.strip_prefix('@') {
        return ParsedPathPrefix {
            raw_prefix: rest.to_string(),
            is_at_prefix: true,
            is_quoted_prefix: false,
        };
    }
    ParsedPathPrefix {
        raw_prefix: prefix.to_string(),
        is_at_prefix: false,
        is_quoted_prefix: false,
    }
}

fn build_completion_value(path: &str, is_at_prefix: bool, is_quoted_prefix: bool) -> String {
    let needs_quotes = is_quoted_prefix || path.contains(' ');
    let prefix = if is_at_prefix { "@" } else { "" };
    if !needs_quotes {
        return format!("{prefix}{path}");
    }
    format!("{prefix}\"{path}\"")
}

#[derive(Clone, Debug, Default)]
pub struct AbortSignal(Rc<std::cell::Cell<bool>>);

impl AbortSignal {
    /// Whether the operation was cancelled.
    pub fn aborted(&self) -> bool {
        self.0.get()
    }
}

/// Owner side of an [`AbortSignal`] (`AbortController`).
#[derive(Clone, Debug, Default)]
pub struct AbortController(Rc<std::cell::Cell<bool>>);

impl AbortController {
    /// New controller with an unaborted signal.
    pub fn new() -> Self {
        Self::default()
    }

    /// Signal handed to the running operation.
    pub fn signal(&self) -> AbortSignal {
        AbortSignal(Rc::clone(&self.0))
    }

    /// Cancel every operation holding a signal of this controller.
    pub fn abort(&self) {
        self.0.set(true);
    }
}

/// One completion entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AutocompleteItem {
    /// Text inserted on acceptance.
    pub value: String,
    /// Text shown in the list.
    pub label: String,
    /// Secondary text shown next to the label.
    pub description: Option<String>,
}

impl AutocompleteItem {
    /// Item without a description.
    pub fn new(value: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            description: None,
        }
    }
}

/// Completions of a slash command argument (`getArgumentCompletions`).
pub type ArgumentCompletions =
    Rc<dyn Fn(&str) -> Pin<Box<dyn Future<Output = Option<Vec<AutocompleteItem>>>>>>;

/// A registered slash command.
#[derive(Clone)]
pub struct SlashCommand {
    /// Command name without the leading slash.
    pub name: String,
    /// Description shown in the list.
    pub description: Option<String>,
    /// Hint for the expected arguments.
    pub argument_hint: Option<String>,
    /// Argument completions, if the command offers any.
    pub get_argument_completions: Option<ArgumentCompletions>,
}

impl SlashCommand {
    /// Command with only a name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: None,
            argument_hint: None,
            get_argument_completions: None,
        }
    }
}

/// A command entry: either a full [`SlashCommand`] or a plain item.
#[derive(Clone)]
pub enum CommandEntry {
    /// Command with an optional argument completion.
    Command(SlashCommand),
    /// Plain item; `value` acts as the command name.
    Item(AutocompleteItem),
}

impl CommandEntry {
    fn name(&self) -> &str {
        match self {
            Self::Command(command) => &command.name,
            Self::Item(item) => &item.value,
        }
    }

    fn description(&self) -> Option<&str> {
        match self {
            Self::Command(command) => command.description.as_deref(),
            Self::Item(item) => item.description.as_deref(),
        }
    }

    fn argument_hint(&self) -> Option<&str> {
        match self {
            Self::Command(command) => command.argument_hint.as_deref(),
            Self::Item(_) => None,
        }
    }
}

impl From<SlashCommand> for CommandEntry {
    fn from(command: SlashCommand) -> Self {
        Self::Command(command)
    }
}

impl From<AutocompleteItem> for CommandEntry {
    fn from(item: AutocompleteItem) -> Self {
        Self::Item(item)
    }
}

/// The suggestions for the current cursor position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AutocompleteSuggestions {
    /// Matching entries.
    pub items: Vec<AutocompleteItem>,
    /// Text the entries were matched against.
    pub prefix: String,
}

/// Result of applying a completion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppliedCompletion {
    /// The updated lines.
    pub lines: Vec<String>,
    /// Line the cursor ends up on.
    pub cursor_line: usize,
    /// Column the cursor ends up at (byte index into the line).
    pub cursor_col: usize,
}

/// Options of a suggestion request.
#[derive(Clone, Debug, Default)]
pub struct SuggestionOptions {
    /// Cancels the request.
    pub signal: AbortSignal,
    /// Whether the user forced the request (Tab).
    pub force: bool,
}

/// Source of editor completions.
#[async_trait::async_trait(?Send)]
pub trait AutocompleteProvider {
    /// Characters that trigger this provider at a token boundary.
    fn trigger_characters(&self) -> Vec<String> {
        Vec::new()
    }

    /// Suggestions for the cursor position, or `None` when there are none.
    async fn get_suggestions(
        &self,
        lines: &[String],
        cursor_line: usize,
        cursor_col: usize,
        options: SuggestionOptions,
    ) -> Option<AutocompleteSuggestions>;

    /// Apply the selected entry.
    fn apply_completion(
        &self,
        lines: &[String],
        cursor_line: usize,
        cursor_col: usize,
        item: &AutocompleteItem,
        prefix: &str,
    ) -> AppliedCompletion;

    /// Whether an explicit Tab completion should search files.
    fn should_trigger_file_completion(
        &self,
        _lines: &[String],
        _cursor_line: usize,
        _cursor_col: usize,
    ) -> bool {
        true
    }
}

/// Provider for slash commands and file paths.
pub struct CombinedAutocompleteProvider {
    commands: Vec<CommandEntry>,
    base_path: String,
    fd_path: Option<String>,
}

/// A file system entry found by `fd`.
struct FdEntry {
    path: String,
    is_directory: bool,
}

impl CombinedAutocompleteProvider {
    /// New provider for `base_path`; `fd_path` enables the fuzzy `@` search.
    pub fn new(
        commands: Vec<CommandEntry>,
        base_path: impl Into<String>,
        fd_path: Option<String>,
    ) -> Self {
        Self {
            commands,
            base_path: base_path.into(),
            fd_path,
        }
    }

    /// Extract the `@` prefix for the fuzzy file search.
    fn extract_at_prefix<'a>(&self, text: &'a str) -> Option<&'a str> {
        if let Some(quoted_prefix) = extract_quoted_prefix(text)
            && quoted_prefix.starts_with("@\"")
        {
            return Some(quoted_prefix);
        }

        let token_start = find_last_delimiter(text).map_or(0, |index| {
            index + text[index..].chars().next().map_or(1, char::len_utf8)
        });

        if text[token_start..].starts_with('@') {
            return Some(&text[token_start..]);
        }

        None
    }

    /// Extract a path-like prefix from the text before the cursor.
    fn extract_path_prefix<'a>(&self, text: &'a str, force_extract: bool) -> Option<&'a str> {
        if let Some(quoted_prefix) = extract_quoted_prefix(text) {
            return Some(quoted_prefix);
        }

        let path_prefix = match find_last_delimiter(text) {
            None => text,
            Some(index) => &text[index + text[index..].chars().next().map_or(1, char::len_utf8)..],
        };

        if force_extract {
            return Some(path_prefix);
        }

        if path_prefix.contains('/')
            || path_prefix.starts_with('.')
            || path_prefix.starts_with("~/")
        {
            return Some(path_prefix);
        }

        if path_prefix.is_empty() && text.ends_with(' ') {
            return Some(path_prefix);
        }

        None
    }

    /// Expand a leading `~`.
    fn expand_home_path(&self, path: &str) -> String {
        if let Some(rest) = path.strip_prefix("~/") {
            let expanded = join(&[&homedir(), rest]);
            return if path.ends_with('/') && !expanded.ends_with('/') {
                format!("{expanded}/")
            } else {
                expanded
            };
        }
        if path == "~" {
            return homedir();
        }
        path.to_string()
    }

    fn resolve_scoped_fuzzy_query(&self, raw_query: &str) -> Option<(String, String, String)> {
        let normalized_query = to_display_path(raw_query);
        let slash_index = normalized_query.rfind('/')?;

        let display_base = normalized_query[..=slash_index].to_string();
        let query = normalized_query[slash_index + 1..].to_string();

        let base_dir = if display_base.starts_with("~/") {
            self.expand_home_path(&display_base)
        } else if display_base.starts_with('/') {
            display_base.clone()
        } else {
            join(&[&self.base_path, &display_base])
        };

        if !std::fs::metadata(&base_dir).is_ok_and(|metadata| metadata.is_dir()) {
            return None;
        }

        Some((base_dir, query, display_base))
    }

    fn scoped_path_for_display(display_base: &str, relative_path: &str) -> String {
        let normalized_relative_path = to_display_path(relative_path);
        if display_base == "/" {
            return format!("/{normalized_relative_path}");
        }
        format!(
            "{}{normalized_relative_path}",
            to_display_path(display_base)
        )
    }

    /// File and directory suggestions for a path prefix.
    fn get_file_suggestions(&self, prefix: &str) -> Vec<AutocompleteItem> {
        let ParsedPathPrefix {
            raw_prefix,
            is_at_prefix,
            is_quoted_prefix,
        } = parse_path_prefix(prefix);
        let mut expanded_prefix = raw_prefix.clone();
        if expanded_prefix.starts_with('~') {
            expanded_prefix = self.expand_home_path(&expanded_prefix);
        }

        let is_root_prefix = matches!(raw_prefix.as_str(), "" | "./" | "../" | "~" | "~/" | "/");

        let search_dir;
        let search_prefix;
        if is_root_prefix || raw_prefix.ends_with('/') {
            search_dir = if raw_prefix.starts_with('~') || expanded_prefix.starts_with('/') {
                expanded_prefix.clone()
            } else {
                join(&[&self.base_path, &expanded_prefix])
            };
            search_prefix = String::new();
        } else {
            let directory = dirname(&expanded_prefix);
            let file = basename(&expanded_prefix);
            search_dir = if raw_prefix.starts_with('~') || expanded_prefix.starts_with('/') {
                directory
            } else {
                join(&[&self.base_path, &directory])
            };
            search_prefix = file;
        }

        let Ok(entries) = std::fs::read_dir(&search_dir) else {
            return Vec::new();
        };

        let mut suggestions: Vec<AutocompleteItem> = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name
                .to_lowercase()
                .starts_with(&search_prefix.to_lowercase())
            {
                continue;
            }

            let file_type = entry.file_type();
            let mut is_directory = file_type.as_ref().is_ok_and(std::fs::FileType::is_dir);
            if !is_directory && file_type.is_ok_and(|file_type| file_type.is_symlink()) {
                let full_path = join(&[&search_dir, &name]);
                is_directory =
                    std::fs::metadata(&full_path).is_ok_and(|metadata| metadata.is_dir());
            }

            let display_prefix = raw_prefix.as_str();
            let mut relative_path = if display_prefix.ends_with('/') {
                format!("{display_prefix}{name}")
            } else if display_prefix.contains('/') || display_prefix.contains('\\') {
                if let Some(home_relative_dir) = display_prefix.strip_prefix("~/") {
                    let directory = dirname(home_relative_dir);
                    if directory == "." {
                        format!("~/{name}")
                    } else {
                        format!("~/{}", join(&[&directory, &name]))
                    }
                } else if display_prefix.starts_with('/') {
                    let directory = dirname(display_prefix);
                    if directory == "/" {
                        format!("/{name}")
                    } else {
                        format!("{directory}/{name}")
                    }
                } else {
                    let joined = join(&[&dirname(display_prefix), &name]);
                    if display_prefix.starts_with("./") && !joined.starts_with("./") {
                        format!("./{joined}")
                    } else {
                        joined
                    }
                }
            } else if display_prefix.starts_with('~') {
                format!("~/{name}")
            } else {
                name.clone()
            };

            relative_path = to_display_path(&relative_path);
            let path_value = if is_directory {
                format!("{relative_path}/")
            } else {
                relative_path
            };
            let value = build_completion_value(&path_value, is_at_prefix, is_quoted_prefix);

            suggestions.push(AutocompleteItem::new(
                value,
                format!("{name}{}", if is_directory { "/" } else { "" }),
            ));
        }

        // Directories first, then by label.
        suggestions.sort_by(|a, b| {
            let a_is_dir = a.value.ends_with('/');
            let b_is_dir = b.value.ends_with('/');
            b_is_dir.cmp(&a_is_dir).then_with(|| a.label.cmp(&b.label))
        });

        suggestions
    }

    /// Score an entry against the query; higher is better.
    fn score_entry(file_path: &str, query: &str, is_directory: bool) -> i64 {
        let file_name = basename(file_path);
        let lower_file_name = file_name.to_lowercase();
        let lower_query = query.to_lowercase();

        let mut score = if lower_file_name == lower_query {
            100
        } else if lower_file_name.starts_with(&lower_query) {
            80
        } else if lower_file_name.contains(&lower_query) {
            50
        } else if file_path.to_lowercase().contains(&lower_query) {
            30
        } else {
            0
        };

        if is_directory && score > 0 {
            score += 10;
        }

        score
    }

    /// Fuzzy file search through `fd`.
    async fn get_fuzzy_file_suggestions(
        &self,
        query: &str,
        is_quoted_prefix: bool,
        signal: &AbortSignal,
    ) -> Vec<AutocompleteItem> {
        let Some(fd_path) = &self.fd_path else {
            return Vec::new();
        };
        if signal.aborted() {
            return Vec::new();
        }

        let scoped_query = self.resolve_scoped_fuzzy_query(query);
        let fd_base_dir = scoped_query
            .as_ref()
            .map_or_else(|| self.base_path.clone(), |scoped| scoped.0.clone());
        let fd_query = scoped_query
            .as_ref()
            .map_or_else(|| query.to_string(), |scoped| scoped.1.clone());
        let entries =
            walk_directory_with_fd(&fd_base_dir, fd_path, &fd_query, 100, signal.clone()).await;
        if signal.aborted() {
            return Vec::new();
        }

        let mut scored_entries: Vec<(FdEntry, i64)> = entries
            .into_iter()
            .map(|entry| {
                let score = if fd_query.is_empty() {
                    1
                } else {
                    Self::score_entry(&entry.path, &fd_query, entry.is_directory)
                };
                (entry, score)
            })
            .filter(|(_, score)| *score > 0)
            .collect();

        // JS `Array#sort` is stable, so equal scores keep the `fd` order.
        scored_entries.sort_by_key(|(_, score)| std::cmp::Reverse(*score));
        scored_entries.truncate(20);

        scored_entries
            .into_iter()
            .map(|(entry, _)| {
                let path_without_slash = if entry.is_directory {
                    entry.path[..entry.path.len() - 1].to_string()
                } else {
                    entry.path.clone()
                };
                let display_path = match &scoped_query {
                    Some((_, _, display_base)) => {
                        Self::scoped_path_for_display(display_base, &path_without_slash)
                    }
                    None => path_without_slash.clone(),
                };
                let entry_name = basename(&path_without_slash);
                let completion_path = if entry.is_directory {
                    format!("{display_path}/")
                } else {
                    display_path.clone()
                };
                AutocompleteItem {
                    value: build_completion_value(&completion_path, true, is_quoted_prefix),
                    label: format!("{entry_name}{}", if entry.is_directory { "/" } else { "" }),
                    description: Some(display_path),
                }
            })
            .collect()
    }
}

#[async_trait::async_trait(?Send)]
impl AutocompleteProvider for CombinedAutocompleteProvider {
    async fn get_suggestions(
        &self,
        lines: &[String],
        cursor_line: usize,
        cursor_col: usize,
        options: SuggestionOptions,
    ) -> Option<AutocompleteSuggestions> {
        let current_line = lines.get(cursor_line).cloned().unwrap_or_default();
        let text_before_cursor = &current_line[..cursor_col.min(current_line.len())];

        if let Some(at_prefix) = self.extract_at_prefix(text_before_cursor) {
            let at_prefix = at_prefix.to_string();
            let ParsedPathPrefix {
                raw_prefix,
                is_quoted_prefix,
                ..
            } = parse_path_prefix(&at_prefix);
            let suggestions = self
                .get_fuzzy_file_suggestions(&raw_prefix, is_quoted_prefix, &options.signal)
                .await;
            if suggestions.is_empty() {
                return None;
            }
            return Some(AutocompleteSuggestions {
                items: suggestions,
                prefix: at_prefix,
            });
        }

        if !options.force && text_before_cursor.starts_with('/') {
            let space_index = text_before_cursor.find(' ');

            let Some(space_index) = space_index else {
                let prefix = &text_before_cursor[1..];
                let command_items: Vec<(String, String, Option<String>)> = self
                    .commands
                    .iter()
                    .map(|command| {
                        let name = command.name().to_string();
                        let hint = command.argument_hint();
                        let description = command.description().unwrap_or("");
                        let full_description = match hint {
                            Some(hint) if !description.is_empty() => {
                                format!("{hint} — {description}")
                            }
                            Some(hint) => hint.to_string(),
                            None => description.to_string(),
                        };
                        (
                            name.clone(),
                            name,
                            (!full_description.is_empty()).then_some(full_description),
                        )
                    })
                    .collect();

                let filtered: Vec<AutocompleteItem> =
                    fuzzy_filter(&command_items, prefix, |item| item.0.clone())
                        .into_iter()
                        .map(|(name, label, description)| AutocompleteItem {
                            value: name,
                            label,
                            description,
                        })
                        .collect();

                if filtered.is_empty() {
                    return None;
                }

                return Some(AutocompleteSuggestions {
                    items: filtered,
                    prefix: text_before_cursor.to_string(),
                });
            };

            let command_name = &text_before_cursor[1..space_index];
            let argument_text = text_before_cursor[space_index + 1..].to_string();

            let command = self
                .commands
                .iter()
                .find(|command| command.name() == command_name)?;
            let CommandEntry::Command(command) = command else {
                return None;
            };
            let get_argument_completions = command.get_argument_completions.clone()?;

            let argument_suggestions = get_argument_completions(&argument_text).await?;
            if argument_suggestions.is_empty() {
                return None;
            }

            return Some(AutocompleteSuggestions {
                items: argument_suggestions,
                prefix: argument_text,
            });
        }

        let path_match = self.extract_path_prefix(text_before_cursor, options.force)?;
        let path_match = path_match.to_string();

        let suggestions = self.get_file_suggestions(&path_match);
        if suggestions.is_empty() {
            return None;
        }

        Some(AutocompleteSuggestions {
            items: suggestions,
            prefix: path_match,
        })
    }

    fn apply_completion(
        &self,
        lines: &[String],
        cursor_line: usize,
        cursor_col: usize,
        item: &AutocompleteItem,
        prefix: &str,
    ) -> AppliedCompletion {
        let current_line = lines.get(cursor_line).cloned().unwrap_or_default();
        let before_prefix = &current_line[..cursor_col.saturating_sub(prefix.len())];
        let after_cursor = &current_line[cursor_col.min(current_line.len())..];
        let is_quoted_prefix = prefix.starts_with('"') || prefix.starts_with("@\"");
        let has_leading_quote_after_cursor = after_cursor.starts_with('"');
        let has_trailing_quote_in_item = item.value.ends_with('"');
        let adjusted_after_cursor =
            if is_quoted_prefix && has_trailing_quote_in_item && has_leading_quote_after_cursor {
                &after_cursor[1..]
            } else {
                after_cursor
            };

        let mut new_lines = lines.to_vec();
        if new_lines.len() <= cursor_line {
            new_lines.resize(cursor_line + 1, String::new());
        }

        let is_slash_command = prefix.starts_with('/')
            && before_prefix.trim().is_empty()
            && !prefix[1..].contains('/');
        if is_slash_command {
            new_lines[cursor_line] =
                format!("{before_prefix}/{} {adjusted_after_cursor}", item.value);
            return AppliedCompletion {
                lines: new_lines,
                cursor_line,
                // +2 for the slash and the trailing space.
                cursor_col: before_prefix.len() + item.value.len() + 2,
            };
        }

        let is_directory = item.label.ends_with('/');
        let has_trailing_quote = item.value.ends_with('"');
        let cursor_offset = if is_directory && has_trailing_quote {
            item.value.len() - 1
        } else {
            item.value.len()
        };

        if prefix.starts_with('@') {
            // Directories get no trailing space so completion can continue.
            let suffix = if is_directory { "" } else { " " };
            new_lines[cursor_line] = format!(
                "{before_prefix}{}{suffix}{adjusted_after_cursor}",
                item.value
            );
            return AppliedCompletion {
                lines: new_lines,
                cursor_line,
                cursor_col: before_prefix.len() + cursor_offset + suffix.len(),
            };
        }

        new_lines[cursor_line] = format!("{before_prefix}{}{adjusted_after_cursor}", item.value);
        AppliedCompletion {
            lines: new_lines,
            cursor_line,
            cursor_col: before_prefix.len() + cursor_offset,
        }
    }

    fn should_trigger_file_completion(
        &self,
        lines: &[String],
        cursor_line: usize,
        cursor_col: usize,
    ) -> bool {
        let current_line = lines.get(cursor_line).cloned().unwrap_or_default();
        let text_before_cursor = &current_line[..cursor_col.min(current_line.len())];

        // Never while a slash command name is being typed.
        let trimmed = text_before_cursor.trim();
        !trimmed.starts_with('/') || trimmed.contains(' ')
    }
}

/// Walk the directory tree with `fd` (fast, honours `.gitignore`).
async fn walk_directory_with_fd(
    base_dir: &str,
    fd_path: &str,
    query: &str,
    max_results: usize,
    signal: AbortSignal,
) -> Vec<FdEntry> {
    let mut args: Vec<String> = [
        "--base-directory",
        base_dir,
        "--max-results",
        &max_results.to_string(),
        "--type",
        "f",
        "--type",
        "d",
        "--follow",
        "--hidden",
        "--exclude",
        ".git",
        "--exclude",
        ".git/*",
        "--exclude",
        ".git/**",
    ]
    .iter()
    .map(|argument| (*argument).to_string())
    .collect();

    if to_display_path(query).contains('/') {
        args.push("--full-path".to_string());
    }

    if !query.is_empty() {
        args.push(build_fd_path_query(query));
    }

    if signal.aborted() {
        return Vec::new();
    }

    let Ok(mut child) = tokio::process::Command::new(fd_path)
        .args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
    else {
        return Vec::new();
    };

    let Some(stdout_pipe) = child.stdout.take() else {
        return Vec::new();
    };
    let read = async {
        use tokio::io::AsyncReadExt;
        let mut stdout_pipe = stdout_pipe;
        let mut stdout = String::new();
        let _ = stdout_pipe.read_to_string(&mut stdout).await;
        (stdout, child.wait().await)
    };
    tokio::pin!(read);

    // the flag, since the signal has no callback registry.
    let (stdout, status) = loop {
        tokio::select! {
            result = &mut read => break result,
            () = tokio::time::sleep(std::time::Duration::from_millis(5)) => {
                if signal.aborted() {
                    return Vec::new();
                }
            }
        }
    };

    if signal.aborted() || !status.is_ok_and(|status| status.success()) || stdout.is_empty() {
        return Vec::new();
    }

    let mut results: Vec<FdEntry> = Vec::new();
    for line in stdout.trim().split('\n').filter(|line| !line.is_empty()) {
        let display_line = to_display_path(line);
        let has_trailing_separator = display_line.ends_with('/');
        let normalized_path = if has_trailing_separator {
            &display_line[..display_line.len() - 1]
        } else {
            display_line.as_str()
        };
        if normalized_path == ".git"
            || normalized_path.starts_with(".git/")
            || normalized_path.contains("/.git/")
        {
            continue;
        }

        results.push(FdEntry {
            path: display_line.clone(),
            is_directory: has_trailing_separator,
        });
    }

    results
}
