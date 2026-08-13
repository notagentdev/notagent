//! Terminal capability detection and image helpers.
//!
//! Partial port of `packages/tui/src/terminal-image.ts` (657 LOC): the parts the
//! TUI core and both renderers need (capability detection, cell dimensions,
//! image line detection). The image protocols, header parsers and `hyperlink()`
//! follow with task 12 of the workstream plan.

use std::sync::{Mutex, OnceLock};

/// Image protocol supported by the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageProtocol {
    /// Kitty graphics protocol.
    Kitty,
    /// iTerm2 inline images.
    ITerm2,
}

/// What the terminal supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalCapabilities {
    /// Image protocol, or `None` when images are unsupported.
    pub images: Option<ImageProtocol>,
    /// 24-bit color support.
    pub true_color: bool,
    /// OSC 8 hyperlink support.
    pub hyperlinks: bool,
}

/// Pixel size of a terminal cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellDimensions {
    /// Cell width in pixels.
    pub width_px: u32,
    /// Cell height in pixels.
    pub height_px: u32,
}

fn cell_dimensions_cell() -> &'static Mutex<CellDimensions> {
    static CELL: OnceLock<Mutex<CellDimensions>> = OnceLock::new();
    CELL.get_or_init(|| {
        Mutex::new(CellDimensions {
            width_px: 9,
            height_px: 18,
        })
    })
}

/// Current cell dimensions (default 9×18 px, updated by the CSI 16 t response).
pub fn get_cell_dimensions() -> CellDimensions {
    *cell_dimensions_cell()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Store cell dimensions reported by the terminal.
pub fn set_cell_dimensions(dimensions: CellDimensions) {
    *cell_dimensions_cell()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = dimensions;
}

fn capabilities_cell() -> &'static Mutex<Option<TerminalCapabilities>> {
    static CELL: OnceLock<Mutex<Option<TerminalCapabilities>>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(None))
}

/// Whether the attached tmux client forwards OSC 8 hyperlinks.
///
/// tmux only re-emits them when its `client_termfeatures` lists `hyperlinks`
/// and strips them otherwise. Any error falls back to `false`.
fn probe_tmux_hyperlinks() -> bool {
    let Ok(output) = std::process::Command::new("tmux")
        .args(["display-message", "-p", "#{client_termfeatures}"])
        .output()
    else {
        return false;
    };
    String::from_utf8_lossy(&output.stdout)
        .split(',')
        .any(|feature| feature.trim() == "hyperlinks")
}

fn env_lower(name: &str) -> String {
    std::env::var(name).unwrap_or_default().to_lowercase()
}

fn env_set(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|value| !value.is_empty())
}

/// Detect terminal capabilities from the environment.
pub fn detect_capabilities(tmux_forwards_hyperlink: &dyn Fn() -> bool) -> TerminalCapabilities {
    let term_program = env_lower("TERM_PROGRAM");
    let terminal_emulator = env_lower("TERMINAL_EMULATOR");
    let term = env_lower("TERM");
    let color_term = env_lower("COLORTERM");
    let has_true_color_hint = color_term == "truecolor" || color_term == "24bit";
    let is_windows_console = cfg!(target_os = "windows");

    // Image protocols are unreliable under tmux; emit OSC 8 only when tmux
    // confirms it forwards them.
    if env_set("TMUX") || term.starts_with("tmux") {
        return TerminalCapabilities {
            images: None,
            true_color: has_true_color_hint,
            hyperlinks: tmux_forwards_hyperlink(),
        };
    }

    // screen does not forward OSC 8 hyperlinks.
    if term.starts_with("screen") {
        return TerminalCapabilities {
            images: None,
            true_color: has_true_color_hint,
            hyperlinks: false,
        };
    }

    let kitty = TerminalCapabilities {
        images: Some(ImageProtocol::Kitty),
        true_color: true,
        hyperlinks: true,
    };
    if env_set("KITTY_WINDOW_ID") || term_program == "kitty" {
        return kitty;
    }
    if term_program == "ghostty" || term.contains("ghostty") || env_set("GHOSTTY_RESOURCES_DIR") {
        return kitty;
    }
    if env_set("WEZTERM_PANE") || term_program == "wezterm" {
        return kitty;
    }
    // Warp supports the Kitty graphics protocol and OSC 8 hyperlinks.
    if term_program == "warpterminal"
        || env_set("WARP_SESSION_ID")
        || env_set("WARP_TERMINAL_SESSION_UUID")
    {
        return kitty;
    }
    if env_set("ITERM_SESSION_ID") || term_program == "iterm.app" {
        return TerminalCapabilities {
            images: Some(ImageProtocol::ITerm2),
            true_color: true,
            hyperlinks: true,
        };
    }
    if env_set("WT_SESSION") || term_program == "vscode" || term_program == "alacritty" {
        return TerminalCapabilities {
            images: None,
            true_color: true,
            hyperlinks: true,
        };
    }
    if terminal_emulator == "jetbrains-jediterm" {
        return TerminalCapabilities {
            images: None,
            true_color: true,
            hyperlinks: false,
        };
    }
    // Windows Terminal does not always set WT_SESSION. Modern Windows consoles
    // support truecolor; keep hyperlinks off unless positively detected above.
    if is_windows_console {
        return TerminalCapabilities {
            images: None,
            true_color: true,
            hyperlinks: false,
        };
    }
    // Unknown terminal: be conservative. OSC 8 renders invisibly on terminals
    // that swallow it, so default to the legacy `text (url)` behaviour.
    TerminalCapabilities {
        images: None,
        true_color: has_true_color_hint,
        hyperlinks: false,
    }
}

