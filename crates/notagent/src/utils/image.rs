//! Port of `packages/coding-agent/src/utils/image-process.ts`,
//! `image-convert.ts` and `image-resize-core.ts`.
//!
//! Tech substitution (class 3): Photon/WASM and its worker thread are replaced by
//! the `image` crate, which decodes, resizes and encodes in process. EXIF
//! orientation comes from the decoder instead of the hand-rolled parser in
//! `exif-orientation.ts`.

use base64::Engine;
use image::{DynamicImage, ImageFormat, ImageReader};

/// 4.5 MB of base64 payload, with headroom below Anthropic's 5 MB limit.
const DEFAULT_MAX_BYTES: usize = (4.5 * 1024.0 * 1024.0) as usize;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImageResizeOptions {
    pub max_width: u32,
    pub max_height: u32,
    pub max_bytes: usize,
    pub jpeg_quality: u8,
}

impl Default for ImageResizeOptions {
    fn default() -> Self {
        Self {
            max_width: 2000,
            max_height: 2000,
            max_bytes: DEFAULT_MAX_BYTES,
            jpeg_quality: 80,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResizedImage {
    /// base64
    pub data: String,
    pub mime_type: String,
    pub original_width: u32,
    pub original_height: u32,
    pub width: u32,
    pub height: u32,
    pub was_resized: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessImageResult {
    Ok {
        data: String,
        mime_type: String,
        hints: Vec<String>,
    },
    Failed {
        message: String,
    },
}

fn base64_encode(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn base_mime_type(mime_type: &str) -> String {
    mime_type
        .split(';')
        .next()
        .unwrap_or(mime_type)
        .trim()
        .to_lowercase()
}

fn normalize_supported_image_mime_type(mime_type: &str) -> Option<&'static str> {
    match base_mime_type(mime_type).as_str() {
        "image/png" => Some("image/png"),
        "image/jpeg" | "image/jpg" => Some("image/jpeg"),
        "image/gif" => Some("image/gif"),
        "image/webp" => Some("image/webp"),
        _ => None,
    }
}

/// Decode `bytes` and apply the EXIF orientation, as Photon's loader did.
fn decode(bytes: &[u8]) -> Option<DynamicImage> {
    let reader = ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let mut decoder = reader.into_decoder().ok()?;
    let orientation = image::ImageDecoder::orientation(&mut decoder).ok();
    let mut decoded = DynamicImage::from_decoder(decoder).ok()?;
    if let Some(orientation) = orientation {
        decoded.apply_orientation(orientation);
    }
    Some(decoded)
}

fn encode(image: &DynamicImage, format: ImageFormat, jpeg_quality: Option<u8>) -> Option<Vec<u8>> {
    let mut buffer: Vec<u8> = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut buffer);
    match (format, jpeg_quality) {
        (ImageFormat::Jpeg, Some(quality)) => {
            let mut encoder =
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut cursor, quality);
            // JPEG has no alpha channel.
            encoder.encode_image(&image.to_rgb8()).ok()?;
        }
        _ => image.write_to(&mut cursor, format).ok()?,
    }
    Some(buffer)
}

/// Convert arbitrary image bytes to PNG (`convertImageBytesToPng`).
pub fn convert_image_bytes_to_png(bytes: &[u8]) -> Option<Vec<u8>> {
    let decoded = decode(bytes)?;
    encode(&decoded, ImageFormat::Png, None)
}

/// `convertToPng(base64Data, mimeType)` — the base64 shell around it.
///
/// The kitty graphics protocol only takes PNG (`f=100`), so the tool row
/// converts an inline image before it shows one (`components/tool-execution.ts:191`).
/// Returns the data and its mime type, or `None` when the bytes cannot be
/// decoded.
pub fn convert_to_png(base64_data: &str, mime_type: &str) -> Option<(String, String)> {
    if mime_type == "image/png" {
        return Some((base64_data.to_owned(), mime_type.to_owned()));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(base64_data)
        .ok()?;
    let png_bytes = convert_image_bytes_to_png(&bytes)?;
    Some((base64_encode(&png_bytes), "image/png".to_owned()))
}

struct EncodedCandidate {
    data: String,
    encoded_size: usize,
    mime_type: &'static str,
}

fn encode_candidate(bytes: &[u8], mime_type: &'static str) -> EncodedCandidate {
    let data = base64_encode(bytes);
    EncodedCandidate {
        encoded_size: data.len(),
        data,
        mime_type,
    }
}

/// Resize an image to fit the dimension and encoded-size limits.
///
/// Strategy: fit the max dimensions, try PNG and JPEG and pick the first that
/// fits, then lower the JPEG quality, then shrink by 25 % until 1×1.
pub fn resize_image(
    input_bytes: &[u8],
    mime_type: &str,
    options: Option<ImageResizeOptions>,
) -> Option<ResizedImage> {
    let options = options.unwrap_or_default();
    let input_base64_size = input_bytes.len().div_ceil(3) * 4;
    let image = decode(input_bytes)?;

    let original_width = image.width();
    let original_height = image.height();
    let format = mime_type.split('/').nth(1).unwrap_or("png");

    if original_width <= options.max_width
        && original_height <= options.max_height
        && input_base64_size < options.max_bytes
    {
        return Some(ResizedImage {
            data: base64_encode(input_bytes),
            mime_type: if mime_type.is_empty() {
                format!("image/{format}")
            } else {
                mime_type.to_owned()
            },
            original_width,
            original_height,
            width: original_width,
            height: original_height,
            was_resized: false,
        });
    }

    let mut target_width = original_width;
    let mut target_height = original_height;
    if target_width > options.max_width {
        target_height = ((f64::from(target_height) * f64::from(options.max_width)
            / f64::from(target_width))
        .round()) as u32;
        target_width = options.max_width;
    }
    if target_height > options.max_height {
        target_width = ((f64::from(target_width) * f64::from(options.max_height)
            / f64::from(target_height))
        .round()) as u32;
        target_height = options.max_height;
    }

    let mut quality_steps: Vec<u8> = Vec::new();
    for quality in [options.jpeg_quality, 85, 70, 55, 40] {
        if !quality_steps.contains(&quality) {
            quality_steps.push(quality);
        }
    }

    let mut current_width = target_width.max(1);
    let mut current_height = target_height.max(1);
    loop {
        let resized = image.resize_exact(
            current_width,
            current_height,
            image::imageops::FilterType::Lanczos3,
        );
        let mut candidates: Vec<EncodedCandidate> = Vec::new();
        if let Some(png) = encode(&resized, ImageFormat::Png, None) {
            candidates.push(encode_candidate(&png, "image/png"));
        }
        for quality in &quality_steps {
            if let Some(jpeg) = encode(&resized, ImageFormat::Jpeg, Some(*quality)) {
                candidates.push(encode_candidate(&jpeg, "image/jpeg"));
            }
        }
        for candidate in candidates {
            if candidate.encoded_size < options.max_bytes {
                return Some(ResizedImage {
                    data: candidate.data,
                    mime_type: candidate.mime_type.to_owned(),
                    original_width,
                    original_height,
                    width: current_width,
                    height: current_height,
                    was_resized: true,
                });
            }
        }

        if current_width == 1 && current_height == 1 {
            break;
        }
        let next_width = if current_width == 1 {
            1
        } else {
            ((current_width as f64) * 0.75).floor().max(1.0) as u32
        };
        let next_height = if current_height == 1 {
            1
        } else {
            ((current_height as f64) * 0.75).floor().max(1.0) as u32
        };
        if next_width == current_width && next_height == current_height {
            break;
        }
        current_width = next_width;
        current_height = next_height;
    }
    None
}

/// Tell the model how resized coordinates map back to the original image.
pub fn format_dimension_note(result: &ResizedImage) -> Option<String> {
    if !result.was_resized {
        return None;
    }
    let scale = f64::from(result.original_width) / f64::from(result.width);
    Some(format!(
        "[Image: original {}x{}, displayed at {}x{}. Multiply coordinates by {scale:.2} to map to original image.]",
        result.original_width, result.original_height, result.width, result.height
    ))
}

fn conversion_hint(from: Option<&str>, to: &str) -> Option<String> {
    let from = from?;
    (from != to).then(|| format!("[Image converted from {from} to {to}.]"))
}

struct NormalizedImage {
    bytes: Vec<u8>,
    mime_type: String,
    converted_from: Option<String>,
}

fn normalize_image(bytes: &[u8], mime_type: &str) -> Option<NormalizedImage> {
    if let Some(normalized) = normalize_supported_image_mime_type(mime_type) {
        return Some(NormalizedImage {
            bytes: bytes.to_vec(),
            mime_type: normalized.to_owned(),
            converted_from: None,
        });
    }
    let png_bytes = convert_image_bytes_to_png(bytes)?;
    Some(NormalizedImage {
        bytes: png_bytes,
        mime_type: "image/png".to_owned(),
        converted_from: Some(base_mime_type(mime_type)),
    })
}

/// `processImage(bytes, mimeType, { autoResizeImages })`.
pub fn process_image(
    bytes: &[u8],
    mime_type: &str,
    auto_resize_images: bool,
    resize_options: Option<ImageResizeOptions>,
) -> ProcessImageResult {
    let Some(normalized) = normalize_image(bytes, mime_type) else {
        return ProcessImageResult::Failed {
            message: "[Image omitted: could not be converted to a supported inline image format.]"
                .to_owned(),
        };
    };

    if auto_resize_images {
        let Some(resized) = resize_image(&normalized.bytes, &normalized.mime_type, resize_options)
        else {
            return ProcessImageResult::Failed {
                message: "[Image omitted: could not be resized below the inline image size limit.]"
                    .to_owned(),
            };
        };
        let mut hints: Vec<String> = Vec::new();
        if let Some(hint) =
            conversion_hint(normalized.converted_from.as_deref(), &resized.mime_type)
        {
            hints.push(hint);
        }
        if let Some(note) = format_dimension_note(&resized) {
            hints.push(note);
        }
        return ProcessImageResult::Ok {
            data: resized.data,
            mime_type: resized.mime_type,
            hints,
        };
    }

    let mut hints: Vec<String> = Vec::new();
    if let Some(hint) = conversion_hint(normalized.converted_from.as_deref(), &normalized.mime_type)
    {
        hints.push(hint);
    }
    ProcessImageResult::Ok {
        data: base64_encode(&normalized.bytes),
        mime_type: normalized.mime_type,
        hints,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_bytes(width: u32, height: u32) -> Vec<u8> {
        let image = DynamicImage::new_rgb8(width, height);
        encode(&image, ImageFormat::Png, None).expect("encode")
    }

    #[test]
    fn keeps_a_small_image_untouched() {
        let bytes = png_bytes(10, 10);
        let result = process_image(&bytes, "image/png", true, None);
        let ProcessImageResult::Ok {
            data,
            mime_type,
            hints,
        } = result
        else {
            panic!("expected an image");
        };
        assert_eq!(mime_type, "image/png");
        assert_eq!(data, base64_encode(&bytes));
        assert!(hints.is_empty());
    }

    #[test]
    fn converts_to_png_only_when_it_has_to() {
        let png = base64_encode(&png_bytes(4, 4));
        assert_eq!(
            convert_to_png(&png, "image/png"),
            Some((png.clone(), "image/png".to_owned())),
            "a PNG is passed through untouched"
        );

        let jpeg = base64_encode(
            &encode(&DynamicImage::new_rgb8(4, 4), ImageFormat::Jpeg, Some(80)).expect("encode"),
        );
        let (data, mime_type) = convert_to_png(&jpeg, "image/jpeg").expect("converted");
        assert_eq!(mime_type, "image/png");
        assert!(data != jpeg);

        assert_eq!(convert_to_png("not base64 @@@", "image/jpeg"), None);
        assert_eq!(
            convert_to_png(&base64_encode(b"not an image"), "image/jpeg"),
            None
        );
    }

    #[test]
    fn resizes_an_image_beyond_the_dimension_limit() {
        let bytes = png_bytes(2400, 1200);
        let resized = resize_image(&bytes, "image/png", None).expect("resized");
        assert!(resized.was_resized);
        assert_eq!((resized.width, resized.height), (2000, 1000));
        assert_eq!(
            (resized.original_width, resized.original_height),
            (2400, 1200)
        );
        let note = format_dimension_note(&resized).expect("note");
        assert_eq!(
            note,
            "[Image: original 2400x1200, displayed at 2000x1000. Multiply coordinates by 1.20 to map to original image.]"
        );
    }

    #[test]
    fn reports_the_dimension_note_through_process_image() {
        let bytes = png_bytes(2400, 1200);
        let ProcessImageResult::Ok { hints, .. } = process_image(&bytes, "image/png", true, None)
        else {
            panic!("expected an image");
        };
        assert_eq!(hints.len(), 1);
        assert!(hints[0].starts_with("[Image: original 2400x1200"));
    }

    #[test]
    fn converts_an_unsupported_type_to_png() {
        // BMP is sniffed as an image but is not an inline provider format.
        let image = DynamicImage::new_rgb8(4, 4);
        let bmp = encode(&image, ImageFormat::Bmp, None).expect("encode");
        let ProcessImageResult::Ok {
            mime_type, hints, ..
        } = process_image(&bmp, "image/bmp", false, None)
        else {
            panic!("expected an image");
        };
        assert_eq!(mime_type, "image/png");
        assert_eq!(
            hints,
            vec!["[Image converted from image/bmp to image/png.]".to_owned()]
        );
    }

    #[test]
    fn reports_content_that_cannot_be_decoded() {
        let result = process_image(b"not an image", "application/octet-stream", true, None);
        assert_eq!(
            result,
            ProcessImageResult::Failed {
                message:
                    "[Image omitted: could not be converted to a supported inline image format.]"
                        .to_owned()
            }
        );
    }

    #[test]
    fn shrinks_until_the_payload_fits() {
        let bytes = png_bytes(400, 400);
        // A tiny budget forces the resize loop down to very small dimensions.
        let options = ImageResizeOptions {
            max_bytes: 400,
            ..ImageResizeOptions::default()
        };
        let resized = resize_image(&bytes, "image/png", Some(options)).expect("resized");
        assert!(resized.was_resized);
        assert!(resized.data.len() < 400);
        assert!(resized.width < 400);
    }

    #[test]
    fn without_auto_resize_the_bytes_are_passed_through() {
        let bytes = png_bytes(2400, 1200);
        let ProcessImageResult::Ok {
            data,
            mime_type,
            hints,
        } = process_image(&bytes, "image/png", false, None)
        else {
            panic!("expected an image");
        };
        assert_eq!(mime_type, "image/png");
        assert_eq!(data, base64_encode(&bytes));
        assert!(hints.is_empty());
    }
}
