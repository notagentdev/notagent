use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

use notagent::config::env_agent_dir;
use notagent::modes::interactive::theme::theme::{ThemeExportColors, get_theme_export_colors};

/// `NOTAGENT_CODING_AGENT_DIR` is process global, so these tests run serially.
fn agent_dir_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

struct AgentDir {
    root: PathBuf,
    agent_dir: PathBuf,
    previous: Option<String>,
}

impl AgentDir {
    fn new() -> Self {
        let directory = tempfile::Builder::new()
            .prefix("notagent-theme-export-")
            .tempdir()
            .expect("temp dir");
        let root = directory.path().to_path_buf();
        let _ = directory.keep();
        let agent_dir = root.join("agent");
        let previous = std::env::var(env_agent_dir()).ok();
        unsafe { std::env::set_var(env_agent_dir(), &agent_dir) };
        std::fs::create_dir_all(agent_dir.join("themes")).expect("creates");
        Self {
            root,
            agent_dir,
            previous,
        }
    }

    fn write_theme(&self, name: &str, theme: &serde_json::Value) {
        std::fs::write(
            self.agent_dir.join("themes").join(format!("{name}.json")),
            serde_json::to_string_pretty(theme).expect("serializes"),
        )
        .expect("writes");
    }
}

impl Drop for AgentDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
        match self.previous.take() {
            Some(previous) => unsafe { std::env::set_var(env_agent_dir(), previous) },
            None => unsafe { std::env::remove_var(env_agent_dir()) },
        }
    }
}

fn dark_theme() -> serde_json::Value {
    serde_json::from_str(include_str!("../src/modes/interactive/theme/dark.json"))
        .expect("dark.json parses")
}

#[test]
fn resolves_export_variable_references_using_the_same_syntax_as_colors() {
    let _guard = agent_dir_lock();
    let agent_dir = AgentDir::new();

    let mut custom_theme = dark_theme();
    custom_theme["name"] = serde_json::json!("custom-export-vars");
    let vars = custom_theme["vars"].as_object_mut().expect("vars object");
    vars.insert("pageBgVar".to_string(), serde_json::json!("#112233"));
    vars.insert("pageBgAlias".to_string(), serde_json::json!("pageBgVar"));
    vars.insert("infoBgVar".to_string(), serde_json::json!("#445566"));
    vars.insert("cardBgVar".to_string(), serde_json::json!("#223344"));
    custom_theme["export"] = serde_json::json!({
        "pageBg": "pageBgAlias",
        "cardBg": "cardBgVar",
        "infoBg": "infoBgVar",
    });
    agent_dir.write_theme("custom-export-vars", &custom_theme);

    assert_eq!(
        get_theme_export_colors(Some("custom-export-vars")),
        ThemeExportColors {
            page_bg: Some("#112233".to_string()),
            card_bg: Some("#223344".to_string()),
            info_bg: Some("#445566".to_string()),
        }
    );
}

#[test]
fn resolves_recursive_vars_and_converts_256_color_export_values_to_hex() {
    let _guard = agent_dir_lock();
    let agent_dir = AgentDir::new();

    let mut custom_theme = dark_theme();
    custom_theme["name"] = serde_json::json!("custom-export-recursive");
    let vars = custom_theme["vars"].as_object_mut().expect("vars object");
    vars.insert("deepPageBg".to_string(), serde_json::json!("#abcdef"));
    vars.insert("pageBgAlias".to_string(), serde_json::json!("deepPageBg"));
    vars.insert("cardBgAnsi".to_string(), serde_json::json!(24));
    custom_theme["export"] = serde_json::json!({
        "pageBg": "pageBgAlias",
        "cardBg": "cardBgAnsi",
        "infoBg": "",
    });
    agent_dir.write_theme("custom-export-recursive", &custom_theme);

    assert_eq!(
        get_theme_export_colors(Some("custom-export-recursive")),
        ThemeExportColors {
            page_bg: Some("#abcdef".to_string()),
            card_bg: Some("#005f87".to_string()),
            info_bg: None,
        }
    );
}