/// Cached terminal capabilities.
pub fn get_capabilities() -> TerminalCapabilities {
    let mut cached = capabilities_cell()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *cached.get_or_insert_with(|| detect_capabilities(&probe_tmux_hyperlinks))
}

/// Drop the capability cache.
pub fn reset_capabilities_cache() {
    *capabilities_cell()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
}

/// Override the cached capabilities (used by tests to exercise both paths).
pub fn set_capabilities(capabilities: TerminalCapabilities) {
    *capabilities_cell()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(capabilities);
}

const KITTY_PREFIX: &str = "\x1b_G";
const ITERM2_PREFIX: &str = "\x1b]1337;File=";

/// Whether a rendered line carries an inline image.
pub fn is_image_line(line: &str) -> bool {
    // Fast path: sequence at line start (single-row images).
    if line.starts_with(KITTY_PREFIX) || line.starts_with(ITERM2_PREFIX) {
        return true;
    }
    // Slow path: sequence elsewhere (multi-row images have a cursor-up prefix).
    line.contains(KITTY_PREFIX) || line.contains(ITERM2_PREFIX)
}

/// Delete a single Kitty image by id.
pub fn delete_kitty_image(image_id: u32) -> String {
    format!("\x1b_Ga=d,d=I,i={image_id},q=2\x1b\\")
}

/// Pixel dimensions of an image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageDimensions {
    /// Width in pixels.
    pub width_px: u32,
    /// Height in pixels.
    pub height_px: u32,
}

/// Size of an image in terminal cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageCellSize {
    /// Width in columns.
    pub columns: usize,
    /// Height in rows.
    pub rows: usize,
}

/// Metadata of a transmitted Kitty image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KittyImageMetadata {
    /// Kitty image id.
    pub image_id: u32,
    /// Placement width in cells.
    pub columns: usize,
    /// Placement height in cells.
    pub rows: usize,
    /// Source width in pixels.
    pub width_px: u32,
    /// Source height in pixels.
    pub height_px: u32,
}

fn kitty_image_registry() -> &'static Mutex<std::collections::HashMap<u32, KittyImageMetadata>> {
    static CELL: OnceLock<Mutex<std::collections::HashMap<u32, KittyImageMetadata>>> =
        OnceLock::new();
    CELL.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

/// Remember the metadata of an encoded image (called by the image encoders).
pub fn register_kitty_image_metadata(metadata: KittyImageMetadata) {
    kitty_image_registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(metadata.image_id, metadata);
}

/// Image id of a Kitty placement line, if it carries one.
fn kitty_image_id(line: &str) -> Option<u32> {
    let start = line.find(KITTY_PREFIX)? + KITTY_PREFIX.len();
    let end = line[start..].find(';')? + start;
    line[start..end].split(',').find_map(|control| {
        control
            .strip_prefix("i=")
            .and_then(|value| value.parse::<u32>().ok())
    })
}

