use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use notagent::config::env_agent_dir;
use notagent::modes::interactive::theme::theme::{
    ColorMode, ThemeBg, ThemeColor, init_theme, is_theme_initialized, load_theme_from_path,
    notify_theme_directory_event, notify_theme_watcher_error, on_theme_change, set_theme,
    stop_theme_watcher, theme,
};

/// The global theme, the registry and `NOTAGENT_CODING_AGENT_DIR` are process
/// globals, so these tests run serially.
fn global_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

fn dark_theme() -> serde_json::Value {
    serde_json::from_str(include_str!("../src/modes/interactive/theme/dark.json"))
        .expect("dark.json parses")
}

/// The dark theme's accent as the sequence a slot holds, read from the theme
/// rather than written out again.
/// A literal here is a second copy of a value that lives in `dark.json`, and it
/// goes stale the moment the colour is changed — which is a change to how the
/// app looks, not to what any of these cases are about.
fn dark_accent_ansi() -> String {
    let theme = dark_theme();
    let token = theme["colors"]["accent"]
        .as_str()
        .expect("the accent slot names a var")
        .to_string();
    let hex = theme["vars"][&token]
        .as_str()
        .unwrap_or(&token)
        .trim_start_matches('#')
        .to_string();
    let channel = |range: std::ops::Range<usize>| {
        u8::from_str_radix(&hex[range], 16).expect("the accent is a hex colour")
    };
    format!(
        "\x1b[38;2;{};{};{}m",
        channel(0..2),
        channel(2..4),
        channel(4..6)
    )
}

struct AgentDir {
    root: PathBuf,
    themes_dir: PathBuf,
    previous: Option<String>,
}

impl AgentDir {
    fn new() -> Self {
        let directory = tempfile::Builder::new()
            .prefix("notagent-theme-runtime-")
            .tempdir()
            .expect("temp dir");
        let root = directory.path().to_path_buf();
        let _ = directory.keep();
        let agent_dir = root.join("agent");
        let previous = std::env::var(env_agent_dir()).ok();
        unsafe { std::env::set_var(env_agent_dir(), &agent_dir) };
        let themes_dir = agent_dir.join("themes");
        std::fs::create_dir_all(&themes_dir).expect("creates");
        Self {
            root,
            themes_dir,
            previous,
        }
    }

    fn write_theme(&self, file_stem: &str, theme: &serde_json::Value) -> PathBuf {
        let path = self.themes_dir.join(format!("{file_stem}.json"));
        std::fs::write(&path, theme.to_string()).expect("writes");
        path
    }
}

impl Drop for AgentDir {
    fn drop(&mut self) {
        stop_theme_watcher();
        let _ = std::fs::remove_dir_all(&self.root);
        match self.previous.take() {
            Some(previous) => unsafe { std::env::set_var(env_agent_dir(), previous) },
            None => unsafe { std::env::remove_var(env_agent_dir()) },
        }
    }
}

// --- chalk replacement --------------------------------------------------------

#[test]
fn styles_text_exactly_like_chalk() {
    let _guard = global_lock();
    init_theme(Some("dark"), false);
    let theme = theme();

    assert_eq!(theme.bold("X"), "\x1b[1mX\x1b[22m");
    assert_eq!(theme.italic("X"), "\x1b[3mX\x1b[23m");
    assert_eq!(theme.underline("X"), "\x1b[4mX\x1b[24m");
    assert_eq!(theme.inverse("X"), "\x1b[7mX\x1b[27m");
    assert_eq!(theme.strikethrough("X"), "\x1b[9mX\x1b[29m");
    assert_eq!(theme.bold(""), "");

    // A nested close code is kept and the open code appended after it.
    assert_eq!(
        theme.bold(&format!("a{}c", theme.bold("b"))),
        "\x1b[1ma\x1b[1mb\x1b[22m\x1b[1mc\x1b[22m"
    );
    assert_eq!(
        theme.bold(&format!("a{}c", theme.italic("b"))),
        "\x1b[1ma\x1b[3mb\x1b[23mc\x1b[22m"
    );
    // Every line is encased separately.
    assert_eq!(theme.bold("a\nb"), "\x1b[1ma\x1b[22m\n\x1b[1mb\x1b[22m");
    assert_eq!(theme.bold("a\r\nb"), "\x1b[1ma\x1b[22m\r\n\x1b[1mb\x1b[22m");
    assert_eq!(
        theme.bold("a\nb\nc"),
        "\x1b[1ma\x1b[22m\n\x1b[1mb\x1b[22m\n\x1b[1mc\x1b[22m"
    );
    assert_eq!(theme.bold("a\n"), "\x1b[1ma\x1b[22m\n\x1b[1m\x1b[22m");
    assert_eq!(theme.bold("\na"), "\x1b[1m\x1b[22m\n\x1b[1ma\x1b[22m");
    assert_eq!(theme.bold("\n"), "\x1b[1m\x1b[22m\n\x1b[1m\x1b[22m");
    assert_eq!(
        theme.bold(&format!("a{}\nc", theme.bold("b"))),
        "\x1b[1ma\x1b[1mb\x1b[22m\x1b[1m\x1b[22m\n\x1b[1mc\x1b[22m"
    );
    assert_eq!(
        theme.bold(&format!("{}{}", theme.bold("a"), theme.bold("b"))),
        "\x1b[1m\x1b[1ma\x1b[22m\x1b[1m\x1b[1mb\x1b[22m\x1b[1m\x1b[22m"
    );
}

