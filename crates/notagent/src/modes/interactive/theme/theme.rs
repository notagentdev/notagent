//! 1:1 port of `packages/coding-agent/src/modes/interactive/theme/theme.ts` (1 335 LOC).
//!
//! Deviations (see `crates/notagent/PARITY.md`):
//! - chalk → direct ANSI sequences (master plan tech substitution). chalk's
//!   nesting and newline handling is reproduced exactly; its TTY colour-level
//!   gating is not, because the port emits ANSI unconditionally like the
//!   hand-written `fg`/`bg` sequences already do in TypeScript.
//! - TypeBox `Compile` → hand-written validator that reproduces the TypeBox
//!   error strings, the schema-declaration error order and the 8-error cap
//!   (verified against the TypeScript implementation).
//! - Built-in themes ship inside the binary (`include_str!`) instead of next to
//!   it (`dist/theme/*.json`) — distribution mechanics.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use notagent_agent::ThinkingLevel;
use notagent_tui::components::markdown::{HighlightCodeFn, StyleFn};
use notagent_tui::components::settings_list::{ColorFn, SelectionAwareColorFn};
use notagent_tui::{
    EditorTheme, MarkdownTheme, RgbColor, SelectListTheme, SettingsListTheme, get_capabilities,
};

use crate::config::{get_custom_themes_dir, get_themes_dir};
use crate::core::source_info::SourceInfo;
use crate::utils::syntax_highlight::{
    HighlightFormatter, HighlightOptions, HighlightTheme, highlight, supports_language,
};

// ============================================================================
// Types & Schema
// ============================================================================

/// Error raised by the theme system. TypeScript throws `Error` with exactly
/// these messages; they are user visible through `setTheme`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ThemeError(pub String);

impl ThemeError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

type Result<T> = std::result::Result<T, ThemeError>;

/// `ColorValueSchema`: a hex string `#rrggbb`, a variable reference, the empty
/// string (terminal default) or a 256-colour palette index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ColorValue {
    /// Hex colour, variable reference or `""`.
    Text(String),
    /// 256-colour palette index.
    Index(u8),
}

/// Insertion-ordered map of colour values — mirrors a JavaScript object so that
/// `Object.entries` order and the spread semantics of
/// `withThemeColorFallbacks` are preserved.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ColorMap(Vec<(String, ColorValue)>);

impl ColorMap {
    /// Look up a key.
    pub fn get(&self, key: &str) -> Option<&ColorValue> {
        self.0
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value)
    }

    /// Insert or overwrite; an existing key keeps its position (JS spread).
    pub fn set(&mut self, key: &str, value: ColorValue) {
        match self.0.iter_mut().find(|(name, _)| name == key) {
            Some(entry) => entry.1 = value,
            None => self.0.push((key.to_string(), value)),
        }
    }

    /// Iterate in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &ColorValue)> {
        self.0.iter().map(|(key, value)| (key, value))
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the map is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// The `export` section of a theme file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ThemeExportJson {
    /// Page background of the HTML export.
    pub page_bg: Option<ColorValue>,
    /// Card background of the HTML export.
    pub card_bg: Option<ColorValue>,
    /// Info-box background of the HTML export.
    pub info_bg: Option<ColorValue>,
}

/// A parsed and validated theme file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThemeJson {
    /// `$schema` reference (editor tooling only).
    pub schema: Option<String>,
    /// Theme name.
    pub name: String,
    /// Reusable colour variables.
    pub vars: ColorMap,
    /// Colour slots.
    pub colors: ColorMap,
    /// Optional HTML export overrides.
    pub export: Option<ThemeExportJson>,
}

/// Foreground colour slots (`ThemeColor` in TypeScript).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[allow(missing_docs)]
pub enum ThemeColor {
    Accent,
    Border,
    BorderAccent,
    BorderMuted,
    Success,
    Error,
    Warning,
    Muted,
    Dim,
    Text,
    ThinkingText,
    /// Label colour of a state badge (port addition, v0.1.12). Absent means
    /// the label follows [`ThemeColor::Text`], which is what every theme did
    /// before the badge style existed.
    BadgeText,
    SearchMatchText,
    UserMessageText,
    CustomMessageText,
    CustomMessageLabel,
    ToolTitle,
    ToolOutput,
    MdHeading,
    MdLink,
    MdLinkUrl,
    MdCode,
    MdCodeBlock,
    MdCodeBlockBorder,
    MdQuote,
    MdQuoteBorder,
    MdHr,
    MdListBullet,
    ToolDiffAdded,
    ToolDiffRemoved,
    ToolDiffContext,
    SyntaxComment,
    SyntaxKeyword,
    SyntaxFunction,
    SyntaxVariable,
    SyntaxString,
    SyntaxNumber,
    SyntaxType,
    SyntaxOperator,
    SyntaxPunctuation,
    ThinkingOff,
    ThinkingMinimal,
    ThinkingLow,
    ThinkingMedium,
    ThinkingHigh,
    ThinkingXhigh,
    ThinkingMax,
    BashMode,
    /// Footer label of the built-in modes (takeover of the reference's mode
    /// palette, user decision 2026-08-18). All optional; a theme that names
    /// none keeps the previous shell-based colours via the fallbacks.
    ModePlan,
    ModeAcceptEdits,
    ModeAuto,
    ModeYolo,
}

impl ThemeColor {
    /// The TypeScript slot name.
    pub fn as_str(self) -> &'static str {
        match self {
            ThemeColor::Accent => "accent",
            ThemeColor::Border => "border",
            ThemeColor::BorderAccent => "borderAccent",
            ThemeColor::BorderMuted => "borderMuted",
            ThemeColor::Success => "success",
            ThemeColor::Error => "error",
            ThemeColor::Warning => "warning",
            ThemeColor::Muted => "muted",
            ThemeColor::Dim => "dim",
            ThemeColor::Text => "text",
            ThemeColor::ThinkingText => "thinkingText",
            ThemeColor::BadgeText => "badgeText",
            ThemeColor::SearchMatchText => "searchMatchText",
            ThemeColor::UserMessageText => "userMessageText",
            ThemeColor::CustomMessageText => "customMessageText",
            ThemeColor::CustomMessageLabel => "customMessageLabel",
            ThemeColor::ToolTitle => "toolTitle",
            ThemeColor::ToolOutput => "toolOutput",
            ThemeColor::MdHeading => "mdHeading",
            ThemeColor::MdLink => "mdLink",
            ThemeColor::MdLinkUrl => "mdLinkUrl",
            ThemeColor::MdCode => "mdCode",
            ThemeColor::MdCodeBlock => "mdCodeBlock",
            ThemeColor::MdCodeBlockBorder => "mdCodeBlockBorder",
            ThemeColor::MdQuote => "mdQuote",
            ThemeColor::MdQuoteBorder => "mdQuoteBorder",
            ThemeColor::MdHr => "mdHr",
            ThemeColor::MdListBullet => "mdListBullet",
            ThemeColor::ToolDiffAdded => "toolDiffAdded",
            ThemeColor::ToolDiffRemoved => "toolDiffRemoved",
            ThemeColor::ToolDiffContext => "toolDiffContext",
            ThemeColor::SyntaxComment => "syntaxComment",
            ThemeColor::SyntaxKeyword => "syntaxKeyword",
            ThemeColor::SyntaxFunction => "syntaxFunction",
            ThemeColor::SyntaxVariable => "syntaxVariable",
            ThemeColor::SyntaxString => "syntaxString",
            ThemeColor::SyntaxNumber => "syntaxNumber",
            ThemeColor::SyntaxType => "syntaxType",
            ThemeColor::SyntaxOperator => "syntaxOperator",
            ThemeColor::SyntaxPunctuation => "syntaxPunctuation",
            ThemeColor::ThinkingOff => "thinkingOff",
            ThemeColor::ThinkingMinimal => "thinkingMinimal",
            ThemeColor::ThinkingLow => "thinkingLow",
            ThemeColor::ThinkingMedium => "thinkingMedium",
            ThemeColor::ThinkingHigh => "thinkingHigh",
            ThemeColor::ThinkingXhigh => "thinkingXhigh",
            ThemeColor::ThinkingMax => "thinkingMax",
            ThemeColor::BashMode => "bashMode",
            ThemeColor::ModePlan => "modePlan",
            ThemeColor::ModeAcceptEdits => "modeAcceptEdits",
            ThemeColor::ModeAuto => "modeAuto",
            ThemeColor::ModeYolo => "modeYolo",
        }
    }

    /// Parse a slot name.
    pub fn from_name(name: &str) -> Option<Self> {
        ALL_THEME_COLORS
            .iter()
            .copied()
            .find(|color| color.as_str() == name)
    }
}

/// Every foreground slot, in schema declaration order.
pub const ALL_THEME_COLORS: [ThemeColor; 52] = [
    ThemeColor::Accent,
    ThemeColor::Border,
    ThemeColor::BorderAccent,
    ThemeColor::BorderMuted,
    ThemeColor::Success,
    ThemeColor::Error,
    ThemeColor::Warning,
    ThemeColor::Muted,
    ThemeColor::Dim,
    ThemeColor::Text,
    ThemeColor::ThinkingText,
    ThemeColor::BadgeText,
    ThemeColor::SearchMatchText,
    ThemeColor::UserMessageText,
    ThemeColor::CustomMessageText,
    ThemeColor::CustomMessageLabel,
    ThemeColor::ToolTitle,
    ThemeColor::ToolOutput,
    ThemeColor::MdHeading,
    ThemeColor::MdLink,
    ThemeColor::MdLinkUrl,
    ThemeColor::MdCode,
    ThemeColor::MdCodeBlock,
    ThemeColor::MdCodeBlockBorder,
    ThemeColor::MdQuote,
    ThemeColor::MdQuoteBorder,
    ThemeColor::MdHr,
    ThemeColor::MdListBullet,
    ThemeColor::ToolDiffAdded,
    ThemeColor::ToolDiffRemoved,
    ThemeColor::ToolDiffContext,
    ThemeColor::SyntaxComment,
    ThemeColor::SyntaxKeyword,
    ThemeColor::SyntaxFunction,
    ThemeColor::SyntaxVariable,
    ThemeColor::SyntaxString,
    ThemeColor::SyntaxNumber,
    ThemeColor::SyntaxType,
    ThemeColor::SyntaxOperator,
    ThemeColor::SyntaxPunctuation,
    ThemeColor::ThinkingOff,
    ThemeColor::ThinkingMinimal,
    ThemeColor::ThinkingLow,
    ThemeColor::ThinkingMedium,
    ThemeColor::ThinkingHigh,
    ThemeColor::ThinkingXhigh,
    ThemeColor::ThinkingMax,
    ThemeColor::BashMode,
    ThemeColor::ModePlan,
    ThemeColor::ModeAcceptEdits,
    ThemeColor::ModeAuto,
    ThemeColor::ModeYolo,
];

/// Background colour slots (`ThemeBg` in TypeScript).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[allow(missing_docs)]
pub enum ThemeBg {
    SelectedBg,
    ScrollbarThumb,
    SearchMatchBg,
    UserMessageBg,
    CustomMessageBg,
    ToolPendingBg,
    ToolSuccessBg,
    ToolErrorBg,
    // The badge fills (port addition, v0.1.12). A block background is chosen
    // to sit behind twenty lines of text, so it is barely tinted; the same
    // value on an eight-character badge shows no state at all. These carry the
    // saturated tone the reference's themes use, and each falls back to its
    // block background for a theme that does not name one.
    ToolPendingBadgeBg,
    ToolSuccessBadgeBg,
    ToolErrorBadgeBg,
    CustomMessageBadgeBg,
}

