use std::path::PathBuf;

use notagent::modes::interactive::theme::theme::{ColorMode, load_theme_from_path};
use notagent_agent::ThinkingLevel;

fn dark_theme() -> serde_json::Value {
    serde_json::from_str(include_str!("../src/modes/interactive/theme/dark.json"))
        .expect("dark.json parses")
}

/// Writes `content` to a temp file and returns its path.
fn write(content: &str) -> PathBuf {
    let directory = tempfile::Builder::new()
        .prefix("notagent-theme-validation-")
        .tempdir()
        .expect("temp dir");
    let path = directory.path().join("probe.json");
    let _ = directory.keep();
    std::fs::write(&path, content).expect("writes");
    path
}

/// Loads a mutated dark theme and returns the error message.
fn error_of(mutate: impl FnOnce(&mut serde_json::Value)) -> (String, PathBuf) {
    let mut theme = dark_theme();
    mutate(&mut theme);
    let path = write(&theme.to_string());
    let error = load_theme_from_path(&path, Some(ColorMode::TrueColor))
        .expect_err("theme is invalid")
        .0;
    (error, path)
}

fn colors_mut(theme: &mut serde_json::Value) -> &mut serde_json::Map<String, serde_json::Value> {
    theme["colors"].as_object_mut().expect("colors object")
}

#[test]
fn reports_missing_required_color_tokens_sorted() {
    let (error, path) = error_of(|theme| {
        colors_mut(theme).remove("accent");
        colors_mut(theme).remove("border");
    });
    assert_eq!(
        error,
        format!(
            "Invalid theme \"{}\":\n\nMissing required color tokens:\n  - accent\n  - border\n\nPlease add these colors to your theme's \"colors\" object.\nSee the built-in themes (dark.json, light.json) for reference values.",
            path.to_string_lossy()
        )
    );
}

#[test]
fn reports_the_union_error_triple_for_a_non_color_value() {
    let (error, path) = error_of(|theme| {
        colors_mut(theme).insert("accent".to_string(), serde_json::json!(true));
    });
    assert_eq!(
        error,
        format!(
            "Invalid theme \"{}\":\n\n\nOther errors:\n  - /colors/accent: must be string\n  - /colors/accent: must be integer\n  - /colors/accent: must match a schema in anyOf",
            path.to_string_lossy()
        )
    );
}

#[test]
fn reports_the_range_violation_of_a_palette_index() {
    let (error, _) = error_of(|theme| {
        colors_mut(theme).insert("accent".to_string(), serde_json::json!(300));
    });
    assert!(
        error.contains("  - /colors/accent: must be <= 255"),
        "{error}"
    );
    assert!(!error.contains("must be integer"), "{error}");

    let (error, _) = error_of(|theme| {
        colors_mut(theme).insert("accent".to_string(), serde_json::json!(-1));
    });
    assert!(
        error.contains("  - /colors/accent: must be >= 0"),
        "{error}"
    );

    let (error, _) = error_of(|theme| {
        colors_mut(theme).insert("accent".to_string(), serde_json::json!(1.5));
    });
    assert!(
        error.contains("  - /colors/accent: must be integer"),
        "{error}"
    );
}

#[test]
fn reports_missing_root_properties_in_one_error() {
    let (error, path) = error_of(|theme| {
        let root = theme.as_object_mut().expect("root object");
        root.remove("name");
        root.remove("colors");
    });
    assert_eq!(
        error,
        format!(
            "Invalid theme \"{}\":\n\n\nOther errors:\n  - /: must have required properties name, colors",
            path.to_string_lossy()
        )
    );
}

#[test]
fn rejects_theme_names_containing_a_slash() {
    let (error, _) = error_of(|theme| theme["name"] = serde_json::json!("a/b"));
    assert_eq!(
        error,
        "Invalid theme name \"a/b\": theme names cannot contain \"/\" because it is reserved for automatic light/dark theme settings."
    );
}

#[test]
fn reports_wrong_types_of_the_remaining_sections() {
    let (error, _) = error_of(|theme| theme["name"] = serde_json::json!(5));
    assert!(error.ends_with("  - /name: must be string"), "{error}");

    let (error, _) = error_of(|theme| theme["$schema"] = serde_json::json!(5));
    assert!(error.ends_with("  - /$schema: must be string"), "{error}");

    let (error, _) = error_of(|theme| theme["colors"] = serde_json::json!("x"));
    assert!(error.ends_with("  - /colors: must be object"), "{error}");

    let (error, _) = error_of(|theme| theme["vars"] = serde_json::json!(5));
    assert!(error.ends_with("  - /vars: must be object"), "{error}");

    let (error, _) = error_of(|theme| theme["export"] = serde_json::json!("x"));
    assert!(error.ends_with("  - /export: must be object"), "{error}");
}

#[test]
fn reports_missing_colors_before_the_other_errors() {
    let (error, path) = error_of(|theme| {
        colors_mut(theme).remove("accent");
        colors_mut(theme).insert("border".to_string(), serde_json::json!(true));
    });
    assert_eq!(
        error,
        format!(
            "Invalid theme \"{}\":\n\nMissing required color tokens:\n  - accent\n\nPlease add these colors to your theme's \"colors\" object.\nSee the built-in themes (dark.json, light.json) for reference values.\n\nOther errors:\n  - /colors/border: must be string\n  - /colors/border: must be integer\n  - /colors/border: must match a schema in anyOf",
            path.to_string_lossy()
        )
    );
}