// --- 256 colour quantisation ---------------------------------------------------

#[test]
fn quantises_hex_colors_with_cube_search() {
    let _guard = global_lock();
    const EXPECTED: [(&str, u16); 40] = [
        ("#000000", 16),
        ("#ffffff", 231),
        ("#808080", 244),
        ("#7f7f7f", 244),
        ("#818181", 244),
        ("#080808", 232),
        ("#eeeeee", 255),
        ("#ff0000", 196),
        ("#00ff00", 46),
        ("#0000ff", 21),
        ("#123456", 23),
        ("#abcdef", 153),
        ("#5f87ff", 69),
        ("#00d7ff", 45),
        ("#b5bd68", 143),
        ("#cc6666", 167),
        ("#8abeb7", 109),
        ("#3a3a4a", 59),
        ("#343541", 59),
        ("#2d2838", 17),
        ("#6a9955", 65),
        ("#569cd6", 74),
        ("#dcdcaa", 187),
        ("#9cdcfe", 153),
        ("#ce9178", 174),
        ("#b5cea8", 151),
        ("#4ec9b0", 79),
        ("#d4d4d4", 188),
        ("#505050", 239),
        ("#666666", 241),
        ("#010203", 16),
        ("#fefefe", 231),
        ("#111111", 233),
        ("#222222", 235),
        ("#5f5f5f", 59),
        ("#606060", 59),
        ("#e5e5e7", 254),
        ("#f0c674", 222),
        ("#81a2be", 109),
        ("#9575cd", 104),
    ];

    let directory = tempfile::Builder::new()
        .prefix("notagent-theme-256-")
        .tempdir()
        .expect("temp dir");
    let path = directory.path().join("probe.json");

    for (hex, index) in EXPECTED {
        let mut theme_json = dark_theme();
        theme_json["name"] = serde_json::json!("probe");
        theme_json["colors"]["accent"] = serde_json::json!(hex);
        std::fs::write(&path, theme_json.to_string()).expect("writes");
        let loaded = load_theme_from_path(&path, Some(ColorMode::Color256)).expect("loads");
        assert_eq!(
            loaded.get_fg_ansi(ThemeColor::Accent),
            format!("\x1b[38;5;{index}m"),
            "{hex}"
        );
    }
}

#[test]
fn emits_the_terminal_default_for_empty_color_values() {
    let _guard = global_lock();
    let directory = tempfile::Builder::new()
        .prefix("notagent-theme-empty-")
        .tempdir()
        .expect("temp dir");
    let path = directory.path().join("probe.json");
    let mut theme_json = dark_theme();
    theme_json["name"] = serde_json::json!("probe");
    theme_json["colors"]["accent"] = serde_json::json!("");
    theme_json["colors"]["selectedBg"] = serde_json::json!("");
    std::fs::write(&path, theme_json.to_string()).expect("writes");

    let loaded = load_theme_from_path(&path, Some(ColorMode::TrueColor)).expect("loads");
    assert_eq!(loaded.get_fg_ansi(ThemeColor::Accent), "\x1b[39m");
    assert_eq!(loaded.get_bg_ansi(ThemeBg::SelectedBg), "\x1b[49m");
    assert_eq!(loaded.fg(ThemeColor::Accent, "x"), "\x1b[39mx\x1b[39m");
    assert_eq!(loaded.bg(ThemeBg::SelectedBg, "x"), "\x1b[49mx\x1b[49m");
}