impl ThemeBg {
    /// The TypeScript slot name.
    pub fn as_str(self) -> &'static str {
        match self {
            ThemeBg::SelectedBg => "selectedBg",
            ThemeBg::ScrollbarThumb => "scrollbarThumb",
            ThemeBg::SearchMatchBg => "searchMatchBg",
            ThemeBg::UserMessageBg => "userMessageBg",
            ThemeBg::CustomMessageBg => "customMessageBg",
            ThemeBg::ToolPendingBg => "toolPendingBg",
            ThemeBg::ToolSuccessBg => "toolSuccessBg",
            ThemeBg::ToolErrorBg => "toolErrorBg",
            ThemeBg::ToolPendingBadgeBg => "toolPendingBadgeBg",
            ThemeBg::ToolSuccessBadgeBg => "toolSuccessBadgeBg",
            ThemeBg::ToolErrorBadgeBg => "toolErrorBadgeBg",
            ThemeBg::CustomMessageBadgeBg => "customMessageBadgeBg",
        }
    }

    /// The badge fill standing for this block background, or the token itself
    /// when it is not one a block wears.
    pub fn badge_fill(self) -> Self {
        match self {
            ThemeBg::ToolPendingBg => ThemeBg::ToolPendingBadgeBg,
            ThemeBg::ToolSuccessBg => ThemeBg::ToolSuccessBadgeBg,
            ThemeBg::ToolErrorBg => ThemeBg::ToolErrorBadgeBg,
            ThemeBg::CustomMessageBg => ThemeBg::CustomMessageBadgeBg,
            other => other,
        }
    }

    /// Parse a slot name.
    pub fn from_name(name: &str) -> Option<Self> {
        ALL_THEME_BGS.iter().copied().find(|bg| bg.as_str() == name)
    }
}

/// Every background slot.
pub const ALL_THEME_BGS: [ThemeBg; 12] = [
    ThemeBg::SelectedBg,
    ThemeBg::ScrollbarThumb,
    ThemeBg::SearchMatchBg,
    ThemeBg::UserMessageBg,
    ThemeBg::CustomMessageBg,
    ThemeBg::ToolPendingBg,
    ThemeBg::ToolSuccessBg,
    ThemeBg::ToolErrorBg,
    ThemeBg::ToolPendingBadgeBg,
    ThemeBg::ToolSuccessBadgeBg,
    ThemeBg::ToolErrorBadgeBg,
    ThemeBg::CustomMessageBadgeBg,
];

/// The two chat-block styles (port addition, user decision 2026-08-17,
/// v0.1.9; takeover of the reference's `BlockStyle`): `Standard` washes the
/// block colour across the full width, `Badge` leads with a state badge and
/// leaves the terminal background untouched. Held as a process global like
/// the theme itself; the `blockStyle` setting feeds it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BlockStyle {
    Standard,
    /// The default (user decision): badge chips instead of filled surfaces.
    #[default]
    Badge,
}

static BLOCK_STYLE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(1);

pub fn block_style() -> BlockStyle {
    if BLOCK_STYLE.load(std::sync::atomic::Ordering::Relaxed) == 0 {
        BlockStyle::Standard
    } else {
        BlockStyle::Badge
    }
}

pub fn set_block_style(style: BlockStyle) {
    BLOCK_STYLE.store(
        match style {
            BlockStyle::Standard => 0,
            BlockStyle::Badge => 1,
        },
        std::sync::atomic::Ordering::Relaxed,
    );
}

/// A state badge: the label uppercased on the state background, the
/// reference's `block_paint::badge`.
///
/// The label carries the theme's ordinary text colour, so it reads like every
/// other word in the transcript and only the fill says which block this is —
/// without naming it, the label would inherit whatever colour was last set and
/// vanish into its own fill. Closed with a full reset so a repaint after it
/// starts clean.
pub fn badge(theme: &Theme, background: ThemeBg, label: &str) -> String {
    format!(
        "{}{}{}\x1b[0m",
        theme.get_bg_ansi(background.badge_fill()),
        badge_text_ansi(theme),
        badge_label(label)
    )
}

/// Like [`badge`], but painted directly in a foreground colour's tone — the
/// thinking block's grey, for instance (the reference's
/// `block_paint::color_badge`). The foreground sequence becomes the badge's
/// background by swapping the ANSI parameter (38 → 48).
pub fn color_badge(theme: &Theme, color: ThemeColor, label: &str) -> String {
    format!(
        "{}{}{}\x1b[0m",
        fg_ansi_as_badge_fill(theme.get_fg_ansi(color)),
        badge_text_ansi(theme),
        badge_label(label)
    )
}

/// The foreground sequence a badge label is painted in: the theme's
/// `badgeText`, which falls back to its ordinary text colour.
fn badge_text_ansi(theme: &Theme) -> &str {
    theme.get_fg_ansi(ThemeColor::BadgeText)
}

/// Badge labels read as words: uppercase with pill padding, underscores
/// become spaces (the reference's `badge_label`).
fn badge_label(label: &str) -> String {
    format!(" {} ", label.to_uppercase().replace('_', " "))
}

/// Serialises the tests that install a theme or a block style.
///
/// Both are process globals, so a test that sets one and a test that reads it
/// have to take the same lock — one guard per module would leave them racing
/// against each other inside the same test binary.
#[cfg(test)]
pub(crate) fn test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A foreground colour sequence (`38;2;…`/`38;5;…`) as the matching
/// background sequence.
fn fg_ansi_as_bg(ansi: &str) -> String {
    ansi.replace("\x1b[38;", "\x1b[48;")
}

/// The RGB triple of a truecolor ANSI sequence (`38;2;r;g;b`/`48;2;r;g;b`).
pub(crate) fn ansi_rgb(ansi: &str) -> Option<(u8, u8, u8)> {
    let start = ansi.find(";2;")? + 3;
    let mut parts = ansi[start..].trim_end_matches('m').split(';');
    let red = parts.next()?.parse().ok()?;
    let green = parts.next()?.parse().ok()?;
    let blue = parts.next()?.parse().ok()?;
    Some((red, green, blue))
}

/// Perceived brightness on 0–255 (Rec. 709 luma).
fn luminance((red, green, blue): (u8, u8, u8)) -> f32 {
    0.2126 * f32::from(red) + 0.7152 * f32::from(green) + 0.0722 * f32::from(blue)
}

/// A foreground colour as a badge fill, dark enough for the label to stay
/// legible.
///
/// A tone picked to be read *as* text on the terminal background is far too
/// light to sit *behind* text — the thinking grey against the label's grey is
/// the case that gave this away — so a light one is pulled toward black until
/// it carries the label. A tone that is already dark passes through, and a
/// palette without truecolor keeps the plain swap.
fn fg_ansi_as_badge_fill(ansi: &str) -> String {
    // Chosen to sit with the block badge fills, whose luminance runs from 14
    // (the error red) to 67 (the compaction violet), so a tone badge does not
    // glare next to them.
    const MAX_FILL_LUMINANCE: f32 = 72.0;
    let Some(rgb) = ansi_rgb(ansi) else {
        return fg_ansi_as_bg(ansi);
    };
    let luminance = luminance(rgb);
    if luminance <= MAX_FILL_LUMINANCE {
        return fg_ansi_as_bg(ansi);
    }
    let scale = MAX_FILL_LUMINANCE / luminance;
    let (red, green, blue) = rgb;
    format!(
        "\x1b[48;2;{};{};{}m",
        (f32::from(red) * scale) as u8,
        (f32::from(green) * scale) as u8,
        (f32::from(blue) * scale) as u8
    )
}

/// Elapsed time for a running badge: invisible below one second — a
/// counting-up ms display is noise (the reference's `format_elapsed_live`).
pub fn format_elapsed_live(elapsed: std::time::Duration) -> Option<String> {
    (elapsed >= std::time::Duration::from_secs(1)).then(|| format_elapsed(elapsed))
}

/// Final badge runtime: sub-second durations read as milliseconds.
pub fn format_elapsed_precise(elapsed: std::time::Duration) -> String {
    if elapsed < std::time::Duration::from_secs(1) {
        format!("{}ms", elapsed.as_millis())
    } else {
        format_elapsed(elapsed)
    }
}

/// The one way a duration is written in this interface: `42s`, `3m 12s`,
/// `1h 4m`.
///
/// Everything that shows a running time goes through here — the badges on
/// blocks, the task roster, the subagent panel, the status line. Three copies
/// of this used to exist and one of them rounded where the others floored, so
/// the same second could read as two different numbers in two places on the
/// same screen.
pub fn format_elapsed(elapsed: std::time::Duration) -> String {
    let total_secs = elapsed.as_secs();
    let (hours, minutes, seconds) = (total_secs / 3600, (total_secs % 3600) / 60, total_secs % 60);
    if hours > 0 {
        format!("{hours}h {minutes}m")
    } else if minutes > 0 {
        format!("{minutes}m {seconds}s")
    } else {
        format!("{seconds}s")
    }
}

/// `bgColorKeys` of `createTheme`.
fn is_bg_color_key(key: &str) -> bool {
    matches!(
        key,
        "selectedBg"
            | "scrollbarThumb"
            | "searchMatchBg"
            | "userMessageBg"
            | "customMessageBg"
            | "toolPendingBg"
            | "toolSuccessBg"
            | "toolErrorBg"
            | "toolPendingBadgeBg"
            | "toolSuccessBadgeBg"
            | "toolErrorBadgeBg"
            | "customMessageBadgeBg"
    )
}

/// Colour depth the theme renders for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorMode {
    /// 24-bit colour (`38;2;r;g;b`).
    TrueColor,
    /// 256-colour palette (`38;5;index`).
    Color256,
}

impl ColorMode {
    /// The TypeScript literal.
    pub fn as_str(self) -> &'static str {
        match self {
            ColorMode::TrueColor => "truecolor",
            ColorMode::Color256 => "256color",
        }
    }
}

// ============================================================================
// Color Utilities
// ============================================================================

/// RGB triple used by the colour conversions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rgb {
    r: u32,
    g: u32,
    b: u32,
}

fn hex_to_rgb(hex: &str) -> Result<Rgb> {
    let cleaned = hex.replace('#', "");
    if cleaned.chars().count() != 6 {
        return Err(ThemeError::new(format!("Invalid hex color: {hex}")));
    }
    // `parseInt(..., 16)` yields NaN for non-hex digits, which TypeScript turns
    // into the same error.
    let parse = |slice: &str| -> Option<u32> { u32::from_str_radix(slice, 16).ok() };
    let (r, g, b) = (
        parse(&cleaned[0..2]),
        parse(&cleaned[2..4]),
        parse(&cleaned[4..6]),
    );
    match (r, g, b) {
        (Some(r), Some(g), Some(b)) => Ok(Rgb { r, g, b }),
        _ => Err(ThemeError::new(format!("Invalid hex color: {hex}"))),
    }
}

/// The 6x6x6 color cube channel values (indices 0-5)
const CUBE_VALUES: [i64; 6] = [0, 95, 135, 175, 215, 255];

/// Grayscale ramp values (indices 232-255, 24 grays from 8 to 238)
fn gray_values() -> [i64; 24] {
    let mut values = [0i64; 24];
    for (index, value) in values.iter_mut().enumerate() {
        *value = 8 + index as i64 * 10;
    }
    values
}

fn find_closest_cube_index(value: i64) -> usize {
    let mut min_dist = i64::MAX;
    let mut min_idx = 0;
    for (index, cube) in CUBE_VALUES.iter().enumerate() {
        let dist = (value - cube).abs();
        if dist < min_dist {
            min_dist = dist;
            min_idx = index;
        }
    }
    min_idx
}

fn find_closest_gray_index(gray: i64) -> usize {
    let mut min_dist = i64::MAX;
    let mut min_idx = 0;
    for (index, value) in gray_values().iter().enumerate() {
        let dist = (gray - value).abs();
        if dist < min_dist {
            min_dist = dist;
            min_idx = index;
        }
    }
    min_idx
}

