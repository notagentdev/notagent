//! Port of `packages/coding-agent/src/core/export-html/index.ts` (316 LOC).
//!
//! `template.html`, `template.css`, `template.js` and the two vendored browser
//! libraries are adopted verbatim; they run client-side in the exported file and
//! are not code this port owns. Deviation (class 4): they are compiled into the
//! binary with `include_str!` instead of being read from `getExportTemplateDir()`
//! at runtime — a Rust binary ships one file, and the TypeScript path juggling
//! between `src/`, `dist/` and the bun bundle has no counterpart.

pub mod ansi_to_html;
pub mod tool_renderer;

use std::collections::HashSet;
use std::path::Path;
use std::sync::LazyLock;

use notagent_agent::types::AgentState;
use regex::Regex;
use serde::Serialize;
use serde_json::{Map, Value};

use crate::config::APP_NAME;
use crate::core::session_manager::{SessionEntry, SessionHeader, SessionManager};
use crate::modes::interactive::theme::theme::{get_resolved_theme_colors, get_theme_export_colors};
use crate::utils::paths::{current_dir, normalize_path_default, resolve_path_default};

pub use tool_renderer::{RenderedToolResult, ToolHtmlRenderer};

/// The verbatim assets of `src/core/export-html/`. Public so the suites that
/// pin the client-side sanitization can assert on them, exactly as the
/// TypeScript tests read the files from disk.
pub const TEMPLATE_HTML: &str = include_str!("export_html/assets/template.html");
pub const TEMPLATE_CSS: &str = include_str!("export_html/assets/template.css");
pub const TEMPLATE_JS: &str = include_str!("export_html/assets/template.js");
pub const MARKED_JS: &str = include_str!("export_html/assets/vendor/marked.min.js");
pub const HIGHLIGHT_JS: &str = include_str!("export_html/assets/vendor/highlight.min.js");

/// The export failures the TypeScript raises as `Error`.
#[derive(Debug, thiserror::Error)]
pub enum ExportHtmlError {
    #[error("Cannot export in-memory session to HTML")]
    InMemorySession,
    #[error("Nothing to export yet - start a conversation first")]
    EmptySession,
    #[error("File not found: {0}")]
    FileNotFound(String),
    #[error("{0}")]
    Session(String),
    #[error("{0}")]
    Theme(String),
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

/// The pre-rendered tools of a session, in the order they were rendered.
/// `serde_json` keeps the insertion order of a map, so this serializes exactly
/// like the TypeScript `Record`.
pub type RenderedTools = Vec<(String, RenderedToolHtml)>;

/// Pre-rendered HTML for a custom tool call and result.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderedToolHtml {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub call_html: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_html_collapsed: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_html_expanded: Option<String>,
}

/// `ExportOptions`
#[derive(Default)]
pub struct ExportOptions<'a> {
    pub output_path: Option<String>,
    pub theme_name: Option<String>,
    /// Optional tool renderer for custom tools.
    pub tool_renderer: Option<&'a dyn ToolHtmlRenderer>,
}

/// Parse a color string to RGB values. Supports hex (#RRGGBB) and rgb(r,g,b) formats.
fn parse_color(color: &str) -> Option<(i64, i64, i64)> {
    static HEX: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^#([0-9a-fA-F]{2})([0-9a-fA-F]{2})([0-9a-fA-F]{2})$").expect("hex color regex")
    });
    static RGB: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^rgb\s*\(\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)\s*\)$").expect("rgb color regex")
    });

    if let Some(captures) = HEX.captures(color) {
        let component = |index: usize| {
            i64::from_str_radix(captures.get(index).expect("group").as_str(), 16).unwrap_or(0)
        };
        return Some((component(1), component(2), component(3)));
    }
    if let Some(captures) = RGB.captures(color) {
        let component = |index: usize| {
            captures
                .get(index)
                .expect("group")
                .as_str()
                .parse::<i64>()
                .unwrap_or(0)
        };
        return Some((component(1), component(2), component(3)));
    }
    None
}

