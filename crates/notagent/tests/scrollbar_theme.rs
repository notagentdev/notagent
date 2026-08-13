//! Port of `packages/coding-agent/test/scrollbar-theme.test.ts` (70 LOC).

use std::path::PathBuf;

use notagent::modes::interactive::theme::theme::{
    ColorMode, Theme, ThemeBg, ThemeColor, load_theme_from_path,
};

fn load_dark_theme() -> serde_json::Value {
    serde_json::from_str(include_str!("../src/modes/interactive/theme/dark.json"))
        .expect("dark.json parses")
}

/// Writes the theme into a fresh temporary directory and returns its path.
fn write_theme(theme: &serde_json::Value) -> PathBuf {
    let directory = tempfile::Builder::new()
        .prefix("notagent-scrollbar-theme-")
        .tempdir()
        .expect("temp dir");
    let test_dir = directory.path().to_path_buf();
    let _ = directory.keep();
    let name = theme["name"].as_str().expect("theme name");
    let theme_path = test_dir.join(format!("{name}.json"));
    std::fs::write(&theme_path, theme.to_string()).expect("writes");
    theme_path
}

fn load(theme: &serde_json::Value) -> Theme {
    load_theme_from_path(&write_theme(theme), Some(ColorMode::TrueColor)).expect("theme loads")
}

#[test]
fn falls_back_to_selected_bg_when_scrollbar_thumb_is_omitted() {
    let mut theme_json = load_dark_theme();
    theme_json["name"] = serde_json::json!("legacy-scrollbar-theme");
    theme_json["colors"]
        .as_object_mut()
        .expect("colors")
        .remove("scrollbarThumb");

    let loaded_theme = load(&theme_json);
    assert_eq!(
        loaded_theme.get_bg_ansi(ThemeBg::ScrollbarThumb),
        loaded_theme.get_bg_ansi(ThemeBg::SelectedBg)
    );
}

#[test]
fn uses_an_explicitly_configured_scrollbar_thumb() {
    let mut theme_json = load_dark_theme();
    theme_json["name"] = serde_json::json!("custom-scrollbar-theme");
    theme_json["colors"]["scrollbarThumb"] = serde_json::json!("#123456");

    let loaded_theme = load(&theme_json);
    assert_eq!(
        loaded_theme.get_bg_ansi(ThemeBg::ScrollbarThumb),
        "\x1b[48;2;18;52;86m"
    );
}

#[test]
fn falls_back_to_existing_selection_and_text_colors_for_search_highlights() {
    let mut theme_json = load_dark_theme();
    theme_json["name"] = serde_json::json!("legacy-search-theme");
    let colors = theme_json["colors"].as_object_mut().expect("colors");
    colors.remove("searchMatchBg");
    colors.remove("searchMatchText");

    let loaded_theme = load(&theme_json);
    assert_eq!(
        loaded_theme.get_bg_ansi(ThemeBg::SearchMatchBg),
        loaded_theme.get_bg_ansi(ThemeBg::SelectedBg)
    );
    assert_eq!(
        loaded_theme.get_fg_ansi(ThemeColor::SearchMatchText),
        loaded_theme.get_fg_ansi(ThemeColor::Text)
    );
}

#[test]
fn uses_explicitly_configured_search_highlight_colors() {
    let mut theme_json = load_dark_theme();
    theme_json["name"] = serde_json::json!("custom-search-theme");
    theme_json["colors"]["searchMatchBg"] = serde_json::json!("#112233");
    theme_json["colors"]["searchMatchText"] = serde_json::json!("#223344");

    let loaded_theme = load(&theme_json);
    assert_eq!(
        loaded_theme.get_bg_ansi(ThemeBg::SearchMatchBg),
        "\x1b[48;2;17;34;51m"
    );
    assert_eq!(
        loaded_theme.get_fg_ansi(ThemeColor::SearchMatchText),
        "\x1b[38;2;34;51;68m"
    );
}