/// Metadata of the image a rendered line places, if it is registered.
pub fn get_kitty_image_metadata(line: &str) -> Option<KittyImageMetadata> {
    let image_id = kitty_image_id(line)?;
    kitty_image_registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&image_id)
        .copied()
}

/// Crop a Kitty placement line to a vertical slice of its rows.
pub fn crop_kitty_image_line(line: &str, hidden_rows: usize, visible_rows: usize) -> String {
    let Some(metadata) = get_kitty_image_metadata(line) else {
        return line.to_string();
    };
    let Some(sequence_start) = line.find(KITTY_PREFIX) else {
        return line.to_string();
    };
    let controls_start = sequence_start + KITTY_PREFIX.len();
    let Some(controls_end) = line[controls_start..].find(';').map(|i| i + controls_start) else {
        return line.to_string();
    };
    if hidden_rows >= metadata.rows || visible_rows == 0 {
        return line.to_string();
    }
    let cropped_rows = visible_rows.min(metadata.rows - hidden_rows);
    if hidden_rows == 0 && cropped_rows == metadata.rows {
        return line.to_string();
    }
    let source_y = (u64::from(metadata.height_px) * hidden_rows as u64) / metadata.rows as u64;
    let source_end = (u64::from(metadata.height_px) * (hidden_rows + cropped_rows) as u64)
        .div_ceil(metadata.rows as u64);
    let source_height = (u64::from(metadata.height_px).min(source_end) - source_y).max(1);

    let mut controls: Vec<String> = line[controls_start..controls_end]
        .split(',')
        .filter(|control| {
            !control.starts_with("y=") && !control.starts_with("h=") && !control.starts_with("r=")
        })
        .map(str::to_string)
        .collect();
    controls.push(format!("y={source_y}"));
    controls.push(format!("h={source_height}"));
    controls.push(format!("r={cropped_rows}"));

    format!(
        "{}{KITTY_PREFIX}{};{}",
        &line[..sequence_start],
        controls.join(","),
        &line[controls_end + 1..]
    )
}

const KITTY_CHUNK_SIZE: usize = 4096;

/// Random Kitty image id in `[1, 0xffffffff]`, avoiding collisions between
/// module instances.
pub fn allocate_image_id() -> u32 {
    // `Math.floor(Math.random() * 0xfffffffe) + 1`
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(1);
    let counter = {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    };
    (nanos.wrapping_mul(2_654_435_761).wrapping_add(counter) % 0xffff_fffe) + 1
}

/// Options of [`encode_kitty`].
#[derive(Debug, Clone, Copy, Default)]
pub struct EncodeKittyOptions {
    /// Placement width in cells.
    pub columns: Option<usize>,
    /// Placement height in cells.
    pub rows: Option<usize>,
    /// Kitty image id.
    pub image_id: Option<u32>,
    /// Whether Kitty applies its default cursor movement (default: true).
    pub move_cursor: Option<bool>,
}

/// Encode base64 image data as a Kitty graphics placement.
pub fn encode_kitty(base64_data: &str, options: EncodeKittyOptions) -> String {
    let mut params: Vec<String> = vec!["a=T".into(), "f=100".into(), "q=2".into()];
    if options.move_cursor == Some(false) {
        params.push("C=1".into());
    }
    if let Some(columns) = options.columns {
        params.push(format!("c={columns}"));
    }
    if let Some(rows) = options.rows {
        params.push(format!("r={rows}"));
    }
    if let Some(image_id) = options.image_id {
        params.push(format!("i={image_id}"));
    }

    if base64_data.len() <= KITTY_CHUNK_SIZE {
        return format!("\x1b_G{};{base64_data}\x1b\\", params.join(","));
    }

    let mut chunks = String::new();
    let mut offset = 0;
    let mut is_first = true;
    while offset < base64_data.len() {
        let end = (offset + KITTY_CHUNK_SIZE).min(base64_data.len());
        let chunk = &base64_data[offset..end];
        let is_last = end >= base64_data.len();
        if is_first {
            chunks.push_str(&format!("\x1b_G{},m=1;{chunk}\x1b\\", params.join(",")));
            is_first = false;
        } else if is_last {
            chunks.push_str(&format!("\x1b_Gm=0;{chunk}\x1b\\"));
        } else {
            chunks.push_str(&format!("\x1b_Gm=1;{chunk}\x1b\\"));
        }
        offset = end;
    }
    chunks
}