/// Weighted Euclidean distance (human eye is more sensitive to green)
fn color_distance(r1: i64, g1: i64, b1: i64, r2: i64, g2: i64, b2: i64) -> f64 {
    let dr = (r1 - r2) as f64;
    let dg = (g1 - g2) as f64;
    let db = (b1 - b2) as f64;
    dr * dr * 0.299 + dg * dg * 0.587 + db * db * 0.114
}

fn rgb_to_256(r: i64, g: i64, b: i64) -> u16 {
    // Find closest color in the 6x6x6 cube
    let r_idx = find_closest_cube_index(r);
    let g_idx = find_closest_cube_index(g);
    let b_idx = find_closest_cube_index(b);
    let cube_r = CUBE_VALUES[r_idx];
    let cube_g = CUBE_VALUES[g_idx];
    let cube_b = CUBE_VALUES[b_idx];
    let cube_index = 16 + 36 * r_idx + 6 * g_idx + b_idx;
    let cube_dist = color_distance(r, g, b, cube_r, cube_g, cube_b);

    // Find closest grayscale
    let gray = js_round(0.299 * r as f64 + 0.587 * g as f64 + 0.114 * b as f64);
    let gray_idx = find_closest_gray_index(gray);
    let gray_value = gray_values()[gray_idx];
    let gray_index = 232 + gray_idx;
    let gray_dist = color_distance(r, g, b, gray_value, gray_value, gray_value);

    // Check if color has noticeable saturation (hue matters)
    // If max-min spread is significant, prefer cube to preserve tint
    let max_c = r.max(g).max(b);
    let min_c = r.min(g).min(b);
    let spread = max_c - min_c;

    // Only consider grayscale if color is nearly neutral (spread < 10)
    // AND grayscale is actually closer
    if spread < 10 && gray_dist < cube_dist {
        return gray_index as u16;
    }

    cube_index as u16
}

/// `Math.round`: halves round towards positive infinity.
fn js_round(value: f64) -> i64 {
    (value + 0.5).floor() as i64
}

fn hex_to_256(hex: &str) -> Result<u16> {
    let Rgb { r, g, b } = hex_to_rgb(hex)?;
    Ok(rgb_to_256(r as i64, g as i64, b as i64))
}

fn fg_ansi(color: &ColorValue, mode: ColorMode) -> Result<String> {
    match color {
        ColorValue::Index(index) => Ok(format!("\x1b[38;5;{index}m")),
        ColorValue::Text(text) if text.is_empty() => Ok("\x1b[39m".to_string()),
        ColorValue::Text(text) if text.starts_with('#') => {
            if mode == ColorMode::TrueColor {
                let Rgb { r, g, b } = hex_to_rgb(text)?;
                Ok(format!("\x1b[38;2;{r};{g};{b}m"))
            } else {
                let index = hex_to_256(text)?;
                Ok(format!("\x1b[38;5;{index}m"))
            }
        }
        ColorValue::Text(text) => Err(ThemeError::new(format!("Invalid color value: {text}"))),
    }
}

fn bg_ansi(color: &ColorValue, mode: ColorMode) -> Result<String> {
    match color {
        ColorValue::Index(index) => Ok(format!("\x1b[48;5;{index}m")),
        ColorValue::Text(text) if text.is_empty() => Ok("\x1b[49m".to_string()),
        ColorValue::Text(text) if text.starts_with('#') => {
            if mode == ColorMode::TrueColor {
                let Rgb { r, g, b } = hex_to_rgb(text)?;
                Ok(format!("\x1b[48;2;{r};{g};{b}m"))
            } else {
                let index = hex_to_256(text)?;
                Ok(format!("\x1b[48;5;{index}m"))
            }
        }
        ColorValue::Text(text) => Err(ThemeError::new(format!("Invalid color value: {text}"))),
    }
}

fn resolve_var_refs(
    value: &ColorValue,
    vars: &ColorMap,
    visited: &mut Vec<String>,
) -> Result<ColorValue> {
    let text = match value {
        ColorValue::Index(_) => return Ok(value.clone()),
        ColorValue::Text(text) => text,
    };
    if text.is_empty() || text.starts_with('#') {
        return Ok(value.clone());
    }
    if visited.iter().any(|seen| seen == text) {
        return Err(ThemeError::new(format!(
            "Circular variable reference detected: {text}"
        )));
    }
    let Some(next) = vars.get(text) else {
        return Err(ThemeError::new(format!(
            "Variable reference not found: {text}"
        )));
    };
    visited.push(text.clone());
    let next = next.clone();
    resolve_var_refs(&next, vars, visited)
}

fn resolve_theme_colors(colors: &ColorMap, vars: &ColorMap) -> Result<ColorMap> {
    let mut resolved = ColorMap::default();
    for (key, value) in colors.iter() {
        let mut visited = Vec::new();
        resolved.set(key, resolve_var_refs(value, vars, &mut visited)?);
    }
    Ok(resolved)
}

fn with_theme_color_fallbacks(colors: &ColorMap) -> ColorMap {
    let mut result = colors.clone();
    let thinking_max = colors
        .get("thinkingMax")
        .or_else(|| colors.get("thinkingXhigh"))
        .cloned();
    let scrollbar_thumb = colors
        .get("scrollbarThumb")
        .or_else(|| colors.get("selectedBg"))
        .cloned();
    let search_match_bg = colors
        .get("searchMatchBg")
        .or_else(|| colors.get("selectedBg"))
        .cloned();
    let search_match_text = colors
        .get("searchMatchText")
        .or_else(|| colors.get("text"))
        .cloned();
    if let Some(value) = thinking_max {
        result.set("thinkingMax", value);
    }
    if let Some(value) = scrollbar_thumb {
        result.set("scrollbarThumb", value);
    }
    if let Some(value) = search_match_bg {
        result.set("searchMatchBg", value);
    }
    if let Some(value) = search_match_text {
        result.set("searchMatchText", value);
    }
    result
}

// ============================================================================
// chalk replacement (master plan: chalk -> direct ANSI sequences)
// ============================================================================

/// Reproduces chalk's `applyStyle`: every nested close code is followed by the
/// open code again (`stringReplaceAll` keeps the substring and appends the
/// replacer) and every line is encased separately, so a style never bleeds
/// across a line break.
fn apply_style(open: &str, close: &str, text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    let mut string = text.to_string();
    if string.contains('\x1b') {
        string = string.replace(close, &format!("{close}{open}"));
    }
    if let Some(index) = string.find('\n') {
        string = encase_crlf_with_first_index(&string, close, open, index);
    }
    format!("{open}{string}{close}")
}

/// Port of chalk's `stringEncaseCRLFWithFirstIndex`.
fn encase_crlf_with_first_index(
    string: &str,
    prefix: &str,
    postfix: &str,
    first_index: usize,
) -> String {
    let bytes = string.as_bytes();
    let mut end_index = 0usize;
    let mut result = String::new();
    let mut index = Some(first_index);
    while let Some(lf) = index {
        let got_cr = lf > 0 && bytes[lf - 1] == b'\r';
        let slice_end = if got_cr { lf - 1 } else { lf };
        result.push_str(&string[end_index..slice_end]);
        result.push_str(prefix);
        result.push_str(if got_cr { "\r\n" } else { "\n" });
        result.push_str(postfix);
        end_index = lf + 1;
        index = string[end_index..]
            .find('\n')
            .map(|offset| end_index + offset);
    }
    result.push_str(&string[end_index..]);
    result
}

// ============================================================================
// Theme Class
// ============================================================================

/// Construction options of [`Theme`].
#[derive(Clone, Debug, Default)]
pub struct ThemeOptions {
    /// Theme name.
    pub name: Option<String>,
    /// Absolute path the theme was loaded from.
    pub source_path: Option<String>,
    /// Where the theme file came from; set by the resource loader.
    pub source_info: Option<SourceInfo>,
}

/// A resolved theme: every slot holds its ready-made ANSI sequence.
#[derive(Clone, Debug)]
pub struct Theme {
    /// Theme name.
    pub name: Option<String>,
    /// Absolute path the theme was loaded from.
    pub source_path: Option<String>,
    /// Where the theme file came from; set by the resource loader and read by
    /// the `/theme` command.
    pub source_info: Option<SourceInfo>,
    fg_colors: HashMap<ThemeColor, String>,
    bg_colors: HashMap<ThemeBg, String>,
    mode: ColorMode,
}

impl Theme {
    /// Build a theme from resolved colour values.
    pub fn new(
        fg_colors: Vec<(ThemeColor, ColorValue)>,
        bg_colors: Vec<(ThemeBg, ColorValue)>,
        mode: ColorMode,
        options: ThemeOptions,
    ) -> Result<Self> {
        let mut colors = fg_colors;
        apply_fg_fallback(
            &mut colors,
            ThemeColor::ThinkingMax,
            ThemeColor::ThinkingXhigh,
        );
        apply_fg_fallback(&mut colors, ThemeColor::SearchMatchText, ThemeColor::Text);
        // A badge label follows the ordinary text colour unless the theme
        // names one of its own — a light theme carrying deep badge fills needs
        // a light label (the reference's `badgeText`).
        apply_fg_fallback(&mut colors, ThemeColor::BadgeText, ThemeColor::Text);
        // A theme without the mode palette keeps the previous shell signal:
        // read-only modes were green, working modes yellow, yolo is a warning
        // by nature.
        apply_fg_fallback(&mut colors, ThemeColor::ModePlan, ThemeColor::Success);
        apply_fg_fallback(&mut colors, ThemeColor::ModeAcceptEdits, ThemeColor::Accent);
        apply_fg_fallback(&mut colors, ThemeColor::ModeAuto, ThemeColor::Warning);
        apply_fg_fallback(&mut colors, ThemeColor::ModeYolo, ThemeColor::Error);
        let mut fg_map = HashMap::new();
        for (key, value) in &colors {
            fg_map.insert(*key, fg_ansi(value, mode)?);
        }

        let mut backgrounds = bg_colors;
        apply_bg_fallback(
            &mut backgrounds,
            ThemeBg::ScrollbarThumb,
            ThemeBg::SelectedBg,
        );
        apply_bg_fallback(
            &mut backgrounds,
            ThemeBg::SearchMatchBg,
            ThemeBg::SelectedBg,
        );
        // A theme that names no badge fill keeps its block background there,
        // which is what every theme did before the badge style existed.
        for block in [
            ThemeBg::ToolPendingBg,
            ThemeBg::ToolSuccessBg,
            ThemeBg::ToolErrorBg,
            ThemeBg::CustomMessageBg,
        ] {
            apply_bg_fallback(&mut backgrounds, block.badge_fill(), block);
        }
        let mut bg_map = HashMap::new();
        for (key, value) in &backgrounds {
            bg_map.insert(*key, bg_ansi(value, mode)?);
        }

        Ok(Self {
            name: options.name,
            source_path: options.source_path,
            source_info: options.source_info,
            fg_colors: fg_map,
            bg_colors: bg_map,
            mode,
        })
    }

    /// Colour `text` with a foreground slot and reset only the foreground.
    ///
    /// Panics with the TypeScript message when the slot is missing — the
    /// TypeScript getter throws an uncaught `Error` in the same situation.
    pub fn fg(&self, color: ThemeColor, text: &str) -> String {
        let ansi = self.get_fg_ansi(color);
        format!("{ansi}{text}\x1b[39m")
    }

    /// Colour `text` with a background slot and reset only the background.
    pub fn bg(&self, color: ThemeBg, text: &str) -> String {
        let ansi = self.get_bg_ansi(color);
        format!("{ansi}{text}\x1b[49m")
    }

    /// Bold text.
    pub fn bold(&self, text: &str) -> String {
        apply_style("\x1b[1m", "\x1b[22m", text)
    }

    /// Italic text.
    pub fn italic(&self, text: &str) -> String {
        apply_style("\x1b[3m", "\x1b[23m", text)
    }

    /// Underlined text.
    pub fn underline(&self, text: &str) -> String {
        apply_style("\x1b[4m", "\x1b[24m", text)
    }

