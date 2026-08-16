//! `cli/file-processor.ts` — the `@file` arguments of the CLI.
//!
//! Without a TypeScript suite of its own (the file is exercised through
//! `main.ts` there), the cases here pin the three shapes it produces: a text
//! block, an image attachment with its empty block, and the skipped empty file.
//! The two `process.exit(1)` paths are deliberately not driven — they end the
//! process in both languages, so a case could only assert them by spawning one.

use image::{DynamicImage, ImageFormat};
use notagent::cli::file_processor::process_file_arguments;

fn write(dir: &std::path::Path, name: &str, bytes: &[u8]) -> String {
    let path = dir.join(name);
    std::fs::write(&path, bytes).expect("writes");
    path.to_string_lossy().into_owned()
}

#[tokio::test]
async fn a_text_file_becomes_one_named_block() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write(dir.path(), "notes.md", b"hello\nworld");

    let processed = process_file_arguments(std::slice::from_ref(&path), true).await;
    assert_eq!(
        processed.text,
        format!("<file name=\"{path}\">\nhello\nworld\n</file>\n")
    );
    assert!(processed.images.is_empty());
}

#[tokio::test]
async fn an_image_becomes_an_attachment_with_an_empty_block() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut bytes = std::io::Cursor::new(Vec::new());
    DynamicImage::new_rgb8(4, 4)
        .write_to(&mut bytes, ImageFormat::Png)
        .expect("encode");
    let path = write(dir.path(), "shot.png", &bytes.into_inner());

    let processed = process_file_arguments(std::slice::from_ref(&path), true).await;
    assert_eq!(processed.text, format!("<file name=\"{path}\"></file>\n"));
    assert_eq!(processed.images.len(), 1);
    assert_eq!(processed.images[0].mime_type, "image/png");
}

#[tokio::test]
async fn an_oversized_image_carries_its_hint_inside_the_block() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut bytes = std::io::Cursor::new(Vec::new());
    DynamicImage::new_rgb8(2400, 100)
        .write_to(&mut bytes, ImageFormat::Png)
        .expect("encode");
    let path = write(dir.path(), "wide.png", &bytes.into_inner());

    let processed = process_file_arguments(std::slice::from_ref(&path), true).await;
    assert!(
        processed.text.contains("original 2400x100"),
        "the resize note is the block's content: {}",
        processed.text
    );
    assert_eq!(processed.images.len(), 1);
}

#[tokio::test]
async fn an_empty_file_is_skipped_and_the_others_keep_their_order() {
    let dir = tempfile::tempdir().expect("temp dir");
    let empty = write(dir.path(), "empty.txt", b"");
    let first = write(dir.path(), "one.txt", b"1");
    let second = write(dir.path(), "two.txt", b"2");

    let processed = process_file_arguments(&[first.clone(), empty, second.clone()], true).await;
    assert_eq!(
        processed.text,
        format!("<file name=\"{first}\">\n1\n</file>\n<file name=\"{second}\">\n2\n</file>\n")
    );
}