#[test]
fn accepts_palette_indices() {
    let _guard = global_lock();
    let directory = tempfile::Builder::new()
        .prefix("notagent-theme-index-")
        .tempdir()
        .expect("temp dir");
    let path = directory.path().join("probe.json");
    let mut theme_json = dark_theme();
    theme_json["name"] = serde_json::json!("probe");
    theme_json["colors"]["accent"] = serde_json::json!(24);
    std::fs::write(&path, theme_json.to_string()).expect("writes");

    let loaded = load_theme_from_path(&path, Some(ColorMode::TrueColor)).expect("loads");
    assert_eq!(loaded.get_fg_ansi(ThemeColor::Accent), "\x1b[38;5;24m");
}

// --- global theme lifecycle ----------------------------------------------------

#[test]
fn falls_back_to_dark_when_the_configured_theme_is_invalid() {
    let _guard = global_lock();
    let agent_dir = AgentDir::new();
    let mut broken = dark_theme();
    broken["name"] = serde_json::json!("broken");
    broken["colors"]
        .as_object_mut()
        .expect("colors")
        .remove("accent");
    agent_dir.write_theme("broken", &broken);

    init_theme(Some("broken"), true);
    assert!(is_theme_initialized());
    // A theme missing a required token falls back to dark, so the accent is
    // dark's.
    assert_eq!(theme().get_fg_ansi(ThemeColor::Accent), dark_accent_ansi());

    let result = set_theme("broken", true);
    assert!(!result.success);
    assert!(
        result
            .error
            .expect("error message")
            .contains("Missing required color tokens")
    );
    assert_eq!(theme().get_fg_ansi(ThemeColor::Accent), dark_accent_ansi());
}

#[test]
fn notifies_the_change_callback_on_every_switch() {
    let _guard = global_lock();
    let calls = Arc::new(Mutex::new(0usize));
    let sink = Arc::clone(&calls);
    on_theme_change(Arc::new(move || *sink.lock().unwrap() += 1));

    init_theme(Some("dark"), false);
    assert_eq!(*calls.lock().unwrap(), 0, "initTheme does not notify");

    set_theme("light", false);
    assert_eq!(*calls.lock().unwrap(), 1);
    set_theme("dark", false);
    assert_eq!(*calls.lock().unwrap(), 2);
    // The fallback path notifies as well.
    set_theme("does-not-exist", false);
    assert_eq!(*calls.lock().unwrap(), 2, "failures do not notify");

    on_theme_change(Arc::new(|| {}));
}

// --- live reload ---------------------------------------------------------------
// The guard serialises against the other tests in this binary, which run on
// their own threads; the await points below never contend for it.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn reloads_from_a_real_filesystem_event() {
    let _guard = global_lock();
    let agent_dir = AgentDir::new();
    let mut custom = dark_theme();
    custom["name"] = serde_json::json!("watched");
    custom["colors"]["accent"] = serde_json::json!("#010203");
    agent_dir.write_theme("watched", &custom);

    init_theme(Some("watched"), true);
    assert_eq!(theme().get_fg_ansi(ThemeColor::Accent), "\x1b[38;2;1;2;3m");

    // No `notify_theme_directory_event` call here: the OS watcher registered by
    // `start_theme_watcher` has to deliver the event on its own.
    custom["colors"]["accent"] = serde_json::json!("#0a0b0c");
    agent_dir.write_theme("watched", &custom);

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while theme().get_fg_ansi(ThemeColor::Accent) != "\x1b[38;2;10;11;12m" {
        assert!(
            std::time::Instant::now() < deadline,
            "the watcher never reloaded the theme"
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }

    // An unrelated file in the same directory does not switch the theme.
    agent_dir.write_theme("unrelated", &dark_theme());
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert_eq!(
        theme().get_fg_ansi(ThemeColor::Accent),
        "\x1b[38;2;10;11;12m"
    );

    stop_theme_watcher();
}

