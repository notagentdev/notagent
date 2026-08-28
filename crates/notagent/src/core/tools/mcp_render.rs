//! How an MCP tool's call and result are drawn (v0.1.22).
//! A server's tools are not known at build time, so there is no hand-written
//! renderer for any of them, and the shape of both the arguments and the result
//! is whatever that server chose. The fallback of printing the raw JSON on both
//! sides is what this module exists to avoid: a call header that is one long
//! brace-heavy line reads as noise, and a result that is a minified document
//! fills the transcript with something nobody can scan.
//! So the call becomes a header in the same shape every built-in tool uses —
//! the server as the block title, the tool name, then its arguments as
//! `key: value` with each value cut to a length that still fits a line. And the
//! result is re-indented when it is JSON, then previewed at a fixed number of
//! lines with the same expand hint the other tools carry.

use serde_json::Value;

use crate::core::tools::render_utils::{get_text_output, invalid_arg_text};
use crate::core::tools::tool_definition::ToolRenderResult;
use crate::modes::interactive::components::keybinding_hints::key_hint;
use crate::modes::interactive::theme::theme::{Theme, ThemeColor};

/// Lines of a result shown before it is collapsed.
const MCP_PREVIEW_LINES: usize = 12;

/// How long a single argument value may print.
const MAX_VALUE_CHARS: usize = 48;

/// How many arguments print before the rest are counted instead.
const MAX_ARGUMENTS: usize = 4;

/// The call header: the tool's name and a readable digest of its arguments.
/// While the model is still streaming the arguments there is nothing worth
/// showing — a half-written object would print as a value that changes shape on
/// every frame — so the name stands alone until they are complete.
pub fn format_mcp_call(tool: &str, args: &Value, args_complete: bool, theme: &Theme) -> String {
    let name = theme.fg(ThemeColor::ToolOutput, tool);
    if !args_complete {
        return name;
    }
    let Some(fields) = args.as_object() else {
        // A tool called with something that is not an object is a call that
        // will fail; saying so here is better than printing `null`.
        return match args {
            Value::Null => name,
            _ => format!("{name} {}", invalid_arg_text(theme)),
        };
    };
    if fields.is_empty() {
        return name;
    }

    let mut parts: Vec<String> = Vec::new();
    for (key, value) in fields.iter().take(MAX_ARGUMENTS) {
        parts.push(format!(
            "{}: {}",
            theme.fg(ThemeColor::Muted, key),
            theme.fg(ThemeColor::ToolOutput, &summarize(value))
        ));
    }
    let hidden = fields.len().saturating_sub(MAX_ARGUMENTS);
    if hidden > 0 {
        parts.push(theme.fg(ThemeColor::Muted, &format!("+{hidden} more")));
    }
    format!(
        "{name} {}",
        parts.join(theme.fg(ThemeColor::Muted, ", ").as_str())
    )
}

/// One argument value, short enough to sit on a header line.
/// Containers are counted rather than printed: a header is for recognising the
/// call, and a nested object spelled out in full stops being a header.
fn summarize(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(value) => value.to_string(),
        Value::Number(number) => number.to_string(),
        Value::String(text) => quote(&clip(&collapse_whitespace(text))),
        Value::Array(items) => match items.len() {
            0 => "[]".to_owned(),
            1 => "[1 item]".to_owned(),
            count => format!("[{count} items]"),
        },
        Value::Object(fields) => match fields.len() {
            0 => "{}".to_owned(),
            1 => "{1 field}".to_owned(),
            count => format!("{{{count} fields}}"),
        },
    }
}

/// Newlines in a header would break the row; they become spaces.
fn collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_space = false;
    for character in text.chars() {
        if character.is_whitespace() {
            if !in_space && !out.is_empty() {
                out.push(' ');
            }
            in_space = true;
        } else {
            out.push(character);
            in_space = false;
        }
    }
    out.trim_end().to_owned()
}

fn clip(text: &str) -> String {
    if text.chars().count() <= MAX_VALUE_CHARS {
        return text.to_owned();
    }
    let kept: String = text
        .chars()
        .take(MAX_VALUE_CHARS.saturating_sub(1))
        .collect();
    format!("{kept}…")
}

fn quote(text: &str) -> String {
    format!("\"{text}\"")
}

