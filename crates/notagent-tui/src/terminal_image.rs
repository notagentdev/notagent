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