// The guard serialises against the other tests in this binary, which run on
// their own threads; the await points below never contend for it.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn reloads_the_watched_theme_after_the_debounce() {
    let _guard = global_lock();
    let agent_dir = AgentDir::new();
    let mut custom = dark_theme();
    custom["name"] = serde_json::json!("live");
    custom["colors"]["accent"] = serde_json::json!("#112233");
    agent_dir.write_theme("live", &custom);

    init_theme(Some("live"), true);
    assert_eq!(
        theme().get_fg_ansi(ThemeColor::Accent),
        "\x1b[38;2;17;34;51m"
    );

    custom["colors"]["accent"] = serde_json::json!("#445566");
    agent_dir.write_theme("live", &custom);
    notify_theme_directory_event(Some("live.json"));
    // Unrelated files never schedule a reload.
    notify_theme_directory_event(Some("other.json"));

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert_eq!(
        theme().get_fg_ansi(ThemeColor::Accent),
        "\x1b[38;2;68;85;102m"
    );
}

// The guard serialises against the other tests in this binary, which run on
// their own threads; the await points below never contend for it.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn does_not_reload_after_the_watcher_stopped() {
    let _guard = global_lock();
    let agent_dir = AgentDir::new();
    let mut custom = dark_theme();
    custom["name"] = serde_json::json!("stopped");
    custom["colors"]["accent"] = serde_json::json!("#112233");
    agent_dir.write_theme("stopped", &custom);

    init_theme(Some("stopped"), true);
    custom["colors"]["accent"] = serde_json::json!("#445566");
    agent_dir.write_theme("stopped", &custom);
    notify_theme_directory_event(Some("stopped.json"));
    stop_theme_watcher();

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert_eq!(
        theme().get_fg_ansi(ThemeColor::Accent),
        "\x1b[38;2;17;34;51m"
    );
}

// `process._getActiveHandles()` and emits a synthetic `error` on it: without a
// listener, `EventEmitter.emit("error")` throws and takes the process down.
// `notify` has no such rule — it hands the failure to the same closure as a
// the live reload stops instead of running on a broken watch.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn survives_an_error_reported_by_the_theme_watcher() {
    let _guard = global_lock();
    let agent_dir = AgentDir::new();
    let mut custom = dark_theme();
    custom["name"] = serde_json::json!("custom-test");
    custom["colors"]["accent"] = serde_json::json!("#112233");
    agent_dir.write_theme("custom-test", &custom);

    assert!(set_theme("custom-test", true).success, "theme loads");
    assert_eq!(
        theme().get_fg_ansi(ThemeColor::Accent),
        "\x1b[38;2;17;34;51m"
    );

    notify_theme_watcher_error();

    // The theme in place at the time of the failure stays active.
    assert_eq!(
        theme().get_fg_ansi(ThemeColor::Accent),
        "\x1b[38;2;17;34;51m"
    );

    // Events after the failure are dropped: the watch is no longer trustworthy.
    custom["colors"]["accent"] = serde_json::json!("#445566");
    agent_dir.write_theme("custom-test", &custom);
    notify_theme_directory_event(Some("custom-test.json"));
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert_eq!(
        theme().get_fg_ansi(ThemeColor::Accent),
        "\x1b[38;2;17;34;51m"
    );

    // Restarting the watcher clears the failure.
    assert!(set_theme("custom-test", true).success, "theme loads");
    assert_eq!(
        theme().get_fg_ansi(ThemeColor::Accent),
        "\x1b[38;2;68;85;102m"
    );
    custom["colors"]["accent"] = serde_json::json!("#778899");
    agent_dir.write_theme("custom-test", &custom);
    notify_theme_directory_event(Some("custom-test.json"));
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert_eq!(
        theme().get_fg_ansi(ThemeColor::Accent),
        "\x1b[38;2;119;136;153m"
    );

    stop_theme_watcher();
}

#[test]
fn does_not_watch_the_built_in_themes() {
    let _guard = global_lock();
    let _agent_dir = AgentDir::new();
    init_theme(Some("dark"), true);
    // No watcher was installed, so a directory event is ignored.
    notify_theme_directory_event(Some("dark.json"));
    assert_eq!(theme().get_fg_ansi(ThemeColor::Accent), dark_accent_ansi());
}

