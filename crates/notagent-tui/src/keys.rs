//! Keyboard input handling for terminal applications.
//!
//! 1:1 port of `packages/tui/src/keys.ts` (1401 LOC). Supports both legacy
//! terminal sequences and the Kitty keyboard protocol.
//! See <https://sw.kovidgoyal.net/kitty/keyboard-protocol/>.
//!
//! Symbol keys are supported as well, however some ctrl+symbol combos overlap
//! with ASCII codes, e.g. ctrl+[ = ESC. Those can still be used for ctrl+shift
//! combos.

use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};

use regex::Regex;

// =============================================================================
// Global Kitty Protocol State
// =============================================================================

/// Process-global like the module-level `_kittyProtocolActive` in TS.
static KITTY_PROTOCOL_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Set the global Kitty keyboard protocol state.
///
/// Called by `ProcessTerminal` after detecting protocol support.
pub fn set_kitty_protocol_active(active: bool) {
    KITTY_PROTOCOL_ACTIVE.store(active, Ordering::SeqCst);
}

/// Query whether the Kitty keyboard protocol is currently active.
pub fn is_kitty_protocol_active() -> bool {
    KITTY_PROTOCOL_ACTIVE.load(Ordering::SeqCst)
}

// =============================================================================
// Constants
// =============================================================================

const SYMBOL_KEYS: [char; 31] = [
    '`', '-', '=', '[', ']', '\\', ';', '\'', ',', '.', '/', '!', '@', '#', '$', '%', '^', '&',
    '*', '(', ')', '_', '+', '|', '~', '{', '}', ':', '<', '>', '?',
];

fn is_symbol_key(c: char) -> bool {
    SYMBOL_KEYS.contains(&c)
}

/// `String.fromCharCode(code)`: JavaScript truncates to 16 bits.
///
/// Returns `None` for lone surrogates, which Rust `char` cannot represent; no
/// surrogate is a symbol key, so the classification result is unchanged.
fn js_from_char_code(code: i64) -> Option<char> {
    let unit = code.rem_euclid(65536) as u32;
    char::from_u32(unit)
}

mod modifiers {
    pub const SHIFT: i64 = 1;
    pub const ALT: i64 = 2;
    pub const CTRL: i64 = 4;
    pub const SUPER: i64 = 8;
}

/// Caps Lock + Num Lock.
const LOCK_MASK: i64 = 64 + 128;

mod codepoints {
    pub const ESCAPE: i64 = 27;
    pub const TAB: i64 = 9;
    pub const ENTER: i64 = 13;
    pub const SPACE: i64 = 32;
    pub const BACKSPACE: i64 = 127;
    /// Numpad Enter (Kitty protocol).
    pub const KP_ENTER: i64 = 57414;
}

mod arrow_codepoints {
    pub const UP: i64 = -1;
    pub const DOWN: i64 = -2;
    pub const RIGHT: i64 = -3;
    pub const LEFT: i64 = -4;
}

mod functional_codepoints {
    pub const DELETE: i64 = -10;
    pub const INSERT: i64 = -11;
    pub const PAGE_UP: i64 = -12;
    pub const PAGE_DOWN: i64 = -13;
    pub const HOME: i64 = -14;
    pub const END: i64 = -15;
}

/// Numpad normalization of Kitty functional keys (`KITTY_FUNCTIONAL_KEY_EQUIVALENTS`).
const KITTY_FUNCTIONAL_KEY_EQUIVALENTS: [(i64, i64); 27] = [
    (57399, 48), // KP_0 -> 0
    (57400, 49), // KP_1 -> 1
    (57401, 50), // KP_2 -> 2
    (57402, 51), // KP_3 -> 3
    (57403, 52), // KP_4 -> 4
    (57404, 53), // KP_5 -> 5
    (57405, 54), // KP_6 -> 6
    (57406, 55), // KP_7 -> 7
    (57407, 56), // KP_8 -> 8
    (57408, 57), // KP_9 -> 9
    (57409, 46), // KP_DECIMAL -> .
    (57410, 47), // KP_DIVIDE -> /
    (57411, 42), // KP_MULTIPLY -> *
    (57412, 45), // KP_SUBTRACT -> -
    (57413, 43), // KP_ADD -> +
    (57415, 61), // KP_EQUAL -> =
    (57416, 44), // KP_SEPARATOR -> ,
    (57417, arrow_codepoints::LEFT),
    (57418, arrow_codepoints::RIGHT),
    (57419, arrow_codepoints::UP),
    (57420, arrow_codepoints::DOWN),
    (57421, functional_codepoints::PAGE_UP),
    (57422, functional_codepoints::PAGE_DOWN),
    (57423, functional_codepoints::HOME),
    (57424, functional_codepoints::END),
    (57425, functional_codepoints::INSERT),
    (57426, functional_codepoints::DELETE),
];

fn normalize_kitty_functional_codepoint(codepoint: i64) -> i64 {
    KITTY_FUNCTIONAL_KEY_EQUIVALENTS
        .iter()
        .find(|(from, _)| *from == codepoint)
        .map_or(codepoint, |(_, to)| *to)
}

fn normalize_shifted_letter_identity_codepoint(codepoint: i64, modifier: i64) -> i64 {
    let effective_modifier = modifier & !LOCK_MASK;
    if (effective_modifier & modifiers::SHIFT) != 0 && (65..=90).contains(&codepoint) {
        return codepoint + 32;
    }
    codepoint
}

