use notagent_ai::types::{ImageContent, TextContent, TextOrImageContent};

use super::image::{ProcessImageResult, process_image};

/// `normalizeToolResultImages(content, options)`.
/// and the caller compares by identity (`normalizedContent === content`). Rust
/// has no such identity, so "nothing changed" is `None` — the caller reads it
/// the same way.
pub fn normalize_tool_result_images(
    content: &[TextOrImageContent],
    auto_resize_images: bool,
) -> Option<Vec<TextOrImageContent>> {
    if !content
        .iter()
        .any(|block| matches!(block, TextOrImageContent::Image(_)))
    {
        return None;
    }

    let mut normalized: Vec<TextOrImageContent> = Vec::new();
    let mut changed = false;

    for block in content {
        let TextOrImageContent::Image(image) = block else {
            normalized.push(block.clone());
            continue;
        };

        let Some(bytes) = decode_base64(&image.data) else {
            // `Buffer.from(data, "base64")` never throws: it stops at the first
            // character that is not base64 and hands the decoder whatever it
            // has, which then fails to decode — the branch below.
            normalized.push(block.clone());
            continue;
        };
        let processed = process_image(&bytes, &image.mime_type, auto_resize_images, None);
        let ProcessImageResult::Ok {
            data,
            mime_type,
            hints,
        } = processed
        else {
            // Unlike `read`, keep the original block: the tool produced this
            // image and the failure may just be an unusable payload.
            normalized.push(block.clone());
            continue;
        };

        if data == image.data && mime_type == image.mime_type && hints.is_empty() {
            normalized.push(block.clone());
            continue;
        }

        normalized.push(TextOrImageContent::Image(ImageContent { data, mime_type }));
        if !hints.is_empty() {
            normalized.push(TextOrImageContent::Text(TextContent::new(hints.join("\n"))));
        }
        changed = true;
    }

    changed.then_some(normalized)
}

/// `Buffer.from(value, "base64")` — lenient like Node: characters outside the
/// alphabet end the decode instead of failing it.
fn decode_base64(value: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    let mut cleaned = String::with_capacity(value.len());
    for character in value.chars() {
        if character.is_ascii_alphanumeric() || character == '+' || character == '/' {
            cleaned.push(character);
            continue;
        }
        if character == '=' || character.is_whitespace() {
            continue;
        }
        break;
    }
    base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(cleaned)
        .ok()
}
