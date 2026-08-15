//! Port of `packages/coding-agent/src/core/export-html/ansi-to-html.ts` (258 LOC).
//!
//! Converts terminal ANSI color/style codes to HTML with inline styles.
//! Supports:
//! - Standard foreground colors (30-37) and bright variants (90-97)
//! - Standard background colors (40-47) and bright variants (100-107)
//! - 256-color palette (38;5;N and 48;5;N)
//! - RGB true color (38;2;R;G;B and 48;2;R;G;B)
//! - Text styles: bold (1), dim (2), italic (3), underline (4)
//! - Reset (0)

use std::sync::LazyLock;

use regex::Regex;

/// Standard ANSI color palette (0-15).
const ANSI_COLORS: [&str; 16] = [
    "#000000", // 0: black
    "#800000", // 1: red
    "#008000", // 2: green
    "#808000", // 3: yellow
    "#000080", // 4: blue
    "#800080", // 5: magenta
    "#008080", // 6: cyan
    "#c0c0c0", // 7: white
    "#808080", // 8: bright black
    "#ff0000", // 9: bright red
    "#00ff00", // 10: bright green
    "#ffff00", // 11: bright yellow
    "#0000ff", // 12: bright blue
    "#ff00ff", // 13: bright magenta
    "#00ffff", // 14: bright cyan
    "#ffffff", // 15: bright white
];

/// Convert 256-color index to hex.
fn color256_to_hex(index: i64) -> String {
    // Standard colors (0-15)
    if index < 16 {
        // A negative index reads past the array in JavaScript and yields
        // `undefined`, which the caller stores as the style colour. Rust has no
        // `undefined`; the parser below never produces a negative index because
        // `parseInt` failures become 0.
        return ANSI_COLORS[index.max(0) as usize].to_owned();
    }

    // Color cube (16-231): 6x6x6 = 216 colors
    if index < 232 {
        let cube_index = index - 16;
        let red = cube_index / 36;
        let green = (cube_index % 36) / 6;
        let blue = cube_index % 6;
        let to_component = |value: i64| if value == 0 { 0 } else { 55 + value * 40 };
        let to_hex = |value: i64| format!("{:02x}", to_component(value));
        return format!("#{}{}{}", to_hex(red), to_hex(green), to_hex(blue));
    }

    // Grayscale (232-255): 24 shades
    let gray = 8 + (index - 232) * 10;
    let gray_hex = format!("{gray:02x}");
    format!("#{gray_hex}{gray_hex}{gray_hex}")
}

/// Escape HTML special characters.
fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#039;")
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct TextStyle {
    fg: Option<String>,
    bg: Option<String>,
    bold: bool,
    dim: bool,
    italic: bool,
    underline: bool,
}

fn style_to_inline_css(style: &TextStyle) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(fg) = &style.fg {
        parts.push(format!("color:{fg}"));
    }
    if let Some(bg) = &style.bg {
        parts.push(format!("background-color:{bg}"));
    }
    if style.bold {
        parts.push("font-weight:bold".to_owned());
    }
    if style.dim {
        parts.push("opacity:0.6".to_owned());
    }
    if style.italic {
        parts.push("font-style:italic".to_owned());
    }
    if style.underline {
        parts.push("text-decoration:underline".to_owned());
    }
    parts.join(";")
}

fn has_style(style: &TextStyle) -> bool {
    style.fg.is_some()
        || style.bg.is_some()
        || style.bold
        || style.dim
        || style.italic
        || style.underline
}

