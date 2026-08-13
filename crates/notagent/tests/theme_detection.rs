//! Port of `packages/coding-agent/test/theme-detection.test.ts` (174 LOC).

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, OnceLock};

use notagent::modes::interactive::theme::theme::{
    EnvMap, TerminalAutoThemeDetector, TerminalBackgroundThemeDetector, TerminalQueryError,
    TerminalQueryFuture, TerminalTheme, TerminalThemeConfidence, TerminalThemeSource,
    detect_terminal_background_from_env, detect_terminal_background_theme,
    detect_terminal_theme_for_auto, get_theme_by_name, get_theme_for_rgb_color,
    parse_auto_theme_setting, resolve_theme_setting,
};
use notagent_tui::{RgbColor, TerminalCapabilities, reset_capabilities_cache, set_capabilities};

/// Terminal capabilities are a process global, so the tests that stub them run
/// serially (`afterEach(resetCapabilitiesCache)` in the TS suite).
fn capabilities_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

fn env(entries: &[(&str, &str)]) -> EnvMap {
    entries
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect::<HashMap<_, _>>()
}

// --- detectTerminalBackgroundFromEnv -----------------------------------------

#[test]
fn uses_the_colorfgbg_background_color_index() {
    let detection = detect_terminal_background_from_env(Some(&env(&[("COLORFGBG", "0;15")])));
    assert_eq!(detection.theme, TerminalTheme::Light);
    assert_eq!(detection.source, TerminalThemeSource::ColorFgBg);
    assert_eq!(detection.confidence, TerminalThemeConfidence::High);

    let detection = detect_terminal_background_from_env(Some(&env(&[("COLORFGBG", "15;0")])));
    assert_eq!(detection.theme, TerminalTheme::Dark);
    assert_eq!(detection.source, TerminalThemeSource::ColorFgBg);
    assert_eq!(detection.confidence, TerminalThemeConfidence::High);
}

#[test]
fn uses_the_last_colorfgbg_field_as_the_background() {
    assert_eq!(
        detect_terminal_background_from_env(Some(&env(&[("COLORFGBG", "0;7;15")]))).theme,
        TerminalTheme::Light
    );
}

#[test]
fn defaults_to_dark_without_terminal_background_hints() {
    let detection = detect_terminal_background_from_env(Some(&env(&[])));
    assert_eq!(detection.theme, TerminalTheme::Dark);
    assert_eq!(detection.source, TerminalThemeSource::Fallback);
    assert_eq!(detection.confidence, TerminalThemeConfidence::Low);
}

// --- detectTerminalBackgroundTheme -------------------------------------------

/// Records the timeout it was queried with and answers with a fixed result.
struct BackgroundStub {
    queried_timeout_ms: Cell<Option<u64>>,
    answer: BackgroundAnswer,
}

enum BackgroundAnswer {
    Color(RgbColor),
    None,
    Fails,
}

impl TerminalBackgroundThemeDetector for BackgroundStub {
    fn query_terminal_background_color(
        &self,
        timeout_ms: u64,
    ) -> TerminalQueryFuture<'_, Option<RgbColor>> {
        self.queried_timeout_ms.set(Some(timeout_ms));
        Box::pin(async move {
            match self.answer {
                BackgroundAnswer::Color(rgb) => Ok(Some(rgb)),
                BackgroundAnswer::None => Ok(None),
                BackgroundAnswer::Fails => {
                    Err(TerminalQueryError("terminal write failed".to_string()))
                }
            }
        })
    }
}

#[tokio::test]
async fn uses_the_queried_terminal_background_before_environment_hints() {
    let ui = BackgroundStub {
        queried_timeout_ms: Cell::new(None),
        answer: BackgroundAnswer::Color(RgbColor {
            r: 250,
            g: 250,
            b: 250,
        }),
    };
    let detection =
        detect_terminal_background_theme(&ui, 250, Some(&env(&[("COLORFGBG", "15;0")]))).await;

    assert_eq!(ui.queried_timeout_ms.get(), Some(250));
    assert_eq!(detection.theme, TerminalTheme::Light);
    assert_eq!(detection.source, TerminalThemeSource::TerminalBackground);
    assert_eq!(detection.confidence, TerminalThemeConfidence::High);
}

#[tokio::test]
async fn falls_back_to_environment_hints_when_the_terminal_query_returns_no_color() {
    let ui = BackgroundStub {
        queried_timeout_ms: Cell::new(None),
        answer: BackgroundAnswer::None,
    };
    let detection =
        detect_terminal_background_theme(&ui, 250, Some(&env(&[("COLORFGBG", "15;0")]))).await;

    assert_eq!(detection.theme, TerminalTheme::Dark);
    assert_eq!(detection.source, TerminalThemeSource::ColorFgBg);
    assert_eq!(detection.confidence, TerminalThemeConfidence::High);
}

#[tokio::test]
async fn falls_back_to_environment_hints_when_the_terminal_query_fails() {
    let ui = BackgroundStub {
        queried_timeout_ms: Cell::new(None),
        answer: BackgroundAnswer::Fails,
    };
    let detection =
        detect_terminal_background_theme(&ui, 250, Some(&env(&[("COLORFGBG", "0;15")]))).await;

    assert_eq!(detection.theme, TerminalTheme::Light);
    assert_eq!(detection.source, TerminalThemeSource::ColorFgBg);
    assert_eq!(detection.confidence, TerminalThemeConfidence::High);
}