    /// Inverted text.
    pub fn inverse(&self, text: &str) -> String {
        apply_style("\x1b[7m", "\x1b[27m", text)
    }

    /// Struck-through text.
    pub fn strikethrough(&self, text: &str) -> String {
        apply_style("\x1b[9m", "\x1b[29m", text)
    }

    /// The raw foreground sequence of a slot.
    pub fn get_fg_ansi(&self, color: ThemeColor) -> &str {
        match self.fg_colors.get(&color) {
            Some(ansi) => ansi,
            None => panic!("Unknown theme color: {}", color.as_str()),
        }
    }

    /// The channel values of a foreground slot, when the theme was resolved in
    /// 24-bit colour.
    ///
    /// A slot only keeps its ready-made escape sequence, so the numbers are
    /// read back out of it. In the 256-colour mode there are no exact channel
    /// values to recover, and callers that need to mix colours have to do
    /// without.
    pub fn get_fg_rgb(&self, color: ThemeColor) -> Option<(u8, u8, u8)> {
        let ansi = self.fg_colors.get(&color)?;
        let channels = ansi.strip_prefix("\x1b[38;2;")?.strip_suffix('m')?;
        let mut parts = channels.split(';');
        let mut next = || parts.next()?.parse::<u8>().ok();
        let (red, green, blue) = (next()?, next()?, next()?);
        parts.next().is_none().then_some((red, green, blue))
    }

    /// The raw background sequence of a slot.
    pub fn get_bg_ansi(&self, color: ThemeBg) -> &str {
        match self.bg_colors.get(&color) {
            Some(ansi) => ansi,
            None => panic!("Unknown theme background color: {}", color.as_str()),
        }
    }

    /// The colour depth this theme was built for.
    pub fn get_color_mode(&self) -> ColorMode {
        self.mode
    }

    /// Border colour of the editor for a thinking level.
    pub fn get_thinking_border_color(&self, level: ThinkingLevel) -> StyleFn {
        // Map thinking levels to dedicated theme colors
        let color = match level {
            ThinkingLevel::Off => ThemeColor::ThinkingOff,
            ThinkingLevel::Minimal => ThemeColor::ThinkingMinimal,
            ThinkingLevel::Low => ThemeColor::ThinkingLow,
            ThinkingLevel::Medium => ThemeColor::ThinkingMedium,
            ThinkingLevel::High => ThemeColor::ThinkingHigh,
            ThinkingLevel::Xhigh => ThemeColor::ThinkingXhigh,
            ThinkingLevel::Max => ThemeColor::ThinkingMax,
        };
        self.color_fn(color)
    }

    /// Border colour of the editor in bash mode.
    pub fn get_bash_mode_border_color(&self) -> StyleFn {
        self.color_fn(ThemeColor::BashMode)
    }

    /// A closure that applies a foreground slot; the sequence is captured
    /// eagerly because a `Theme` is immutable once built.
    fn color_fn(&self, color: ThemeColor) -> StyleFn {
        let ansi = self.get_fg_ansi(color).to_string();
        Rc::new(move |text: &str| format!("{ansi}{text}\x1b[39m"))
    }
}

fn apply_fg_fallback(
    colors: &mut Vec<(ThemeColor, ColorValue)>,
    key: ThemeColor,
    fallback: ThemeColor,
) {
    if colors.iter().any(|(name, _)| *name == key) {
        return;
    }
    if let Some((_, value)) = colors.iter().find(|(name, _)| *name == fallback) {
        let value = value.clone();
        colors.push((key, value));
    }
}

fn apply_bg_fallback(colors: &mut Vec<(ThemeBg, ColorValue)>, key: ThemeBg, fallback: ThemeBg) {
    if colors.iter().any(|(name, _)| *name == key) {
        return;
    }
    if let Some((_, value)) = colors.iter().find(|(name, _)| *name == fallback) {
        let value = value.clone();
        colors.push((key, value));
    }
}

// ============================================================================
// Theme Loading
// ============================================================================

/// The colour slots of `ThemeJsonSchema` in declaration order; `true` marks the
/// optional ones (`Type.Optional`).
const COLOR_SCHEMA_PROPERTIES: [(&str, bool); 64] = [
    ("accent", false),
    ("border", false),
    ("borderAccent", false),
    ("borderMuted", false),
    ("success", false),
    ("error", false),
    ("warning", false),
    ("muted", false),
    ("dim", false),
    ("text", false),
    ("thinkingText", false),
    // Port additions (v0.1.12), all optional: the badge fills and the badge
    // label colour, each falling back to what a theme already names.
    ("badgeText", true),
    ("selectedBg", false),
    ("scrollbarThumb", true),
    ("searchMatchBg", true),
    ("searchMatchText", true),
    ("userMessageBg", false),
    ("userMessageText", false),
    ("customMessageBg", false),
    ("customMessageText", false),
    ("customMessageLabel", false),
    ("toolPendingBg", false),
    ("toolSuccessBg", false),
    ("toolErrorBg", false),
    ("toolPendingBadgeBg", true),
    ("toolSuccessBadgeBg", true),
    ("toolErrorBadgeBg", true),
    ("customMessageBadgeBg", true),
    ("toolTitle", false),
    ("toolOutput", false),
    ("mdHeading", false),
    ("mdLink", false),
    ("mdLinkUrl", false),
    ("mdCode", false),
    ("mdCodeBlock", false),
    ("mdCodeBlockBorder", false),
    ("mdQuote", false),
    ("mdQuoteBorder", false),
    ("mdHr", false),
    ("mdListBullet", false),
    ("toolDiffAdded", false),
    ("toolDiffRemoved", false),
    ("toolDiffContext", false),
    ("syntaxComment", false),
    ("syntaxKeyword", false),
    ("syntaxFunction", false),
    ("syntaxVariable", false),
    ("syntaxString", false),
    ("syntaxNumber", false),
    ("syntaxType", false),
    ("syntaxOperator", false),
    ("syntaxPunctuation", false),
    ("thinkingOff", false),
    ("thinkingMinimal", false),
    ("thinkingLow", false),
    ("thinkingMedium", false),
    ("thinkingHigh", false),
    ("thinkingXhigh", false),
    ("thinkingMax", true),
    ("bashMode", false),
    // The mode palette (reference takeover, 2026-08-18); optional, with
    // shell-signal fallbacks in `Theme::new`.
    ("modePlan", true),
    ("modeAcceptEdits", true),
    ("modeAuto", true),
    ("modeYolo", true),
];

/// TypeBox's `Errors()` iterator stops after this many errors (verified against
/// the TypeScript implementation).
const MAX_VALIDATION_ERRORS: usize = 8;

/// A single validation error, in the shape `parseThemeJson` consumes.
struct ValidationError {
    instance_path: String,
    message: String,
    /// Set for `required` errors.
    required_properties: Option<Vec<String>>,
}

/// Collects errors and stops once TypeBox's cap is reached.
#[derive(Default)]
struct ErrorSink {
    errors: Vec<ValidationError>,
}

impl ErrorSink {
    fn full(&self) -> bool {
        self.errors.len() >= MAX_VALIDATION_ERRORS
    }

    fn push(&mut self, instance_path: &str, message: impl Into<String>) {
        if self.full() {
            return;
        }
        self.errors.push(ValidationError {
            instance_path: instance_path.to_string(),
            message: message.into(),
            required_properties: None,
        });
    }

    fn push_required(&mut self, instance_path: &str, properties: Vec<String>) {
        if self.full() {
            return;
        }
        let message = format!("must have required properties {}", properties.join(", "));
        self.errors.push(ValidationError {
            instance_path: instance_path.to_string(),
            message,
            required_properties: Some(properties),
        });
    }
}

/// Validates a `ColorValueSchema` union member and mirrors TypeBox's error
/// triple (`must be string`, the integer failure, `must match a schema in
/// anyOf`).
fn check_color_value(value: &serde_json::Value, path: &str, sink: &mut ErrorSink) -> bool {
    if value.is_string() {
        return true;
    }
    let integer_error = match value.as_i64() {
        Some(number) if value.is_i64() || value.is_u64() => {
            if number < 0 {
                Some("must be >= 0")
            } else if number > 255 {
                Some("must be <= 255")
            } else {
                None
            }
        }
        _ => Some("must be integer"),
    };
    let Some(integer_error) = integer_error else {
        return true;
    };
    sink.push(path, "must be string");
    sink.push(path, integer_error);
    sink.push(path, "must match a schema in anyOf");
    false
}

fn color_value_from_json(value: &serde_json::Value) -> ColorValue {
    match value {
        serde_json::Value::String(text) => ColorValue::Text(text.clone()),
        other => ColorValue::Index(other.as_u64().unwrap_or_default() as u8),
    }
}

/// Port of `validateThemeJson.Check`/`Errors` for `ThemeJsonSchema`.
fn validate_theme_json(
    json: &serde_json::Value,
) -> std::result::Result<ThemeJson, Vec<ValidationError>> {
    let mut sink = ErrorSink::default();
    let Some(root) = json.as_object() else {
        sink.push("", "must be object");
        return Err(sink.errors);
    };

    let missing_root: Vec<String> = ["name", "colors"]
        .iter()
        .filter(|key| !root.contains_key(**key))
        .map(|key| (*key).to_string())
        .collect();
    if !missing_root.is_empty() {
        sink.push_required("", missing_root);
    }

    let mut schema = None;
    if let Some(value) = root.get("$schema") {
        match value.as_str() {
            Some(text) => schema = Some(text.to_string()),
            None => sink.push("/$schema", "must be string"),
        }
    }

    let mut name = String::new();
    if let Some(value) = root.get("name") {
        match value.as_str() {
            Some(text) => name = text.to_string(),
            None => sink.push("/name", "must be string"),
        }
    }

    let mut vars = ColorMap::default();
    if let Some(value) = root.get("vars") {
        match value.as_object() {
            Some(entries) => {
                for (key, entry) in entries {
                    if check_color_value(entry, &format!("/vars/{key}"), &mut sink) {
                        vars.set(key, color_value_from_json(entry));
                    }
                }
            }
            None => sink.push("/vars", "must be object"),
        }
    }

    let mut colors = ColorMap::default();
    if let Some(value) = root.get("colors") {
        match value.as_object() {
            Some(entries) => {
                let missing: Vec<String> = COLOR_SCHEMA_PROPERTIES
                    .iter()
                    .filter(|(key, optional)| !optional && !entries.contains_key(*key))
                    .map(|(key, _)| (*key).to_string())
                    .collect();
                if !missing.is_empty() {
                    sink.push_required("/colors", missing);
                }
                // TypeBox validates in schema declaration order, not file order.
                for (key, _) in COLOR_SCHEMA_PROPERTIES.iter() {
                    if let Some(entry) = entries.get(*key) {
                        check_color_value(entry, &format!("/colors/{key}"), &mut sink);
                    }
                }
                // The resolved map keeps the file's insertion order.
                for (key, entry) in entries {
                    if entry.is_string() || entry.is_number() {
                        colors.set(key, color_value_from_json(entry));
                    }
                }
            }
            None => sink.push("/colors", "must be object"),
        }
    }

    let mut export = None;
    if let Some(value) = root.get("export") {
        match value.as_object() {
            Some(entries) => {
                let mut section = ThemeExportJson::default();
                for key in ["pageBg", "cardBg", "infoBg"] {
                    if let Some(entry) = entries.get(key)
                        && check_color_value(entry, &format!("/export/{key}"), &mut sink)
                    {
                        let parsed = Some(color_value_from_json(entry));
                        match key {
                            "pageBg" => section.page_bg = parsed,
                            "cardBg" => section.card_bg = parsed,
                            _ => section.info_bg = parsed,
                        }
                    }
                }
                export = Some(section);
            }
            None => sink.push("/export", "must be object"),
        }
    }

    if !sink.errors.is_empty() {
        return Err(sink.errors);
    }
    Ok(ThemeJson {
        schema,
        name,
        vars,
        colors,
        export,
    })
}

