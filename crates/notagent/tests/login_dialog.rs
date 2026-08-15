//! Tests for the port of
//! `packages/coding-agent/src/modes/interactive/components/login-dialog.ts`.
//!
//! The TypeScript repository has no suite for this component; the expected
//! values come from `tools/gen-login-dialog-oracle.mjs`, which drives the
//! TypeScript class with these fixtures and prints the rendered lines, the
//! `requestRender` calls and how each input step resolved.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::OnceLock;

use notagent::core::keybindings::KeybindingsManager;
use notagent::modes::interactive::components::login_dialog::{
    LoginCancelled, LoginDialogComponent,
};
use notagent::modes::interactive::theme::theme::init_theme;
use notagent_ai::auth::types::AuthInfoLink;
use notagent_ai::compat::extension_oauth_types::OAuthDeviceCodeInfo;
use notagent_tui::keybindings::set_keybindings;
use notagent_tui::tui::Component;

fn test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    let guard = LOCK
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    init_theme(Some("dark"), false);
    set_keybindings(KeybindingsManager::default().to_tui());
    guard
}

const ESCAPE: &str = "\x1b";
const ENTER: &str = "\r";

#[derive(Debug, Clone, PartialEq, Eq)]
enum Event {
    RequestRender,
    Complete(bool, Option<String>),
}

/// The oracle's line transformation: colours dropped, OSC 8 links spelled out.
fn render_line(line: &str) -> String {
    let mut rendered = String::new();
    let mut rest = line;
    while let Some(start) = rest.find("\x1b]8;;") {
        rendered.push_str(&rest[..start]);
        let after = &rest[start + 5..];
        let Some(end) = after.find('\x07') else {
            break;
        };
        let url = &after[..end];
        if url.is_empty() {
            rendered.push_str("</link>");
        } else {
            rendered.push_str(&format!("<link {url}>"));
        }
        rest = &after[end + 1..];
    }
    rendered.push_str(rest);
    notagent::utils::ansi::strip_ansi(&rendered)
}

struct Harness {
    dialog: LoginDialogComponent,
    events: Rc<RefCell<Vec<Event>>>,
}

impl Harness {
    fn new(provider_id: &str, name_override: Option<&str>, title_override: Option<&str>) -> Self {
        let events: Rc<RefCell<Vec<Event>>> = Rc::new(RefCell::new(Vec::new()));
        let render_events = Rc::clone(&events);
        let complete_events = Rc::clone(&events);
        let dialog = LoginDialogComponent::new(
            Rc::new(move || render_events.borrow_mut().push(Event::RequestRender)),
            provider_id,
            Box::new(move |success, message| {
                complete_events
                    .borrow_mut()
                    .push(Event::Complete(success, message))
            }),
            name_override,
            title_override,
        );
        Self { dialog, events }
    }

    fn lines(&mut self) -> Vec<String> {
        self.dialog
            .render(60)
            .into_iter()
            .map(|line| render_line(&line).trim_end().to_string())
            .collect()
    }

    fn take_events(&self) -> Vec<Event> {
        self.events.borrow_mut().drain(..).collect()
    }
}

/// `case1` of the oracle.
#[test]
fn the_title_follows_the_provider_name_and_the_overrides() {
    let _guard = test_lock();

    let mut harness = Harness::new("anthropic", None, None);
    assert_eq!(harness.lines()[1], " Login to anthropic");

    let mut harness = Harness::new("anthropic", Some("Anthropic Pro"), None);
    assert_eq!(harness.lines()[1], " Login to Anthropic Pro");

    let mut harness = Harness::new("anthropic", Some("Anthropic Pro"), Some("Sign in"));
    assert_eq!(harness.lines()[1], " Sign in");
}

/// Shadow the platform browser launcher for the duration of the process, the
/// way `tools/gen-login-dialog-oracle.mjs` does: `show_auth` spawns `open` or
/// `xdg-open`, and a test must not open a browser window.
fn shadow_browser_launcher() {
    static SHADOW: OnceLock<tempfile::TempDir> = OnceLock::new();
    let directory = SHADOW.get_or_init(|| {
        let directory = tempfile::Builder::new()
            .prefix("notagent-no-browser-")
            .tempdir()
            .expect("temp dir");
        for launcher in ["open", "xdg-open", "rundll32"] {
            let path = directory.path().join(launcher);
            std::fs::write(&path, "#!/bin/sh\nexit 0\n").expect("writes");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                    .expect("chmod");
            }
        }
        directory
    });
    let path = std::env::var("PATH").unwrap_or_default();
    let shadow = directory.path().to_string_lossy();
    if !path.starts_with(shadow.as_ref()) {
        // SAFETY: every case of this file runs under `test_lock`, so no other
        // thread of the binary reads the environment while it is replaced.
        unsafe { std::env::set_var("PATH", format!("{shadow}:{path}")) };
    }
}