/// Calculate relative luminance of a color (0-1, higher = lighter).
fn get_luminance(red: i64, green: i64, blue: i64) -> f64 {
    let to_linear = |channel: i64| {
        let scaled = channel as f64 / 255.0;
        if scaled <= 0.03928 {
            scaled / 12.92
        } else {
            ((scaled + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * to_linear(red) + 0.7152 * to_linear(green) + 0.0722 * to_linear(blue)
}

/// Adjust color brightness. Factor > 1 lightens, < 1 darkens.
fn adjust_brightness(color: &str, factor: f64) -> String {
    let Some((red, green, blue)) = parse_color(color) else {
        return color.to_owned();
    };
    let adjust = |channel: i64| js_round(channel as f64 * factor).clamp(0.0, 255.0) as i64;
    format!("rgb({}, {}, {})", adjust(red), adjust(green), adjust(blue))
}

/// `Math.round` — halves go towards positive infinity, unlike Rust's `round`.
fn js_round(value: f64) -> f64 {
    (value + 0.5).floor()
}

/// Derive export background colors from a base color (e.g. `userMessageBg`).
fn derive_export_colors(base_color: &str) -> (String, String, String) {
    let Some((red, green, blue)) = parse_color(base_color) else {
        return (
            "rgb(24, 24, 30)".to_owned(),
            "rgb(30, 30, 36)".to_owned(),
            "rgb(60, 55, 40)".to_owned(),
        );
    };

    let luminance = get_luminance(red, green, blue);
    let is_light = luminance > 0.5;

    if is_light {
        return (
            adjust_brightness(base_color, 0.96),
            base_color.to_owned(),
            format!(
                "rgb({}, {}, {})",
                (red + 10).min(255),
                (green + 5).min(255),
                (blue - 20).max(0)
            ),
        );
    }
    (
        adjust_brightness(base_color, 0.7),
        adjust_brightness(base_color, 0.85),
        format!(
            "rgb({}, {}, {})",
            (red + 20).min(255),
            (green + 15).min(255),
            blue
        ),
    )
}

/// The three colours the CSS placeholders take.
struct ExportColors {
    page_bg: String,
    card_bg: String,
    info_bg: String,
}

fn resolve_export_colors(colors: &[(String, String)], theme_name: Option<&str>) -> ExportColors {
    let user_message_bg = colors
        .iter()
        .find(|(key, _)| key == "userMessageBg")
        .map(|(_, value)| value.as_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("#343541");
    let (page_bg, card_bg, info_bg) = derive_export_colors(user_message_bg);
    let theme_export = get_theme_export_colors(theme_name);
    ExportColors {
        page_bg: theme_export.page_bg.unwrap_or(page_bg),
        card_bg: theme_export.card_bg.unwrap_or(card_bg),
        info_bg: theme_export.info_bg.unwrap_or(info_bg),
    }
}

/// Generate CSS custom property declarations from theme colors.
fn generate_theme_vars(colors: &[(String, String)], export_colors: &ExportColors) -> String {
    let mut lines: Vec<String> = colors
        .iter()
        .map(|(key, value)| format!("--{key}: {value};"))
        .collect();

    // Use explicit theme export colors if available, otherwise derive from userMessageBg
    lines.push(format!("--exportPageBg: {};", export_colors.page_bg));
    lines.push(format!("--exportCardBg: {};", export_colors.card_bg));
    lines.push(format!("--exportInfoBg: {};", export_colors.info_bg));

    lines.join("\n      ")
}

/// `String.prototype.replace(searchString, replaceString)`.
///
/// Replaces the first occurrence, and expands the `$` patterns of
/// `GetSubstitution` in the replacement — `$$` for a literal `$`, `` $` `` and
/// `$'` for the text around the match, `$&` for the match itself. The vendored
/// `highlight.min.js` contains a `$&` and a `$$` inside regex literals and the
/// template a `$$` in a price label, so the exported HTML carries those
/// expansions; they are reproduced here rather than fixed (bug compatibility).
fn js_replace_first(haystack: &str, needle: &str, replacement: &str) -> String {
    let Some(position) = haystack.find(needle) else {
        return haystack.to_owned();
    };
    let prefix = &haystack[..position];
    let suffix = &haystack[position + needle.len()..];

    let mut expanded = String::with_capacity(replacement.len());
    let mut characters = replacement.char_indices();
    while let Some((_, character)) = characters.next() {
        if character != '$' {
            expanded.push(character);
            continue;
        }
        match characters.clone().next() {
            Some((_, '$')) => {
                expanded.push('$');
                characters.next();
            }
            Some((_, '&')) => {
                expanded.push_str(needle);
                characters.next();
            }
            Some((_, '`')) => {
                expanded.push_str(prefix);
                characters.next();
            }
            Some((_, '\'')) => {
                expanded.push_str(suffix);
                characters.next();
            }
            // `$1`…`$9` and `$<name>` have no captures to refer to for a string
            // pattern and stay literal, as does a trailing `$`.
            _ => expanded.push('$'),
        }
    }

    format!("{prefix}{expanded}{suffix}")
}

/// `SessionData` — the payload the template decodes on load.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionData<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    header: Option<&'a SessionHeader>,
    entries: &'a [SessionEntry],
    leaf_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_prompt: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<ExportedTool>>,
    /// Pre-rendered HTML for custom tool calls/results, keyed by tool call ID.
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_rendered_tools"
    )]
    rendered_tools: Option<&'a RenderedTools>,
}

#[derive(Debug, Clone, Serialize)]
struct ExportedTool {
    name: String,
    description: String,
    parameters: Value,
}

/// Core HTML generation logic shared by both export functions.
fn generate_html(
    session_data: &SessionData<'_>,
    theme_name: Option<&str>,
) -> Result<String, ExportHtmlError> {
    let colors = get_resolved_theme_colors(theme_name)
        .map_err(|error| ExportHtmlError::Theme(error.to_string()))?;
    let export_colors = resolve_export_colors(&colors, theme_name);
    let theme_vars = generate_theme_vars(&colors, &export_colors);

    // Base64 encode session data to avoid escaping issues
    let session_data_base64 = {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(
            serde_json::to_string(session_data)
                .map_err(|error| ExportHtmlError::Session(error.to_string()))?,
        )
    };

    // Build the CSS with theme variables injected
    let css = js_replace_first(TEMPLATE_CSS, "{{THEME_VARS}}", &theme_vars);
    let css = js_replace_first(&css, "{{BODY_BG}}", &export_colors.page_bg);
    let css = js_replace_first(&css, "{{CONTAINER_BG}}", &export_colors.card_bg);
    let css = js_replace_first(&css, "{{INFO_BG}}", &export_colors.info_bg);

    let html = js_replace_first(TEMPLATE_HTML, "{{CSS}}", &css);
    let html = js_replace_first(&html, "{{JS}}", TEMPLATE_JS);
    let html = js_replace_first(&html, "{{SESSION_DATA}}", &session_data_base64);
    let html = js_replace_first(&html, "{{MARKED_JS}}", MARKED_JS);
    Ok(js_replace_first(&html, "{{HIGHLIGHT_JS}}", HIGHLIGHT_JS))
}

/// Tools rendered directly by the HTML template (not pre-rendered via TUI→ANSI→HTML pipeline).
static TEMPLATE_RENDERED_TOOLS: LazyLock<HashSet<&'static str>> =
    LazyLock::new(|| HashSet::from(["bash", "read", "write", "edit", "ls"]));

/// Pre-render custom tools to HTML using their TUI renderers.
fn pre_render_custom_tools(
    entries: &[SessionEntry],
    tool_renderer: &dyn ToolHtmlRenderer,
) -> RenderedTools {
    let mut rendered_tools: RenderedTools = Vec::new();

    for entry in entries {
        let SessionEntry::Message(entry) = entry else {
            continue;
        };
        let message = &entry.message;
        let role = message.get("role").and_then(Value::as_str);

        // Find tool calls in assistant messages
        if role == Some("assistant")
            && let Some(content) = message.get("content").and_then(Value::as_array)
        {
            for block in content {
                let is_tool_call = block.get("type").and_then(Value::as_str) == Some("toolCall");
                let name = block
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if is_tool_call && !TEMPLATE_RENDERED_TOOLS.contains(name) {
                    let id = block.get("id").and_then(Value::as_str).unwrap_or_default();
                    let arguments = block.get("arguments").cloned().unwrap_or(Value::Null);
                    if let Some(call_html) = tool_renderer.render_call(id, name, &arguments) {
                        insert_rendered(
                            &mut rendered_tools,
                            id,
                            RenderedToolHtml {
                                call_html: Some(call_html),
                                ..RenderedToolHtml::default()
                            },
                        );
                    }
                }
            }
        }

        // Find tool results
        if role == Some("toolResult")
            && let Some(tool_call_id) = message
                .get("toolCallId")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
        {
            let tool_name = message
                .get("toolName")
                .and_then(Value::as_str)
                .unwrap_or_default();
            // Only render if we have a pre-rendered call OR it's not template-rendered
            let existing = rendered_tools
                .iter()
                .find(|(key, _)| key == tool_call_id)
                .map(|(_, value)| value.clone());
            if existing.is_some() || !TEMPLATE_RENDERED_TOOLS.contains(tool_name) {
                let content = message
                    .get("content")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let details = message.get("details").cloned().unwrap_or(Value::Null);
                let is_error = message
                    .get("isError")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if let Some(rendered) = tool_renderer.render_result(
                    tool_call_id,
                    tool_name,
                    &content,
                    &details,
                    is_error,
                ) {
                    let mut merged = existing.unwrap_or_default();
                    merged.result_html_collapsed = rendered.collapsed;
                    merged.result_html_expanded = rendered.expanded;
                    insert_rendered(&mut rendered_tools, tool_call_id, merged);
                }
            }
        }
    }

    rendered_tools
}

/// Export session to HTML using `SessionManager` and `AgentState`.
/// Used by the TUI's `/export` command.
pub fn export_session_to_html(
    session_manager: &SessionManager,
    state: Option<&AgentState>,
    options: ExportOptions<'_>,
) -> Result<String, ExportHtmlError> {
    let Some(session_file) = session_manager.get_session_file() else {
        return Err(ExportHtmlError::InMemorySession);
    };
    if !Path::new(session_file).exists() {
        return Err(ExportHtmlError::EmptySession);
    }

    let entries = session_manager.get_entries();

    // Pre-render custom tools if a tool renderer is provided
    let rendered_tools = options
        .tool_renderer
        .map(|renderer| pre_render_custom_tools(&entries, renderer))
        // Only include if we actually rendered something
        .filter(|rendered| !rendered.is_empty());
    let rendered_tools = rendered_tools.as_ref();

    let session_data = SessionData {
        header: session_manager.get_header(),
        entries: &entries,
        leaf_id: session_manager.get_leaf_id(),
        system_prompt: state.map(|state| state.system_prompt.as_str()),
        tools: state.map(|state| {
            state
                .tools
                .iter()
                .map(|tool| ExportedTool {
                    name: tool.name().to_owned(),
                    description: tool.description().to_owned(),
                    parameters: tool.parameters().clone(),
                })
                .collect()
        }),
        rendered_tools,
    };

    let html = generate_html(&session_data, options.theme_name.as_deref())?;

    let output_path = match options.output_path {
        Some(path) => normalize_path_default(&path)
            .map_err(|error| ExportHtmlError::Session(error.to_string()))?,
        None => format!(
            "{APP_NAME}-session-{}.html",
            basename_without_extension(session_file, ".jsonl")
        ),
    };

    std::fs::write(&output_path, html)?;
    Ok(output_path)
}

/// Export a session file to HTML (standalone, without `AgentState`).
/// Used by the CLI for exporting arbitrary session files.
pub fn export_from_file(
    input_path: &str,
    options: ExportOptions<'_>,
) -> Result<String, ExportHtmlError> {
    let resolved_input_path = resolve_path_default(input_path, &current_dir())
        .map_err(|error| ExportHtmlError::Session(error.to_string()))?;
    if !Path::new(&resolved_input_path).exists() {
        return Err(ExportHtmlError::FileNotFound(resolved_input_path));
    }

    let session_manager = SessionManager::open(&resolved_input_path, None, None)
        .map_err(|error| ExportHtmlError::Session(error.to_string()))?;
    let entries = session_manager.get_entries();

    let session_data = SessionData {
        header: session_manager.get_header(),
        entries: &entries,
        leaf_id: session_manager.get_leaf_id(),
        system_prompt: None,
        tools: None,
        rendered_tools: None,
    };

    let html = generate_html(&session_data, options.theme_name.as_deref())?;

    let output_path = match options.output_path {
        Some(path) => normalize_path_default(&path)
            .map_err(|error| ExportHtmlError::Session(error.to_string()))?,
        None => format!(
            "{APP_NAME}-session-{}.html",
            basename_without_extension(&resolved_input_path, ".jsonl")
        ),
    };

    std::fs::write(&output_path, html)?;
    Ok(output_path)
}

/// `basename(path, extension)` — the trailing component without the extension.
fn basename_without_extension(path: &str, extension: &str) -> String {
    let base = path
        .rsplit(['/', std::path::MAIN_SEPARATOR])
        .next()
        .unwrap_or(path);
    base.strip_suffix(extension).unwrap_or(base).to_owned()
}

/// `record[key] = value` — overwrite in place, append at the end otherwise.
fn insert_rendered(rendered: &mut RenderedTools, key: &str, value: RenderedToolHtml) {
    match rendered.iter_mut().find(|(existing, _)| existing == key) {
        Some((_, slot)) => *slot = value,
        None => rendered.push((key.to_owned(), value)),
    }
}

fn serialize_rendered_tools<S: serde::Serializer>(
    rendered: &Option<&RenderedTools>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let Some(rendered) = rendered else {
        return serializer.serialize_none();
    };
    let mut map = Map::new();
    for (key, value) in rendered.iter() {
        map.insert(
            key.clone(),
            serde_json::to_value(value).map_err(serde::ser::Error::custom)?,
        );
    }
    Value::Object(map).serialize(serializer)
}
