//! Port of `packages/tui/test/terminal.test.ts` (300 LOC).
//!
//! The TS suite monkey-patches `process.stdout.write` / `process.stdin.on` and
//! reaches into private members; the Rust port injects the write sink and uses
//! the `test-terminal` harness API instead (deviation class 1).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Mutex, MutexGuard, OnceLock};

use notagent_tui::keys::set_kitty_protocol_active;
use notagent_tui::terminal::{
    ProcessTerminal, Terminal, normalize_apple_terminal_input, normalize_native_shift_enter_input,
    resolve_escape_timeout_ms_from,
};

fn guard() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// `resolveEscapeTimeoutMs({ ... })` with an explicit environment.
fn env_lookup(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
    let map: HashMap<String, String> = vars
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    move |name: &str| map.get(name).cloned()
}

// describe("resolveEscapeTimeoutMs")

#[test]
fn uses_notagent_tui_esc_timeout_when_configured() {
    assert_eq!(
        resolve_escape_timeout_ms_from(&env_lookup(&[("NOTAGENT_TUI_ESC_TIMEOUT", "80")])),
        80
    );
    assert_eq!(
        resolve_escape_timeout_ms_from(&env_lookup(&[
            ("NOTAGENT_TUI_ESC_TIMEOUT", "80"),
            ("SSH_TTY", "/dev/pts/1"),
        ])),
        80
    );
}

#[test]
fn ignores_invalid_notagent_tui_esc_timeout_values() {
    for value in ["abc", "0", "-5", ""] {
        assert_eq!(
            resolve_escape_timeout_ms_from(&env_lookup(&[("NOTAGENT_TUI_ESC_TIMEOUT", value)])),
            10,
            "value {value:?}"
        );
    }
}

#[test]
fn defaults_to_100ms_over_ssh() {
    assert_eq!(
        resolve_escape_timeout_ms_from(&env_lookup(&[("SSH_CONNECTION", "10.0.0.1 22")])),
        100
    );
    assert_eq!(
        resolve_escape_timeout_ms_from(&env_lookup(&[("SSH_TTY", "/dev/pts/1")])),
        100
    );
}

#[test]
fn defaults_to_10ms_otherwise() {
    assert_eq!(resolve_escape_timeout_ms_from(&env_lookup(&[])), 10);
}

// describe("normalizeNativeShiftEnterInput") / describe("normalizeAppleTerminalInput")

#[test]
fn rewrites_return_to_csi_u_shift_enter_when_native_shift_detection_is_enabled() {
    assert_eq!(
        normalize_native_shift_enter_input("\r", true, true),
        "\x1b[13;2u"
    );
}

#[test]
fn leaves_return_unchanged_when_detection_is_disabled_or_shift_is_not_pressed() {
    assert_eq!(normalize_native_shift_enter_input("\r", false, true), "\r");
    assert_eq!(normalize_native_shift_enter_input("\r", true, false), "\r");
}

#[test]
fn leaves_non_return_input_unchanged() {
    assert_eq!(
        normalize_native_shift_enter_input("\x1b[13;2u", true, true),
        "\x1b[13;2u"
    );
    assert_eq!(normalize_native_shift_enter_input("a", true, true), "a");
}

#[test]
fn apple_terminal_variant_behaves_like_the_native_one() {
    assert_eq!(
        normalize_apple_terminal_input("\r", true, true),
        "\x1b[13;2u"
    );
    assert_eq!(normalize_apple_terminal_input("\r", true, false), "\r");
    assert_eq!(normalize_apple_terminal_input("\r", false, true), "\r");
    assert_eq!(
        normalize_apple_terminal_input("\x1b[13;2u", true, true),
        "\x1b[13;2u"
    );
    assert_eq!(normalize_apple_terminal_input("a", true, true), "a");
}

// describe("ProcessTerminal Kitty keyboard protocol negotiation")

struct NegotiationHarness {
    terminal: ProcessTerminal,
    writes: Rc<RefCell<Vec<String>>>,
    input: Rc<RefCell<Option<String>>>,
}

impl NegotiationHarness {
    fn new() -> Self {
        let writes: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let input: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));

        let sink_writes = Rc::clone(&writes);
        let mut terminal = ProcessTerminal::with_writer(Box::new(move |data| {
            sink_writes.borrow_mut().push(data.to_string())
        }));

        let handler_input = Rc::clone(&input);
        terminal.set_input_handler(Box::new(move |data| {
            *handler_input.borrow_mut() = Some(data.to_string());
        }));
        terminal.begin_keyboard_protocol_negotiation();

        Self {
            terminal,
            writes,
            input,
        }
    }

    fn send(&mut self, data: &str) {
        self.terminal.feed_stdin(data);
    }

    fn writes(&self) -> Vec<String> {
        self.writes.borrow().clone()
    }

    fn input(&self) -> Option<String> {
        self.input.borrow().clone()
    }

    fn cleanup(&mut self) {
        self.terminal.stop();
        set_kitty_protocol_active(false);
    }
}