// --- detectTerminalThemeForAuto ----------------------------------------------

/// Color scheme resolves through a channel; the background query never settles
/// but records that it was started.
struct AutoStub {
    background_query_started: Cell<bool>,
    color_scheme: RefCellReceiver,
    background: Option<RgbColor>,
    color_scheme_fails: bool,
}

type RefCellReceiver = std::cell::RefCell<Option<tokio::sync::oneshot::Receiver<TerminalTheme>>>;

impl TerminalBackgroundThemeDetector for AutoStub {
    fn query_terminal_background_color(
        &self,
        _timeout_ms: u64,
    ) -> TerminalQueryFuture<'_, Option<RgbColor>> {
        Box::pin(async move {
            self.background_query_started.set(true);
            match self.background {
                Some(rgb) => Ok(Some(rgb)),
                None => std::future::pending().await,
            }
        })
    }
}

impl TerminalAutoThemeDetector for AutoStub {
    fn query_terminal_color_scheme(
        &self,
        _timeout_ms: u64,
    ) -> TerminalQueryFuture<'_, Option<TerminalTheme>> {
        Box::pin(async move {
            if self.color_scheme_fails {
                return Err(TerminalQueryError("color-scheme query failed".to_string()));
            }
            let receiver = self.color_scheme.borrow_mut().take();
            match receiver {
                Some(receiver) => Ok(receiver.await.ok()),
                None => Ok(None),
            }
        })
    }
}

#[tokio::test]
async fn starts_both_queries_and_returns_the_preferred_color_scheme_result_without_waiting() {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let ui = AutoStub {
        background_query_started: Cell::new(false),
        color_scheme: std::cell::RefCell::new(Some(receiver)),
        background: None,
        color_scheme_fails: false,
    };

    let detection = detect_terminal_theme_for_auto(&ui, 100, None);
    futures::pin_mut!(detection);
    // Rust futures are lazy: one poll is what running the TS statement does.
    assert!(futures::poll!(detection.as_mut()).is_pending());

    assert!(ui.background_query_started.get());
    sender.send(TerminalTheme::Dark).expect("receiver alive");
    assert_eq!(detection.await, TerminalTheme::Dark);
}

#[tokio::test]
async fn uses_the_background_result_when_the_color_scheme_query_fails() {
    let ui = AutoStub {
        background_query_started: Cell::new(false),
        color_scheme: std::cell::RefCell::new(None),
        background: Some(RgbColor {
            r: 250,
            g: 250,
            b: 250,
        }),
        color_scheme_fails: true,
    };
    assert_eq!(
        detect_terminal_theme_for_auto(&ui, 100, None).await,
        TerminalTheme::Light
    );
}

// --- theme color mode ---------------------------------------------------------

#[test]
fn uses_terminal_capabilities() {
    let _guard = capabilities_lock();
    set_capabilities(TerminalCapabilities {
        images: None,
        true_color: false,
        hyperlinks: false,
    });
    let ansi256_theme = get_theme_by_name("dark").expect("dark theme not found");
    assert_eq!(
        ansi256_theme.get_color_mode(),
        notagent::modes::interactive::theme::theme::ColorMode::Color256
    );
    let accent =
        ansi256_theme.get_fg_ansi(notagent::modes::interactive::theme::theme::ThemeColor::Accent);
    assert!(
        accent.starts_with("\x1b[38;5;") && accent.ends_with('m'),
        "{accent:?} does not match /^\\x1b\\[38;5;\\d+m$/"
    );

    set_capabilities(TerminalCapabilities {
        images: None,
        true_color: true,
        hyperlinks: false,
    });
    let truecolor_theme = get_theme_by_name("dark").expect("dark theme not found");
    assert_eq!(
        truecolor_theme.get_color_mode(),
        notagent::modes::interactive::theme::theme::ColorMode::TrueColor
    );
    let accent =
        truecolor_theme.get_fg_ansi(notagent::modes::interactive::theme::theme::ThemeColor::Accent);
    assert!(
        accent.starts_with("\x1b[38;2;") && accent.ends_with('m'),
        "{accent:?} does not match /^\\x1b\\[38;2;\\d+;\\d+;\\d+m$/"
    );
    reset_capabilities_cache();
}

// --- theme detection from RGB -------------------------------------------------

#[test]
fn classifies_rgb_colors_by_luminance() {
    assert_eq!(
        get_theme_for_rgb_color(RgbColor { r: 8, g: 8, b: 8 }),
        TerminalTheme::Dark
    );
    assert_eq!(
        get_theme_for_rgb_color(RgbColor {
            r: 250,
            g: 250,
            b: 250
        }),
        TerminalTheme::Light
    );
}

// --- theme setting helpers ----------------------------------------------------

#[test]
fn parses_and_resolves_automatic_theme_settings() {
    assert_eq!(
        parse_auto_theme_setting(Some("light/dark")),
        Some(("light".to_string(), "dark".to_string()))
    );
    assert_eq!(
        resolve_theme_setting(Some("dark"), TerminalTheme::Light).as_deref(),
        Some("dark")
    );
    assert_eq!(
        resolve_theme_setting(Some("light/dark"), TerminalTheme::Light).as_deref(),
        Some("light")
    );
    assert_eq!(
        resolve_theme_setting(Some("light/dark"), TerminalTheme::Dark).as_deref(),
        Some("dark")
    );
    assert_eq!(
        resolve_theme_setting(Some("light/dark/extra"), TerminalTheme::Dark),
        None
    );
}
