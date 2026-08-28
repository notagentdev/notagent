use std::cell::RefCell;
use std::rc::Rc;
use std::sync::OnceLock;

use notagent::core::keybindings::KeybindingsManager;
use notagent::core::settings_manager::{
    DefaultProjectTrust, DoubleEscapeAction, FullscreenExitOutput, MermaidRenderingMode, QueueMode,
    TreeFilterMode, TuiMode, WarningSettings,
};
use notagent::modes::interactive::components::settings_selector::{
    SettingsCallbacks, SettingsConfig, SettingsSelectorComponent,
};
use notagent::modes::interactive::theme::theme::{TerminalTheme, init_theme};
use notagent::utils::ansi::strip_ansi;
use notagent_agent::types::ThinkingLevel;
use notagent_ai::types::Transport;
use notagent_tui::components::scroll_view::ScrollViewScrollbar;
use notagent_tui::keybindings::set_keybindings;
use notagent_tui::tui::Component;

/// The theme and the keybindings registry are process globals.
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

const ENTER: &str = "\r";
const ESCAPE: &str = "\x1b";
const UP: &str = "\x1b[A";
const DOWN: &str = "\x1b[B";

type Calls = Rc<RefCell<Vec<(&'static str, String)>>>;

fn config() -> SettingsConfig {
    SettingsConfig {
        auto_compact: true,
        block_style_badge: true,
        atomic_leases: false,
        bash_filter: false,
        show_images: false,
        image_width_cells: 80,
        auto_resize_images: true,
        block_images: false,
        enable_skill_commands: true,
        steering_mode: QueueMode::OneAtATime,
        follow_up_mode: QueueMode::All,
        transport: Transport::Auto,
        http_idle_timeout_ms: 300_000,
        thinking_level: ThinkingLevel::Medium,
        available_thinking_levels: vec![
            ThinkingLevel::Off,
            ThinkingLevel::Low,
            ThinkingLevel::Medium,
            ThinkingLevel::High,
        ],
        current_theme: "dark".to_string(),
        terminal_theme: TerminalTheme::Dark,
        available_themes: vec![
            "dark".to_string(),
            "light".to_string(),
            "solarized".to_string(),
        ],
        hide_thinking_block: false,
        mermaid_rendering_mode: MermaidRenderingMode::Streaming,
        show_cache_miss_notices: true,
        enable_install_telemetry: false,
        double_escape_action: DoubleEscapeAction::Tree,
        tree_filter_mode: TreeFilterMode::Default,
        show_hardware_cursor: false,
        editor_padding_x: 1,
        output_pad: 0,
        autocomplete_max_visible: 10,
        quiet_startup: false,
        default_project_trust: DefaultProjectTrust::Ask,
        clear_on_shrink: true,
        show_terminal_progress: true,
        show_workspace_in_footer: false,
        tiered_thinking: false,
        tui_mode: TuiMode::Regular,
        fullscreen_exit_output: FullscreenExitOutput::Transcript,
        fullscreen_scrollbar: ScrollViewScrollbar::Auto,
        warnings: WarningSettings::default(),
    }
}

fn callbacks(calls: &Calls) -> SettingsCallbacks {
    macro_rules! record {
        ($name:literal, $calls:expr, $value:ident => $rendered:expr) => {{
            let calls = Rc::clone($calls);
            Box::new(move |$value| calls.borrow_mut().push(($name, $rendered)))
        }};
    }

    SettingsCallbacks {
        on_fullscreen_exit_output_change: record!("onFullscreenExitOutputChange", calls, value =>
        match value {
            FullscreenExitOutput::Transcript => "transcript".to_string(),
            FullscreenExitOutput::ResumeHint => "resume-hint".to_string(),
        }),
        on_fullscreen_scrollbar_change: record!("onFullscreenScrollbarChange", calls, value =>
        match value {
            ScrollViewScrollbar::Auto => "auto".to_string(),
            ScrollViewScrollbar::Always => "always".to_string(),
            ScrollViewScrollbar::Hidden => "hidden".to_string(),
        }),
        on_http_idle_timeout_ms_change: record!("onHttpIdleTimeoutMsChange", calls, value =>
            value.to_string()),
        on_thinking_level_change: record!("onThinkingLevelChange", calls, value =>
            format!("{value:?}").to_lowercase()),
        on_warnings_change: record!("onWarningsChange", calls, value =>
            format!("{:?}", value.anthropic_extra_usage)),
        on_theme_change: record!("onThemeChange", calls, value => value.to_string()),
        on_theme_preview: Some(record!("onThemePreview", calls, value => value.to_string())),
        ..SettingsCallbacks::default()
    }
}

struct Harness {
    component: SettingsSelectorComponent,
    calls: Calls,
}

impl Harness {
    fn new(config: SettingsConfig) -> Self {
        let calls: Calls = Rc::new(RefCell::new(Vec::new()));
        let component = SettingsSelectorComponent::new(config, callbacks(&calls));
        Self { component, calls }
    }

    fn key(&mut self, data: &str) {
        self.component.handle_input(data);
    }

    fn search(&mut self, label: &str) {
        for character in label.chars() {
            self.key(&character.to_string());
        }
    }

    fn lines(&mut self) -> Vec<String> {
        self.component
            .render(80)
            .into_iter()
            .map(|line| strip_ansi(&line).trim_end().to_string())
            .collect()
    }

    fn take_calls(&self) -> Vec<(&'static str, String)> {
        self.calls.borrow_mut().drain(..).collect()
    }
}

#[test]
fn cycles_through_fullscreen_settings() {
    let _guard = test_lock();

    let mut harness = Harness::new(config());
    harness.search("Fullscreen exit output");
    harness.key(ENTER);
    harness.key(ENTER);
    assert_eq!(
        harness
            .take_calls()
            .into_iter()
            .map(|(_, value)| value)
            .collect::<Vec<_>>(),
        vec!["resume-hint", "transcript"]
    );

    let mut harness = Harness::new(config());
    harness.search("Fullscreen scrollbar");
    harness.key(ENTER);
    harness.key(ENTER);
    harness.key(ENTER);
    assert_eq!(
        harness
            .take_calls()
            .into_iter()
            .map(|(_, value)| value)
            .collect::<Vec<_>>(),
        vec!["always", "hidden", "auto"]
    );
}

#[test]
fn the_http_idle_timeout_row_cycles_through_its_labels() {
    let _guard = test_lock();
    let mut harness = Harness::new(config());
    harness.search("HTTP idle timeout");
    assert_eq!(harness.lines()[3], "→ HTTP idle timeout       5 min");

    harness.key(ENTER);
    assert_eq!(harness.lines()[3], "→ HTTP idle timeout       disabled");
    assert_eq!(
        harness.take_calls(),
        vec![("onHttpIdleTimeoutMsChange", "0".to_string())]
    );
}

#[test]
fn the_warnings_submenu_toggles_and_returns() {
    let _guard = test_lock();
    let mut harness = Harness::new(config());
    harness.search("Warnings");
    harness.key(ENTER);
    assert_eq!(harness.lines()[1], "→ Anthropic extra usage  true");

    harness.key(ENTER);
    assert_eq!(harness.lines()[1], "→ Anthropic extra usage  false");
    assert_eq!(
        harness.take_calls(),
        vec![("onWarningsChange", "Some(false)".to_string())]
    );

    // Escape closes the submenu without touching the row.
    harness.key(ESCAPE);
    assert_eq!(harness.lines()[3], "→ Warnings                configure");
    assert!(harness.take_calls().is_empty());
}

/// into the row it came from.
#[test]
fn the_thinking_submenu_selects_a_level() {
    let _guard = test_lock();
    let mut harness = Harness::new(config());
    harness.search("Thinking level");
    harness.key(ENTER);
    assert_eq!(
        &harness.lines()[5..9],
        &[
            "  off         No reasoning",
            "  low         Light reasoning (~2k tokens)",
            "→ medium      Moderate reasoning (~8k tokens)",
            "  high        Deep reasoning (~16k tokens)",
        ]
    );

    harness.key(DOWN);
    harness.key(ENTER);
    assert_eq!(harness.lines()[3], "→ Thinking level          high");
    assert_eq!(
        harness.take_calls(),
        vec![("onThinkingLevelChange", "high".to_string())]
    );
}

#[test]
fn the_theme_submenu_selects_and_cancels() {
    let _guard = test_lock();
    let mut harness = Harness::new(config());
    harness.search("Theme");
    harness.key(ENTER);
    assert_eq!(
        &harness.lines()[1..10],
        &[
            "Theme",
            "",
            "Select a theme, or choose Automatic to follow terminal appearance.",
            "",
            "  Automatic   Use separate themes for light and dark terminal appearance",
            "→ dark",
            "  light",
            "  solarized",
            "",
        ]
    );

    // Moving the selection previews the theme under the cursor.
    harness.key(DOWN);
    assert_eq!(
        harness.take_calls(),
        vec![("onThemePreview", "light".to_string())]
    );

    harness.key(ENTER);
    assert_eq!(harness.lines()[3], "→ Theme                   light");
    assert_eq!(
        harness.take_calls(),
        vec![("onThemeChange", "light".to_string())]
    );

    // Escape restores the theme the dialog started with.
    let mut harness = Harness::new(config());
    harness.search("Theme");
    harness.key(ENTER);
    harness.key(ESCAPE);
    assert_eq!(harness.lines()[3], "→ Theme                   dark");
    assert_eq!(
        harness.take_calls(),
        vec![("onThemePreview", "dark".to_string())]
    );
}

#[test]
fn the_theme_submenu_builds_an_automatic_setting() {
    let _guard = test_lock();
    let mut harness = Harness::new(config());
    harness.search("Theme");
    harness.key(ENTER);

    // Moving onto "Automatic" previews the pair it would produce.
    harness.key(UP);
    assert_eq!(
        harness.take_calls(),
        vec![("onThemePreview", "dark/dark".to_string())]
    );

    harness.key(ENTER);
    assert_eq!(
        &harness.lines()[1..10],
        &[
            "Automatic Theme",
            "",
            "Choose themes for terminal light and dark appearance.",
            "Light/dark detection requires terminal support.",
            "",
            "→ Light theme  dark",
            "  Dark theme   dark",
            "  Apply        save and go back",
            "  Change mode  switch to single theme",
        ]
    );
    assert_eq!(
        harness.take_calls(),
        vec![("onThemePreview", "dark/dark".to_string())]
    );

    // The light row opens its own select, nested inside the automatic menu.
    harness.key(ENTER);
    assert_eq!(
        &harness.lines()[6..12],
        &[
            "Light Theme",
            "",
            "Select the theme to use for light terminal appearance",
            "",
            "→ dark",
            "  light",
        ]
    );

    harness.key(DOWN);
    harness.key(DOWN);
    harness.key(ENTER);
    assert_eq!(harness.lines()[6], "→ Light theme  solarized");
    assert_eq!(
        harness.take_calls(),
        vec![
            ("onThemePreview", "light".to_string()),
            ("onThemePreview", "solarized".to_string()),
            ("onThemePreview", "solarized/dark".to_string()),
        ]
    );

    // Apply closes the whole submenu with the pair as the new value.
    harness.key(DOWN);
    harness.key(DOWN);
    harness.key(ENTER);
    assert_eq!(
        harness.lines()[3],
        "→ Theme                   solarized/dark"
    );
    assert_eq!(
        harness.take_calls(),
        vec![("onThemeChange", "solarized/dark".to_string())]
    );
}

/// "Change mode" goes back to the single list with the active theme selected.
#[test]
fn an_automatic_theme_setting_opens_the_automatic_menu() {
    let _guard = test_lock();
    let mut harness = Harness::new(SettingsConfig {
        current_theme: "light/solarized".to_string(),
        ..config()
    });
    harness.search("Theme");
    harness.key(ENTER);
    assert_eq!(
        &harness.lines()[6..10],
        &[
            "→ Light theme  light",
            "  Dark theme   solarized",
            "  Apply        save and go back",
            "  Change mode  switch to single theme",
        ]
    );

    harness.key(DOWN);
    harness.key(DOWN);
    harness.key(DOWN);
    harness.key(ENTER);
    assert_eq!(
        &harness.lines()[5..9],
        &[
            "  Automatic   Use separate themes for light and dark terminal appearance",
            "  dark",
            "  light",
            "→ solarized",
        ]
    );
    assert_eq!(
        harness.take_calls(),
        vec![("onThemePreview", "solarized".to_string())]
    );
}
