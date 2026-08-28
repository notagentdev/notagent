use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::{Captures, Regex};

use crate::config::CONFIG_DIR_NAME;
use crate::core::source_info::{
    SourceInfo, SourceScope, SyntheticSourceInfoOptions, create_synthetic_source_info,
};
use crate::utils::frontmatter::parse_frontmatter;
use crate::utils::paths::{PathInputOptions, current_dir, resolve_path};

/// A prompt template loaded from a markdown file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptTemplate {
    pub name: String,
    pub description: String,
    pub argument_hint: Option<String>,
    pub content: String,
    pub source_info: SourceInfo,
    /// Absolute path to the template file.
    pub file_path: String,
}

/// Splits a command's argument string the way a shell would, honouring quotes.
/// Quotes are removed and never re-inserted, and an unterminated quote simply
/// ends at the end of the string rather than being an error: the input comes
/// from a person typing into an editor line, not from a script.
pub fn parse_command_args(args_string: &str) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_quote: Option<char> = None;

    for character in args_string.chars() {
        match in_quote {
            Some(quote) => {
                if character == quote {
                    in_quote = None;
                } else {
                    current.push(character);
                }
            }
            None => {
                if character == '"' || character == '\'' {
                    in_quote = Some(character);
                } else if character.is_whitespace() {
                    if !current.is_empty() {
                        args.push(std::mem::take(&mut current));
                    }
                } else {
                    current.push(character);
                }
            }
        }
    }

    if !current.is_empty() {
        args.push(current);
    }

    args
}

/// The one expression that recognizes every supported form, in the order the
/// bare references. Group order matters — a `${@:2}` must not be read as a
/// bare `$@` followed by literal text.
static SUBSTITUTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\$\{(\d+|ARGUMENTS|@):-([^}]*)\}|\$\{@:(\d+)(?::(\d+))?\}|\$(ARGUMENTS|@|\d+)")
        .expect("substitution pattern")
});

/// Substitutes argument placeholders in template content.
/// Supported:
/// - `$1`, `$2`, … positional arguments
/// - `$@` and `$ARGUMENTS` for all arguments
/// - `${N:-default}` for a positional argument with a fallback when missing or empty
/// - `${@:-default}` / `${ARGUMENTS:-default}` for all arguments with a fallback
/// - `${@:N}` for arguments from the Nth onwards, `${@:N:L}` for L of them
pub fn substitute_args(content: &str, args: &[String]) -> String {
    let all_args = args.join(" ");

    SUBSTITUTION
        .replace_all(content, |captures: &Captures<'_>| {
            if let Some(target) = captures.get(1) {
                let default_value = captures.get(2).map(|m| m.as_str()).unwrap_or("");
                let value = match target.as_str() {
                    "@" | "ARGUMENTS" => Some(all_args.clone()),
                    index => index
                        .parse::<usize>()
                        .ok()
                        .and_then(|index| index.checked_sub(1))
                        .and_then(|index| args.get(index).cloned()),
                };
                return match value {
                    Some(value) if !value.is_empty() => value,
                    _ => default_value.to_string(),
                };
            }

            if let Some(slice_start) = captures.get(3) {
                // Users count from 1; bash treats 0 as 1 as well.
                let start = slice_start
                    .as_str()
                    .parse::<usize>()
                    .unwrap_or(1)
                    .saturating_sub(1);

                if let Some(slice_length) = captures.get(4) {
                    let length = slice_length.as_str().parse::<usize>().unwrap_or(0);
                    let end = start.saturating_add(length).min(args.len());
                    let start = start.min(args.len());
                    return args[start..end].join(" ");
                }
                let start = start.min(args.len());
                return args[start..].join(" ");
            }

            let simple = captures.get(5).map(|m| m.as_str()).unwrap_or("");
            if simple == "ARGUMENTS" || simple == "@" {
                return all_args.clone();
            }
            simple
                .parse::<usize>()
                .ok()
                .and_then(|index| index.checked_sub(1))
                .and_then(|index| args.get(index).cloned())
                .unwrap_or_default()
        })
        .into_owned()
}