/// Delete all visible Kitty images (frees the image data too).
pub fn delete_all_kitty_images() -> String {
    "\x1b_Ga=d,d=A,q=2\x1b\\".to_string()
}

/// Delete all visible Kitty placements, keeping the uploaded image data.
pub fn delete_all_kitty_placements() -> String {
    "\x1b_Ga=d,d=a,q=2\x1b\\".to_string()
}

/// Options of [`encode_iterm2`].
#[derive(Debug, Clone, Default)]
pub struct EncodeITerm2Options {
    /// Width parameter (cells or `"auto"`).
    pub width: Option<String>,
    /// Height parameter (cells or `"auto"`).
    pub height: Option<String>,
    /// File name shown by the terminal.
    pub name: Option<String>,
    /// Whether the aspect ratio is preserved (default: true).
    pub preserve_aspect_ratio: Option<bool>,
    /// Whether the image is inline (default: true).
    pub inline: Option<bool>,
}

/// Encode base64 image data as an iTerm2 inline image.
pub fn encode_iterm2(base64_data: &str, options: EncodeITerm2Options) -> String {
    let inline = usize::from(options.inline != Some(false));
    let decoded_size = base64_decoded_len(base64_data);
    let mut params = vec![format!("inline={inline}"), format!("size={decoded_size}")];
    if let Some(width) = &options.width {
        params.push(format!("width={width}"));
    }
    if let Some(height) = &options.height {
        params.push(format!("height={height}"));
    }
    if let Some(name) = &options.name {
        params.push(format!("name={}", base64_encode(name.as_bytes())));
    }
    if options.preserve_aspect_ratio == Some(false) {
        params.push("preserveAspectRatio=0".to_string());
    }
    format!("\x1b]1337;File={}:{base64_data}\x07", params.join(";"))
}

/// `Buffer.byteLength(data, "base64")`.
fn base64_decoded_len(base64_data: &str) -> usize {
    let trimmed = base64_data.trim_end_matches('=');
    trimmed.len() * 3 / 4
}

fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(triple >> 18) as usize & 0x3f] as char);
        out.push(ALPHABET[(triple >> 12) as usize & 0x3f] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(triple >> 6) as usize & 0x3f] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[triple as usize & 0x3f] as char
        } else {
            '='
        });
    }
    out
}

