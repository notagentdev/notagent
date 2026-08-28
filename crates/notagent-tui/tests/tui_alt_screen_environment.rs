use notagent_tui::test_terminal::VirtualTerminal;
use notagent_tui::tui::{Line, TuiStopOptions, component_ref};
use notagent_tui::tui_alt_screen::{TuiAltScreen, TuiAltScreenOptions};

const ENVIRONMENT_KEYS: [&str; 4] = ["TMUX", "ZELLIJ", "STY", "TERM"];

/// Serializes the cases of this binary, which all read or write the environment.
static ENVIRONMENT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn lock_environment() -> std::sync::MutexGuard<'static, ()> {
    ENVIRONMENT_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Apply an environment for the duration of one case.
/// binary is single-threaded here, which is the safety condition.
fn set_environment(entries: &[(&str, &str)]) {
    unsafe {
        for key in ENVIRONMENT_KEYS {
            std::env::remove_var(key);
        }
        for (key, value) in entries {
            std::env::set_var(key, value);
        }
    }
}

#[test]
fn uses_button_motion_tracking_inside_terminal_multiplexers() {
    let _environment = lock_environment();
    let previous: Vec<(&str, Option<String>)> = ENVIRONMENT_KEYS
        .iter()
        .map(|key| (*key, std::env::var(key).ok()))
        .collect();

    set_environment(&[("TERM", "xterm-256color")]);
    let direct_terminal = VirtualTerminal::new(80, 24);
    let mut direct_tui = TuiAltScreen::new(
        Box::new(direct_terminal.clone()),
        TuiAltScreenOptions::default(),
    );
    direct_tui.start();
    assert!(direct_terminal.get_writes().contains("\x1b[?1003h"));
    direct_tui.stop(TuiStopOptions::default());

    let multiplexers: [(&str, &[(&str, &str)]); 5] = [
        ("tmux environment", &[("TMUX", "/tmp/tmux/default,1,0")]),
        ("tmux TERM", &[("TERM", "tmux-256color")]),
        ("Zellij environment", &[("ZELLIJ", "0")]),
        ("Screen environment", &[("STY", "123.session")]),
        ("Screen TERM", &[("TERM", "screen-256color")]),
    ];
    for (name, environment) in multiplexers {
        set_environment(environment);
        let terminal = VirtualTerminal::new(80, 24);
        let mut tui = TuiAltScreen::new(Box::new(terminal.clone()), TuiAltScreenOptions::default());
        tui.start();
        let writes = terminal.get_writes();
        assert!(
            writes.contains("\x1b[?1002h"),
            "{name} should enable button-motion tracking"
        );
        assert!(
            !writes.contains("\x1b[?1003h"),
            "{name} should not enable all-motion tracking"
        );
        assert!(
            writes.contains("\x1b[?1006h"),
            "{name} should enable SGR mouse encoding"
        );
        tui.stop(TuiStopOptions::default());
    }

    unsafe {
        for (key, value) in previous {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}

#[test]
fn invokes_the_right_click_paste_handler_only_on_windows() {
    let _environment = lock_environment();
    let terminal = VirtualTerminal::new(80, 24);
    let mut tui = TuiAltScreen::new(
        Box::new(terminal.clone()),
        TuiAltScreenOptions {
            right_click_paste: true,
            ..TuiAltScreenOptions::default()
        },
    );
    tui.start();
    tui.handle_terminal_input("\x1b[<2;1;1M");
    tui.handle_terminal_input("\x1b[<2;1;1m");

    // port decides at compile time, so each platform asserts its own branch.
    if cfg!(windows) {
        assert!(tui.take_right_click_paste());
    } else {
        assert!(!tui.take_right_click_paste());
    }
    assert!(!tui.take_right_click_paste());
    tui.stop(TuiStopOptions::default());
}

#[test]
fn invalidates_overlays_with_an_explicit_layout_root() {
    let _environment = lock_environment();
    use notagent_tui::components::text::Text;
    use notagent_tui::tui::Component;

    struct Probe {
        invalidated: std::rc::Rc<std::cell::RefCell<bool>>,
    }

    impl Component for Probe {
        fn render(&mut self, _width: usize) -> Vec<Line> {
            vec![Line::from("overlay")]
        }

        fn invalidate(&mut self) {
            *self.invalidated.borrow_mut() = true;
        }
    }

    let terminal = VirtualTerminal::new(80, 24);
    let mut tui = TuiAltScreen::new(Box::new(terminal), TuiAltScreenOptions::default());
    let invalidated = std::rc::Rc::new(std::cell::RefCell::new(false));
    let overlay = component_ref(Probe {
        invalidated: invalidated.clone(),
    });
    tui.set_layout_root(Some(component_ref(Text::new("root", 0, 0))));
    tui.core().show_overlay(overlay, None);

    tui.core().invalidate();

    assert!(*invalidated.borrow());
    tui.stop(TuiStopOptions::default());
}
