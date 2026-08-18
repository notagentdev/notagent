//! Turning an MCP result into what the model reads (v0.1.22).
//!
//! Text passes through and images become attachments where the session takes
//! them. Audio and embedded resources are refused with their type named rather
//! than dropped: a content block that vanishes silently is a wrong answer the
//! model cannot see is wrong.
//!
//! The same truncation the bash tool applies applies here, so a server
//! returning a megabyte costs what a command returning a megabyte costs.

use notagent_ai::types::{TextContent, TextOrImageContent};

use crate::core::tools::truncate::{TruncationOptions, truncate_tail};

/// What came back, ready for a tool result.
#[derive(Debug, Clone, PartialEq)]
pub struct McpOutput {
    pub content: Vec<TextOrImageContent>,
    pub is_error: bool,
    /// Set when the text was cut, for the notice the caller appends.
    pub truncated: bool,
}

/// Converts a result, keeping what this port can carry and naming what it
/// cannot.
pub fn convert_result(result: &rmcp::model::CallToolResult, accepts_images: bool) -> McpOutput {
    let mut text = String::new();
    let mut images = Vec::new();
    let mut refused: Vec<&'static str> = Vec::new();

    for content in &result.content {
        if let Some(block) = content.as_text() {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&block.text);
            continue;
        }
        if let Some(block) = content.as_image() {
            if accepts_images {
                images.push(TextOrImageContent::Image(
                    notagent_ai::types::ImageContent {
                        data: block.data.clone(),
                        mime_type: block.mime_type.clone(),
                    },
                ));
            } else if !refused.contains(&"an image") {
                refused.push("an image");
            }
            continue;
        }
        // Audio and embedded resources have no place to go in this port's
        // result shape. Naming them is the difference between the model
        // knowing the answer is incomplete and not.
        let kind = if content.as_resource().is_some() {
            "an embedded resource"
        } else {
            "an audio clip"
        };
        if !refused.contains(&kind) {
            refused.push(kind);
        }
    }

    if !refused.is_empty() {
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str(&format!(
            "[The server also returned {}, which this tool cannot carry.]",
            join_kinds(&refused)
        ));
    }

    let truncation = truncate_tail(&text, TruncationOptions::default());
    let mut content = Vec::new();
    if !truncation.content.is_empty() {
        content.push(TextOrImageContent::Text(TextContent::new(
            truncation.content.clone(),
        )));
    }
    content.extend(images);

    McpOutput {
        content,
        is_error: result.is_error.unwrap_or(false),
        truncated: truncation.truncated,
    }
}

fn join_kinds(kinds: &[&str]) -> String {
    match kinds {
        [] => String::new(),
        [only] => (*only).to_owned(),
        [first, second] => format!("{first} and {second}"),
        [rest @ .., last] => format!("{}, and {last}", rest.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::{CallToolResult, Content};

    fn text_of(output: &McpOutput) -> String {
        output
            .content
            .iter()
            .filter_map(|block| match block {
                TextOrImageContent::Text(text) => Some(text.text.clone()),
                TextOrImageContent::Image(_) => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn images_in(output: &McpOutput) -> usize {
        output
            .content
            .iter()
            .filter(|block| matches!(block, TextOrImageContent::Image(_)))
            .count()
    }

    #[test]
    fn text_passes_through_and_several_blocks_join() {
        let result = CallToolResult::success(vec![Content::text("first"), Content::text("second")]);
        let output = convert_result(&result, true);

        assert_eq!(text_of(&output), "first\nsecond");
        assert!(!output.is_error);
    }

    #[test]
    fn an_error_result_stays_an_error() {
        let result = CallToolResult::error(vec![Content::text("no")]);
        assert!(convert_result(&result, true).is_error);
    }

    #[test]
    fn an_image_becomes_an_attachment_where_the_session_takes_one() {
        let result = CallToolResult::success(vec![Content::image(
            "AAAA".to_owned(),
            "image/png".to_owned(),
        )]);

        assert_eq!(images_in(&convert_result(&result, true)), 1);
    }

    #[test]
    fn an_image_is_named_rather_than_dropped_where_it_cannot_go() {
        let result = CallToolResult::success(vec![Content::image(
            "AAAA".to_owned(),
            "image/png".to_owned(),
        )]);
        let output = convert_result(&result, false);

        assert_eq!(images_in(&output), 0);
        assert!(
            text_of(&output).contains("an image"),
            "{}",
            text_of(&output)
        );
    }

    #[test]
    fn a_long_result_is_cut_and_says_so() {
        let result = CallToolResult::success(vec![Content::text("y".repeat(200_000))]);
        let output = convert_result(&result, true);

        assert!(output.truncated);
        assert!(text_of(&output).len() < 200_000);
    }

    #[test]
    fn several_refused_kinds_are_listed_once_each() {
        assert_eq!(join_kinds(&["an image"]), "an image");
        assert_eq!(
            join_kinds(&["an image", "an audio clip"]),
            "an image and an audio clip"
        );
        assert_eq!(
            join_kinds(&["an image", "an audio clip", "an embedded resource"]),
            "an image, an audio clip, and an embedded resource"
        );
    }
}