fn builtin_themes() -> &'static HashMap<String, ThemeJson> {
    static BUILTIN_THEMES: OnceLock<HashMap<String, ThemeJson>> = OnceLock::new();
    BUILTIN_THEMES.get_or_init(|| {
        // Distribution mechanics: the built-in themes ship inside the binary
        // instead of in `dist/theme/` next to it.
        let dark = include_str!("dark.json");
        let light = include_str!("light.json");
        let mut themes = HashMap::new();
        themes.insert(
            "dark".to_string(),
            parse_theme_json_content("dark", dark).expect("built-in dark theme is valid"),
        );
        themes.insert(
            "light".to_string(),
            parse_theme_json_content("light", light).expect("built-in light theme is valid"),
        );
        themes
    })
}

/// Built-in theme names in the order `Object.keys` yields them.
const BUILTIN_THEME_NAMES: [&str; 2] = ["dark", "light"];

/// Names of every theme the picker offers.
pub fn get_available_themes() -> Vec<String> {
    get_available_themes_with_paths()
        .into_iter()
        .map(|info| info.name)
        .collect()
}

/// A theme offered by the picker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThemeInfo {
    /// Theme name (from the file's `name`, not the file name).
    pub name: String,
    /// Path the theme was loaded from, if any.
    pub path: Option<String>,
}

/// Built-in, custom and registered themes, deduplicated and sorted by name.
pub fn get_available_themes_with_paths() -> Vec<ThemeInfo> {
    let themes_dir = get_themes_dir();
    let mut result: Vec<ThemeInfo> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let mut add_theme = |theme_info: ThemeInfo, result: &mut Vec<ThemeInfo>| {
        if seen.contains(&theme_info.name) {
            return;
        }
        seen.push(theme_info.name.clone());
        result.push(theme_info);
    };

    // Built-in themes
    for name in BUILTIN_THEME_NAMES {
        add_theme(
            ThemeInfo {
                name: name.to_string(),
                path: Some(path_to_string(&themes_dir.join(format!("{name}.json")))),
            },
            &mut result,
        );
    }

    // Custom themes
    for theme_info in get_custom_theme_infos() {
        add_theme(theme_info, &mut result);
    }

    for (name, theme) in registered_themes().read().unwrap().iter() {
        add_theme(
            ThemeInfo {
                name: name.clone(),
                path: theme.source_path.clone(),
            },
            &mut result,
        );
    }

    result.sort_by(|a, b| locale_compare(&a.name, &b.name));
    result
}

/// `String.prototype.localeCompare` for the ASCII-dominant theme names: case
/// insensitive first, lowercase before uppercase on ties.
fn locale_compare(a: &str, b: &str) -> std::cmp::Ordering {
    let folded = a.to_lowercase().cmp(&b.to_lowercase());
    if folded != std::cmp::Ordering::Equal {
        return folded;
    }
    b.cmp(a)
}

fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn get_custom_theme_infos() -> Vec<ThemeInfo> {
    let custom_themes_dir = get_custom_themes_dir();
    let mut result = Vec::new();
    if !custom_themes_dir.exists() {
        return result;
    }

    let Ok(entries) = std::fs::read_dir(&custom_themes_dir) else {
        return result;
    };
    let mut files: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    // `fs.readdirSync` returns directory order; sort for a stable listing.
    files.sort();
    for file in files {
        if !file.ends_with(".json") {
            continue;
        }
        let theme_path = custom_themes_dir.join(&file);
        // Invalid themes are ignored here; the resource loader reports them
        // during normal startup/reload.
        if let Ok(custom_theme) = load_theme_from_path(&theme_path, None)
            && let Some(name) = custom_theme.name.clone()
        {
            result.push(ThemeInfo {
                name,
                path: Some(path_to_string(&theme_path)),
            });
        }
    }
    result
}

fn assert_theme_name_is_valid(name: &str) -> Result<()> {
    if name.contains('/') {
        return Err(ThemeError::new(format!(
            "Invalid theme name \"{name}\": theme names cannot contain \"/\" because it is reserved for automatic light/dark theme settings."
        )));
    }
    Ok(())
}

fn parse_theme_json(label: &str, json: &serde_json::Value) -> Result<ThemeJson> {
    let theme_json = match validate_theme_json(json) {
        Ok(theme_json) => theme_json,
        Err(errors) => {
            let mut missing_colors: Vec<String> = Vec::new();
            let mut other_errors: Vec<String> = Vec::new();

            for error in errors {
                if let Some(required_properties) = &error.required_properties
                    && error.instance_path == "/colors"
                {
                    for required_property in required_properties {
                        if !missing_colors.contains(required_property) {
                            missing_colors.push(required_property.clone());
                        }
                    }
                    continue;
                }

                let path = if error.instance_path.is_empty() {
                    "/"
                } else {
                    &error.instance_path
                };
                other_errors.push(format!("  - {path}: {}", error.message));
            }

            let mut error_message = format!("Invalid theme \"{label}\":\n");
            if !missing_colors.is_empty() {
                missing_colors.sort();
                error_message.push_str("\nMissing required color tokens:\n");
                error_message.push_str(
                    &missing_colors
                        .iter()
                        .map(|color| format!("  - {color}"))
                        .collect::<Vec<_>>()
                        .join("\n"),
                );
                error_message
                    .push_str("\n\nPlease add these colors to your theme's \"colors\" object.");
                error_message.push_str(
                    "\nSee the built-in themes (dark.json, light.json) for reference values.",
                );
            }
            if !other_errors.is_empty() {
                error_message.push_str(&format!("\n\nOther errors:\n{}", other_errors.join("\n")));
            }

            return Err(ThemeError::new(error_message));
        }
    };

    assert_theme_name_is_valid(&theme_json.name)?;
    Ok(theme_json)
}

fn parse_theme_json_content(label: &str, content: &str) -> Result<ThemeJson> {
    let json: serde_json::Value = serde_json::from_str(content)
        .map_err(|error| ThemeError::new(format!("Failed to parse theme {label}: {error}")))?;
    parse_theme_json(label, &json)
}

fn load_theme_json(name: &str) -> Result<ThemeJson> {
    if let Some(builtin) = builtin_themes().get(name) {
        return Ok(builtin.clone());
    }
    let registered = registered_themes()
        .read()
        .unwrap()
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, theme)| Arc::clone(theme));
    if let Some(registered_theme) = registered {
        let Some(source_path) = registered_theme.source_path.clone() else {
            return Err(ThemeError::new(format!(
                "Theme \"{name}\" does not have a source path for export"
            )));
        };
        let content = read_to_string(&source_path)?;
        return parse_theme_json_content(&source_path, &content);
    }
    let custom_themes_dir = get_custom_themes_dir();
    let theme_path = custom_themes_dir.join(format!("{name}.json"));
    if !theme_path.exists() {
        return Err(ThemeError::new(format!("Theme not found: {name}")));
    }
    let content = read_to_string(&path_to_string(&theme_path))?;
    parse_theme_json_content(name, &content)
}

/// `fs.readFileSync` — a read failure throws in TypeScript and is turned into a
/// `ThemeError` here (language idiom, no behaviour change).
fn read_to_string(path: &str) -> Result<String> {
    std::fs::read_to_string(path)
        .map_err(|error| ThemeError::new(format!("ENOENT: {error}, open '{path}'")))
}

fn create_theme(
    theme_json: &ThemeJson,
    mode: Option<ColorMode>,
    source_path: Option<String>,
) -> Result<Theme> {
    let color_mode = mode.unwrap_or_else(|| {
        if get_capabilities().true_color {
            ColorMode::TrueColor
        } else {
            ColorMode::Color256
        }
    });
    let resolved_colors = resolve_theme_colors(
        &with_theme_color_fallbacks(&theme_json.colors),
        &theme_json.vars,
    )?;
    let mut fg_colors: Vec<(ThemeColor, ColorValue)> = Vec::new();
    let mut bg_colors: Vec<(ThemeBg, ColorValue)> = Vec::new();
    for (key, value) in resolved_colors.iter() {
        if is_bg_color_key(key) {
            if let Some(slot) = ThemeBg::from_name(key) {
                bg_colors.push((slot, value.clone()));
            }
        } else if let Some(slot) = ThemeColor::from_name(key) {
            fg_colors.push((slot, value.clone()));
        }
        // Unknown keys are not rejected by the schema and are ignored, exactly
        // as the TypeScript `ThemeColor`-typed record does at runtime.
    }
    Theme::new(
        fg_colors,
        bg_colors,
        color_mode,
        ThemeOptions {
            name: Some(theme_json.name.clone()),
            source_path,
            source_info: None,
        },
    )
}

/// Load a theme file from an absolute path.
pub fn load_theme_from_path(theme_path: &Path, mode: Option<ColorMode>) -> Result<Theme> {
    let label = path_to_string(theme_path);
    let content = read_to_string(&label)?;
    let theme_json = parse_theme_json_content(&label, &content)?;
    create_theme(&theme_json, mode, Some(label))
}

fn load_theme(name: &str, mode: Option<ColorMode>) -> Result<Theme> {
    let registered = registered_themes()
        .read()
        .unwrap()
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, theme)| Arc::clone(theme));
    if let Some(registered_theme) = registered {
        return Ok((*registered_theme).clone());
    }
    let theme_json = load_theme_json(name)?;
    create_theme(&theme_json, mode, None)
}

/// Load a theme by name, or `None` when it cannot be loaded.
pub fn get_theme_by_name(name: &str) -> Option<Theme> {
    load_theme(name, None).ok()
}

// ============================================================================
// Terminal Theme Detection
// ============================================================================

/// Light or dark terminal background.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalTheme {
    /// Dark background.
    Dark,
    /// Light background.
    Light,
}

impl TerminalTheme {
    /// The TypeScript literal, which is also the built-in theme name.
    pub fn as_str(self) -> &'static str {
        match self {
            TerminalTheme::Dark => "dark",
            TerminalTheme::Light => "light",
        }
    }
}

impl From<notagent_tui::TerminalColorScheme> for TerminalTheme {
    fn from(scheme: notagent_tui::TerminalColorScheme) -> Self {
        match scheme {
            notagent_tui::TerminalColorScheme::Dark => TerminalTheme::Dark,
            notagent_tui::TerminalColorScheme::Light => TerminalTheme::Light,
        }
    }
}

/// Split a `light/dark` theme setting.
pub fn parse_auto_theme_setting(theme_setting: Option<&str>) -> Option<(String, String)> {
    let theme_setting = theme_setting?;
    if theme_setting.is_empty() {
        return None;
    }
    let slash_index = theme_setting.find('/')?;
    if theme_setting[slash_index + 1..].contains('/') {
        return None;
    }

    let light_theme = theme_setting[..slash_index].trim();
    let dark_theme = theme_setting[slash_index + 1..].trim();
    if light_theme.is_empty() || dark_theme.is_empty() {
        return None;
    }
    Some((light_theme.to_string(), dark_theme.to_string()))
}

/// Resolve a theme setting against the detected terminal theme.
pub fn resolve_theme_setting(
    theme_setting: Option<&str>,
    terminal_theme: TerminalTheme,
) -> Option<String> {
    if let Some((light_theme, dark_theme)) = parse_auto_theme_setting(theme_setting) {
        return Some(if terminal_theme == TerminalTheme::Light {
            light_theme
        } else {
            dark_theme
        });
    }
    if theme_setting.is_some_and(|setting| setting.contains('/')) {
        return None;
    }
    theme_setting.map(str::to_string)
}

/// Where a terminal theme detection came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalThemeSource {
    /// OSC 11 reply.
    TerminalBackground,
    /// `COLORFGBG` environment variable.
    ColorFgBg,
    /// No hint available.
    Fallback,
}