fn base64_decode(base64_data: &str) -> Option<Vec<u8>> {
    const INVALID: u8 = 0xff;
    let mut table = [INVALID; 256];
    for (index, byte) in b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
        .iter()
        .enumerate()
    {
        table[*byte as usize] = index as u8;
    }
    let mut out = Vec::new();
    let mut buffer = 0u32;
    let mut bits = 0u32;
    for byte in base64_data.bytes() {
        if byte == b'=' || byte.is_ascii_whitespace() {
            continue;
        }
        let value = table[byte as usize];
        if value == INVALID {
            return None;
        }
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    Some(out)
}

/// Cell size an image occupies at the given constraints.
pub fn calculate_image_cell_size(
    image_dimensions: ImageDimensions,
    max_width_cells: usize,
    max_height_cells: Option<usize>,
    cell_dimensions: CellDimensions,
) -> ImageCellSize {
    let max_width = max_width_cells.max(1);
    let max_height = max_height_cells.map(|height| height.max(1));
    let image_width = f64::from(image_dimensions.width_px.max(1));
    let image_height = f64::from(image_dimensions.height_px.max(1));

    let width_scale = (max_width as f64 * f64::from(cell_dimensions.width_px)) / image_width;
    let height_scale = max_height.map_or(width_scale, |height| {
        (height as f64 * f64::from(cell_dimensions.height_px)) / image_height
    });
    let scale = width_scale.min(height_scale);

    let scaled_width_px = image_width * scale;
    let scaled_height_px = image_height * scale;
    let columns = (scaled_width_px / f64::from(cell_dimensions.width_px)).ceil() as usize;
    let rows = (scaled_height_px / f64::from(cell_dimensions.height_px)).ceil() as usize;

    ImageCellSize {
        columns: columns.min(max_width).max(1),
        rows: max_height.map_or(rows, |height| rows.min(height)).max(1),
    }
}

/// Rows an image occupies at a target width.
pub fn calculate_image_rows(
    image_dimensions: ImageDimensions,
    target_width_cells: usize,
    cell_dimensions: CellDimensions,
) -> usize {
    calculate_image_cell_size(image_dimensions, target_width_cells, None, cell_dimensions).rows
}

/// PNG dimensions from base64 data.
pub fn get_png_dimensions(base64_data: &str) -> Option<ImageDimensions> {
    let buffer = base64_decode(base64_data)?;
    if buffer.len() < 24 || buffer[0..4] != [0x89, 0x50, 0x4e, 0x47] {
        return None;
    }
    Some(ImageDimensions {
        width_px: u32::from_be_bytes([buffer[16], buffer[17], buffer[18], buffer[19]]),
        height_px: u32::from_be_bytes([buffer[20], buffer[21], buffer[22], buffer[23]]),
    })
}

/// JPEG dimensions from base64 data.
pub fn get_jpeg_dimensions(base64_data: &str) -> Option<ImageDimensions> {
    let buffer = base64_decode(base64_data)?;
    if buffer.len() < 2 || buffer[0] != 0xff || buffer[1] != 0xd8 {
        return None;
    }
    let mut offset = 2;
    while offset + 9 < buffer.len() {
        if buffer[offset] != 0xff {
            offset += 1;
            continue;
        }
        let marker = buffer[offset + 1];
        if (0xc0..=0xc2).contains(&marker) {
            return Some(ImageDimensions {
                height_px: u32::from(u16::from_be_bytes([buffer[offset + 5], buffer[offset + 6]])),
                width_px: u32::from(u16::from_be_bytes([buffer[offset + 7], buffer[offset + 8]])),
            });
        }
        if offset + 3 >= buffer.len() {
            return None;
        }
        let length = u16::from_be_bytes([buffer[offset + 2], buffer[offset + 3]]);
        if length < 2 {
            return None;
        }
        offset += 2 + usize::from(length);
    }
    None
}

/// GIF dimensions from base64 data.
pub fn get_gif_dimensions(base64_data: &str) -> Option<ImageDimensions> {
    let buffer = base64_decode(base64_data)?;
    if buffer.len() < 10 {
        return None;
    }
    let signature = std::str::from_utf8(&buffer[0..6]).ok()?;
    if signature != "GIF87a" && signature != "GIF89a" {
        return None;
    }
    Some(ImageDimensions {
        width_px: u32::from(u16::from_le_bytes([buffer[6], buffer[7]])),
        height_px: u32::from(u16::from_le_bytes([buffer[8], buffer[9]])),
    })
}

/// WebP dimensions from base64 data.
pub fn get_webp_dimensions(base64_data: &str) -> Option<ImageDimensions> {
    let buffer = base64_decode(base64_data)?;
    if buffer.len() < 30 {
        return None;
    }
    if std::str::from_utf8(&buffer[0..4]).ok()? != "RIFF"
        || std::str::from_utf8(&buffer[8..12]).ok()? != "WEBP"
    {
        return None;
    }
    match std::str::from_utf8(&buffer[12..16]).ok()? {
        "VP8 " => Some(ImageDimensions {
            width_px: u32::from(u16::from_le_bytes([buffer[26], buffer[27]]) & 0x3fff),
            height_px: u32::from(u16::from_le_bytes([buffer[28], buffer[29]]) & 0x3fff),
        }),
        "VP8L" => {
            let bits = u32::from_le_bytes([buffer[21], buffer[22], buffer[23], buffer[24]]);
            Some(ImageDimensions {
                width_px: (bits & 0x3fff) + 1,
                height_px: ((bits >> 14) & 0x3fff) + 1,
            })
        }
        "VP8X" => Some(ImageDimensions {
            width_px: (u32::from(buffer[24])
                | (u32::from(buffer[25]) << 8)
                | (u32::from(buffer[26]) << 16))
                + 1,
            height_px: (u32::from(buffer[27])
                | (u32::from(buffer[28]) << 8)
                | (u32::from(buffer[29]) << 16))
                + 1,
        }),
        _ => None,
    }
}

/// Dimensions for a supported mime type.
pub fn get_image_dimensions(base64_data: &str, mime_type: &str) -> Option<ImageDimensions> {
    match mime_type {
        "image/png" => get_png_dimensions(base64_data),
        "image/jpeg" => get_jpeg_dimensions(base64_data),
        "image/gif" => get_gif_dimensions(base64_data),
        "image/webp" => get_webp_dimensions(base64_data),
        _ => None,
    }
}

/// Options of [`render_image`].
#[derive(Debug, Clone, Copy, Default)]
pub struct ImageRenderOptions {
    /// Maximum width in cells (default 80).
    pub max_width_cells: Option<usize>,
    /// Maximum height in cells.
    pub max_height_cells: Option<usize>,
    /// Whether the aspect ratio is preserved.
    pub preserve_aspect_ratio: Option<bool>,
    /// Kitty image id to reuse.
    pub image_id: Option<u32>,
    /// Whether Kitty applies its default cursor movement.
    pub move_cursor: Option<bool>,
}

/// A rendered inline image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedImage {
    /// Escape sequence drawing the image.
    pub sequence: String,
    /// Width in cells.
    pub columns: usize,
    /// Height in cells.
    pub rows: usize,
    /// Kitty image id, when one was used.
    pub image_id: Option<u32>,
}