// ---------------------------------------------------------------------------
// Badge colours (v0.1.12)
// ---------------------------------------------------------------------------

/// Perceived brightness on 0–255, the measure the badge fills are chosen by.
fn luminance(ansi: &str) -> f32 {
    let start = ansi.find(";2;").expect("truecolor sequence") + 3;
    let values: Vec<f32> = ansi[start..]
        .split('m')
        .next()
        .expect("ANSI color terminator")
        .split(';')
        .filter_map(|part| part.parse::<f32>().ok())
        .collect();
    0.2126 * values[0] + 0.7152 * values[1] + 0.0722 * values[2]
}

/// A block background is tinted just enough to sit behind twenty lines of
/// text; the same value on an eight-character badge shows no state at all, so
/// the badge fills are their own, saturated tokens.
#[test]
fn badge_fills_are_darker_and_more_saturated_than_the_block_backgrounds() {
    let _guard = global_lock();
    init_theme(Some("dark"), false);
    let theme = theme();

    for (block, badge) in [
        (ThemeBg::ToolSuccessBg, ThemeBg::ToolSuccessBadgeBg),
        (ThemeBg::ToolErrorBg, ThemeBg::ToolErrorBadgeBg),
        (ThemeBg::CustomMessageBg, ThemeBg::CustomMessageBadgeBg),
    ] {
        assert_ne!(
            theme.get_bg_ansi(block),
            theme.get_bg_ansi(badge),
            "{badge:?} must not fall back to {block:?} in the built-in theme"
        );
    }
}

/// Every badge label has to stand off its fill; the thinking grey was the case
/// that gave this away — as a fill it sat at the label's own brightness.
#[test]
fn every_badge_keeps_its_label_legible_against_its_fill() {
    let _guard = global_lock();
    init_theme(Some("dark"), false);
    let theme = theme();
    use notagent::modes::interactive::theme::theme::{
        BlockStyle, badge, block_style, color_badge, set_block_style,
    };
    let previous_style = block_style();
    set_block_style(BlockStyle::Badge);
    let mut badges: Vec<String> = [
        ThemeBg::ToolPendingBadgeBg,
        ThemeBg::ToolSuccessBadgeBg,
        ThemeBg::ToolErrorBadgeBg,
        ThemeBg::CustomMessageBadgeBg,
    ]
    .into_iter()
    .map(|fill| badge(&theme, fill, "status"))
    .collect();
    badges.push(color_badge(&theme, ThemeColor::ThinkingText, "thought"));
    set_block_style(previous_style);

    for rendered in badges {
        let fill = luminance(&rendered);
        let foreground = rendered.find("\x1b[38;2;").expect("truecolor badge label");
        let label = luminance(&rendered[foreground..]);
        assert!(
            (label - fill).abs() > 100.0,
            "a badge label at {label} is unreadable on a fill at {fill}"
        );
    }
}

/// A theme that names no badge fill keeps its block background there, which is
/// what every theme did before the badge style existed.
#[test]
fn a_theme_without_badge_fills_falls_back_to_its_block_backgrounds() {
    let _guard = global_lock();
    let mut json = dark_theme();
    let colors = json
        .get_mut("colors")
        .and_then(serde_json::Value::as_object_mut)
        .expect("colors object");
    for key in [
        "toolPendingBadgeBg",
        "toolSuccessBadgeBg",
        "toolErrorBadgeBg",
        "customMessageBadgeBg",
        "badgeText",
    ] {
        colors.remove(key);
    }

    let directory = tempfile::tempdir().expect("temp dir");
    let path = directory.path().join("no-badges.json");
    std::fs::write(&path, serde_json::to_string(&json).expect("serialises"))
        .expect("theme written");
    let theme = load_theme_from_path(&path, Some(ColorMode::TrueColor))
        .expect("the theme loads without its badge fills");

    assert_eq!(
        theme.get_bg_ansi(ThemeBg::ToolSuccessBadgeBg),
        theme.get_bg_ansi(ThemeBg::ToolSuccessBg)
    );
    assert_eq!(
        theme.get_fg_ansi(ThemeColor::BadgeText),
        theme.get_fg_ansi(ThemeColor::Text)
    );
}
