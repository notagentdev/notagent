use std::sync::{Mutex, MutexGuard, OnceLock};

use notagent::core::tools::render_utils::{
    get_text_output, invalid_arg_text, link_path, normalize_display_text, render_tool_path,
    replace_tabs, shorten_path, str_arg,
};
use notagent::modes::interactive::theme::theme::{init_theme, theme};
use notagent::utils::ansi::strip_ansi;
use notagent_ai::types::{ImageContent, TextContent, TextOrImageContent};
use notagent_tui::terminal_image::{
    ImageProtocol, TerminalCapabilities, reset_capabilities_cache, set_capabilities,
};
use serde_json::json;

/// The theme and the terminal capabilities are process globals.
fn test_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let guard = LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    init_theme(Some("dark"), false);
    guard
}

fn with_capabilities(images: Option<ImageProtocol>, hyperlinks: bool) {
    set_capabilities(TerminalCapabilities {
        images,
        true_color: true,
        hyperlinks,
    });
}

fn text(value: &str) -> TextOrImageContent {
    TextOrImageContent::Text(TextContent {
        text: value.to_string(),
        ..Default::default()
    })
}

fn image(data: &str, mime_type: &str) -> TextOrImageContent {
    TextOrImageContent::Image(ImageContent {
        data: data.to_string(),
        mime_type: mime_type.to_string(),
    })
}

// --- path helpers ------------------------------------------------------------------

#[test]
fn shortens_a_path_below_the_home_directory() {
    let home = dirs::home_dir().expect("a home directory");
    let home = home.to_string_lossy().into_owned();

    assert_eq!(shorten_path(Some(&format!("{home}/project"))), "~/project");
    assert_eq!(
        shorten_path(Some("/elsewhere/project")),
        "/elsewhere/project"
    );
    // `typeof path !== "string"` renders as the empty string.
    assert_eq!(shorten_path(None), "");
}

#[test]
fn reads_a_string_argument_and_reports_a_wrong_type() {
    let arguments = json!({ "path": "src/main.rs", "empty": null, "wrong": 7 });

    assert_eq!(
        str_arg(arguments.get("path")).as_deref(),
        Some("src/main.rs")
    );
    // `value == null` — both `null` and a missing key become the empty string.
    assert_eq!(str_arg(arguments.get("empty")).as_deref(), Some(""));
    assert_eq!(str_arg(arguments.get("missing")).as_deref(), Some(""));
    // Anything else is `null`, which the callers render as `[invalid arg]`.
    assert_eq!(str_arg(arguments.get("wrong")), None);
}

#[test]
fn replaces_tabs_and_drops_carriage_returns() {
    assert_eq!(replace_tabs("a\tb\tc"), "a   b   c");
    assert_eq!(normalize_display_text("a\r\nb\rc"), "a\nbc");
}

// --- tool path column ----------------------------------------------------------------

#[test]
fn renders_the_path_column() {
    let _guard = test_lock();
    with_capabilities(None, false);
    let theme = theme();

    // A wrong argument type never renders as a path.
    assert_eq!(
        strip_ansi(&render_tool_path(None, &theme, "/tmp", None)),
        "[invalid arg]"
    );
    assert_eq!(
        render_tool_path(None, &theme, "/tmp", None),
        invalid_arg_text(&theme)
    );

    // An empty path takes the fallback, and without one the placeholder.
    assert_eq!(
        strip_ansi(&render_tool_path(Some(""), &theme, "/tmp", Some("."))),
        "."
    );
    assert_eq!(
        strip_ansi(&render_tool_path(Some(""), &theme, "/tmp", None)),
        "..."
    );

    // A real path is shortened and coloured.
    let home = dirs::home_dir().expect("a home directory");
    let home = home.to_string_lossy().into_owned();
    let rendered = render_tool_path(Some(&format!("{home}/x.rs")), &theme, "/tmp", None);
    assert_eq!(strip_ansi(&rendered), "~/x.rs");
    assert!(rendered.starts_with(
        theme.get_fg_ansi(notagent::modes::interactive::theme::theme::ThemeColor::Accent)
    ));

    reset_capabilities_cache();
}

#[test]
fn links_the_path_only_when_the_terminal_supports_hyperlinks() {
    let _guard = test_lock();

    with_capabilities(None, false);
    assert_eq!(link_path("label", "src/main.rs", "/tmp"), "label");

    with_capabilities(None, true);
    let linked = link_path("label", "src/main.rs", "/tmp");
    assert!(
        linked.contains("\x1b]8;;file:///tmp/src/main.rs"),
        "{linked:?}"
    );
    assert!(linked.contains("label"), "{linked:?}");

    reset_capabilities_cache();
}

// --- result text -----------------------------------------------------------------------

#[test]
fn joins_text_blocks_and_strips_ansi_and_carriage_returns() {
    let _guard = test_lock();
    with_capabilities(None, false);

    let content = [text("\x1b[31mred\x1b[39m"), text("second\r\nline")];
    assert_eq!(get_text_output(Some(&content), true), "red\nsecond\nline");

    // `!result` — no result at all is the empty string.
    assert_eq!(get_text_output(None, true), "");

    reset_capabilities_cache();
}

#[test]
fn appends_an_image_indicator_when_images_cannot_be_shown() {
    let _guard = test_lock();
    // A 1x1 PNG, so the fallback can report dimensions.
    const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

    // No image support: the indicator replaces the picture.
    with_capabilities(None, false);
    let content = [text("output"), image(PNG, "image/png")];
    let rendered = get_text_output(Some(&content), true);
    assert!(rendered.starts_with("output\n"), "{rendered:?}");
    assert!(rendered.contains("image/png"), "{rendered:?}");
    assert!(rendered.contains("1x1"), "{rendered:?}");

    // Images supported but switched off: same fallback.
    with_capabilities(Some(ImageProtocol::Kitty), false);
    let rendered = get_text_output(Some(&content), false);
    assert!(rendered.contains("image/png"), "{rendered:?}");

    // Supported and switched on: the component renders the picture itself.
    let rendered = get_text_output(Some(&content), true);
    assert_eq!(rendered, "output");

    // Without any text the indicator stands alone.
    with_capabilities(None, false);
    let images_only = [image(PNG, "image/png")];
    let rendered = get_text_output(Some(&images_only), true);
    assert!(!rendered.starts_with('\n'), "{rendered:?}");
    assert!(rendered.contains("image/png"), "{rendered:?}");

    reset_capabilities_cache();
}