fn load_template_from_file(file_path: &Path, source_info: SourceInfo) -> Option<PromptTemplate> {
    let raw_content = std::fs::read_to_string(file_path).ok()?;
    let parsed = parse_frontmatter(&raw_content).ok()?;

    let name = file_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name = name.strip_suffix(".md").unwrap_or(&name).to_string();

    // Description from frontmatter, else the first non-empty body line,
    // truncated so a paragraph does not become a menu entry.
    let mut description = parsed
        .get_str("description")
        .map(|text| text.to_string())
        .unwrap_or_default();
    if description.is_empty()
        && let Some(first_line) = parsed.body.split('\n').find(|line| !line.trim().is_empty())
    {
        let units: Vec<u16> = first_line.encode_utf16().collect();
        if units.len() > 60 {
            description = format!("{}...", String::from_utf16_lossy(&units[..60]));
        } else {
            description = first_line.to_string();
        }
    }

    Some(PromptTemplate {
        name,
        description,
        argument_hint: parsed
            .get_str("argument-hint")
            .filter(|hint| !hint.is_empty())
            .map(|hint| hint.to_string()),
        content: parsed.body.clone(),
        source_info,
        file_path: file_path.to_string_lossy().into_owned(),
    })
}

/// Scans a directory for `.md` files (non-recursive) and loads them.
fn load_templates_from_dir(
    dir: &Path,
    get_source_info: &dyn Fn(&Path) -> SourceInfo,
) -> Vec<PromptTemplate> {
    let mut templates: Vec<PromptTemplate> = Vec::new();

    if !dir.exists() {
        return templates;
    }

    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return templates;
    };

    let mut entries: Vec<(String, PathBuf, std::fs::FileType)> = Vec::new();
    for entry in read_dir.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        entries.push((
            entry.file_name().to_string_lossy().into_owned(),
            entry.path(),
            file_type,
        ));
    }
    entries.sort_by(|left, right| left.0.cmp(&right.0));

    for (name, full_path, file_type) in entries {
        let mut is_file = file_type.is_file();
        if file_type.is_symlink() {
            match std::fs::metadata(&full_path) {
                Ok(stats) => is_file = stats.is_file(),
                // Broken symlink, skip it.
                Err(_) => continue,
            }
        }

        if is_file
            && name.ends_with(".md")
            && let Some(template) = load_template_from_file(&full_path, get_source_info(&full_path))
        {
            templates.push(template);
        }
    }

    templates
}

/// Inputs of [`load_prompt_templates`].
#[derive(Debug, Clone, Default)]
pub struct LoadPromptTemplatesOptions {
    /// Working directory for project-local templates.
    pub cwd: String,
    /// Agent config directory for global templates.
    pub agent_dir: String,
    /// Explicit template paths (files or directories).
    pub prompt_paths: Vec<String>,
    /// Whether the two default prompt directories are included.
    pub include_defaults: bool,
}

fn is_under_path(target: &str, root: &str) -> bool {
    let normalized_root = resolve_path(root, &current_dir(), &PathInputOptions::default())
        .unwrap_or_else(|_| root.to_string());
    if target == normalized_root {
        return true;
    }
    let separator = std::path::MAIN_SEPARATOR;
    let prefix = if normalized_root.ends_with(separator) {
        normalized_root
    } else {
        format!("{normalized_root}{separator}")
    };
    target.starts_with(&prefix)
}