fn legacy_key_sequences(key: &str) -> &'static [&'static str] {
    match key {
        "up" => &["\x1b[A", "\x1bOA"],
        "down" => &["\x1b[B", "\x1bOB"],
        "right" => &["\x1b[C", "\x1bOC"],
        "left" => &["\x1b[D", "\x1bOD"],
        "home" => &["\x1b[H", "\x1bOH", "\x1b[1~", "\x1b[7~"],
        "end" => &["\x1b[F", "\x1bOF", "\x1b[4~", "\x1b[8~"],
        "insert" => &["\x1b[2~"],
        "delete" => &["\x1b[3~"],
        "pageUp" => &["\x1b[5~", "\x1b[[5~"],
        "pageDown" => &["\x1b[6~", "\x1b[[6~"],
        "clear" => &["\x1b[E", "\x1bOE"],
        "f1" => &["\x1bOP", "\x1b[11~", "\x1b[[A"],
        "f2" => &["\x1bOQ", "\x1b[12~", "\x1b[[B"],
        "f3" => &["\x1bOR", "\x1b[13~", "\x1b[[C"],
        "f4" => &["\x1bOS", "\x1b[14~", "\x1b[[D"],
        "f5" => &["\x1b[15~", "\x1b[[E"],
        "f6" => &["\x1b[17~"],
        "f7" => &["\x1b[18~"],
        "f8" => &["\x1b[19~"],
        "f9" => &["\x1b[20~"],
        "f10" => &["\x1b[21~"],
        "f11" => &["\x1b[23~"],
        "f12" => &["\x1b[24~"],
        _ => &[],
    }
}

fn legacy_shift_sequences(key: &str) -> &'static [&'static str] {
    match key {
        "up" => &["\x1b[a"],
        "down" => &["\x1b[b"],
        "right" => &["\x1b[c"],
        "left" => &["\x1b[d"],
        "clear" => &["\x1b[e"],
        "insert" => &["\x1b[2$"],
        "delete" => &["\x1b[3$"],
        "pageUp" => &["\x1b[5$"],
        "pageDown" => &["\x1b[6$"],
        "home" => &["\x1b[7$"],
        "end" => &["\x1b[8$"],
        _ => &[],
    }
}

fn legacy_ctrl_sequences(key: &str) -> &'static [&'static str] {
    match key {
        "up" => &["\x1bOa"],
        "down" => &["\x1bOb"],
        "right" => &["\x1bOc"],
        "left" => &["\x1bOd"],
        "clear" => &["\x1bOe"],
        "insert" => &["\x1b[2^"],
        "delete" => &["\x1b[3^"],
        "pageUp" => &["\x1b[5^"],
        "pageDown" => &["\x1b[6^"],
        "home" => &["\x1b[7^"],
        "end" => &["\x1b[8^"],
        _ => &[],
    }
}

/// `LEGACY_SEQUENCE_KEY_IDS`: raw sequence -> key identifier.
fn legacy_sequence_key_id(data: &str) -> Option<&'static str> {
    Some(match data {
        "\x1bOA" => "up",
        "\x1bOB" => "down",
        "\x1bOC" => "right",
        "\x1bOD" => "left",
        "\x1bOH" => "home",
        "\x1bOF" => "end",
        "\x1b[E" => "clear",
        "\x1bOE" => "clear",
        "\x1bOe" => "ctrl+clear",
        "\x1b[e" => "shift+clear",
        "\x1b[2~" => "insert",
        "\x1b[2$" => "shift+insert",
        "\x1b[2^" => "ctrl+insert",
        "\x1b[3$" => "shift+delete",
        "\x1b[3^" => "ctrl+delete",
        "\x1b[[5~" => "pageUp",
        "\x1b[[6~" => "pageDown",
        "\x1b[a" => "shift+up",
        "\x1b[b" => "shift+down",
        "\x1b[c" => "shift+right",
        "\x1b[d" => "shift+left",
        "\x1bOa" => "ctrl+up",
        "\x1bOb" => "ctrl+down",
        "\x1bOc" => "ctrl+right",
        "\x1bOd" => "ctrl+left",
        "\x1b[5$" => "shift+pageUp",
        "\x1b[6$" => "shift+pageDown",
        "\x1b[7$" => "shift+home",
        "\x1b[8$" => "shift+end",
        "\x1b[5^" => "ctrl+pageUp",
        "\x1b[6^" => "ctrl+pageDown",
        "\x1b[7^" => "ctrl+home",
        "\x1b[8^" => "ctrl+end",
        "\x1bOP" => "f1",
        "\x1bOQ" => "f2",
        "\x1bOR" => "f3",
        "\x1bOS" => "f4",
        "\x1b[11~" => "f1",
        "\x1b[12~" => "f2",
        "\x1b[13~" => "f3",
        "\x1b[14~" => "f4",
        "\x1b[[A" => "f1",
        "\x1b[[B" => "f2",
        "\x1b[[C" => "f3",
        "\x1b[[D" => "f4",
        "\x1b[[E" => "f5",
        "\x1b[15~" => "f5",
        "\x1b[17~" => "f6",
        "\x1b[18~" => "f7",
        "\x1b[19~" => "f8",
        "\x1b[20~" => "f9",
        "\x1b[21~" => "f10",
        "\x1b[23~" => "f11",
        "\x1b[24~" => "f12",
        "\x1bb" => "alt+left",
        "\x1bf" => "alt+right",
        "\x1bp" => "alt+up",
        "\x1bn" => "alt+down",
        _ => return None,
    })
}

fn matches_legacy_sequence(data: &str, sequences: &[&str]) -> bool {
    sequences.contains(&data)
}