/// The result body: JSON laid out, anything else as it came, both previewed.
pub fn format_mcp_result(
    result: ToolRenderResult<'_>,
    expanded: bool,
    show_images: bool,
    theme: &Theme,
) -> String {
    let output = get_text_output(Some(result.content), show_images);
    let output = layout(output.trim());
    if output.is_empty() {
        return String::new();
    }

    let lines: Vec<&str> = output.split('\n').collect();
    let shown = if expanded {
        lines.len()
    } else {
        MCP_PREVIEW_LINES.min(lines.len())
    };
    let mut text = format!(
        "\n{}",
        lines[..shown]
            .iter()
            .map(|line| theme.fg(ThemeColor::ToolOutput, line))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let remaining = lines.len() - shown;
    if remaining > 0 {
        text += &format!(
            "{} {}{}",
            theme.fg(
                ThemeColor::Muted,
                &format!("\n... ({remaining} more lines,")
            ),
            key_hint("app.tools.expand", "to expand"),
            theme.fg(ThemeColor::Muted, ")")
        );
    }
    text
}

/// Re-indents a JSON document, and leaves everything else alone.
/// Servers commonly answer with a single minified line. Expanding it is the
/// difference between a result that can be read down the page and one that
/// wraps into a paragraph.
fn layout(text: &str) -> String {
    let trimmed = text.trim_start();
    if !trimmed.starts_with('{') && !trimmed.starts_with('[') {
        return text.to_owned();
    }
    // Already laid out: leave the server's own formatting alone rather than
    // re-flowing it into a shape it did not choose.
    if text.contains('\n') {
        return text.to_owned();
    }
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return text.to_owned();
    };
    serde_json::to_string_pretty(&value).unwrap_or_else(|_| text.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use notagent_ai::types::{TextContent, TextOrImageContent};

    fn theme() -> std::sync::Arc<Theme> {
        crate::modes::interactive::theme::theme::init_theme(None, false);
        crate::modes::interactive::theme::theme::theme()
    }

    /// The colours are the theme's business; these tests are about the shape,
    /// and a length measured through escape sequences measures the wrong thing.
    fn plain(text: &str) -> String {
        crate::utils::ansi::strip_ansi(text)
    }

    fn call(tool: &str, args: &Value, args_complete: bool) -> String {
        plain(&format_mcp_call(tool, args, args_complete, &theme()))
    }

    fn text_result(text: &str) -> Vec<TextOrImageContent> {
        vec![TextOrImageContent::Text(TextContent::new(text.to_owned()))]
    }

    fn rendered(content: &[TextOrImageContent], expanded: bool) -> String {
        plain(&format_mcp_result(
            ToolRenderResult {
                content,
                details: None,
            },
            expanded,
            false,
            &theme(),
        ))
    }

    #[test]
    fn a_call_shows_its_arguments_as_pairs() {
        let args = serde_json::json!({ "repo": "acme/app", "number": 7 });

        let line = call("create_issue", &args, true);

        assert!(line.contains("create_issue"), "{line}");
        assert!(line.contains("repo"), "{line}");
        assert!(line.contains("\"acme/app\""), "{line}");
        assert!(line.contains("number"), "{line}");
        assert!(line.contains('7'), "{line}");
    }

    #[test]
    fn streaming_arguments_are_not_drawn_half_written() {
        let args = serde_json::json!({ "repo": "acm" });

        let line = call("create_issue", &args, false);

        assert_eq!(line.trim(), "create_issue");
    }

    #[test]
    fn a_long_value_is_cut_rather_than_wrapping_the_header() {
        let args = serde_json::json!({ "body": "x".repeat(500) });

        let line = call("create_issue", &args, true);

        assert!(line.chars().count() < 120, "{} chars", line.chars().count());
        assert!(line.contains('…'), "{line}");
    }

    #[test]
    fn a_value_with_newlines_stays_on_one_line() {
        let args = serde_json::json!({ "body": "first\nsecond\nthird" });

        let line = call("write", &args, true);

        assert!(!line.contains('\n'), "{line}");
        assert!(line.contains("first second third"), "{line}");
    }

    #[test]
    fn a_nested_container_is_counted_not_spelled_out() {
        let args = serde_json::json!({
            "labels": ["bug", "urgent"],
            "meta": { "a": 1, "b": 2, "c": 3 },
        });

        let line = call("create_issue", &args, true);

        assert!(line.contains("[2 items]"), "{line}");
        assert!(line.contains("{3 fields}"), "{line}");
        assert!(!line.contains("bug"), "{line}");
    }

    #[test]
    fn only_the_first_few_arguments_print() {
        let args = serde_json::json!({
            "a": 1, "b": 2, "c": 3, "d": 4, "e": 5, "f": 6,
        });

        let line = call("call", &args, true);

        assert!(line.contains("+2 more"), "{line}");
    }

    #[test]
    fn a_call_without_arguments_is_just_its_name() {
        let line = call("list", &serde_json::json!({}), true);
        assert_eq!(line.trim(), "list");
    }

    #[test]
    fn arguments_that_are_not_an_object_are_marked_invalid() {
        let line = call("list", &serde_json::json!("nope"), true);
        assert!(line.contains("invalid arg"), "{line}");
    }

    #[test]
    fn a_minified_json_result_is_laid_out() {
        let content = text_result(r#"{"name":"acme","items":[1,2,3]}"#);

        let text = rendered(&content, true);

        assert!(text.contains("\"name\": \"acme\""), "{text}");
        assert!(text.lines().count() > 3, "{text}");
    }

    #[test]
    fn a_result_the_server_already_laid_out_is_left_alone() {
        let content = text_result("{\n  \"name\": \"acme\"\n}");

        let text = rendered(&content, true);

        assert!(text.contains("  \"name\": \"acme\""), "{text}");
    }

    #[test]
    fn text_that_is_not_json_passes_through() {
        let content = text_result("just a sentence");

        assert!(rendered(&content, true).contains("just a sentence"));
    }

    #[test]
    fn something_that_starts_like_json_but_is_not_passes_through() {
        let content = text_result("{not json at all");

        assert!(rendered(&content, true).contains("{not json at all"));
    }

    #[test]
    fn a_long_result_is_previewed_with_an_expand_hint() {
        let content = text_result(
            &(1..=40)
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join("\n"),
        );

        let collapsed = rendered(&content, false);

        assert!(collapsed.contains("more lines"), "{collapsed}");
        assert_eq!(
            collapsed.trim_start().lines().count(),
            MCP_PREVIEW_LINES + 1,
            "{collapsed}"
        );
    }

    #[test]
    fn expanding_shows_everything() {
        let content = text_result(
            &(1..=40)
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join("\n"),
        );

        let expanded = rendered(&content, true);

        assert!(!expanded.contains("more lines"), "{expanded}");
        assert!(expanded.contains("\n40"), "{expanded}");
    }

    #[test]
    fn an_empty_result_draws_nothing() {
        assert!(rendered(&[], true).is_empty());
        assert!(rendered(&text_result("   "), true).is_empty());
    }
}