/// Loads templates from the global directory, the project directory and any
/// explicit paths, in that order.
pub fn load_prompt_templates(options: &LoadPromptTemplatesOptions) -> Vec<PromptTemplate> {
    let resolved_cwd = resolve_path(&options.cwd, &current_dir(), &PathInputOptions::default())
        .unwrap_or_else(|_| options.cwd.clone());
    let resolved_agent_dir = resolve_path(
        &options.agent_dir,
        &current_dir(),
        &PathInputOptions::default(),
    )
    .unwrap_or_else(|_| options.agent_dir.clone());

    let mut templates: Vec<PromptTemplate> = Vec::new();

    let global_prompts_dir = Path::new(&resolved_agent_dir)
        .join("prompts")
        .to_string_lossy()
        .into_owned();
    let project_prompts_dir = Path::new(&resolved_cwd)
        .join(CONFIG_DIR_NAME)
        .join("prompts")
        .to_string_lossy()
        .into_owned();

    let get_source_info = |resolved_path: &Path| -> SourceInfo {
        let path_text = resolved_path.to_string_lossy().into_owned();
        if is_under_path(&path_text, &global_prompts_dir) {
            return create_synthetic_source_info(
                &path_text,
                SyntheticSourceInfoOptions {
                    source: "local".to_string(),
                    scope: Some(SourceScope::User),
                    origin: None,
                    base_dir: Some(global_prompts_dir.clone()),
                },
            );
        }
        if is_under_path(&path_text, &project_prompts_dir) {
            return create_synthetic_source_info(
                &path_text,
                SyntheticSourceInfoOptions {
                    source: "local".to_string(),
                    scope: Some(SourceScope::Project),
                    origin: None,
                    base_dir: Some(project_prompts_dir.clone()),
                },
            );
        }
        let base_dir = match std::fs::metadata(resolved_path) {
            Ok(stats) if stats.is_dir() => path_text.clone(),
            _ => resolved_path
                .parent()
                .map(|parent| parent.to_string_lossy().into_owned())
                .unwrap_or_default(),
        };
        create_synthetic_source_info(
            &path_text,
            SyntheticSourceInfoOptions {
                source: "local".to_string(),
                scope: None,
                origin: None,
                base_dir: Some(base_dir),
            },
        )
    };

    if options.include_defaults {
        templates.extend(load_templates_from_dir(
            Path::new(&global_prompts_dir),
            &get_source_info,
        ));
        templates.extend(load_templates_from_dir(
            Path::new(&project_prompts_dir),
            &get_source_info,
        ));
    }

    for raw_path in &options.prompt_paths {
        let Ok(resolved_path) = resolve_path(
            raw_path,
            &resolved_cwd,
            &PathInputOptions {
                trim: true,
                ..PathInputOptions::default()
            },
        ) else {
            continue;
        };
        let path = Path::new(&resolved_path);
        if !path.exists() {
            continue;
        }

        let Ok(stats) = std::fs::metadata(path) else {
            // Ignore read failures.
            continue;
        };
        if stats.is_dir() {
            templates.extend(load_templates_from_dir(path, &get_source_info));
        } else if stats.is_file()
            && resolved_path.ends_with(".md")
            && let Some(template) = load_template_from_file(path, get_source_info(path))
        {
            templates.push(template);
        }
    }

    templates
}

static TEMPLATE_INVOCATION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)^/([^\s]+)(?:\s+(.*))?$").expect("invocation pattern"));

/// Expands a prompt template when the text names one, and returns the text
/// unchanged otherwise.
pub fn expand_prompt_template(text: &str, templates: &[PromptTemplate]) -> String {
    if !text.starts_with('/') {
        return text.to_string();
    }

    let Some(captures) = TEMPLATE_INVOCATION.captures(text) else {
        return text.to_string();
    };

    let template_name = captures.get(1).map(|m| m.as_str()).unwrap_or("");
    let args_string = captures.get(2).map(|m| m.as_str()).unwrap_or("");

    match templates
        .iter()
        .find(|template| template.name == template_name)
    {
        Some(template) => {
            let args = parse_command_args(args_string);
            substitute_args(&template.content, &args)
        }
        None => text.to_string(),
    }
}
