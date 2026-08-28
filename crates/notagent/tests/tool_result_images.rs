use image::{DynamicImage, ImageFormat};
use notagent::utils::tool_result_images::normalize_tool_result_images;
use notagent_ai::types::{ImageContent, TextContent, TextOrImageContent};

const TINY_PNG_BASE64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFBQIAX8jx0gAAAABJRU5ErkJggg==";

fn encode(image: &DynamicImage, format: ImageFormat) -> Vec<u8> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    image.write_to(&mut bytes, format).expect("encode");
    bytes.into_inner()
}

/// `createPng(width, height)`
fn png(width: u32, height: u32) -> Vec<u8> {
    encode(&DynamicImage::new_rgb8(width, height), ImageFormat::Png)
}

/// `createTinyBmp1x1Red24bpp()`
fn bmp() -> Vec<u8> {
    encode(&DynamicImage::new_rgb8(1, 1), ImageFormat::Bmp)
}

fn image_block(bytes: &[u8], mime_type: &str) -> TextOrImageContent {
    use base64::Engine;
    TextOrImageContent::Image(ImageContent {
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
        mime_type: mime_type.to_owned(),
    })
}

fn text_block(text: &str) -> TextOrImageContent {
    TextOrImageContent::Text(TextContent::new(text))
}

/// `readPngDimensions(base64Data)`
fn png_dimensions(data: &str) -> (u32, u32) {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .expect("base64");
    (
        u32::from_be_bytes(bytes[16..20].try_into().expect("width")),
        u32::from_be_bytes(bytes[20..24].try_into().expect("height")),
    )
}

#[test]
fn returns_the_original_array_when_there_are_no_image_blocks() {
    let content = vec![text_block("no images here")];
    assert_eq!(normalize_tool_result_images(&content, true), None);
}

#[test]
fn returns_the_original_array_when_images_are_already_within_limits() {
    let content = vec![
        text_block("screenshot"),
        TextOrImageContent::Image(ImageContent {
            data: TINY_PNG_BASE64.to_owned(),
            mime_type: "image/png".to_owned(),
        }),
    ];
    assert_eq!(normalize_tool_result_images(&content, true), None);
}

#[test]
fn resizes_oversized_images_and_reports_the_original_dimensions() {
    let content = vec![image_block(&png(2400, 4800), "image/png")];
    let normalized = normalize_tool_result_images(&content, true).expect("the content changed");
    assert_eq!(normalized.len(), 2);
    let TextOrImageContent::Image(image) = &normalized[0] else {
        panic!("the image stays the first block");
    };
    let (width, height) = png_dimensions(&image.data);
    assert!(width <= 2000 && height <= 2000, "{width}x{height}");
    let TextOrImageContent::Text(note) = &normalized[1] else {
        panic!("the note follows the image");
    };
    assert!(
        note.text.contains("original 2400x4800"),
        "the note names the original size: {}",
        note.text
    );
}

#[test]
fn leaves_oversized_images_alone_when_auto_resize_is_disabled() {
    let content = vec![image_block(&png(2400, 4800), "image/png")];
    assert_eq!(normalize_tool_result_images(&content, false), None);
}

#[test]
fn converts_unsupported_image_formats_even_when_auto_resize_is_disabled() {
    let content = vec![image_block(&bmp(), "image/bmp")];
    let normalized = normalize_tool_result_images(&content, false).expect("the content changed");
    let TextOrImageContent::Image(image) = &normalized[0] else {
        panic!("the converted image stays the first block");
    };
    assert_eq!(image.mime_type, "image/png");
    assert_eq!(
        normalized[1],
        text_block("[Image converted from image/bmp to image/png.]")
    );
}

#[test]
fn keeps_undecodable_images_instead_of_dropping_tool_output() {
    let content = vec![TextOrImageContent::Image(ImageContent {
        data: "bm90LWFuLWltYWdl".to_owned(),
        mime_type: "image/png".to_owned(),
    })];
    assert_eq!(normalize_tool_result_images(&content, true), None);
}

#[test]
fn preserves_surrounding_text_blocks_and_their_order() {
    let content = vec![
        text_block("before"),
        image_block(&png(2400, 100), "image/png"),
        text_block("after"),
    ];
    let normalized = normalize_tool_result_images(&content, true).expect("the content changed");
    let kinds: Vec<&str> = normalized
        .iter()
        .map(|block| match block {
            TextOrImageContent::Text(_) => "text",
            TextOrImageContent::Image(_) => "image",
        })
        .collect();
    assert_eq!(kinds, vec!["text", "image", "text", "text"]);
    assert_eq!(normalized[0], text_block("before"));
    assert_eq!(normalized[3], text_block("after"));
}
