//! Inline image component.
//!
//! 1:1 port of `packages/tui/src/components/image.ts` (127 LOC).

use std::rc::Rc;

use crate::terminal_image::{
    ImageDimensions, ImageProtocol, ImageRenderOptions, allocate_image_id, get_capabilities,
    get_cell_dimensions, get_image_dimensions, image_fallback, render_image,
};
use crate::tui::Component;
use crate::utils::truncate_to_width;

/// Colouring of the text fallback.
pub struct ImageTheme {
    /// Applied to the fallback line.
    pub fallback_color: Rc<dyn Fn(&str) -> String>,
}

/// Options of an [`Image`].
#[derive(Debug, Clone, Default)]
pub struct ImageOptions {
    /// Maximum width in cells (default 60).
    pub max_width_cells: Option<usize>,
    /// Maximum height in cells.
    pub max_height_cells: Option<usize>,
    /// File name shown in the fallback.
    pub filename: Option<String>,
    /// Kitty image id to reuse (for animations/updates).
    pub image_id: Option<u32>,
}

/// Renders an image inline, reserving `rows` lines.
pub struct Image {
    base64_data: String,
    mime_type: String,
    dimensions: ImageDimensions,
    theme: ImageTheme,
    options: ImageOptions,
    image_id: Option<u32>,
    cached_lines: Option<Vec<String>>,
    cached_width: Option<usize>,
}

impl Image {
    /// New image component.
    pub fn new(
        base64_data: impl Into<String>,
        mime_type: impl Into<String>,
        theme: ImageTheme,
        options: ImageOptions,
        dimensions: Option<ImageDimensions>,
    ) -> Self {
        let base64_data = base64_data.into();
        let mime_type = mime_type.into();
        let dimensions = dimensions
            .or_else(|| get_image_dimensions(&base64_data, &mime_type))
            .unwrap_or(ImageDimensions {
                width_px: 800,
                height_px: 600,
            });
        let image_id = options.image_id;
        Self {
            base64_data,
            mime_type,
            dimensions,
            theme,
            options,
            image_id,
            cached_lines: None,
            cached_width: None,
        }
    }

    /// Kitty image id used by this image, if any.
    pub fn get_image_id(&self) -> Option<u32> {
        self.image_id
    }

    fn fallback_lines(&self, width: usize) -> Vec<String> {
        let fallback = image_fallback(
            &self.mime_type,
            Some(self.dimensions),
            self.options.filename.as_deref(),
        );
        vec![truncate_to_width(
            &(self.theme.fallback_color)(&fallback),
            width,
        )]
    }
}

impl Component for Image {
    fn render(&mut self, width: usize) -> Vec<String> {
        if let Some(lines) = &self.cached_lines
            && self.cached_width == Some(width)
        {
            return lines.clone();
        }

        let max_width = width
            .saturating_sub(2)
            .min(self.options.max_width_cells.unwrap_or(60))
            .max(1);
        let cell_dimensions = get_cell_dimensions();
        let default_max_height = ((max_width as f64 * f64::from(cell_dimensions.width_px))
            / f64::from(cell_dimensions.height_px))
        .ceil()
        .max(1.0) as usize;
        let max_height = self.options.max_height_cells.unwrap_or(default_max_height);

        let capabilities = get_capabilities();
        let lines = match capabilities.images {
            None => self.fallback_lines(width),
            Some(protocol) => {
                if protocol == ImageProtocol::Kitty && self.image_id.is_none() {
                    self.image_id = Some(allocate_image_id());
                }
                let result = render_image(
                    &self.base64_data,
                    self.dimensions,
                    ImageRenderOptions {
                        max_width_cells: Some(max_width),
                        max_height_cells: Some(max_height),
                        image_id: self.image_id,
                        move_cursor: Some(false),
                        ..ImageRenderOptions::default()
                    },
                );
                match result {
                    None => self.fallback_lines(width),
                    Some(result) => {
                        if let Some(image_id) = result.image_id {
                            self.image_id = Some(image_id);
                        }
                        if protocol == ImageProtocol::Kitty {
                            // C=1 prevents cursor movement; the remaining lines
                            // let the TUI account for the image height.
                            let mut lines = vec![result.sequence];
                            lines.extend(std::iter::repeat_n(
                                String::new(),
                                result.rows.saturating_sub(1),
                            ));
                            lines
                        } else {
                            // The first rows-1 lines are cleared before drawing;
                            // the last line moves the cursor up, draws and moves
                            // back down so cursor accounting stays in the scroll
                            // area.
                            let mut lines: Vec<String> =
                                vec![String::new(); result.rows.saturating_sub(1)];
                            let row_offset = result.rows.saturating_sub(1);
                            let move_up = if row_offset > 0 {
                                format!("\x1b[{row_offset}A")
                            } else {
                                String::new()
                            };
                            lines.push(format!("{move_up}{}", result.sequence));
                            lines
                        }
                    }
                }
            }
        };

        self.cached_lines = Some(lines.clone());
        self.cached_width = Some(width);
        lines
    }

    fn invalidate(&mut self) {
        self.cached_lines = None;
        self.cached_width = None;
    }
}
