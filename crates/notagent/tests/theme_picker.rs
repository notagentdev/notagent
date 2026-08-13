//! Port of `packages/coding-agent/test/theme-picker.test.ts` (51 LOC).

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

use notagent::config::env_agent_dir;
use notagent::modes::interactive::theme::theme::{
    ThemeInfo, get_available_themes, get_available_themes_with_paths, set_registered_themes,
};

/// `NOTAGENT_CODING_AGENT_DIR` and the theme registry are process global.
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
            .prefix("notagent-theme-picker-")
            .tempdir()
            .expect("temp dir");
        let root = directory.path().to_path_buf();
        let _ = directory.keep();
        let agent_dir = root.join("agent");
        let previous = std::env::var(env_agent_dir()).ok();
        unsafe { std::env::set_var(env_agent_dir(), &agent_dir) };
        std::fs::create_dir_all(agent_dir.join("themes")).expect("creates");
        set_registered_themes(Vec::new()).expect("empty registry");
        Self {
            root,
            agent_dir,
            previous,
        }
    }
}

impl Drop for AgentDir {
    fn drop(&mut self) {
        set_registered_themes(Vec::new()).expect("empty registry");
        let _ = std::fs::remove_dir_all(&self.root);
        match self.previous.take() {
            Some(previous) => unsafe { std::env::set_var(env_agent_dir(), previous) },
            None => unsafe { std::env::remove_var(env_agent_dir()) },
        }
    }
}

#[test]
fn uses_custom_theme_content_names_instead_of_file_names() {
    let _guard = agent_dir_lock();
    let agent_dir = AgentDir::new();

    let mut custom_theme: serde_json::Value =
        serde_json::from_str(include_str!("../src/modes/interactive/theme/dark.json"))
            .expect("dark.json parses");
    custom_theme["name"] = serde_json::json!("bar");

    let theme_path = agent_dir.agent_dir.join("themes").join("foo.json");
    std::fs::write(
        &theme_path,
        serde_json::to_string_pretty(&custom_theme).expect("serializes"),
    )
    .expect("writes");

    assert!(get_available_themes().contains(&"bar".to_string()));
    assert!(!get_available_themes().contains(&"foo".to_string()));
    assert!(get_available_themes_with_paths().contains(&ThemeInfo {
        name: "bar".to_string(),
        path: Some(theme_path.to_string_lossy().into_owned()),
    }));
    assert!(
        !get_available_themes_with_paths()
            .iter()
            .any(|theme| theme.name == "foo")
    );
}