impl TerminalThemeSource {
    /// The TypeScript literal.
    pub fn as_str(self) -> &'static str {
        match self {
            TerminalThemeSource::TerminalBackground => "terminal background",
            TerminalThemeSource::ColorFgBg => "COLORFGBG",
            TerminalThemeSource::Fallback => "fallback",
        }
    }
}

/// How reliable a detection is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalThemeConfidence {
    /// Derived from an explicit terminal or environment hint.
    High,
    /// Fallback guess.
    Low,
}

impl TerminalThemeConfidence {
    /// The TypeScript literal.
    pub fn as_str(self) -> &'static str {
        match self {
            TerminalThemeConfidence::High => "high",
            TerminalThemeConfidence::Low => "low",
        }
    }
}

/// Result of a terminal background detection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalThemeDetection {
    /// Detected theme.
    pub theme: TerminalTheme,
    /// Where the detection came from.
    pub source: TerminalThemeSource,
    /// Human readable explanation.
    pub detail: String,
    /// How reliable the detection is.
    pub confidence: TerminalThemeConfidence,
}

/// Environment map (`NodeJS.ProcessEnv`).
pub type EnvMap = HashMap<String, String>;

/// A rejected terminal query (`queryTerminal*` throwing in TypeScript).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct TerminalQueryError(pub String);

/// Future returned by the detector queries.
pub type TerminalQueryFuture<'a, T> =
    futures::future::LocalBoxFuture<'a, std::result::Result<T, TerminalQueryError>>;

/// Terminal side of the OSC 11 background query.
pub trait TerminalBackgroundThemeDetector {
    /// Query the terminal's default background colour.
    fn query_terminal_background_color(
        &self,
        timeout_ms: u64,
    ) -> TerminalQueryFuture<'_, Option<RgbColor>>;
}

/// Adds the optional color-scheme query (`queryTerminalColorScheme?`).
pub trait TerminalAutoThemeDetector: TerminalBackgroundThemeDetector {
    /// Query the terminal's color-scheme preference. The default mirrors an
    /// absent TypeScript method.
    fn query_terminal_color_scheme(
        &self,
        _timeout_ms: u64,
    ) -> TerminalQueryFuture<'_, Option<TerminalTheme>> {
        Box::pin(async { Ok(None) })
    }
}

fn get_color_fg_bg_background_index(colorfgbg: &str) -> Option<i64> {
    let parts: Vec<&str> = colorfgbg.split(';').collect();
    for part in parts.iter().rev() {
        // `parseInt` accepts leading digits and ignores the rest.
        if let Some(bg) = js_parse_int(part.trim())
            && (0..=255).contains(&bg)
        {
            return Some(bg);
        }
    }
    None
}

/// `parseInt(value, 10)`: leading integer, trailing garbage ignored.
fn js_parse_int(value: &str) -> Option<i64> {
    let trimmed = value.trim_start();
    let mut end = 0;
    let bytes = trimmed.as_bytes();
    if end < bytes.len() && (bytes[end] == b'+' || bytes[end] == b'-') {
        end += 1;
    }
    let digits_start = end;
    while end < bytes.len() && bytes[end].is_ascii_digit() {
        end += 1;
    }
    if end == digits_start {
        return None;
    }
    trimmed[..end].parse::<i64>().ok()
}

fn get_rgb_color_luminance(color: RgbColor) -> f64 {
    let to_linear = |channel: u32| -> f64 {
        let value = channel as f64 / 255.0;
        if value <= 0.03928 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * to_linear(color.r) + 0.7152 * to_linear(color.g) + 0.0722 * to_linear(color.b)
}

fn get_ansi_color_luminance(index: i64) -> f64 {
    let hex = ansi256_to_hex(index);
    let rgb = hex_to_rgb(&hex).expect("ansi256ToHex always yields a valid hex colour");
    get_rgb_color_luminance(RgbColor {
        r: rgb.r,
        g: rgb.g,
        b: rgb.b,
    })
}

/// Classify an RGB colour as a light or dark terminal background.
pub fn get_theme_for_rgb_color(rgb: RgbColor) -> TerminalTheme {
    if get_rgb_color_luminance(rgb) >= 0.5 {
        TerminalTheme::Light
    } else {
        TerminalTheme::Dark
    }
}

/// Detect the terminal theme from `COLORFGBG`.
pub fn detect_terminal_background_from_env(env: Option<&EnvMap>) -> TerminalThemeDetection {
    let colorfgbg = match env {
        Some(env) => env.get("COLORFGBG").cloned().unwrap_or_default(),
        None => std::env::var("COLORFGBG").unwrap_or_default(),
    };
    if let Some(bg) = get_color_fg_bg_background_index(&colorfgbg) {
        return TerminalThemeDetection {
            theme: if get_ansi_color_luminance(bg) >= 0.5 {
                TerminalTheme::Light
            } else {
                TerminalTheme::Dark
            },
            source: TerminalThemeSource::ColorFgBg,
            detail: format!("background color index {bg}"),
            confidence: TerminalThemeConfidence::High,
        };
    }

    TerminalThemeDetection {
        theme: TerminalTheme::Dark,
        source: TerminalThemeSource::Fallback,
        detail: "no terminal background hint found".to_string(),
        confidence: TerminalThemeConfidence::Low,
    }
}

/// Query the terminal background, falling back to `COLORFGBG`.
pub async fn detect_terminal_background_theme(
    ui: &dyn TerminalBackgroundThemeDetector,
    timeout_ms: u64,
    env: Option<&EnvMap>,
) -> TerminalThemeDetection {
    // Fall back to environment-based detection when the terminal query fails.
    if let Ok(Some(rgb)) = ui.query_terminal_background_color(timeout_ms).await {
        return TerminalThemeDetection {
            theme: get_theme_for_rgb_color(rgb),
            source: TerminalThemeSource::TerminalBackground,
            detail: format!("OSC 11 background rgb({}, {}, {})", rgb.r, rgb.g, rgb.b),
            confidence: TerminalThemeConfidence::High,
        };
    }

    detect_terminal_background_from_env(env)
}

/// Prefer the terminal's color-scheme preference, otherwise the background.
///
/// Both queries are started before either is awaited, exactly as in TypeScript.
pub async fn detect_terminal_theme_for_auto(
    ui: &dyn TerminalAutoThemeDetector,
    timeout_ms: u64,
    env: Option<&EnvMap>,
) -> TerminalTheme {
    let color_scheme_future = ui.query_terminal_color_scheme(timeout_ms);
    let background_theme_future = Box::pin(detect_terminal_background_theme(ui, timeout_ms, env));

    match futures::future::select(color_scheme_future, background_theme_future).await {
        futures::future::Either::Left((color_scheme, background_theme_future)) => {
            if let Ok(Some(color_scheme)) = color_scheme {
                return color_scheme;
            }
            background_theme_future.await.theme
        }
        futures::future::Either::Right((background_theme, color_scheme_future)) => {
            if let Ok(Some(color_scheme)) = color_scheme_future.await {
                return color_scheme;
            }
            background_theme.theme
        }
    }
}

/// The theme name used when the settings do not name one.
pub fn get_default_theme() -> String {
    detect_terminal_background_from_env(None)
        .theme
        .as_str()
        .to_string()
}

// ============================================================================
// Global Theme Instance
// ============================================================================

// TypeScript shares the theme through `globalThis` so that every module loader
// (tsx + jiti) sees the same instance. The port uses process globals, which
// additionally makes the theme readable from the tool tasks running on other
// tokio worker threads.
fn global_theme() -> &'static RwLock<Option<Arc<Theme>>> {
    static THEME: OnceLock<RwLock<Option<Arc<Theme>>>> = OnceLock::new();
    THEME.get_or_init(|| RwLock::new(None))
}

fn current_theme_name() -> &'static RwLock<Option<String>> {
    static CURRENT_THEME_NAME: OnceLock<RwLock<Option<String>>> = OnceLock::new();
    CURRENT_THEME_NAME.get_or_init(|| RwLock::new(None))
}

/// Themes contributed by the resource loader, in insertion order (a JS `Map`).
type ThemeRegistry = Vec<(String, Arc<Theme>)>;

fn registered_themes() -> &'static RwLock<ThemeRegistry> {
    static REGISTERED_THEMES: OnceLock<RwLock<ThemeRegistry>> = OnceLock::new();
    REGISTERED_THEMES.get_or_init(|| RwLock::new(Vec::new()))
}

/// Callback invoked after the active theme changed.
///
/// Must be `Send + Sync` because it is stored in a process global; the
/// TypeScript callback lives on the single JavaScript thread.
pub type ThemeChangeCallback = Arc<dyn Fn() + Send + Sync>;

fn on_theme_change_callback() -> &'static RwLock<Option<ThemeChangeCallback>> {
    static ON_THEME_CHANGE: OnceLock<RwLock<Option<ThemeChangeCallback>>> = OnceLock::new();
    ON_THEME_CHANGE.get_or_init(|| RwLock::new(None))
}

/// The active theme.
///
/// Panics with the TypeScript message when no theme was installed yet — the
/// TypeScript proxy throws the same uncaught error.
pub fn theme() -> Arc<Theme> {
    match global_theme().read().unwrap().as_ref() {
        Some(theme) => Arc::clone(theme),
        None => panic!("Theme not initialized. Call initTheme() first."),
    }
}

/// Whether a theme was installed (used by tests and by callers that must not
/// panic before startup finished).
pub fn is_theme_initialized() -> bool {
    global_theme().read().unwrap().is_some()
}

fn set_global_theme(theme: Theme) {
    *global_theme().write().unwrap() = Some(Arc::new(theme));
}

/// Replace the themes contributed by the resource loader.
pub fn set_registered_themes(themes: Vec<Theme>) -> Result<()> {
    let mut registry = Vec::new();
    for theme in themes {
        if let Some(name) = theme.name.clone() {
            assert_theme_name_is_valid(&name)?;
            match registry
                .iter_mut()
                .find(|(key, _): &&mut (String, Arc<Theme>)| *key == name)
            {
                Some(entry) => entry.1 = Arc::new(theme),
                None => registry.push((name, Arc::new(theme))),
            }
        }
    }
    *registered_themes().write().unwrap() = registry;
    Ok(())
}

/// Install the theme named by the settings, falling back to `dark`.
pub fn init_theme(theme_name: Option<&str>, enable_watcher: bool) {
    let name = match theme_name {
        Some(name) => name.to_string(),
        None => get_default_theme(),
    };
    *current_theme_name().write().unwrap() = Some(name.clone());
    match load_theme(&name, None) {
        Ok(theme) => {
            set_global_theme(theme);
            if enable_watcher {
                start_theme_watcher();
            }
        }
        Err(_error) => {
            // Theme is invalid - fall back to dark theme silently
            *current_theme_name().write().unwrap() = Some("dark".to_string());
            set_global_theme(load_theme("dark", None).expect("built-in dark theme loads"));
            // Don't start watcher for fallback theme
        }
    }
}

/// Result of a theme switch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ThemeResult {
    /// Whether the requested theme was installed.
    pub success: bool,
    /// Failure reason of the requested theme.
    pub error: Option<String>,
}

/// Switch to a named theme, falling back to `dark` on failure.
pub fn set_theme(name: &str, enable_watcher: bool) -> ThemeResult {
    *current_theme_name().write().unwrap() = Some(name.to_string());
    match load_theme(name, None) {
        Ok(theme) => {
            set_global_theme(theme);
            if enable_watcher {
                start_theme_watcher();
            }
            notify_theme_changed();
            ThemeResult {
                success: true,
                error: None,
            }
        }
        Err(error) => {
            // Theme is invalid - fall back to dark theme
            *current_theme_name().write().unwrap() = Some("dark".to_string());
            set_global_theme(load_theme("dark", None).expect("built-in dark theme loads"));
            // Don't start watcher for fallback theme
            ThemeResult {
                success: false,
                error: Some(error.0),
            }
        }
    }
}

