use std::sync::LazyLock;

use regex::Regex;

/// An RGB color with 8 bits per channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RgbColor {
    /// Red channel.
    pub r: u32,
    /// Green channel.
    pub g: u32,
    /// Blue channel.
    pub b: u32,
}

/// The terminal's color-scheme preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalColorScheme {
    /// Dark background.
    Dark,
    /// Light background.
    Light,
}

fn hex_to_rgb(hex: &str) -> RgbColor {
    let normalized = hex.strip_prefix('#').unwrap_or(hex);
    let channel = |range: std::ops::Range<usize>| {
        normalized
            .get(range)
            .and_then(|part| u32::from_str_radix(part, 16).ok())
            .unwrap_or(0)
    };
    RgbColor {
        r: channel(0..2),
        g: channel(2..4),
        b: channel(4..6),
    }
}

fn parse_osc_hex_channel(channel: &str) -> Option<u32> {
    if channel.is_empty() || !channel.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let max = 16f64.powi(i32::try_from(channel.len()).ok()?) - 1.0;
    if max <= 0.0 {
        return None;
    }
    let value = u64::from_str_radix(channel, 16).ok()? as f64;
    Some((value / max * 255.0).round() as u32)
}

static OSC11_BACKGROUND_COLOR_RESPONSE_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)^\x1b\]11;([^\x07\x1b]*)(?:\x07|\x1b\\)$").expect("valid regex")
});
static COLOR_SCHEME_REPORT_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:\x1b\[\?997;(1|2)n)+$").expect("valid regex"));

/// Whether the data is a strict OSC 11 background color response.
pub fn is_osc11_background_color_response(data: &str) -> bool {
    OSC11_BACKGROUND_COLOR_RESPONSE_PATTERN.is_match(data)
}

/// Parse an OSC 11 background color response.
pub fn parse_osc11_background_color(data: &str) -> Option<RgbColor> {
    let captures = OSC11_BACKGROUND_COLOR_RESPONSE_PATTERN.captures(data)?;
    let value = captures.get(1)?.as_str().trim();

    if let Some(hex) = value.strip_prefix('#') {
        if hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return Some(hex_to_rgb(value));
        }
        if hex.len() == 12 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
            let r = parse_osc_hex_channel(&hex[0..4])?;
            let g = parse_osc_hex_channel(&hex[4..8])?;
            let b = parse_osc_hex_channel(&hex[8..12])?;
            return Some(RgbColor { r, g, b });
        }
        return None;
    }

    // `value.replace(/^rgba?:/i, "")`
    let rgb_value = strip_rgb_prefix(value);
    let mut parts = rgb_value.split('/');
    let (red, green, blue) = (parts.next()?, parts.next()?, parts.next()?);
    Some(RgbColor {
        r: parse_osc_hex_channel(red)?,
        g: parse_osc_hex_channel(green)?,
        b: parse_osc_hex_channel(blue)?,
    })
}

fn strip_rgb_prefix(value: &str) -> &str {
    let lowered = value.to_ascii_lowercase();
    for prefix in ["rgba:", "rgb:"] {
        if lowered.starts_with(prefix) {
            return &value[prefix.len()..];
        }
    }
    value
}

/// Parse a `CSI ? 997 ; n` color-scheme report.
/// Repeated reports keep the last value, exactly like the JavaScript capture
/// group semantics of the original.
pub fn parse_terminal_color_scheme_report(data: &str) -> Option<TerminalColorScheme> {
    let captures = COLOR_SCHEME_REPORT_PATTERN.captures(data)?;
    Some(if captures.get(1)?.as_str() == "2" {
        TerminalColorScheme::Light
    } else {
        TerminalColorScheme::Dark
    })
}