/// Parse ANSI SGR (Select Graphic Rendition) codes and update style.
fn apply_sgr_code(params: &[i64], style: &mut TextStyle) {
    let mut index = 0usize;
    while index < params.len() {
        let code = params[index];

        if code == 0 {
            // Reset all
            *style = TextStyle::default();
        } else if code == 1 {
            style.bold = true;
        } else if code == 2 {
            style.dim = true;
        } else if code == 3 {
            style.italic = true;
        } else if code == 4 {
            style.underline = true;
        } else if code == 22 {
            // Reset bold/dim
            style.bold = false;
            style.dim = false;
        } else if code == 23 {
            style.italic = false;
        } else if code == 24 {
            style.underline = false;
        } else if (30..=37).contains(&code) {
            // Standard foreground colors
            style.fg = Some(ANSI_COLORS[(code - 30) as usize].to_owned());
        } else if code == 38 {
            // Extended foreground color
            if params.get(index + 1) == Some(&5) && params.len() > index + 2 {
                // 256-color: 38;5;N
                style.fg = Some(color256_to_hex(params[index + 2]));
                index += 2;
            } else if params.get(index + 1) == Some(&2) && params.len() > index + 4 {
                // RGB: 38;2;R;G;B
                style.fg = Some(format!(
                    "rgb({},{},{})",
                    params[index + 2],
                    params[index + 3],
                    params[index + 4]
                ));
                index += 4;
            }
        } else if code == 39 {
            // Default foreground
            style.fg = None;
        } else if (40..=47).contains(&code) {
            // Standard background colors
            style.bg = Some(ANSI_COLORS[(code - 40) as usize].to_owned());
        } else if code == 48 {
            // Extended background color
            if params.get(index + 1) == Some(&5) && params.len() > index + 2 {
                // 256-color: 48;5;N
                style.bg = Some(color256_to_hex(params[index + 2]));
                index += 2;
            } else if params.get(index + 1) == Some(&2) && params.len() > index + 4 {
                // RGB: 48;2;R;G;B
                style.bg = Some(format!(
                    "rgb({},{},{})",
                    params[index + 2],
                    params[index + 3],
                    params[index + 4]
                ));
                index += 4;
            }
        } else if code == 49 {
            // Default background
            style.bg = None;
        } else if (90..=97).contains(&code) {
            // Bright foreground colors
            style.fg = Some(ANSI_COLORS[(code - 90 + 8) as usize].to_owned());
        } else if (100..=107).contains(&code) {
            // Bright background colors
            style.bg = Some(ANSI_COLORS[(code - 100 + 8) as usize].to_owned());
        }
        // Ignore unrecognized codes

        index += 1;
    }
}

/// Match ANSI escape sequences: ESC[ followed by params and ending with 'm'.
static ANSI_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\x1b\[([\d;]*)m").expect("ansi sgr regex"));

/// `parseInt(part, 10) || 0` — a non-numeric or empty part becomes 0.
fn parse_param(part: &str) -> i64 {
    let digits: String = part
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .collect();
    digits.parse::<i64>().unwrap_or(0)
}

/// Convert ANSI-escaped text to HTML with inline styles.
pub fn ansi_to_html(text: &str) -> String {
    let mut style = TextStyle::default();
    let mut result = String::new();
    let mut last_index = 0usize;
    let mut in_span = false;

    for capture in ANSI_REGEX.captures_iter(text) {
        let whole = capture.get(0).expect("match");
        // Add text before this escape sequence
        let before_text = &text[last_index..whole.start()];
        if !before_text.is_empty() {
            result.push_str(&escape_html(before_text));
        }

        // Parse SGR parameters
        let param_str = capture.get(1).map_or("", |group| group.as_str());
        let params: Vec<i64> = if param_str.is_empty() {
            vec![0]
        } else {
            param_str.split(';').map(parse_param).collect()
        };

        // Close existing span if we have one
        if in_span {
            result.push_str("</span>");
            in_span = false;
        }

        // Apply the codes
        apply_sgr_code(&params, &mut style);

        // Open new span if we have any styling
        if has_style(&style) {
            result.push_str(&format!("<span style=\"{}\">", style_to_inline_css(&style)));
            in_span = true;
        }

        last_index = whole.end();
    }

    // Add remaining text
    let remaining_text = &text[last_index..];
    if !remaining_text.is_empty() {
        result.push_str(&escape_html(remaining_text));
    }

    // Close any open span
    if in_span {
        result.push_str("</span>");
    }

    result
}

/// Convert a slice of ANSI-escaped lines to HTML.
/// Each line is wrapped in a div element.
pub fn ansi_lines_to_html<S: AsRef<str>>(lines: &[S]) -> String {
    lines
        .iter()
        .map(|line| {
            let html = ansi_to_html(line.as_ref());
            let html = if html.is_empty() {
                "&nbsp;".to_owned()
            } else {
                html
            };
            format!("<div class=\"ansi-line\">{html}</div>")
        })
        .collect::<Vec<_>>()
        .join("")
}