/// `case2` of the oracle.
#[test]
fn show_auth_renders_the_url_as_a_hyperlink() {
    let _guard = test_lock();
    shadow_browser_launcher();
    let mut harness = Harness::new("anthropic", None, None);

    harness
        .dialog
        .show_auth("https://example.com/auth?code=1", None);
    assert_eq!(
        &harness.lines()[2..5],
        &[
            "",
            " <link https://example.com/auth?code=1>https://example.com/auth?code=1</link>",
            " <link https://example.com/auth?code=1>Cmd+click to open</link>",
        ]
    );
    assert_eq!(harness.take_events(), vec![Event::RequestRender]);

    // A second call replaces the content instead of appending to it.
    harness.dialog.show_auth(
        "https://example.com/auth?code=1",
        Some("Approve in the browser"),
    );
    let lines = harness.lines();
    assert_eq!(lines.len(), 8);
    assert_eq!(&lines[5..7], &["", " Approve in the browser"]);
}

/// `case3` of the oracle.
#[test]
fn show_device_code_renders_the_verification_url_and_the_code() {
    let _guard = test_lock();
    let mut harness = Harness::new("copilot", None, None);

    harness.dialog.show_device_code(&OAuthDeviceCodeInfo {
        user_code: "ABCD-1234".to_string(),
        verification_uri: "https://github.com/login/device".to_string(),
        ..OAuthDeviceCodeInfo::default()
    });
    assert_eq!(
        &harness.lines()[2..7],
        &[
            "",
            " <link https://github.com/login/device>https://github.com/login/device</link>",
            " <link https://github.com/login/device>Cmd+click to open</link>",
            "",
            " Enter code: ABCD-1234",
        ]
    );
    assert_eq!(harness.take_events(), vec![Event::RequestRender]);
}

/// `case4` of the oracle: submitting replaces the input with the typed text.
#[test]
fn show_prompt_resolves_with_the_submitted_value() {
    let _guard = test_lock();
    let mut harness = Harness::new("anthropic", None, None);

    let mut answer = harness.dialog.show_prompt("Paste the code", Some("abc123"));
    assert_eq!(
        &harness.lines()[2..7],
        &[
            "",
            " Paste the code",
            " e.g., abc123",
            ">",
            " (escape/ctrl+c to cancel, enter to submit)",
        ]
    );
    assert_eq!(harness.take_events(), vec![Event::RequestRender]);
    assert!(answer.try_recv().is_err());

    for character in "xyz".chars() {
        harness.dialog.handle_input(&character.to_string());
    }
    harness.dialog.handle_input(ENTER);

    assert_eq!(answer.try_recv(), Ok(Ok("xyz".to_string())));
    assert_eq!(harness.lines()[5], "> xyz");
    assert!(harness.take_events().is_empty());
}

/// `case5` of the oracle: escape cancels the step, aborts the signal and
/// reports the cancellation once.
#[test]
fn show_manual_input_is_cancelled_by_escape() {
    let _guard = test_lock();
    let mut harness = Harness::new("anthropic", None, None);

    let mut answer = harness.dialog.show_manual_input("Paste the redirect URL");
    assert_eq!(
        &harness.lines()[2..6],
        &[
            "",
            " Paste the redirect URL",
            ">",
            " (escape/ctrl+c to cancel)",
        ]
    );
    assert_eq!(harness.take_events(), vec![Event::RequestRender]);

    let signal = harness.dialog.signal();
    harness.dialog.handle_input(ESCAPE);

    assert!(signal.is_cancelled());
    assert_eq!(answer.try_recv(), Ok(Err(LoginCancelled)));
    assert_eq!(
        harness.take_events(),
        vec![Event::Complete(false, Some("Login cancelled".to_string()))]
    );
    // The content stays on screen, exactly as in TypeScript.
    assert_eq!(harness.lines()[3], " Paste the redirect URL");
}

/// `case6` of the oracle: the three informational steps.
#[test]
fn the_informational_steps_append_to_the_content() {
    let _guard = test_lock();

    let mut harness = Harness::new("anthropic", None, None);
    harness.dialog.show_info(
        "Use the web console",
        &[
            AuthInfoLink {
                url: "https://example.com".to_string(),
                label: Some("Console".to_string()),
            },
            AuthInfoLink {
                url: "https://example.org".to_string(),
                label: None,
            },
        ],
        true,
    );
    assert_eq!(
        &harness.lines()[2..8],
        &[
            "",
            " Use the web console",
            " <link https://example.com>Console: https://example.com</link>",
            " <link https://example.org>https://example.org</link>",
            "",
            " (escape/ctrl+c to close)",
        ]
    );

    let mut harness = Harness::new("anthropic", None, None);
    harness.dialog.show_waiting("Waiting for approval…");
    harness.dialog.show_progress("Polling…");
    assert_eq!(
        &harness.lines()[2..6],
        &[
            "",
            " Waiting for approval…",
            " (escape/ctrl+c to cancel)",
            " Polling…",
        ]
    );
    assert_eq!(
        harness.take_events(),
        vec![Event::RequestRender, Event::RequestRender]
    );

    let mut harness = Harness::new("anthropic", None, None);
    harness
        .dialog
        .show_details(&["First line".to_string(), "Second line".to_string()]);
    assert_eq!(&harness.lines()[2..5], &["", " First line", " Second line"]);
}