/// Install a theme instance directly.
pub fn set_theme_instance(theme_instance: Theme) {
    set_global_theme(theme_instance);
    *current_theme_name().write().unwrap() = Some("<in-memory>".to_string());
    stop_theme_watcher(); // Can't watch a direct instance
    notify_theme_changed();
}

/// Register the callback invoked after every theme change.
pub fn on_theme_change(callback: ThemeChangeCallback) {
    *on_theme_change_callback().write().unwrap() = Some(callback);
}

fn notify_theme_changed() {
    let callback = on_theme_change_callback().read().unwrap().clone();
    if let Some(callback) = callback {
        callback();
    }
}

// --- Live reload watcher ------------------------------------------------

/// State of the live-reload watcher.
#[derive(Default)]
struct ThemeWatcherState {
    /// Theme name the watcher was started for.
    watched_theme_name: Option<String>,
    /// File the watcher reloads from.
    watched_file: Option<PathBuf>,
    /// Bumped by `stopThemeWatcher` and by every new schedule, which makes the
    /// previously scheduled reload stale (`clearTimeout`).
    reload_generation: u64,
    /// Whether a reload timer is pending.
    reload_pending: bool,
    /// Runtime the debounce timer runs on. `notify` delivers events on its own
    /// thread, which is not a tokio worker, so the handle is captured when the
    /// watcher starts.
    runtime: Option<tokio::runtime::Handle>,
    /// Set by the `error` handler of `watchWithErrorHandler`: the watcher stops
    /// delivering events. The backend is dropped by the next start/stop, never
    /// from inside its own callback (that would join the callback's thread).
    failed: bool,
}

fn theme_watcher() -> &'static RwLock<ThemeWatcherState> {
    static WATCHER: OnceLock<RwLock<ThemeWatcherState>> = OnceLock::new();
    WATCHER.get_or_init(|| RwLock::new(ThemeWatcherState::default()))
}

/// The OS-level directory watch; `FSWatcher` of `utils/fs-watch.ts`.
///
/// Kept out of [`ThemeWatcherState`] so it can be dropped without holding that
/// lock — `Drop` joins the backend thread, which may be inside an event.
fn theme_watcher_backend() -> &'static Mutex<Option<notify::RecommendedWatcher>> {
    static BACKEND: OnceLock<Mutex<Option<notify::RecommendedWatcher>>> = OnceLock::new();
    BACKEND.get_or_init(|| Mutex::new(None))
}

/// `closeWatcher(watcher)` — close errors are ignored.
fn close_theme_watcher_backend() {
    let watcher = theme_watcher_backend()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    drop(watcher);
}

/// `watchWithErrorHandler(dir, listener, onError)`.
fn watch_custom_themes_dir(directory: &Path) {
    use notify::Watcher as _;

    let handler = |event: notify::Result<notify::Event>| match event {
        Ok(event) => {
            if event.paths.is_empty() {
                // No name reported — TypeScript schedules a reload as well.
                notify_theme_directory_event(None);
                return;
            }
            for path in &event.paths {
                notify_theme_directory_event(path.file_name().and_then(|name| name.to_str()));
            }
        }
        Err(_) => notify_theme_watcher_error(),
    };

    let Ok(mut watcher) = notify::recommended_watcher(handler) else {
        // `watch()` threw: onError, no watcher.
        return;
    };
    if watcher
        .watch(directory, notify::RecursiveMode::NonRecursive)
        .is_err()
    {
        return;
    }
    *theme_watcher_backend()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(watcher);
}

/// Debounce of the live reload, as in `setTimeout(..., 100)`.
const THEME_RELOAD_DEBOUNCE_MS: u64 = 100;

fn start_theme_watcher() {
    stop_theme_watcher();

    // Only watch if it's a custom theme (not built-in)
    let current = current_theme_name().read().unwrap().clone();
    let Some(watched_theme_name) = current else {
        return;
    };
    if watched_theme_name == "dark" || watched_theme_name == "light" {
        return;
    }

    let custom_themes_dir = get_custom_themes_dir();
    let watched_file_name = format!("{watched_theme_name}.json");
    let theme_file = custom_themes_dir.join(&watched_file_name);

    // Only watch if the file exists
    if !theme_file.exists() {
        return;
    }

    {
        let mut state = theme_watcher().write().unwrap();
        state.watched_theme_name = Some(watched_theme_name);
        state.watched_file = Some(theme_file);
        state.failed = false;
        state.runtime = tokio::runtime::Handle::try_current().ok();
    }

    watch_custom_themes_dir(&custom_themes_dir);
}

/// Handle a failure reported by the directory watcher.
///
/// `watchWithErrorHandler(dir, listener, onError)` of `utils/fs-watch.ts`
/// attaches an `error` listener precisely so an asynchronous OS failure does not
/// terminate the process (regression #2791); `onError` then stops the live
/// reload. The `notify` backend reports the same failures as an `Err` event,
/// which this handles the same way. Public so the regression test can inject the
/// failure, exactly as `notify_theme_directory_event` injects a change.
pub fn notify_theme_watcher_error() {
    theme_watcher()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .failed = true;
}

/// Handle one directory event of the watched custom themes directory.
///
/// `filename` is `None` when the platform does not report one — TypeScript then
/// schedules a reload as well.
pub fn notify_theme_directory_event(filename: Option<&str>) {
    let (watched_theme_name, watched_file_name) = {
        let state = theme_watcher()
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(name) = state.watched_theme_name.clone() else {
            return;
        };
        if state.failed {
            return;
        }
        (name.clone(), format!("{name}.json"))
    };
    if current_theme_name().read().unwrap().as_deref() != Some(watched_theme_name.as_str()) {
        return;
    }
    match filename {
        None => schedule_theme_reload(),
        Some(filename) if filename == watched_file_name => schedule_theme_reload(),
        Some(_) => {}
    }
}

fn schedule_theme_reload() {
    let (generation, runtime) = {
        let mut state = theme_watcher()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.reload_generation += 1;
        state.reload_pending = true;
        (state.reload_generation, state.runtime.clone())
    };
    let Some(handle) = tokio::runtime::Handle::try_current().ok().or(runtime) else {
        // Without a runtime (unit tests, non-interactive modes) the reload is
        // driven by `run_scheduled_theme_reload`.
        return;
    };
    handle.spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(THEME_RELOAD_DEBOUNCE_MS)).await;
        run_scheduled_theme_reload(generation);
    });
}

/// Body of the debounced reload timer.
fn run_scheduled_theme_reload(generation: u64) {
    let (watched_theme_name, watched_file) = {
        let mut state = theme_watcher().write().unwrap();
        if state.reload_generation != generation {
            return;
        }
        state.reload_pending = false;
        match (state.watched_theme_name.clone(), state.watched_file.clone()) {
            (Some(name), Some(file)) => (name, file),
            _ => return,
        }
    };

    // Ignore stale timers after switching themes or stopping the watcher
    if current_theme_name().read().unwrap().as_deref() != Some(watched_theme_name.as_str()) {
        return;
    }

    // Keep the last successfully loaded theme active if the file is temporarily missing
    if !watched_file.exists() {
        return;
    }

    // Reload the theme from disk and refresh the registry cache
    let Ok(reloaded_theme) = load_theme_from_path(&watched_file, None) else {
        // Ignore errors (file might be in invalid state while being edited)
        return;
    };
    let reloaded = Arc::new(reloaded_theme.clone());
    {
        let mut registry = registered_themes().write().unwrap();
        match registry
            .iter_mut()
            .find(|(key, _)| *key == watched_theme_name)
        {
            Some(entry) => entry.1 = reloaded,
            None => registry.push((watched_theme_name, reloaded)),
        }
    }
    set_global_theme(reloaded_theme);
    // Notify callback (to invalidate UI)
    notify_theme_changed();
}

/// Stop the live-reload watcher and cancel a pending reload.
pub fn stop_theme_watcher() {
    {
        let mut state = theme_watcher()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.reload_generation += 1;
        state.reload_pending = false;
        state.watched_theme_name = None;
        state.watched_file = None;
        state.failed = false;
        state.runtime = None;
    }
    close_theme_watcher_backend();
}

// ============================================================================
// HTML Export Helpers
// ============================================================================

/// Convert a 256-color index to hex string.
/// Indices 0-15: basic colors (approximate)
/// Indices 16-231: 6x6x6 color cube
/// Indices 232-255: grayscale ramp
fn ansi256_to_hex(index: i64) -> String {
    // Basic colors (0-15) - approximate common terminal values
    const BASIC_COLORS: [&str; 16] = [
        "#000000", "#800000", "#008000", "#808000", "#000080", "#800080", "#008080", "#c0c0c0",
        "#808080", "#ff0000", "#00ff00", "#ffff00", "#0000ff", "#ff00ff", "#00ffff", "#ffffff",
    ];
    if index < 16 {
        return BASIC_COLORS[index as usize].to_string();
    }

    // Color cube (16-231): 6x6x6 = 216 colors
    if index < 232 {
        let cube_index = index - 16;
        let r = cube_index / 36;
        let g = (cube_index % 36) / 6;
        let b = cube_index % 6;
        let to_hex = |n: i64| -> String {
            let value = if n == 0 { 0 } else { 55 + n * 40 };
            format!("{value:02x}")
        };
        return format!("#{}{}{}", to_hex(r), to_hex(g), to_hex(b));
    }

    // Grayscale (232-255): 24 shades
    let gray = 8 + (index - 232) * 10;
    let gray_hex = format!("{gray:02x}");
    format!("#{gray_hex}{gray_hex}{gray_hex}")
}

/// Get resolved theme colors as CSS-compatible hex strings.
/// Used by HTML export to generate CSS custom properties.
///
/// Returns the entries in file order, like `Object.entries` of the TypeScript
/// record.
pub fn get_resolved_theme_colors(theme_name: Option<&str>) -> Result<Vec<(String, String)>> {
    let name = match theme_name {
        Some(name) => name.to_string(),
        None => current_theme_name()
            .read()
            .unwrap()
            .clone()
            .unwrap_or_else(get_default_theme),
    };
    let is_light = name == "light";
    let theme_json = load_theme_json(&name)?;
    let resolved = resolve_theme_colors(
        &with_theme_color_fallbacks(&theme_json.colors),
        &theme_json.vars,
    )?;

    // Default text color for empty values (terminal uses default fg color)
    let default_text = if is_light { "#000000" } else { "#e5e5e7" };

    let mut css_colors = Vec::new();
    for (key, value) in resolved.iter() {
        let css = match value {
            ColorValue::Index(index) => ansi256_to_hex(*index as i64),
            // Empty means default terminal color - use sensible fallback for HTML
            ColorValue::Text(text) if text.is_empty() => default_text.to_string(),
            ColorValue::Text(text) => text.clone(),
        };
        css_colors.push((key.clone(), css));
    }
    Ok(css_colors)
}

/// Check if a theme is a "light" theme (for CSS that needs light/dark variants).
pub fn is_light_theme(theme_name: Option<&str>) -> bool {
    // Currently just check the name - could be extended to analyze colors
    theme_name == Some("light")
}

/// Explicit export colours of a theme.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ThemeExportColors {
    /// Page background.
    pub page_bg: Option<String>,
    /// Card background.
    pub card_bg: Option<String>,
    /// Info-box background.
    pub info_bg: Option<String>,
}

/// Get explicit export colors from theme JSON, if specified.
/// Returns `None` for each color that isn't explicitly set.
pub fn get_theme_export_colors(theme_name: Option<&str>) -> ThemeExportColors {
    let name = match theme_name {
        Some(name) => name.to_string(),
        None => current_theme_name()
            .read()
            .unwrap()
            .clone()
            .unwrap_or_else(get_default_theme),
    };
    // The whole body is wrapped in try/catch in TypeScript: any failure yields
    // an empty result.
    read_theme_export_colors(&name).unwrap_or_default()
}

