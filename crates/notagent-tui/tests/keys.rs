//! Port of `packages/tui/test/keys.test.ts` (633 LOC).
//!
//! The Kitty protocol flag and the environment are process-global in TS as well
//! as here, but Rust runs tests in parallel threads — every test therefore takes
//! the same guard and restores the previous state.

use std::sync::{Mutex, MutexGuard, OnceLock};

use notagent_tui::keys::{
    decode_kitty_printable, decode_printable_key, matches_key, parse_key, set_kitty_protocol_active,
};

fn guard() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Runs `body` with the given environment variables set, restoring them after.
fn with_env_vars(vars: &[(&str, Option<&str>)], body: impl FnOnce()) {
    let previous: Vec<(String, Option<String>)> = vars
        .iter()
        .map(|(name, _)| ((*name).to_string(), std::env::var(name).ok()))
        .collect();
    for (name, value) in vars {
        // SAFETY: tests are serialized by `guard()`, so no other thread reads
        // or writes the environment concurrently.
        unsafe {
            match value {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
    }
    body();
    for (name, value) in &previous {
        // SAFETY: see above.
        unsafe {
            match value {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
    }
}

fn parsed(data: &str) -> Option<String> {
    parse_key(data)
}

// =============================================================================
// describe("matchesKey") > Kitty protocol with alternate keys (non-Latin layouts)
// =============================================================================

#[test]
fn should_match_ctrl_c_when_pressing_ctrl_cyrillic_with_base_layout_key() {
    let _guard = guard();
    set_kitty_protocol_active(true);
    // Cyrillic 'с' = 1089, Latin 'c' = 99; CSI 1089::99;5u (ctrl=4, +1=5)
    assert!(matches_key("\x1b[1089::99;5u", "ctrl+c"));
    set_kitty_protocol_active(false);
}

#[test]
fn should_match_ctrl_d_when_pressing_ctrl_cyrillic_v_with_base_layout_key() {
    let _guard = guard();
    set_kitty_protocol_active(true);
    assert!(matches_key("\x1b[1074::100;5u", "ctrl+d"));
    set_kitty_protocol_active(false);
}

#[test]
fn should_match_ctrl_z_when_pressing_ctrl_cyrillic_ya_with_base_layout_key() {
    let _guard = guard();
    set_kitty_protocol_active(true);
    assert!(matches_key("\x1b[1103::122;5u", "ctrl+z"));
    set_kitty_protocol_active(false);
}

#[test]
fn should_match_ctrl_shift_p_with_base_layout_key() {
    let _guard = guard();
    set_kitty_protocol_active(true);
    assert!(matches_key("\x1b[1079::112;6u", "ctrl+shift+p"));
    set_kitty_protocol_active(false);
}

#[test]
fn should_still_match_direct_codepoint_when_no_base_layout_key() {
    let _guard = guard();
    set_kitty_protocol_active(true);
    assert!(matches_key("\x1b[99;5u", "ctrl+c"));
    set_kitty_protocol_active(false);
}

#[test]
fn should_match_super_modified_kitty_bindings_including_combined_modifiers() {
    let _guard = guard();
    set_kitty_protocol_active(true);
    assert!(matches_key("\x1b[107;9u", "super+k"));
    assert!(matches_key("\x1b[13;9u", "super+enter"));
    assert!(matches_key("\x1b[107;13u", "ctrl+super+k"));
    assert!(matches_key("\x1b[107;14u", "ctrl+shift+super+k"));
    assert!(!matches_key("\x1b[107;13u", "super+k"));
    assert_eq!(parsed("\x1b[107;9u").as_deref(), Some("super+k"));
    assert_eq!(parsed("\x1b[13;9u").as_deref(), Some("super+enter"));
    assert_eq!(parsed("\x1b[107;13u").as_deref(), Some("ctrl+super+k"));
    assert_eq!(
        parsed("\x1b[107;14u").as_deref(),
        Some("shift+ctrl+super+k")
    );
    set_kitty_protocol_active(false);
}

#[test]
fn should_match_digit_bindings_via_kitty_csi_u() {
    let _guard = guard();
    set_kitty_protocol_active(true);
    assert!(matches_key("\x1b[49u", "1"));
    assert!(matches_key("\x1b[49;5u", "ctrl+1"));
    assert!(!matches_key("\x1b[49;5u", "ctrl+2"));
    assert_eq!(parsed("\x1b[49u").as_deref(), Some("1"));
    assert_eq!(parsed("\x1b[49;5u").as_deref(), Some("ctrl+1"));
    set_kitty_protocol_active(false);
}

#[test]
fn should_normalize_kitty_keypad_functional_keys() {
    let _guard = guard();
    set_kitty_protocol_active(true);
    assert!(matches_key("\x1b[57400u", "1"));
    assert!(matches_key("\x1b[57410u", "/"));
    assert!(matches_key("\x1b[57417u", "left"));
    assert!(matches_key("\x1b[57426u", "delete"));
    assert_eq!(parsed("\x1b[57399u").as_deref(), Some("0"));
    assert_eq!(parsed("\x1b[57409u").as_deref(), Some("."));
    assert_eq!(parsed("\x1b[57413u").as_deref(), Some("+"));
    assert_eq!(parsed("\x1b[57416u").as_deref(), Some(","));
    assert_eq!(parsed("\x1b[57417u").as_deref(), Some("left"));
    assert_eq!(parsed("\x1b[57418u").as_deref(), Some("right"));
    assert_eq!(parsed("\x1b[57419u").as_deref(), Some("up"));
    assert_eq!(parsed("\x1b[57420u").as_deref(), Some("down"));
    assert_eq!(parsed("\x1b[57421u").as_deref(), Some("pageUp"));
    assert_eq!(parsed("\x1b[57422u").as_deref(), Some("pageDown"));
    assert_eq!(parsed("\x1b[57423u").as_deref(), Some("home"));
    assert_eq!(parsed("\x1b[57424u").as_deref(), Some("end"));
    assert_eq!(parsed("\x1b[57425u").as_deref(), Some("insert"));
    assert_eq!(parsed("\x1b[57426u").as_deref(), Some("delete"));
    set_kitty_protocol_active(false);
}

#[test]
fn should_handle_shifted_key_in_format() {
    let _guard = guard();
    set_kitty_protocol_active(true);
    assert!(matches_key("\x1b[99:67:99;2u", "shift+c"));
    set_kitty_protocol_active(false);
}

#[test]
fn should_handle_event_type_in_format() {
    let _guard = guard();
    set_kitty_protocol_active(true);
    assert!(matches_key("\x1b[1089::99;5:3u", "ctrl+c"));
    set_kitty_protocol_active(false);
}

#[test]
fn should_handle_full_format_with_shifted_base_and_event_type() {
    let _guard = guard();
    set_kitty_protocol_active(true);
    assert!(matches_key("\x1b[1089:1057:99;6:2u", "ctrl+shift+c"));
    set_kitty_protocol_active(false);
}

#[test]
fn should_prefer_codepoint_for_latin_letters_even_when_base_layout_differs() {
    let _guard = guard();
    set_kitty_protocol_active(true);
    // Dvorak Ctrl+K reports codepoint 'k' (107) and base layout 'v' (118)
    assert!(matches_key("\x1b[107::118;5u", "ctrl+k"));
    assert!(!matches_key("\x1b[107::118;5u", "ctrl+v"));
    set_kitty_protocol_active(false);
}

#[test]
fn should_prefer_codepoint_for_symbol_keys_even_when_base_layout_differs() {
    let _guard = guard();
    set_kitty_protocol_active(true);
    assert!(matches_key("\x1b[47::91;5u", "ctrl+/"));
    assert!(!matches_key("\x1b[47::91;5u", "ctrl+["));
    set_kitty_protocol_active(false);
}

#[test]
fn should_not_match_wrong_key_even_with_base_layout() {
    let _guard = guard();
    set_kitty_protocol_active(true);
    assert!(!matches_key("\x1b[1089::99;5u", "ctrl+d"));
    set_kitty_protocol_active(false);
}

#[test]
fn should_not_match_wrong_modifiers_even_with_base_layout() {
    let _guard = guard();
    set_kitty_protocol_active(true);
    assert!(!matches_key("\x1b[1089::99;5u", "ctrl+shift+c"));
    set_kitty_protocol_active(false);
}

// =============================================================================
// describe("modifyOtherKeys matching")
// =============================================================================

#[test]
fn should_match_xterm_modify_other_keys_letters() {
    let _guard = guard();
    set_kitty_protocol_active(false);
    assert!(matches_key("\x1b[27;5;99~", "ctrl+c"));
    assert_eq!(parsed("\x1b[27;5;99~").as_deref(), Some("ctrl+c"));
    assert!(matches_key("\x1b[27;5;100~", "ctrl+d"));
    assert_eq!(parsed("\x1b[27;5;100~").as_deref(), Some("ctrl+d"));
    assert!(matches_key("\x1b[27;5;122~", "ctrl+z"));
    assert_eq!(parsed("\x1b[27;5;122~").as_deref(), Some("ctrl+z"));
}

#[test]
fn should_match_xterm_modify_other_keys_enter_variants() {
    let _guard = guard();
    set_kitty_protocol_active(false);
    assert!(matches_key("\x1b[27;5;13~", "ctrl+enter"));
    assert!(matches_key("\x1b[27;2;13~", "shift+enter"));
    assert!(matches_key("\x1b[27;3;13~", "alt+enter"));
    assert_eq!(parsed("\x1b[27;5;13~").as_deref(), Some("ctrl+enter"));
    assert_eq!(parsed("\x1b[27;2;13~").as_deref(), Some("shift+enter"));
    assert_eq!(parsed("\x1b[27;3;13~").as_deref(), Some("alt+enter"));
}

#[test]
fn should_match_xterm_modify_other_keys_tab_variants() {
    let _guard = guard();
    set_kitty_protocol_active(false);
    assert!(matches_key("\x1b[27;2;9~", "shift+tab"));
    assert!(matches_key("\x1b[27;5;9~", "ctrl+tab"));
    assert!(matches_key("\x1b[27;3;9~", "alt+tab"));
    assert_eq!(parsed("\x1b[27;2;9~").as_deref(), Some("shift+tab"));
    assert_eq!(parsed("\x1b[27;5;9~").as_deref(), Some("ctrl+tab"));
    assert_eq!(parsed("\x1b[27;3;9~").as_deref(), Some("alt+tab"));
}

#[test]
fn should_match_xterm_modify_other_keys_backspace_variants() {
    let _guard = guard();
    set_kitty_protocol_active(false);
    assert!(matches_key("\x1b[27;1;127~", "backspace"));
    assert!(matches_key("\x1b[27;5;127~", "ctrl+backspace"));
    assert!(matches_key("\x1b[27;3;127~", "alt+backspace"));
    assert_eq!(parsed("\x1b[27;1;127~").as_deref(), Some("backspace"));
    assert_eq!(parsed("\x1b[27;5;127~").as_deref(), Some("ctrl+backspace"));
    assert_eq!(parsed("\x1b[27;3;127~").as_deref(), Some("alt+backspace"));
}

#[test]
fn should_match_xterm_modify_other_keys_escape() {
    let _guard = guard();
    set_kitty_protocol_active(false);
    assert!(matches_key("\x1b[27;1;27~", "escape"));
    assert_eq!(parsed("\x1b[27;1;27~").as_deref(), Some("escape"));
}

#[test]
fn should_match_xterm_modify_other_keys_space_variants() {
    let _guard = guard();
    set_kitty_protocol_active(false);
    assert!(matches_key("\x1b[27;1;32~", "space"));
    assert!(matches_key("\x1b[27;5;32~", "ctrl+space"));
    assert_eq!(parsed("\x1b[27;1;32~").as_deref(), Some("space"));
    assert_eq!(parsed("\x1b[27;5;32~").as_deref(), Some("ctrl+space"));
}

#[test]
fn should_match_xterm_modify_other_keys_symbol_combos() {
    let _guard = guard();
    set_kitty_protocol_active(false);
    assert!(matches_key("\x1b[27;5;47~", "ctrl+/"));
    assert_eq!(parsed("\x1b[27;5;47~").as_deref(), Some("ctrl+/"));
}

#[test]
fn should_match_xterm_modify_other_keys_digit_combos() {
    let _guard = guard();
    set_kitty_protocol_active(false);
    assert!(matches_key("\x1b[27;5;49~", "ctrl+1"));
    assert!(matches_key("\x1b[27;2;49~", "shift+1"));
    assert_eq!(parsed("\x1b[27;5;49~").as_deref(), Some("ctrl+1"));
    assert_eq!(parsed("\x1b[27;2;49~").as_deref(), Some("shift+1"));
}

#[test]
fn should_match_xterm_modify_other_keys_shifted_uppercase_letters() {
    let _guard = guard();
    set_kitty_protocol_active(false);
    assert!(matches_key("\x1b[27;2;69~", "shift+e"));
    assert!(matches_key("\x1b[27;6;69~", "ctrl+shift+e"));
    assert_eq!(parsed("\x1b[27;2;69~").as_deref(), Some("shift+e"));
    assert_eq!(parsed("\x1b[27;6;69~").as_deref(), Some("shift+ctrl+e"));
}

#[test]
fn should_match_ctrl_alt_letter_via_csi_u_when_kitty_inactive() {
    let _guard = guard();
    set_kitty_protocol_active(false);
    assert!(matches_key("\x1b[104;7u", "ctrl+alt+h"));
    assert_eq!(parsed("\x1b[104;7u").as_deref(), Some("ctrl+alt+h"));
}

#[test]
fn should_match_ctrl_alt_letter_via_xterm_modify_other_keys() {
    let _guard = guard();
    set_kitty_protocol_active(false);
    assert!(matches_key("\x1b[27;7;104~", "ctrl+alt+h"));
    assert_eq!(parsed("\x1b[27;7;104~").as_deref(), Some("ctrl+alt+h"));
}

// =============================================================================
// describe("Legacy key matching")
// =============================================================================

#[test]
fn should_match_legacy_ctrl_c_and_ctrl_d() {
    let _guard = guard();
    set_kitty_protocol_active(false);
    assert!(matches_key("\x03", "ctrl+c"));
    assert!(matches_key("\x04", "ctrl+d"));
}

#[test]
fn should_match_escape_key() {
    let _guard = guard();
    assert!(matches_key("\x1b", "escape"));
}

#[test]
fn should_match_legacy_linefeed_as_enter() {
    let _guard = guard();
    set_kitty_protocol_active(false);
    assert!(matches_key("\n", "enter"));
    assert_eq!(parsed("\n").as_deref(), Some("enter"));
}

#[test]
fn should_treat_linefeed_as_shift_enter_when_kitty_active() {
    let _guard = guard();
    set_kitty_protocol_active(true);
    assert!(matches_key("\n", "shift+enter"));
    assert!(!matches_key("\n", "enter"));
    assert_eq!(parsed("\n").as_deref(), Some("shift+enter"));
    set_kitty_protocol_active(false);
}

#[test]
fn should_parse_ctrl_space() {
    let _guard = guard();
    set_kitty_protocol_active(false);
    assert!(matches_key("\x00", "ctrl+space"));
    assert_eq!(parsed("\x00").as_deref(), Some("ctrl+space"));
}

#[test]
fn should_match_legacy_ctrl_symbol() {
    let _guard = guard();
    set_kitty_protocol_active(false);
    assert!(matches_key("\x1c", "ctrl+\\"));
    assert_eq!(parsed("\x1c").as_deref(), Some("ctrl+\\"));
    assert!(matches_key("\x1d", "ctrl+]"));
    assert_eq!(parsed("\x1d").as_deref(), Some("ctrl+]"));
    assert!(matches_key("\x1f", "ctrl+_"));
    assert!(matches_key("\x1f", "ctrl+-"));
    assert_eq!(parsed("\x1f").as_deref(), Some("ctrl+-"));
}

#[test]
fn should_match_legacy_ctrl_alt_symbol() {
    let _guard = guard();
    set_kitty_protocol_active(false);
    assert!(matches_key("\x1b\x1b", "ctrl+alt+["));
    assert_eq!(parsed("\x1b\x1b").as_deref(), Some("ctrl+alt+["));
    assert!(matches_key("\x1b\x1c", "ctrl+alt+\\"));
    assert_eq!(parsed("\x1b\x1c").as_deref(), Some("ctrl+alt+\\"));
    assert!(matches_key("\x1b\x1d", "ctrl+alt+]"));
    assert_eq!(parsed("\x1b\x1d").as_deref(), Some("ctrl+alt+]"));
    assert!(matches_key("\x1b\x1f", "ctrl+alt+_"));
    assert!(matches_key("\x1b\x1f", "ctrl+alt+-"));
    assert_eq!(parsed("\x1b\x1f").as_deref(), Some("ctrl+alt+-"));
}

#[test]
fn should_treat_raw_0x08_as_plain_backspace_outside_windows_terminal() {
    let _guard = guard();
    set_kitty_protocol_active(false);
    with_env_vars(&[("WT_SESSION", None)], || {
        assert!(matches_key("\x7f", "backspace"));
        assert!(!matches_key("\x7f", "ctrl+backspace"));
        assert_eq!(parsed("\x7f").as_deref(), Some("backspace"));
        assert!(matches_key("\x08", "backspace"));
        assert!(!matches_key("\x08", "ctrl+backspace"));
        assert_eq!(parsed("\x08").as_deref(), Some("backspace"));
        assert!(matches_key("\x08", "ctrl+h"));
    });
}

#[test]
fn should_treat_raw_0x08_as_ctrl_backspace_in_local_windows_terminal() {
    let _guard = guard();
    set_kitty_protocol_active(false);
    with_env_vars(
        &[
            ("WT_SESSION", Some("test-session")),
            ("SSH_CONNECTION", None),
            ("SSH_CLIENT", None),
            ("SSH_TTY", None),
        ],
        || {
            assert!(matches_key("\x08", "ctrl+backspace"));
            assert!(!matches_key("\x08", "backspace"));
            assert_eq!(parsed("\x08").as_deref(), Some("ctrl+backspace"));
            assert!(matches_key("\x08", "ctrl+h"));
        },
    );
}

#[test]
fn should_treat_raw_0x08_as_plain_backspace_in_windows_terminal_over_ssh() {
    let _guard = guard();
    set_kitty_protocol_active(false);
    with_env_vars(
        &[
            ("WT_SESSION", Some("test-session")),
            ("SSH_CONNECTION", Some("1 2 3 4")),
            ("SSH_CLIENT", Some("1 2 3")),
            ("SSH_TTY", Some("/dev/pts/1")),
        ],
        || {
            assert!(!matches_key("\x08", "ctrl+backspace"));
            assert!(matches_key("\x08", "backspace"));
            assert_eq!(parsed("\x08").as_deref(), Some("backspace"));
            assert!(matches_key("\x08", "ctrl+h"));
        },
    );
}

#[test]
fn should_parse_legacy_alt_prefixed_sequences_when_kitty_inactive() {
    let _guard = guard();
    set_kitty_protocol_active(false);
    assert!(matches_key("\x1b ", "alt+space"));
    assert_eq!(parsed("\x1b ").as_deref(), Some("alt+space"));
    assert!(matches_key("\x1b\x08", "alt+backspace"));
    assert_eq!(parsed("\x1b\x08").as_deref(), Some("alt+backspace"));
    assert!(matches_key("\x1b\x03", "ctrl+alt+c"));
    assert_eq!(parsed("\x1b\x03").as_deref(), Some("ctrl+alt+c"));
    assert!(matches_key("\x1bB", "alt+left"));
    assert_eq!(parsed("\x1bB").as_deref(), Some("alt+left"));
    assert!(matches_key("\x1bF", "alt+right"));
    assert_eq!(parsed("\x1bF").as_deref(), Some("alt+right"));
    assert!(matches_key("\x1ba", "alt+a"));
    assert_eq!(parsed("\x1ba").as_deref(), Some("alt+a"));
    assert!(matches_key("\x1b1", "alt+1"));
    assert_eq!(parsed("\x1b1").as_deref(), Some("alt+1"));
    assert!(matches_key("\x1b,", "alt+,"));
    assert_eq!(parsed("\x1b,").as_deref(), Some("alt+,"));
    assert!(matches_key("\x1b.", "alt+."));
    assert_eq!(parsed("\x1b.").as_deref(), Some("alt+."));
    assert!(matches_key("\x1by", "alt+y"));
    assert_eq!(parsed("\x1by").as_deref(), Some("alt+y"));
    assert!(matches_key("\x1bz", "alt+z"));
    assert_eq!(parsed("\x1bz").as_deref(), Some("alt+z"));

    set_kitty_protocol_active(true);
    assert!(!matches_key("\x1b ", "alt+space"));
    assert_eq!(parsed("\x1b "), None);
    assert!(matches_key("\x1b\x08", "alt+backspace"));
    assert_eq!(parsed("\x1b\x08").as_deref(), Some("alt+backspace"));
    assert!(!matches_key("\x1b\x03", "ctrl+alt+c"));
    assert_eq!(parsed("\x1b\x03"), None);
    assert!(!matches_key("\x1bB", "alt+left"));
    assert_eq!(parsed("\x1bB"), None);
    assert!(!matches_key("\x1bF", "alt+right"));
    assert_eq!(parsed("\x1bF"), None);
    assert!(!matches_key("\x1ba", "alt+a"));
    assert_eq!(parsed("\x1ba"), None);
    assert!(!matches_key("\x1b1", "alt+1"));
    assert_eq!(parsed("\x1b1"), None);
    assert!(!matches_key("\x1b,", "alt+,"));
    assert_eq!(parsed("\x1b,"), None);
    assert!(!matches_key("\x1b.", "alt+."));
    assert_eq!(parsed("\x1b."), None);
    assert!(!matches_key("\x1by", "alt+y"));
    assert_eq!(parsed("\x1by"), None);
    set_kitty_protocol_active(false);
}

#[test]
fn should_match_arrow_keys() {
    let _guard = guard();
    assert!(matches_key("\x1b[A", "up"));
    assert!(matches_key("\x1b[B", "down"));
    assert!(matches_key("\x1b[C", "right"));
    assert!(matches_key("\x1b[D", "left"));
}

#[test]
fn should_match_ss3_arrows_and_home_end() {
    let _guard = guard();
    assert!(matches_key("\x1bOA", "up"));
    assert!(matches_key("\x1bOB", "down"));
    assert!(matches_key("\x1bOC", "right"));
    assert!(matches_key("\x1bOD", "left"));
    assert!(matches_key("\x1bOH", "home"));
    assert!(matches_key("\x1bOF", "end"));
}

#[test]
fn should_match_xterm_ctrl_modified_viewport_navigation() {
    let _guard = guard();
    assert!(matches_key("\x1b[1;5H", "ctrl+home"));
    assert!(matches_key("\x1b[1;5F", "ctrl+end"));
    assert!(matches_key("\x1b[5;5~", "ctrl+pageUp"));
    assert!(matches_key("\x1b[6;5~", "ctrl+pageDown"));
    assert_eq!(parsed("\x1b[1;5H").as_deref(), Some("ctrl+home"));
    assert_eq!(parsed("\x1b[1;5F").as_deref(), Some("ctrl+end"));
    assert_eq!(parsed("\x1b[5;5~").as_deref(), Some("ctrl+pageUp"));
    assert_eq!(parsed("\x1b[6;5~").as_deref(), Some("ctrl+pageDown"));
}

#[test]
fn should_match_legacy_function_keys_and_clear() {
    let _guard = guard();
    assert!(matches_key("\x1bOP", "f1"));
    assert!(matches_key("\x1b[24~", "f12"));
    assert!(matches_key("\x1b[E", "clear"));
}

#[test]
fn should_match_alt_arrows() {
    let _guard = guard();
    assert!(matches_key("\x1bp", "alt+up"));
    assert!(!matches_key("\x1bp", "up"));
}

#[test]
fn should_match_rxvt_modifier_sequences() {
    let _guard = guard();
    assert!(matches_key("\x1b[a", "shift+up"));
    assert!(matches_key("\x1bOa", "ctrl+up"));
    assert!(matches_key("\x1b[2$", "shift+insert"));
    assert!(matches_key("\x1b[2^", "ctrl+insert"));
    assert!(matches_key("\x1b[7$", "shift+home"));
}

// =============================================================================
// describe("decodeKittyPrintable") / describe("decodePrintableKey")
// =============================================================================

#[test]
fn should_decode_kitty_keypad_functional_keys_to_printable_characters() {
    let _guard = guard();
    assert_eq!(decode_kitty_printable("\x1b[57399u").as_deref(), Some("0"));
    assert_eq!(decode_kitty_printable("\x1b[57400u").as_deref(), Some("1"));
    assert_eq!(decode_kitty_printable("\x1b[57409u").as_deref(), Some("."));
    assert_eq!(decode_kitty_printable("\x1b[57410u").as_deref(), Some("/"));
    assert_eq!(decode_kitty_printable("\x1b[57411u").as_deref(), Some("*"));
    assert_eq!(decode_kitty_printable("\x1b[57412u").as_deref(), Some("-"));
    assert_eq!(decode_kitty_printable("\x1b[57413u").as_deref(), Some("+"));
    assert_eq!(decode_kitty_printable("\x1b[57415u").as_deref(), Some("="));
    assert_eq!(decode_kitty_printable("\x1b[57416u").as_deref(), Some(","));
    assert_eq!(decode_kitty_printable("\x1b[57417u"), None);
}

#[test]
fn should_decode_printable_xterm_modify_other_keys_sequences() {
    let _guard = guard();
    assert_eq!(decode_printable_key("\x1b[27;2;69~").as_deref(), Some("E"));
    assert_eq!(decode_printable_key("\x1b[27;2;196~").as_deref(), Some("Ä"));
    assert_eq!(decode_printable_key("\x1b[27;2;32~").as_deref(), Some(" "));
    assert_eq!(decode_printable_key("\x1b[27;2;13~"), None);
    assert_eq!(decode_printable_key("\x1b[27;6;69~"), None);
}

// =============================================================================
// describe("parseKey")
// =============================================================================

#[test]
fn should_return_latin_key_name_when_base_layout_key_is_present() {
    let _guard = guard();
    set_kitty_protocol_active(true);
    assert_eq!(parsed("\x1b[1089::99;5u").as_deref(), Some("ctrl+c"));
    set_kitty_protocol_active(false);
}

#[test]
fn parse_key_should_prefer_codepoint_for_latin_letters_when_base_layout_differs() {
    let _guard = guard();
    set_kitty_protocol_active(true);
    assert_eq!(parsed("\x1b[107::118;5u").as_deref(), Some("ctrl+k"));
    set_kitty_protocol_active(false);
}

#[test]
fn parse_key_should_prefer_codepoint_for_symbol_keys_when_base_layout_differs() {
    let _guard = guard();
    set_kitty_protocol_active(true);
    assert_eq!(parsed("\x1b[47::91;5u").as_deref(), Some("ctrl+/"));
    set_kitty_protocol_active(false);
}

#[test]
fn should_return_key_name_from_codepoint_when_no_base_layout() {
    let _guard = guard();
    set_kitty_protocol_active(true);
    assert_eq!(parsed("\x1b[99;5u").as_deref(), Some("ctrl+c"));
    set_kitty_protocol_active(false);
}

#[test]
fn should_parse_shifted_uppercase_csi_u_letters_as_shift_letter() {
    let _guard = guard();
    set_kitty_protocol_active(true);
    assert!(matches_key("\x1b[69;2u", "shift+e"));
    assert_eq!(parsed("\x1b[69;2u").as_deref(), Some("shift+e"));
    set_kitty_protocol_active(false);
}

#[test]
fn should_ignore_kitty_csi_u_with_unsupported_modifiers() {
    let _guard = guard();
    set_kitty_protocol_active(true);
    assert_eq!(parsed("\x1b[99;17u"), None);
    set_kitty_protocol_active(false);
}

#[test]
fn should_parse_legacy_ctrl_letter() {
    let _guard = guard();
    set_kitty_protocol_active(false);
    assert_eq!(parsed("\x03").as_deref(), Some("ctrl+c"));
    assert_eq!(parsed("\x04").as_deref(), Some("ctrl+d"));
}

#[test]
fn should_parse_special_keys() {
    let _guard = guard();
    set_kitty_protocol_active(false);
    assert_eq!(parsed("\x1b").as_deref(), Some("escape"));
    assert_eq!(parsed("\t").as_deref(), Some("tab"));
    assert_eq!(parsed("\r").as_deref(), Some("enter"));
    assert_eq!(parsed("\n").as_deref(), Some("enter"));
    assert_eq!(parsed("\x00").as_deref(), Some("ctrl+space"));
    assert_eq!(parsed(" ").as_deref(), Some("space"));
    assert_eq!(parsed("1").as_deref(), Some("1"));
    assert!(matches_key("1", "1"));
}

#[test]
fn should_parse_arrow_keys() {
    let _guard = guard();
    assert_eq!(parsed("\x1b[A").as_deref(), Some("up"));
    assert_eq!(parsed("\x1b[B").as_deref(), Some("down"));
    assert_eq!(parsed("\x1b[C").as_deref(), Some("right"));
    assert_eq!(parsed("\x1b[D").as_deref(), Some("left"));
}

#[test]
fn should_parse_ss3_arrows_and_home_end() {
    let _guard = guard();
    assert_eq!(parsed("\x1bOA").as_deref(), Some("up"));
    assert_eq!(parsed("\x1bOB").as_deref(), Some("down"));
    assert_eq!(parsed("\x1bOC").as_deref(), Some("right"));
    assert_eq!(parsed("\x1bOD").as_deref(), Some("left"));
    assert_eq!(parsed("\x1bOH").as_deref(), Some("home"));
    assert_eq!(parsed("\x1bOF").as_deref(), Some("end"));
}

#[test]
fn should_parse_legacy_function_and_modifier_sequences() {
    let _guard = guard();
    assert_eq!(parsed("\x1bOP").as_deref(), Some("f1"));
    assert_eq!(parsed("\x1b[24~").as_deref(), Some("f12"));
    assert_eq!(parsed("\x1b[E").as_deref(), Some("clear"));
    assert_eq!(parsed("\x1b[2^").as_deref(), Some("ctrl+insert"));
    assert_eq!(parsed("\x1bp").as_deref(), Some("alt+up"));
}

#[test]
fn should_parse_double_bracket_page_up() {
    let _guard = guard();
    assert_eq!(parsed("\x1b[[5~").as_deref(), Some("pageUp"));
}