fn matches_legacy_modifier_sequence(data: &str, key: &str, modifier: i64) -> bool {
    if modifier == modifiers::SHIFT {
        return matches_legacy_sequence(data, legacy_shift_sequences(key));
    }
    if modifier == modifiers::CTRL {
        return matches_legacy_sequence(data, legacy_ctrl_sequences(key));
    }
    false
}

// =============================================================================
// Kitty Protocol Parsing
// =============================================================================

/// Event types from the Kitty keyboard protocol (flag 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyEventType {
    /// 1 = key press.
    Press,
    /// 2 = key repeat.
    Repeat,
    /// 3 = key release.
    Release,
}

struct ParsedKittySequence {
    codepoint: i64,
    /// Shifted version of the key (when shift is pressed).
    #[allow(dead_code)] // Only `decode_kitty_printable` needs it; kept for 1:1 shape.
    shifted_key: Option<i64>,
    /// Key in the standard PC-101 layout (for non-Latin layouts).
    base_layout_key: Option<i64>,
    modifier: i64,
    #[allow(dead_code)] // TS stores it in `_lastEventType`, which is never read.
    event_type: KeyEventType,
}

struct ParsedModifyOtherKeysSequence {
    codepoint: i64,
    modifier: i64,
}

/// Check whether the input is a key release event.
///
/// Only meaningful when the Kitty keyboard protocol with flag 2 is active.
/// Deliberately substring-based, exactly as in TS.
pub fn is_key_release(data: &str) -> bool {
    // Don't treat bracketed paste content as key release, even if it contains
    // patterns like ":3F" (e.g. bluetooth MAC addresses like "90:62:3F:A5").
    if data.contains("\x1b[200~") {
        return false;
    }

    data.contains(":3u")
        || data.contains(":3~")
        || data.contains(":3A")
        || data.contains(":3B")
        || data.contains(":3C")
        || data.contains(":3D")
        || data.contains(":3H")
        || data.contains(":3F")
}

/// Check whether the input is a key repeat event.
///
/// Only meaningful when the Kitty keyboard protocol with flag 2 is active.
pub fn is_key_repeat(data: &str) -> bool {
    if data.contains("\x1b[200~") {
        return false;
    }

    data.contains(":2u")
        || data.contains(":2~")
        || data.contains(":2A")
        || data.contains(":2B")
        || data.contains(":2C")
        || data.contains(":2D")
        || data.contains(":2H")
        || data.contains(":2F")
}

fn parse_event_type(event_type: Option<&str>) -> KeyEventType {
    match event_type.and_then(|value| value.parse::<i64>().ok()) {
        Some(2) => KeyEventType::Repeat,
        Some(3) => KeyEventType::Release,
        _ => KeyEventType::Press,
    }
}

/// `parseInt(str, 10)` for a digit-only capture; out-of-range values cannot
/// match any key identifier, so they are reported as `None`.
fn parse_int(value: &str) -> Option<i64> {
    value.parse::<i64>().ok()
}

