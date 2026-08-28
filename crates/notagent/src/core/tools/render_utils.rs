use notagent_ai::types::TextOrImageContent;
use notagent_tui::terminal_image::{
    get_capabilities, get_image_dimensions, hyperlink, image_fallback,
};

use crate::modes::interactive::theme::theme::{Theme, ThemeColor};
use crate::utils::ansi::strip_ansi;
use crate::utils::paths::{path_to_file_url, resolve_path_default};
use crate::utils::shell::sanitize_binary_output;

/// `~`-shortened display form of a path.
/// exactly that distinction.
pub fn shorten_path(path: Option<&str>) -> String {
    let Some(path) = path else {
        return String::new();
    };
    let home = dirs::home_dir()
        .map(|home| home.to_string_lossy().into_owned())
        .unwrap_or_default();
    if !home.is_empty()
        && let Some(rest) = path.strip_prefix(&home)
    {
        return format!("~{rest}");
    }
    path.to_string()
}

/// Wrap `styled_text` in an OSC 8 link to `raw_path`, if the terminal supports it.
pub fn link_path(styled_text: &str, raw_path: &str, cwd: &str) -> String {
    if !get_capabilities().hyperlinks {
        return styled_text.to_string();
    }
    // must not panic, so an unresolvable path simply stays unlinked (class 1).
    let Ok(absolute_path) = resolve_path_default(raw_path, cwd) else {
        return styled_text.to_string();
    };
    hyperlink(styled_text, &path_to_file_url(&absolute_path))
}

/// `str(value)` — a string argument as itself, `null`/`undefined` as `""`, and
/// anything else as `null`, which the callers render as `[invalid arg]`.
/// for a string, `Some("")` for a nullish value and `None` for a wrong type.
pub fn str_arg(value: Option<&serde_json::Value>) -> Option<String> {
    match value {
        Some(serde_json::Value::String(text)) => Some(text.clone()),
        None | Some(serde_json::Value::Null) => Some(String::new()),
        Some(_) => None,
    }
}

/// Tabs render as three spaces.
pub fn replace_tabs(text: &str) -> String {
    text.replace('\t', "   ")
}

/// Carriage returns are dropped before display.
pub fn normalize_display_text(text: &str) -> String {
    text.replace('\r', "")
}

/// The text a tool result contributes to the transcript.
pub fn get_text_output(content: Option<&[TextOrImageContent]>, show_images: bool) -> String {
    let Some(content) = content else {
        return String::new();
    };

    let text_blocks: Vec<&TextOrImageContent> = content
        .iter()
        .filter(|block| matches!(block, TextOrImageContent::Text(_)))
        .collect();
    let image_blocks: Vec<&TextOrImageContent> = content
        .iter()
        .filter(|block| matches!(block, TextOrImageContent::Image(_)))
        .collect();

    let mut output = text_blocks
        .iter()
        .map(|block| match block {
            TextOrImageContent::Text(text) => {
                sanitize_binary_output(&strip_ansi(&text.text)).replace('\r', "")
            }
            TextOrImageContent::Image(_) => unreachable!("filtered above"),
        })
        .collect::<Vec<_>>()
        .join("\n");

    let capabilities = get_capabilities();
    if !image_blocks.is_empty() && (capabilities.images.is_none() || !show_images) {
        let image_indicators = image_blocks
            .iter()
            .map(|block| match block {
                TextOrImageContent::Image(image) => {
                    // always carries one, and an empty string takes the same path
                    // as a missing field did.
                    let mime_type = if image.mime_type.is_empty() {
                        "image/unknown"
                    } else {
                        &image.mime_type
                    };
                    let dimensions = if image.data.is_empty() || image.mime_type.is_empty() {
                        None
                    } else {
                        get_image_dimensions(&image.data, &image.mime_type)
                    };
                    image_fallback(mime_type, dimensions, None)
                }
                TextOrImageContent::Text(_) => unreachable!("filtered above"),
            })
            .collect::<Vec<_>>()
            .join("\n");
        output = if output.is_empty() {
            image_indicators
        } else {
            format!("{output}\n{image_indicators}")
        };
    }

    output
}

/// `[invalid arg]` in the error colour.
pub fn invalid_arg_text(theme: &Theme) -> String {
    theme.fg(ThemeColor::Error, "[invalid arg]")
}

/// The call line's bold tool-name prefix, with its trailing space (takeover
/// of the reference's `call_title`). The badge style returns an empty prefix:
/// the badge above already names the tool, so the call line carries the
/// arguments alone and the block header reads `READ (src/lib.rs)` instead of
/// repeating the name.
pub fn call_title(theme: &Theme, title: &str) -> String {
    use crate::modes::interactive::theme::theme::{BlockStyle, block_style};
    if block_style() == BlockStyle::Badge {
        String::new()
    } else {
        format!("{} ", theme.fg(ThemeColor::ToolTitle, &theme.bold(title)))
    }
}

/// The path column of a tool header: linked, `~`-shortened and colour-coded.
pub fn render_tool_path(
    raw_path: Option<&str>,
    theme: &Theme,
    cwd: &str,
    empty_fallback: Option<&str>,
) -> String {
    let Some(raw_path) = raw_path else {
        return invalid_arg_text(theme);
    };
    // `rawPath || options?.emptyFallback` — an empty path falls back.
    let value = if raw_path.is_empty() {
        empty_fallback.unwrap_or("")
    } else {
        raw_path
    };
    if value.is_empty() {
        return theme.fg(ThemeColor::ToolOutput, "...");
    }
    link_path(
        &theme.fg(ThemeColor::Accent, &shorten_path(Some(value))),
        value,
        cwd,
    )
}