/// Render an image with the terminal's protocol, or `None` without support.
pub fn render_image(
    base64_data: &str,
    image_dimensions: ImageDimensions,
    options: ImageRenderOptions,
) -> Option<RenderedImage> {
    let capabilities = get_capabilities();
    let protocol = capabilities.images?;

    let max_width = options.max_width_cells.unwrap_or(80);
    let size = calculate_image_cell_size(
        image_dimensions,
        max_width,
        options.max_height_cells,
        get_cell_dimensions(),
    );

    match protocol {
        ImageProtocol::Kitty => {
            if let Some(image_id) = options.image_id {
                register_kitty_image_metadata(KittyImageMetadata {
                    image_id,
                    columns: size.columns,
                    rows: size.rows,
                    width_px: image_dimensions.width_px,
                    height_px: image_dimensions.height_px,
                });
            }
            let sequence = encode_kitty(
                base64_data,
                EncodeKittyOptions {
                    columns: Some(size.columns),
                    rows: Some(size.rows),
                    image_id: options.image_id,
                    move_cursor: options.move_cursor,
                },
            );
            Some(RenderedImage {
                sequence,
                columns: size.columns,
                rows: size.rows,
                image_id: options.image_id,
            })
        }
        ImageProtocol::ITerm2 => {
            let sequence = encode_iterm2(
                base64_data,
                EncodeITerm2Options {
                    width: Some(size.columns.to_string()),
                    height: Some("auto".to_string()),
                    preserve_aspect_ratio: Some(options.preserve_aspect_ratio.unwrap_or(true)),
                    ..EncodeITerm2Options::default()
                },
            );
            Some(RenderedImage {
                sequence,
                columns: size.columns,
                rows: size.rows,
                image_id: None,
            })
        }
    }
}

/// Wrap text in an OSC 8 hyperlink sequence.
pub fn hyperlink(text: &str, url: &str) -> String {
    format!("\x1b]8;;{url}\x1b\\{text}\x1b]8;;\x1b\\")
}

/// Shorten home-prefixed absolute paths to `~/…`.
fn shorten_image_path(filename: &str) -> String {
    let Ok(home) = std::env::var("HOME") else {
        return filename.to_string();
    };
    if home.is_empty() {
        return filename.to_string();
    }
    if filename == home
        || filename.starts_with(&format!("{home}/"))
        || filename.starts_with(&format!("{home}\\"))
    {
        return format!("~{}", &filename[home.len()..]);
    }
    filename.to_string()
}

/// Text fallback when the terminal cannot render inline images.
pub fn image_fallback(
    mime_type: &str,
    dimensions: Option<ImageDimensions>,
    filename: Option<&str>,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(filename) = filename {
        let display = shorten_image_path(filename);
        if get_capabilities().hyperlinks && std::path::Path::new(filename).is_absolute() {
            parts.push(hyperlink(&display, &format!("file://{filename}")));
        } else {
            parts.push(display);
        }
    }
    parts.push(format!("[{mime_type}]"));
    if let Some(dimensions) = dimensions {
        parts.push(format!("{}x{}", dimensions.width_px, dimensions.height_px));
    }
    format!("[Image: {}]", parts.join(" "))
}