static CSI_U_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\x1b\[(\d+)(?::(\d*))?(?::(\d+))?(?:;(\d+))?(?::(\d+))?u$").expect("valid regex")
});
static ARROW_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\x1b\[1;(\d+)(?::(\d+))?([ABCD])$").expect("valid regex"));
static FUNC_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\x1b\[(\d+)(?:;(\d+))?(?::(\d+))?~$").expect("valid regex"));
static HOME_END_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\x1b\[1;(\d+)(?::(\d+))?([HF])$").expect("valid regex"));
static MODIFY_OTHER_KEYS_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\x1b\[27;(\d+);(\d+)~$").expect("valid regex"));

fn parse_kitty_sequence(data: &str) -> Option<ParsedKittySequence> {
    // CSI u format with alternate keys (flag 4):
    //   \x1b[<codepoint>u
    //   \x1b[<codepoint>;<mod>u
    //   \x1b[<codepoint>;<mod>:<event>u
    //   \x1b[<codepoint>:<shifted>;<mod>u
    //   \x1b[<codepoint>:<shifted>:<base>;<mod>u
    //   \x1b[<codepoint>::<base>;<mod>u
    if let Some(captures) = CSI_U_REGEX.captures(data) {
        let codepoint = parse_int(captures.get(1)?.as_str())?;
        let shifted_key = captures
            .get(2)
            .filter(|m| !m.as_str().is_empty())
            .and_then(|m| parse_int(m.as_str()));
        let base_layout_key = captures.get(3).and_then(|m| parse_int(m.as_str()));
        let mod_value = captures
            .get(4)
            .and_then(|m| parse_int(m.as_str()))
            .unwrap_or(1);
        let event_type = parse_event_type(captures.get(5).map(|m| m.as_str()));
        return Some(ParsedKittySequence {
            codepoint,
            shifted_key,
            base_layout_key,
            modifier: mod_value - 1,
            event_type,
        });
    }

    // Arrow keys with modifier: \x1b[1;<mod>A/B/C/D
    if let Some(captures) = ARROW_REGEX.captures(data) {
        let mod_value = parse_int(captures.get(1)?.as_str())?;
        let event_type = parse_event_type(captures.get(2).map(|m| m.as_str()));
        let codepoint = match captures.get(3)?.as_str() {
            "A" => -1,
            "B" => -2,
            "C" => -3,
            _ => -4,
        };
        return Some(ParsedKittySequence {
            codepoint,
            shifted_key: None,
            base_layout_key: None,
            modifier: mod_value - 1,
            event_type,
        });
    }

    // Functional keys: \x1b[<num>~ with optional modifier and event type
    if let Some(captures) = FUNC_REGEX.captures(data) {
        let key_num = parse_int(captures.get(1)?.as_str())?;
        let mod_value = captures
            .get(2)
            .and_then(|m| parse_int(m.as_str()))
            .unwrap_or(1);
        let event_type = parse_event_type(captures.get(3).map(|m| m.as_str()));
        let codepoint = match key_num {
            2 => Some(functional_codepoints::INSERT),
            3 => Some(functional_codepoints::DELETE),
            5 => Some(functional_codepoints::PAGE_UP),
            6 => Some(functional_codepoints::PAGE_DOWN),
            7 => Some(functional_codepoints::HOME),
            8 => Some(functional_codepoints::END),
            _ => None,
        };
        if let Some(codepoint) = codepoint {
            return Some(ParsedKittySequence {
                codepoint,
                shifted_key: None,
                base_layout_key: None,
                modifier: mod_value - 1,
                event_type,
            });
        }
    }

    // Home/End with modifier: \x1b[1;<mod>H/F
    if let Some(captures) = HOME_END_REGEX.captures(data) {
        let mod_value = parse_int(captures.get(1)?.as_str())?;
        let event_type = parse_event_type(captures.get(2).map(|m| m.as_str()));
        let codepoint = if captures.get(3)?.as_str() == "H" {
            functional_codepoints::HOME
        } else {
            functional_codepoints::END
        };
        return Some(ParsedKittySequence {
            codepoint,
            shifted_key: None,
            base_layout_key: None,
            modifier: mod_value - 1,
            event_type,
        });
    }

    None
}

fn matches_kitty_sequence(data: &str, expected_codepoint: i64, expected_modifier: i64) -> bool {
    let Some(parsed) = parse_kitty_sequence(data) else {
        return false;
    };
    let actual_mod = parsed.modifier & !LOCK_MASK;
    let expected_mod = expected_modifier & !LOCK_MASK;

    if actual_mod != expected_mod {
        return false;
    }

    let normalized_codepoint = normalize_shifted_letter_identity_codepoint(
        normalize_kitty_functional_codepoint(parsed.codepoint),
        parsed.modifier,
    );
    let normalized_expected_codepoint = normalize_shifted_letter_identity_codepoint(
        normalize_kitty_functional_codepoint(expected_codepoint),
        expected_modifier,
    );

    if normalized_codepoint == normalized_expected_codepoint {
        return true;
    }

    // Alternate match: use the base layout key for non-Latin keyboard layouts,
    // but only when the codepoint is not already a recognized Latin letter or
    // symbol — otherwise remapped layouts (Dvorak, Colemak, xremap) would
    // produce false matches.
    if parsed.base_layout_key == Some(expected_codepoint) {
        let cp = normalized_codepoint;
        let is_latin_letter = (97..=122).contains(&cp);
        let is_known_symbol = js_from_char_code(cp).is_some_and(is_symbol_key);
        if !is_latin_letter && !is_known_symbol {
            return true;
        }
    }

    false
}

fn parse_modify_other_keys_sequence(data: &str) -> Option<ParsedModifyOtherKeysSequence> {
    let captures = MODIFY_OTHER_KEYS_REGEX.captures(data)?;
    let mod_value = parse_int(captures.get(1)?.as_str())?;
    let codepoint = parse_int(captures.get(2)?.as_str())?;
    Some(ParsedModifyOtherKeysSequence {
        codepoint,
        modifier: mod_value - 1,
    })
}

/// Match the xterm modifyOtherKeys format: CSI 27 ; modifiers ; keycode ~
fn matches_modify_other_keys(data: &str, expected_keycode: i64, expected_modifier: i64) -> bool {
    parse_modify_other_keys_sequence(data)
        .is_some_and(|p| p.codepoint == expected_keycode && p.modifier == expected_modifier)
}

fn env_is_set(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|value| !value.is_empty())
}

fn is_windows_terminal_session() -> bool {
    env_is_set("WT_SESSION")
        && !env_is_set("SSH_CONNECTION")
        && !env_is_set("SSH_CLIENT")
        && !env_is_set("SSH_TTY")
}

/// Raw 0x08 (BS) is ambiguous in legacy terminals: Windows Terminal uses it for
/// Ctrl+Backspace, some legacy terminals and tmux setups for plain Backspace.
fn matches_raw_backspace(data: &str, expected_modifier: i64) -> bool {
    if data == "\x7f" {
        return expected_modifier == 0;
    }
    if data != "\x08" {
        return false;
    }
    if is_windows_terminal_session() {
        expected_modifier == modifiers::CTRL
    } else {
        expected_modifier == 0
    }
}

// =============================================================================
// Generic Key Matching
// =============================================================================

/// Control character for a key, using the universal formula `code & 0x1f`.
fn raw_ctrl_char(key: char) -> Option<char> {
    let lowered = key.to_ascii_lowercase();
    let code = lowered as u32;
    if lowered.is_ascii_lowercase() || matches!(lowered, '[' | '\\' | ']' | '_') {
        return char::from_u32(code & 0x1f);
    }
    // `-` shares the physical key with `_` on US keyboards.
    if lowered == '-' {
        return char::from_u32(31);
    }
    None
}

fn matches_printable_modify_other_keys(
    data: &str,
    expected_keycode: i64,
    expected_modifier: i64,
) -> bool {
    if expected_modifier == 0 {
        return false;
    }
    let Some(parsed) = parse_modify_other_keys_sequence(data) else {
        return false;
    };
    if parsed.modifier != expected_modifier {
        return false;
    }
    normalize_shifted_letter_identity_codepoint(parsed.codepoint, parsed.modifier)
        == normalize_shifted_letter_identity_codepoint(expected_keycode, expected_modifier)
}

fn format_key_name_with_modifiers(key_name: &str, modifier: i64) -> Option<String> {
    let mut mods: Vec<&str> = Vec::new();
    let effective_mod = modifier & !LOCK_MASK;
    let supported_modifier_mask =
        modifiers::SHIFT | modifiers::CTRL | modifiers::ALT | modifiers::SUPER;
    if (effective_mod & !supported_modifier_mask) != 0 {
        return None;
    }
    if effective_mod & modifiers::SHIFT != 0 {
        mods.push("shift");
    }
    if effective_mod & modifiers::CTRL != 0 {
        mods.push("ctrl");
    }
    if effective_mod & modifiers::ALT != 0 {
        mods.push("alt");
    }
    if effective_mod & modifiers::SUPER != 0 {
        mods.push("super");
    }
    Some(if mods.is_empty() {
        key_name.to_string()
    } else {
        format!("{}+{}", mods.join("+"), key_name)
    })
}

struct ParsedKeyId {
    key: String,
    ctrl: bool,
    shift: bool,
    alt: bool,
    super_modifier: bool,
}

fn parse_key_id(key_id: &str) -> Option<ParsedKeyId> {
    let lowered = key_id.to_lowercase();
    let parts: Vec<&str> = lowered.split('+').collect();
    let key = parts.last().copied()?;
    if key.is_empty() {
        return None;
    }
    Some(ParsedKeyId {
        key: key.to_string(),
        ctrl: parts.contains(&"ctrl"),
        shift: parts.contains(&"shift"),
        alt: parts.contains(&"alt"),
        super_modifier: parts.contains(&"super"),
    })
}

/// Match input data against a key identifier string.
///
/// Supported identifiers: single keys ("escape", "tab", "enter", "backspace",
/// "delete", "home", "end", "space"), arrows, and modifier combinations such as
/// "ctrl+c", "shift+tab", "alt+enter", "super+k", "ctrl+shift+p".
pub fn matches_key(data: &str, key_id: &str) -> bool {
    let Some(parsed) = parse_key_id(key_id) else {
        return false;
    };

    let kitty_active = is_kitty_protocol_active();
    let key = parsed.key.as_str();
    let mut modifier = 0;
    if parsed.shift {
        modifier |= modifiers::SHIFT;
    }
    if parsed.alt {
        modifier |= modifiers::ALT;
    }
    if parsed.ctrl {
        modifier |= modifiers::CTRL;
    }
    if parsed.super_modifier {
        modifier |= modifiers::SUPER;
    }

    match key {
        "escape" | "esc" => {
            if modifier != 0 {
                return false;
            }
            return data == "\x1b"
                || matches_kitty_sequence(data, codepoints::ESCAPE, 0)
                || matches_modify_other_keys(data, codepoints::ESCAPE, 0);
        }

        "space" => {
            if !kitty_active {
                if modifier == modifiers::CTRL && data == "\x00" {
                    return true;
                }
                if modifier == modifiers::ALT && data == "\x1b " {
                    return true;
                }
            }
            if modifier == 0 {
                return data == " "
                    || matches_kitty_sequence(data, codepoints::SPACE, 0)
                    || matches_modify_other_keys(data, codepoints::SPACE, 0);
            }
            return matches_kitty_sequence(data, codepoints::SPACE, modifier)
                || matches_modify_other_keys(data, codepoints::SPACE, modifier);
        }

        "tab" => {
            if modifier == modifiers::SHIFT {
                return data == "\x1b[Z"
                    || matches_kitty_sequence(data, codepoints::TAB, modifiers::SHIFT)
                    || matches_modify_other_keys(data, codepoints::TAB, modifiers::SHIFT);
            }
            if modifier == 0 {
                return data == "\t" || matches_kitty_sequence(data, codepoints::TAB, 0);
            }
            return matches_kitty_sequence(data, codepoints::TAB, modifier)
                || matches_modify_other_keys(data, codepoints::TAB, modifier);
        }

        "enter" | "return" => {
            if modifier == modifiers::SHIFT {
                if matches_kitty_sequence(data, codepoints::ENTER, modifiers::SHIFT)
                    || matches_kitty_sequence(data, codepoints::KP_ENTER, modifiers::SHIFT)
                {
                    return true;
                }
                if matches_modify_other_keys(data, codepoints::ENTER, modifiers::SHIFT) {
                    return true;
                }
                // With Kitty protocol active, legacy sequences are custom
                // terminal mappings: \x1b\r (kitty send_text), \n (Ghostty).
                if kitty_active {
                    return data == "\x1b\r" || data == "\n";
                }
                return false;
            }
            if modifier == modifiers::ALT {
                if matches_kitty_sequence(data, codepoints::ENTER, modifiers::ALT)
                    || matches_kitty_sequence(data, codepoints::KP_ENTER, modifiers::ALT)
                {
                    return true;
                }
                if matches_modify_other_keys(data, codepoints::ENTER, modifiers::ALT) {
                    return true;
                }
                if !kitty_active {
                    return data == "\x1b\r";
                }
                return false;
            }
            if modifier == 0 {
                return data == "\r"
                    || (!kitty_active && data == "\n")
                    || data == "\x1bOM"
                    || matches_kitty_sequence(data, codepoints::ENTER, 0)
                    || matches_kitty_sequence(data, codepoints::KP_ENTER, 0);
            }
            return matches_kitty_sequence(data, codepoints::ENTER, modifier)
                || matches_kitty_sequence(data, codepoints::KP_ENTER, modifier)
                || matches_modify_other_keys(data, codepoints::ENTER, modifier);
        }

        "backspace" => {
            if modifier == modifiers::ALT {
                if data == "\x1b\x7f" || data == "\x1b\x08" {
                    return true;
                }
                return matches_kitty_sequence(data, codepoints::BACKSPACE, modifiers::ALT)
                    || matches_modify_other_keys(data, codepoints::BACKSPACE, modifiers::ALT);
            }
            if modifier == modifiers::CTRL {
                if matches_raw_backspace(data, modifiers::CTRL) {
                    return true;
                }
                return matches_kitty_sequence(data, codepoints::BACKSPACE, modifiers::CTRL)
                    || matches_modify_other_keys(data, codepoints::BACKSPACE, modifiers::CTRL);
            }
            if modifier == 0 {
                return matches_raw_backspace(data, 0)
                    || matches_kitty_sequence(data, codepoints::BACKSPACE, 0)
                    || matches_modify_other_keys(data, codepoints::BACKSPACE, 0);
            }
            return matches_kitty_sequence(data, codepoints::BACKSPACE, modifier)
                || matches_modify_other_keys(data, codepoints::BACKSPACE, modifier);
        }

        "insert" => {
            return matches_functional_key(data, "insert", functional_codepoints::INSERT, modifier);
        }
        "delete" => {
            return matches_functional_key(data, "delete", functional_codepoints::DELETE, modifier);
        }

        "clear" => {
            if modifier == 0 {
                return matches_legacy_sequence(data, legacy_key_sequences("clear"));
            }
            return matches_legacy_modifier_sequence(data, "clear", modifier);
        }

        "home" => {
            return matches_functional_key(data, "home", functional_codepoints::HOME, modifier);
        }
        "end" => {
            return matches_functional_key(data, "end", functional_codepoints::END, modifier);
        }
        "pageup" => {
            return matches_functional_key(
                data,
                "pageUp",
                functional_codepoints::PAGE_UP,
                modifier,
            );
        }
        "pagedown" => {
            return matches_functional_key(
                data,
                "pageDown",
                functional_codepoints::PAGE_DOWN,
                modifier,
            );
        }

        "up" => {
            if modifier == modifiers::ALT {
                return data == "\x1bp"
                    || matches_kitty_sequence(data, arrow_codepoints::UP, modifiers::ALT);
            }
            return matches_functional_key(data, "up", arrow_codepoints::UP, modifier);
        }

        "down" => {
            if modifier == modifiers::ALT {
                return data == "\x1bn"
                    || matches_kitty_sequence(data, arrow_codepoints::DOWN, modifiers::ALT);
            }
            return matches_functional_key(data, "down", arrow_codepoints::DOWN, modifier);
        }

        "left" => {
            if modifier == modifiers::ALT {
                return data == "\x1b[1;3D"
                    || (!kitty_active && data == "\x1bB")
                    || data == "\x1bb"
                    || matches_kitty_sequence(data, arrow_codepoints::LEFT, modifiers::ALT);
            }
            if modifier == modifiers::CTRL {
                return data == "\x1b[1;5D"
                    || matches_legacy_modifier_sequence(data, "left", modifiers::CTRL)
                    || matches_kitty_sequence(data, arrow_codepoints::LEFT, modifiers::CTRL);
            }
            return matches_functional_key(data, "left", arrow_codepoints::LEFT, modifier);
        }

        "right" => {
            if modifier == modifiers::ALT {
                return data == "\x1b[1;3C"
                    || (!kitty_active && data == "\x1bF")
                    || data == "\x1bf"
                    || matches_kitty_sequence(data, arrow_codepoints::RIGHT, modifiers::ALT);
            }
            if modifier == modifiers::CTRL {
                return data == "\x1b[1;5C"
                    || matches_legacy_modifier_sequence(data, "right", modifiers::CTRL)
                    || matches_kitty_sequence(data, arrow_codepoints::RIGHT, modifiers::CTRL);
            }
            return matches_functional_key(data, "right", arrow_codepoints::RIGHT, modifier);
        }

        "f1" | "f2" | "f3" | "f4" | "f5" | "f6" | "f7" | "f8" | "f9" | "f10" | "f11" | "f12" => {
            if modifier != 0 {
                return false;
            }
            return matches_legacy_sequence(data, legacy_key_sequences(key));
        }

        _ => {}
    }

    // Single letter/digit keys and symbols.
    let mut key_chars = key.chars();
    let (Some(key_char), None) = (key_chars.next(), key_chars.next()) else {
        return false;
    };
    if !(key_char.is_ascii_lowercase() || key_char.is_ascii_digit() || is_symbol_key(key_char)) {
        return false;
    }

    let codepoint = i64::from(u32::from(key_char));
    let raw_ctrl = raw_ctrl_char(key_char);
    let is_letter = key_char.is_ascii_lowercase();
    let is_digit = key_char.is_ascii_digit();

    if modifier == modifiers::CTRL + modifiers::ALT
        && !kitty_active
        && let Some(raw_ctrl) = raw_ctrl
    {
        // Legacy: ctrl+alt+key is ESC followed by the control character. If the
        // legacy form does not match, fall through so CSI-u and modifyOtherKeys
        // sequences from tmux are still recognized.
        if data == format!("\x1b{raw_ctrl}") {
            return true;
        }
    }

    if modifier == modifiers::ALT
        && !kitty_active
        && (is_letter || is_digit || is_symbol_key(key_char))
    {
        // Legacy: alt+printable key is ESC followed by the key.
        if data == format!("\x1b{key_char}") {
            return true;
        }
    }

    if modifier == modifiers::CTRL {
        if let Some(raw_ctrl) = raw_ctrl
            && data.chars().eq(std::iter::once(raw_ctrl))
        {
            return true;
        }
        return matches_kitty_sequence(data, codepoint, modifiers::CTRL)
            || matches_printable_modify_other_keys(data, codepoint, modifiers::CTRL);
    }

    if modifier == modifiers::SHIFT + modifiers::CTRL {
        return matches_kitty_sequence(data, codepoint, modifiers::SHIFT + modifiers::CTRL)
            || matches_printable_modify_other_keys(
                data,
                codepoint,
                modifiers::SHIFT + modifiers::CTRL,
            );
    }

    if modifier == modifiers::SHIFT {
        if is_letter && data == key.to_uppercase() {
            return true;
        }
        return matches_kitty_sequence(data, codepoint, modifiers::SHIFT)
            || matches_printable_modify_other_keys(data, codepoint, modifiers::SHIFT);
    }

    if modifier != 0 {
        return matches_kitty_sequence(data, codepoint, modifier)
            || matches_printable_modify_other_keys(data, codepoint, modifier);
    }

    // Check both the raw character and the Kitty sequence (needed for release events).
    data == key || matches_kitty_sequence(data, codepoint, 0)
}

/// Shared shape of the legacy/Kitty matching for functional and arrow keys.
fn matches_functional_key(data: &str, legacy_key: &str, codepoint: i64, modifier: i64) -> bool {
    if modifier == 0 {
        return matches_legacy_sequence(data, legacy_key_sequences(legacy_key))
            || matches_kitty_sequence(data, codepoint, 0);
    }
    if matches_legacy_modifier_sequence(data, legacy_key, modifier) {
        return true;
    }
    matches_kitty_sequence(data, codepoint, modifier)
}

fn format_parsed_key(
    codepoint: i64,
    modifier: i64,
    base_layout_key: Option<i64>,
) -> Option<String> {
    let normalized_codepoint = normalize_kitty_functional_codepoint(codepoint);
    let identity_codepoint =
        normalize_shifted_letter_identity_codepoint(normalized_codepoint, modifier);

    // Use the base layout key only when the codepoint is not a recognized Latin
    // letter, digit, or symbol; for those the codepoint is authoritative
    // regardless of the physical key position.
    let is_latin_letter = (97..=122).contains(&identity_codepoint);
    let is_digit = (48..=57).contains(&identity_codepoint);
    let is_known_symbol = js_from_char_code(identity_codepoint).is_some_and(is_symbol_key);
    let effective_codepoint = if is_latin_letter || is_digit || is_known_symbol {
        identity_codepoint
    } else {
        base_layout_key.unwrap_or(identity_codepoint)
    };

    let key_name: Option<String> = if effective_codepoint == codepoints::ESCAPE {
        Some("escape".to_string())
    } else if effective_codepoint == codepoints::TAB {
        Some("tab".to_string())
    } else if effective_codepoint == codepoints::ENTER
        || effective_codepoint == codepoints::KP_ENTER
    {
        Some("enter".to_string())
    } else if effective_codepoint == codepoints::SPACE {
        Some("space".to_string())
    } else if effective_codepoint == codepoints::BACKSPACE {
        Some("backspace".to_string())
    } else if effective_codepoint == functional_codepoints::DELETE {
        Some("delete".to_string())
    } else if effective_codepoint == functional_codepoints::INSERT {
        Some("insert".to_string())
    } else if effective_codepoint == functional_codepoints::HOME {
        Some("home".to_string())
    } else if effective_codepoint == functional_codepoints::END {
        Some("end".to_string())
    } else if effective_codepoint == functional_codepoints::PAGE_UP {
        Some("pageUp".to_string())
    } else if effective_codepoint == functional_codepoints::PAGE_DOWN {
        Some("pageDown".to_string())
    } else if effective_codepoint == arrow_codepoints::UP {
        Some("up".to_string())
    } else if effective_codepoint == arrow_codepoints::DOWN {
        Some("down".to_string())
    } else if effective_codepoint == arrow_codepoints::LEFT {
        Some("left".to_string())
    } else if effective_codepoint == arrow_codepoints::RIGHT {
        Some("right".to_string())
    } else if (48..=57).contains(&effective_codepoint) || (97..=122).contains(&effective_codepoint)
    {
        js_from_char_code(effective_codepoint).map(|c| c.to_string())
    } else {
        js_from_char_code(effective_codepoint)
            .filter(|c| is_symbol_key(*c))
            .map(|c| c.to_string())
    };

    format_key_name_with_modifiers(&key_name?, modifier)
}

/// Parse input data and return the key identifier if recognized.
pub fn parse_key(data: &str) -> Option<String> {
    if let Some(kitty) = parse_kitty_sequence(data) {
        return format_parsed_key(kitty.codepoint, kitty.modifier, kitty.base_layout_key);
    }

    if let Some(modify_other_keys) = parse_modify_other_keys_sequence(data) {
        return format_parsed_key(
            modify_other_keys.codepoint,
            modify_other_keys.modifier,
            None,
        );
    }

    let kitty_active = is_kitty_protocol_active();

    // Mode-aware legacy sequences: with Kitty protocol active, ambiguous
    // sequences are custom terminal mappings (\x1b\r = kitty, \n = Ghostty).
    if kitty_active && (data == "\x1b\r" || data == "\n") {
        return Some("shift+enter".to_string());
    }

    if let Some(key_id) = legacy_sequence_key_id(data) {
        return Some(key_id.to_string());
    }

    let simple = match data {
        "\x1b" => Some("escape"),
        "\x1c" => Some("ctrl+\\"),
        "\x1d" => Some("ctrl+]"),
        "\x1f" => Some("ctrl+-"),
        "\x1b\x1b" => Some("ctrl+alt+["),
        "\x1b\x1c" => Some("ctrl+alt+\\"),
        "\x1b\x1d" => Some("ctrl+alt+]"),
        "\x1b\x1f" => Some("ctrl+alt+-"),
        "\t" => Some("tab"),
        "\r" | "\x1bOM" => Some("enter"),
        "\x00" => Some("ctrl+space"),
        " " => Some("space"),
        "\x7f" => Some("backspace"),
        "\x1b[Z" => Some("shift+tab"),
        "\x1b\x7f" | "\x1b\x08" => Some("alt+backspace"),
        "\x1b[A" => Some("up"),
        "\x1b[B" => Some("down"),
        "\x1b[C" => Some("right"),
        "\x1b[D" => Some("left"),
        "\x1b[H" | "\x1bOH" => Some("home"),
        "\x1b[F" | "\x1bOF" => Some("end"),
        "\x1b[3~" => Some("delete"),
        "\x1b[5~" => Some("pageUp"),
        "\x1b[6~" => Some("pageDown"),
        _ => None,
    };
    if let Some(key_id) = simple {
        return Some(key_id.to_string());
    }

    if !kitty_active && data == "\n" {
        return Some("enter".to_string());
    }
    if data == "\x08" {
        return Some(
            if is_windows_terminal_session() {
                "ctrl+backspace"
            } else {
                "backspace"
            }
            .to_string(),
        );
    }
    if !kitty_active {
        if data == "\x1b\r" {
            return Some("alt+enter".to_string());
        }
        if data == "\x1b " {
            return Some("alt+space".to_string());
        }
        if data == "\x1bB" {
            return Some("alt+left".to_string());
        }
        if data == "\x1bF" {
            return Some("alt+right".to_string());
        }
        // `data.length === 2 && data[0] === "\x1b"` counts UTF-16 code units.
        let units: Vec<u16> = data.encode_utf16().collect();
        if units.len() == 2 && units[0] == 0x1b {
            let code = i64::from(units[1]);
            if (1..=26).contains(&code) {
                let letter = js_from_char_code(code + 96)?;
                return Some(format!("ctrl+alt+{letter}"));
            }
            if let Some(key) = js_from_char_code(code)
                && ((97..=122).contains(&code) || (48..=57).contains(&code) || is_symbol_key(key))
            {
                return Some(format!("alt+{key}"));
            }
        }
    }

    // Raw Ctrl+letter and printable ASCII.
    let units: Vec<u16> = data.encode_utf16().collect();
    if units.len() == 1 {
        let code = i64::from(units[0]);
        if (1..=26).contains(&code) {
            let letter = js_from_char_code(code + 96)?;
            return Some(format!("ctrl+{letter}"));
        }
        if (32..=126).contains(&code) {
            return Some(data.to_string());
        }
    }

    None
}

// =============================================================================
// Kitty CSI-u Printable Decoding
// =============================================================================

const KITTY_PRINTABLE_ALLOWED_MODIFIERS: i64 = modifiers::SHIFT | LOCK_MASK;

/// Decode a Kitty CSI-u sequence into a printable character, if applicable.
///
/// Only plain or Shift-modified keys are accepted; Ctrl, Alt and unsupported
/// modifier combinations are rejected (they are handled by keybinding matching).
/// Prefers the shifted keycode when Shift is held and one is reported.
pub fn decode_kitty_printable(data: &str) -> Option<String> {
    let captures = CSI_U_REGEX.captures(data)?;
    let codepoint = parse_int(captures.get(1)?.as_str())?;

    let shifted_key = captures
        .get(2)
        .filter(|m| !m.as_str().is_empty())
        .and_then(|m| parse_int(m.as_str()));
    let mod_value = captures
        .get(4)
        .and_then(|m| parse_int(m.as_str()))
        .unwrap_or(1);
    let modifier = mod_value - 1;

    if (modifier & !KITTY_PRINTABLE_ALLOWED_MODIFIERS) != 0 {
        return None;
    }
    if modifier & (modifiers::ALT | modifiers::CTRL) != 0 {
        return None;
    }

    let mut effective_codepoint = codepoint;
    if modifier & modifiers::SHIFT != 0
        && let Some(shifted_key) = shifted_key
    {
        effective_codepoint = shifted_key;
    }
    effective_codepoint = normalize_kitty_functional_codepoint(effective_codepoint);
    if effective_codepoint < 32 {
        return None;
    }

    // `String.fromCodePoint` throws for invalid code points; TS catches it.
    char::from_u32(u32::try_from(effective_codepoint).ok()?).map(|c| c.to_string())
}

fn decode_modify_other_keys_printable(data: &str) -> Option<String> {
    let parsed = parse_modify_other_keys_sequence(data)?;
    let modifier = parsed.modifier & !LOCK_MASK;
    if (modifier & !modifiers::SHIFT) != 0 {
        return None;
    }
    if parsed.codepoint < 32 {
        return None;
    }
    char::from_u32(u32::try_from(parsed.codepoint).ok()?).map(|c| c.to_string())
}

/// Decode a printable character from a Kitty CSI-u or modifyOtherKeys sequence.
pub fn decode_printable_key(data: &str) -> Option<String> {
    decode_kitty_printable(data).or_else(|| decode_modify_other_keys_printable(data))
}
