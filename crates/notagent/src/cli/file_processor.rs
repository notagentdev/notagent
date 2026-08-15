//! Port of `packages/coding-agent/src/cli/file-processor.ts`.
//!
//! `@file` arguments become one text blob of `<file name="…">` blocks plus the
//! image attachments. An unreadable or missing file ends the run right here, as
//! it does in TypeScript: the prompt the user asked for cannot be built, and
//! sending a truncated one would be worse than not sending it.

use std::path::Path;

use notagent_ai::types::ImageContent;

use crate::core::tools::path_utils::resolve_read_path;
use crate::utils::chalk::red;
use crate::utils::image::{ProcessImageResult, process_image};
use crate::utils::mime::detect_supported_image_mime_type_from_file;
use crate::utils::paths::{current_dir, resolve_path_default};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProcessedFiles {
    pub text: String,
    pub images: Vec<ImageContent>,
}

/// Turns the `@file` arguments into prompt text and attachments.
pub async fn process_file_arguments(
    file_args: &[String],
    auto_resize_images: bool,
) -> ProcessedFiles {
    let cwd = current_dir();
    let mut text = String::new();
    let mut images: Vec<ImageContent> = Vec::new();

    for file_arg in file_args {
        // Expands `~` and the Unicode spaces of macOS screenshot names.
        let resolved = resolve_read_path(file_arg, &cwd);
        let absolute_path = resolve_path_default(&resolved, &cwd).unwrap_or(resolved);
        let path = Path::new(&absolute_path);

        let Ok(metadata) = std::fs::metadata(path) else {
            eprintln!(
                "{}",
                red(&format!("Error: File not found: {absolute_path}"))
            );
            std::process::exit(1);
        };

        if metadata.len() == 0 {
            continue;
        }

        let mime_type = detect_supported_image_mime_type_from_file(&absolute_path).await;

        match mime_type {
            Some(mime_type) => {
                let Ok(content) = std::fs::read(path) else {
                    eprintln!(
                        "{}",
                        red(&format!("Error: Could not read file {absolute_path}"))
                    );
                    std::process::exit(1);
                };
                match process_image(&content, mime_type, auto_resize_images, None) {
                    ProcessImageResult::Failed { message } => {
                        text.push_str(&format!(
                            "<file name=\"{absolute_path}\">{message}</file>\n"
                        ));
                    }
                    ProcessImageResult::Ok {
                        data,
                        mime_type,
                        hints,
                    } => {
                        images.push(ImageContent { data, mime_type });
                        if hints.is_empty() {
                            text.push_str(&format!("<file name=\"{absolute_path}\"></file>\n"));
                        } else {
                            text.push_str(&format!(
                                "<file name=\"{absolute_path}\">{}</file>\n",
                                hints.join("\n")
                            ));
                        }
                    }
                }
            }
            None => match std::fs::read_to_string(path) {
                Ok(content) => {
                    text.push_str(&format!(
                        "<file name=\"{absolute_path}\">\n{content}\n</file>\n"
                    ));
                }
                Err(error) => {
                    eprintln!(
                        "{}",
                        red(&format!(
                            "Error: Could not read file {absolute_path}: {error}"
                        ))
                    );
                    std::process::exit(1);
                }
            },
        }
    }

    ProcessedFiles { text, images }
}
