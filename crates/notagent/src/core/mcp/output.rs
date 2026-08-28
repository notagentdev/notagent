//! Turning an MCP result into what the model reads (v0.1.22).
//! Text passes through and images become attachments where the session takes
//! them. An embedded text resource is text and is carried as text; a blob
//! resource that holds an image is carried as one. What is genuinely
//! uncarryable — audio, and binary the session has no slot for — is named with
//! its type and its address rather than dropped, because a content block that
//! vanishes silently is a wrong answer the model cannot see is wrong.
//! `structuredContent` travels too. It is the half of a result a server means
//! to be parsed, and a tool that keeps only the prose beside it hands the model
//! a summary where it asked for data.
//! The same truncation the bash tool applies applies here, so a server
//! returning a megabyte costs what a command returning a megabyte costs.

use notagent_ai::types::{TextContent, TextOrImageContent};
use rmcp::model::ResourceContents;

use crate::core::tools::truncate::{TruncationOptions, truncate_tail};

/// What came back, ready for a tool result.
#[derive(Debug, Clone, PartialEq)]
pub struct McpOutput {
    pub content: Vec<TextOrImageContent>,
    pub is_error: bool,
    /// Set when the text was cut, for the notice the caller appends.
    pub truncated: bool,
}

/// cannot.
pub fn convert_result(result: &rmcp::model::CallToolResult, accepts_images: bool) -> McpOutput {
    let mut text = String::new();
    let mut images = Vec::new();
    let mut refused: Vec<&'static str> = Vec::new();

    for content in &result.content {
        if let Some(block) = content.as_text() {
            append(&mut text, &block.text);
            continue;
        }
        if let Some(block) = content.as_image() {
            match accepts_images {
                true => images.push(TextOrImageContent::Image(
                    notagent_ai::types::ImageContent {
                        data: block.data.clone(),
                        mime_type: block.mime_type.clone(),
                    },
                )),
                false => remember(&mut refused, "an image"),
            }
            continue;
        }
        if let Some(block) = content.as_resource() {
            match &block.resource {
                // A text resource is text. Refusing it because of the envelope
                // it came in loses the answer for no reason.
                ResourceContents::TextResourceContents {
                    uri, text: body, ..
                } => {
                    append(&mut text, &format!("[{uri}]\n{body}"));
                }
                ResourceContents::BlobResourceContents {
                    uri,
                    mime_type,
                    blob,
                    ..
                } => {
                    let mime = mime_type.as_deref().unwrap_or_default();
                    match mime.starts_with("image/") && accepts_images {
                        true => images.push(TextOrImageContent::Image(
                            notagent_ai::types::ImageContent {
                                data: blob.clone(),
                                mime_type: mime.to_owned(),
                            },
                        )),
                        // The bytes cannot travel, but the address can, and a
                        // model that has it can ask for the thing by name.
                        false => append(
                            &mut text,
                            &format!(
                                "[The server returned {} at {uri}, which this tool cannot carry.]",
                                describe_mime(mime)
                            ),
                        ),
                    }
                }
            }
            continue;
        }
        if let Some(link) = content.as_resource_link() {
            // A link is a URI and a label — both of which are text, and both of
            // which the model needs to follow it.
            let mime = link
                .mime_type
                .as_deref()
                .map(|mime| format!(" ({mime})"))
                .unwrap_or_default();
            append(&mut text, &format!("[{}{mime}: {}]", link.name, link.uri));
            continue;
        }
        // text and images. Naming it is the difference between the model
        // knowing the answer is incomplete and not.
        remember(&mut refused, "an audio clip");
    }

    // The structured half of a result is the half a server means to be parsed.
    // Dropping it silently leaves the model with the prose summary and no data.
    if let Some(structured) = &result.structured_content
        && !structured.is_null()
    {
        let rendered =
            serde_json::to_string_pretty(structured).unwrap_or_else(|_| structured.to_string());
        append(
            &mut text,
            &format!("<structured-result>\n{rendered}\n</structured-result>"),
        );
    }

    if !refused.is_empty() {
        // A blank line before the notice: it is about the answer, not part of
        // it, and the model should not read it as the server's last sentence.
        if !text.is_empty() {
            text.push('\n');
        }
        append(
            &mut text,
            &format!(
                "[The server also returned {}, which this tool cannot carry.]",
                join_kinds(&refused)
            ),
        );
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

/// Adds one chunk, keeping the blocks one blank line apart.
fn append(text: &mut String, chunk: &str) {
    if chunk.is_empty() {
        return;
    }
    if !text.is_empty() {
        text.push('\n');
    }
    text.push_str(chunk);
}

fn remember(refused: &mut Vec<&'static str>, kind: &'static str) {
    if !refused.contains(&kind) {
        refused.push(kind);
    }
}

/// Names a media type in words, for a notice a model reads.
fn describe_mime(mime: &str) -> String {
    match mime.split('/').next().unwrap_or_default() {
        "image" => "an image".to_owned(),
        "audio" => "an audio clip".to_owned(),
        "video" => "a video".to_owned(),
        "" => "binary data".to_owned(),
        _ => format!("data of type `{mime}`"),
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
    fn an_embedded_text_resource_is_carried_as_text() {
        let result = CallToolResult::success(vec![Content::embedded_text(
            "file:///notes.md".to_owned(),
            "the whole answer".to_owned(),
        )]);

        let output = convert_result(&result, true);

        assert!(
            text_of(&output).contains("the whole answer"),
            "{}",
            text_of(&output)
        );
        assert!(
            text_of(&output).contains("file:///notes.md"),
            "{}",
            text_of(&output)
        );
    }

    #[test]
    fn an_embedded_image_resource_becomes_an_attachment() {
        let result = CallToolResult::success(vec![Content::resource(
            rmcp::model::ResourceContents::BlobResourceContents {
                uri: "file:///shot.png".to_owned(),
                mime_type: Some("image/png".to_owned()),
                blob: "AAAA".to_owned(),
                meta: None,
            },
        )]);

        assert_eq!(images_in(&convert_result(&result, true)), 1);
    }

    #[test]
    fn a_blob_that_cannot_travel_leaves_its_address_behind() {
        let result = CallToolResult::success(vec![Content::resource(
            rmcp::model::ResourceContents::BlobResourceContents {
                uri: "file:///report.pdf".to_owned(),
                mime_type: Some("application/pdf".to_owned()),
                blob: "AAAA".to_owned(),
                meta: None,
            },
        )]);

        let text = text_of(&convert_result(&result, true));

        // The bytes cannot go, but the model can still ask for the file.
        assert!(text.contains("file:///report.pdf"), "{text}");
        assert!(text.contains("application/pdf"), "{text}");
    }

    #[test]
    fn an_image_resource_is_described_where_images_cannot_go() {
        let result = CallToolResult::success(vec![Content::resource(
            rmcp::model::ResourceContents::BlobResourceContents {
                uri: "file:///shot.png".to_owned(),
                mime_type: Some("image/png".to_owned()),
                blob: "AAAA".to_owned(),
                meta: None,
            },
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
    fn a_resource_link_keeps_its_uri() {
        let result =
            CallToolResult::success(vec![Content::resource_link(rmcp::model::RawResource {
                uri: "https://example.test/doc".to_owned(),
                name: "the design doc".to_owned(),
                title: None,
                description: None,
                mime_type: Some("text/html".to_owned()),
                size: None,
                icons: None,
            })]);

        let text = text_of(&convert_result(&result, true));

        assert!(text.contains("https://example.test/doc"), "{text}");
        assert!(text.contains("the design doc"), "{text}");
    }

    #[test]
    fn structured_content_travels_with_the_prose() {
        let mut result = CallToolResult::success(vec![Content::text("two matches")]);
        result.structured_content = Some(serde_json::json!({ "matches": [1, 2] }));

        let text = text_of(&convert_result(&result, true));

        assert!(text.contains("two matches"), "{text}");
        assert!(text.contains("\"matches\""), "{text}");
        assert!(text.contains("structured-result"), "{text}");
    }

    #[test]
    fn a_result_with_only_structured_content_is_not_empty() {
        let mut result = CallToolResult::success(Vec::new());
        result.structured_content = Some(serde_json::json!({ "ok": true }));

        assert!(text_of(&convert_result(&result, true)).contains("\"ok\""));
    }

    #[test]
    fn a_null_structured_result_adds_nothing() {
        let mut result = CallToolResult::success(vec![Content::text("done")]);
        result.structured_content = Some(serde_json::Value::Null);

        assert_eq!(text_of(&convert_result(&result, true)), "done");
    }

    #[test]
    fn audio_is_still_named_rather_than_dropped() {
        // images, and silence about it would be a wrong answer.
        let result = CallToolResult::success(vec![Content::new(
            rmcp::model::RawContent::Audio(rmcp::model::RawAudioContent {
                data: "AAAA".to_owned(),
                mime_type: "audio/mpeg".to_owned(),
            }),
            None,
        )]);

        assert!(text_of(&convert_result(&result, true)).contains("an audio clip"));
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