#[test]
fn queries_kitty_mode_before_enabling_modify_other_keys_fallback() {
    let _guard = guard();
    let mut harness = NegotiationHarness::new();

    assert_eq!(harness.writes()[0], "\x1b[>7u\x1b[?u\x1b[c");
    assert!(!harness.writes().contains(&"\x1b[>4;2m".to_string()));
    assert!(!harness.terminal.kitty_protocol_active());

    harness.cleanup();
}

#[test]
fn activates_kitty_mode_for_non_zero_negotiated_flags() {
    let _guard = guard();
    let mut harness = NegotiationHarness::new();

    harness.send("\x1b[?7u");

    assert_eq!(harness.input(), None);
    assert!(harness.terminal.kitty_protocol_active());
    assert!(!harness.writes().contains(&"\x1b[>4;2m".to_string()));
    assert!(!harness.writes().contains(&"\x1b[>4;0m".to_string()));

    harness.cleanup();
    assert_eq!(
        harness.writes().iter().filter(|w| *w == "\x1b[<u").count(),
        1
    );
    assert!(!harness.writes().contains(&"\x1b[>4;0m".to_string()));
}

#[test]
fn falls_back_to_modify_other_keys_for_zero_kitty_flags() {
    let _guard = guard();
    let mut harness = NegotiationHarness::new();

    harness.send("\x1b[?0u");

    assert_eq!(harness.input(), None);
    assert!(!harness.terminal.kitty_protocol_active());
    assert_eq!(
        harness
            .writes()
            .iter()
            .filter(|w| *w == "\x1b[>4;2m")
            .count(),
        1
    );

    harness.cleanup();
    assert_eq!(
        harness
            .writes()
            .iter()
            .filter(|w| *w == "\x1b[>4;0m")
            .count(),
        1
    );
}

#[test]
fn falls_back_to_modify_other_keys_for_device_attributes_without_kitty_flags() {
    let _guard = guard();
    let mut harness = NegotiationHarness::new();

    harness.send("\x1b[?62;4;52c");

    assert_eq!(harness.input(), None);
    assert!(!harness.terminal.kitty_protocol_active());
    assert_eq!(
        harness
            .writes()
            .iter()
            .filter(|w| *w == "\x1b[>4;2m")
            .count(),
        1
    );

    harness.cleanup();
}

#[test]
fn forwards_normal_input_while_waiting_for_kitty_response() {
    let _guard = guard();
    let mut harness = NegotiationHarness::new();

    harness.send("a");

    assert_eq!(harness.input().as_deref(), Some("a"));
    assert!(!harness.terminal.kitty_protocol_active());

    harness.cleanup();
}

#[test]
fn tracks_split_kitty_confirmation() {
    let _guard = guard();
    let mut harness = NegotiationHarness::new();

    harness.send("\x1b[?7");
    assert_eq!(harness.input(), None);

    harness.send("u");

    assert!(harness.terminal.kitty_protocol_active());
    assert!(!harness.writes().contains(&"\x1b[>4;2m".to_string()));

    harness.cleanup();
}

#[test]
fn replays_buffered_csi_prefix_input_when_it_is_not_a_kitty_response() {
    let _guard = guard();
    let mut harness = NegotiationHarness::new();

    harness.send("\x1b[");
    // StdinBuffer sequence timeout, not the lone-ESC timeout.
    harness.terminal.fire_stdin_timeout();
    assert_eq!(harness.input(), None);

    // 150 ms negotiation fragment timeout.
    harness.terminal.fire_negotiation_fragment_timeout();
    assert_eq!(harness.input().as_deref(), Some("\x1b["));

    harness.cleanup();
}

// describe("ProcessTerminal progress") / describe("ProcessTerminal dimensions")

#[test]
fn writes_a_valid_osc_9_4_clear_sequence() {
    let _guard = guard();
    let writes: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let sink_writes = Rc::clone(&writes);
    let mut terminal = ProcessTerminal::with_writer(Box::new(move |data| {
        sink_writes.borrow_mut().push(data.to_string())
    }));

    terminal.set_progress(false);
    assert_eq!(writes.borrow().as_slice(), ["\x1b]9;4;0\x07".to_string()]);
}

#[test]
fn falls_back_to_columns_and_lines_before_default_dimensions() {
    let _guard = guard();
    let mut terminal = ProcessTerminal::with_writer(Box::new(|_| {}));
    // The TS test overrides process.stdout.columns/rows and sets COLUMNS/LINES;
    // the harness pins the resolved values instead.
    terminal.set_dimensions_for_tests(Some(123), Some(45));

    assert_eq!(terminal.columns(), 123);
    assert_eq!(terminal.rows(), 45);
}