#[test]
fn reports_errors_in_schema_order_not_file_order() {
    let (error, _) = error_of(|theme| {
        let colors = colors_mut(theme);
        let mut reordered = serde_json::Map::new();
        reordered.insert("bashMode".to_string(), serde_json::json!(true));
        reordered.insert("accent".to_string(), serde_json::json!(true));
        for (key, value) in colors.iter() {
            if key != "bashMode" && key != "accent" {
                reordered.insert(key.clone(), value.clone());
            }
        }
        theme["colors"] = serde_json::Value::Object(reordered);
    });
    let accent = error.find("/colors/accent").expect("accent error");
    let bash_mode = error.find("/colors/bashMode").expect("bashMode error");
    assert!(accent < bash_mode, "{error}");
}

#[test]
fn reports_sections_in_schema_order() {
    let (error, path) = error_of(|theme| {
        theme["name"] = serde_json::json!(5);
        theme["vars"]["cyan"] = serde_json::json!(true);
        colors_mut(theme).insert("accent".to_string(), serde_json::json!(true));
        theme["export"] = serde_json::json!({ "pageBg": true });
    });
    // TypeBox stops after eight errors, so the last two union lines of
    // `/export/pageBg` never appear.
    assert_eq!(
        error,
        format!(
            "Invalid theme \"{}\":\n\n\nOther errors:\n  - /name: must be string\n  - /vars/cyan: must be string\n  - /vars/cyan: must be integer\n  - /vars/cyan: must match a schema in anyOf\n  - /colors/accent: must be string\n  - /colors/accent: must be integer\n  - /colors/accent: must match a schema in anyOf\n  - /export/pageBg: must be string",
            path.to_string_lossy()
        )
    );
}

#[test]
fn stops_after_eight_errors() {
    let (error, _) = error_of(|theme| {
        for key in ["accent", "border", "borderAccent", "borderMuted", "success"] {
            colors_mut(theme).insert(key.to_string(), serde_json::json!(true));
        }
    });
    assert_eq!(error.matches("\n  - ").count(), 8, "{error}");
    assert!(
        error.ends_with("  - /colors/borderAccent: must be integer"),
        "{error}"
    );
}

#[test]
fn rejects_non_object_roots() {
    for content in ["[]", "5", "null"] {
        let path = write(content);
        let error = load_theme_from_path(&path, Some(ColorMode::TrueColor))
            .expect_err("root is not an object")
            .0;
        assert_eq!(
            error,
            format!(
                "Invalid theme \"{}\":\n\n\nOther errors:\n  - /: must be object",
                path.to_string_lossy()
            )
        );
    }
}

#[test]
fn reports_an_empty_object_as_missing_name_and_colors() {
    let path = write("{}");
    let error = load_theme_from_path(&path, Some(ColorMode::TrueColor))
        .expect_err("empty object is invalid")
        .0;
    assert_eq!(
        error,
        format!(
            "Invalid theme \"{}\":\n\n\nOther errors:\n  - /: must have required properties name, colors",
            path.to_string_lossy()
        )
    );
}

#[test]
fn reports_unparsable_json_with_the_file_label() {
    let path = write("{ not json");
    let error = load_theme_from_path(&path, Some(ColorMode::TrueColor))
        .expect_err("invalid json")
        .0;
    // The parser's own wording differs from V8's `SyntaxError` text
    // (language idiom); the label and prefix are identical.
    assert!(
        error.starts_with(&format!(
            "Failed to parse theme {}: ",
            path.to_string_lossy()
        )),
        "{error}"
    );
}

#[test]
fn reports_color_resolution_failures() {
    let (error, _) = error_of(|theme| {
        colors_mut(theme).insert("accent".to_string(), serde_json::json!("#zz"));
    });
    assert_eq!(error, "Invalid hex color: #zz");

    let (error, _) = error_of(|theme| {
        colors_mut(theme).insert("accent".to_string(), serde_json::json!("nosuchvar"));
    });
    assert_eq!(error, "Variable reference not found: nosuchvar");

    let (error, _) = error_of(|theme| {
        theme["vars"]["loopA"] = serde_json::json!("loopB");
        theme["vars"]["loopB"] = serde_json::json!("loopA");
        colors_mut(theme).insert("accent".to_string(), serde_json::json!("loopA"));
    });
    assert_eq!(error, "Circular variable reference detected: loopA");
}

#[test]
fn accepts_unknown_color_keys_and_omitted_optional_slots() {
    let mut theme = dark_theme();
    colors_mut(&mut theme).insert("bogusKey".to_string(), serde_json::json!("#ffffff"));
    colors_mut(&mut theme).remove("thinkingMax");
    let path = write(&theme.to_string());
    assert!(load_theme_from_path(&path, Some(ColorMode::TrueColor)).is_ok());
}

/// thinkingXhigh for legacy themes"); the CLI and settings half belongs to
/// `thinkingXhigh`, so a theme written before the level existed still paints the
/// editor border.
#[test]
fn falls_back_to_thinking_xhigh_for_legacy_themes() {
    let mut legacy = dark_theme();
    legacy["name"] = serde_json::json!("legacy-theme");
    colors_mut(&mut legacy).remove("thinkingMax");
    let path = write(&legacy.to_string());

    let theme = load_theme_from_path(&path, Some(ColorMode::TrueColor)).expect("theme loads");
    assert_eq!(
        theme.get_thinking_border_color(ThinkingLevel::Max)("border"),
        theme.get_thinking_border_color(ThinkingLevel::Xhigh)("border")
    );
}