fn read_theme_export_colors(name: &str) -> Result<ThemeExportColors> {
    let theme_json = load_theme_json(name)?;
    let Some(export_section) = theme_json.export.as_ref() else {
        return Ok(ThemeExportColors::default());
    };

    let vars = &theme_json.vars;
    let resolve = |value: Option<&ColorValue>| -> Result<Option<String>> {
        let Some(value) = value else {
            return Ok(None);
        };
        let mut visited = Vec::new();
        let resolved = resolve_var_refs(value, vars, &mut visited)?;
        Ok(match resolved {
            ColorValue::Index(index) => Some(ansi256_to_hex(index as i64)),
            ColorValue::Text(text) if text.is_empty() => None,
            ColorValue::Text(text) => Some(text),
        })
    };

    Ok(ThemeExportColors {
        page_bg: resolve(export_section.page_bg.as_ref())?,
        card_bg: resolve(export_section.card_bg.as_ref())?,
        info_bg: resolve(export_section.info_bg.as_ref())?,
    })
}

// ============================================================================
// TUI Helpers
// ============================================================================

/// Whether the syntax highlighter supports a language.
///
/// `packages/coding-agent/src/utils/syntax-highlight.ts` (146 LOC — highlight.js
/// Formatter map of the syntax highlighter, keyed by highlight.js scope.
type CliHighlightTheme = HighlightTheme;

fn build_cli_highlight_theme(theme: &Arc<Theme>) -> CliHighlightTheme {
    fn fg(theme: &Arc<Theme>, color: ThemeColor) -> HighlightFormatter {
        // TypeScript closes over the theme instance `t`, not the global.
        let theme = Arc::clone(theme);
        Rc::new(move |text: &str| theme.fg(color, text))
    }
    let style = |apply: fn(&Theme, &str) -> String| -> HighlightFormatter {
        let theme = Arc::clone(theme);
        Rc::new(move |text: &str| apply(&theme, text))
    };
    let entries: Vec<(&str, HighlightFormatter)> = vec![
        ("keyword", fg(theme, ThemeColor::SyntaxKeyword)),
        ("built_in", fg(theme, ThemeColor::SyntaxType)),
        ("literal", fg(theme, ThemeColor::SyntaxNumber)),
        ("number", fg(theme, ThemeColor::SyntaxNumber)),
        ("regexp", fg(theme, ThemeColor::SyntaxString)),
        ("string", fg(theme, ThemeColor::SyntaxString)),
        ("comment", fg(theme, ThemeColor::SyntaxComment)),
        ("doctag", fg(theme, ThemeColor::SyntaxComment)),
        ("meta", fg(theme, ThemeColor::Muted)),
        ("function", fg(theme, ThemeColor::SyntaxFunction)),
        ("title", fg(theme, ThemeColor::SyntaxFunction)),
        ("class", fg(theme, ThemeColor::SyntaxType)),
        ("type", fg(theme, ThemeColor::SyntaxType)),
        ("tag", fg(theme, ThemeColor::SyntaxPunctuation)),
        ("name", fg(theme, ThemeColor::SyntaxKeyword)),
        ("attr", fg(theme, ThemeColor::SyntaxVariable)),
        ("variable", fg(theme, ThemeColor::SyntaxVariable)),
        ("params", fg(theme, ThemeColor::SyntaxVariable)),
        ("operator", fg(theme, ThemeColor::SyntaxOperator)),
        ("punctuation", fg(theme, ThemeColor::SyntaxPunctuation)),
        ("emphasis", style(Theme::italic)),
        ("strong", style(Theme::bold)),
        ("link", style(Theme::underline)),
        ("addition", fg(theme, ThemeColor::ToolDiffAdded)),
        ("deletion", fg(theme, ThemeColor::ToolDiffRemoved)),
    ];
    entries
        .into_iter()
        .map(|(scope, formatter)| (scope.to_string(), formatter))
        .collect()
}

/// `getCliHighlightTheme(t)` — the memoised formatter map.
///
/// TypeScript compares object identity (`cachedHighlightThemeFor !== t`); the
/// port compares the `Arc` the global theme handed out and keeps that `Arc`
/// alive in the cache, so an address can never be reused behind a stale hit.
/// The formatters hold `Rc`, so the cache is per thread rather than global.
fn get_cli_highlight_theme(theme: &Arc<Theme>) -> Rc<CliHighlightTheme> {
    thread_local! {
        static CACHE: RefCell<Option<(Arc<Theme>, Rc<CliHighlightTheme>)>> =
            const { RefCell::new(None) };
    }
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some((cached_theme, cached)) = cache.as_ref()
            && Arc::ptr_eq(cached_theme, theme)
        {
            return Rc::clone(cached);
        }
        let built = Rc::new(build_cli_highlight_theme(theme));
        *cache = Some((Arc::clone(theme), Rc::clone(&built)));
        built
    })
}

/// The shared body of `highlightCode` and `getMarkdownTheme().highlightCode`.
///
/// The two differ only in the `catch` branch, which `on_error` supplies.
fn highlight_code_with(
    code: &str,
    lang: Option<&str>,
    on_error: impl Fn(&str) -> Vec<String>,
) -> Vec<String> {
    // Validate language before highlighting to avoid stderr spam from cli-highlight
    let valid_lang = lang.filter(|lang| supports_language(lang));
    // Skip highlighting when no valid language is specified. cli-highlight's
    // auto-detection is unreliable and can misidentify prose as AppleScript,
    // LiveCodeServer, etc., coloring random English words as keywords.
    let Some(valid_lang) = valid_lang else {
        let theme = theme();
        return code
            .split('\n')
            .map(|line| theme.fg(ThemeColor::MdCodeBlock, line))
            .collect();
    };
    let theme = theme();
    let options = HighlightOptions {
        language: Some(valid_lang.to_string()),
        ignore_illegals: true,
        language_subset: None,
        theme: (*get_cli_highlight_theme(&theme)).clone(),
    };
    match highlight(code, &options) {
        Ok(highlighted) => highlighted.split('\n').map(str::to_string).collect(),
        Err(_) => on_error(code),
    }
}

/// Highlight code with syntax coloring based on file extension or language.
/// Returns array of highlighted lines.
pub fn highlight_code(code: &str, lang: Option<&str>) -> Vec<String> {
    highlight_code_with(code, lang, |code| {
        code.split('\n').map(str::to_string).collect()
    })
}

/// Get language identifier from file path extension.
pub fn get_language_from_path(file_path: &str) -> Option<String> {
    let ext = file_path.split('.').next_back()?.to_lowercase();
    if ext.is_empty() {
        return None;
    }

    const EXT_TO_LANG: [(&str, &str); 58] = [
        ("ts", "typescript"),
        ("tsx", "typescript"),
        ("js", "javascript"),
        ("jsx", "javascript"),
        ("mjs", "javascript"),
        ("cjs", "javascript"),
        ("py", "python"),
        ("rb", "ruby"),
        ("rs", "rust"),
        ("go", "go"),
        ("java", "java"),
        ("kt", "kotlin"),
        ("swift", "swift"),
        ("c", "c"),
        ("h", "c"),
        ("cpp", "cpp"),
        ("cc", "cpp"),
        ("cxx", "cpp"),
        ("hpp", "cpp"),
        ("cs", "csharp"),
        ("php", "php"),
        ("sh", "bash"),
        ("bash", "bash"),
        ("zsh", "bash"),
        ("fish", "fish"),
        ("ps1", "powershell"),
        ("sql", "sql"),
        ("html", "html"),
        ("htm", "html"),
        ("css", "css"),
        ("scss", "scss"),
        ("sass", "sass"),
        ("less", "less"),
        ("json", "json"),
        ("yaml", "yaml"),
        ("yml", "yaml"),
        ("toml", "toml"),
        ("xml", "xml"),
        ("md", "markdown"),
        ("markdown", "markdown"),
        ("dockerfile", "dockerfile"),
        ("makefile", "makefile"),
        ("cmake", "cmake"),
        ("lua", "lua"),
        ("perl", "perl"),
        ("r", "r"),
        ("scala", "scala"),
        ("clj", "clojure"),
        ("ex", "elixir"),
        ("exs", "elixir"),
        ("erl", "erlang"),
        ("hs", "haskell"),
        ("ml", "ocaml"),
        ("vim", "vim"),
        ("graphql", "graphql"),
        ("proto", "protobuf"),
        ("tf", "hcl"),
        ("hcl", "hcl"),
    ];

    EXT_TO_LANG
        .iter()
        .find(|(key, _)| *key == ext)
        .map(|(_, lang)| (*lang).to_string())
}

/// Colour functions for the markdown component.
pub fn get_markdown_theme() -> MarkdownTheme {
    fn slot(color: ThemeColor) -> StyleFn {
        Rc::new(move |text: &str| theme().fg(color, text))
    }
    MarkdownTheme {
        heading: slot(ThemeColor::MdHeading),
        link: slot(ThemeColor::MdLink),
        link_url: slot(ThemeColor::MdLinkUrl),
        code: slot(ThemeColor::MdCode),
        code_block: slot(ThemeColor::MdCodeBlock),
        code_block_border: slot(ThemeColor::MdCodeBlockBorder),
        quote: slot(ThemeColor::MdQuote),
        quote_border: slot(ThemeColor::MdQuoteBorder),
        hr: slot(ThemeColor::MdHr),
        list_bullet: slot(ThemeColor::MdListBullet),
        bold: Rc::new(|text: &str| theme().bold(text)),
        italic: Rc::new(|text: &str| theme().italic(text)),
        underline: Rc::new(|text: &str| theme().underline(text)),
        strikethrough: Rc::new(|text: &str| theme().strikethrough(text)),
        highlight_code: Some(Rc::new(|code: &str, lang: Option<&str>| -> Vec<String> {
            // The markdown theme keeps the code-block colour on failure; the
            // free `highlightCode` returns the raw lines.
            highlight_code_with(code, lang, |code| {
                let theme = theme();
                code.split('\n')
                    .map(|line| theme.fg(ThemeColor::MdCodeBlock, line))
                    .collect()
            })
        }) as HighlightCodeFn),
        code_block_indent: None,
    }
}

/// Colour functions for the select list.
pub fn get_select_list_theme() -> SelectListTheme {
    fn slot(color: ThemeColor) -> Rc<dyn Fn(&str) -> String> {
        Rc::new(move |text: &str| theme().fg(color, text))
    }
    SelectListTheme {
        selected_prefix: slot(ThemeColor::Accent),
        selected_text: slot(ThemeColor::Accent),
        description: slot(ThemeColor::Muted),
        scroll_info: slot(ThemeColor::Muted),
        no_match: slot(ThemeColor::Muted),
    }
}

/// Colour functions for the editor.
pub fn get_editor_theme() -> EditorTheme {
    EditorTheme {
        border_color: Rc::new(|text: &str| theme().fg(ThemeColor::BorderMuted, text)),
        select_list: Rc::new(get_select_list_theme),
    }
}

/// Colour functions for the settings list.
pub fn get_settings_list_theme() -> SettingsListTheme {
    SettingsListTheme {
        label: Rc::new(|text: &str, selected: bool| {
            if selected {
                theme().fg(ThemeColor::Accent, text)
            } else {
                text.to_string()
            }
        }) as SelectionAwareColorFn,
        value: Rc::new(|text: &str, selected: bool| {
            if selected {
                theme().fg(ThemeColor::Accent, text)
            } else {
                theme().fg(ThemeColor::Muted, text)
            }
        }) as SelectionAwareColorFn,
        description: Rc::new(|text: &str| theme().fg(ThemeColor::Dim, text)) as ColorFn,
        cursor: theme().fg(ThemeColor::Accent, "→ "),
        hint: Rc::new(|text: &str| theme().fg(ThemeColor::Dim, text)) as ColorFn,
    }
}
