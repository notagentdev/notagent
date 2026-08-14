//! The `hasTrustRequiringProjectResources` case of
//! `packages/coding-agent/test/trust-manager.test.ts`.
//!
//! In its own file because it replaces `$HOME` for the duration: every test in
//! a file shares one process, and reading the environment while another thread
//! writes it is undefined behaviour.

use std::path::{Path, PathBuf};

use notagent::core::trust_manager::has_trust_requiring_project_resources;

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let directory = tempfile::Builder::new()
            .prefix("trust-home-")
            .tempdir()
            .expect("temp dir");
        let path = directory.path().to_path_buf();
        let _ = directory.keep();
        Self { path }
    }

    fn join(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }

    fn create(&self, name: &str) -> PathBuf {
        let path = self.join(name);
        std::fs::create_dir_all(&path).expect("creates");
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[test]
fn detects_trust_requiring_project_resources() {
    let temp = TempDir::new();
    let cwd = temp.create("project");
    let original_home = std::env::var("HOME").ok();
    // The user's own `~/.agents/skills` is a trusted user resource, so the home
    // directory has to be the one this test controls.
    unsafe { std::env::set_var("HOME", &temp.path) };

    temp.create(".notagent/agent");
    temp.create(".agents/skills");
    assert!(!has_trust_requiring_project_resources(&text(&temp.path)));
    assert!(!has_trust_requiring_project_resources(&text(&cwd)));

    std::fs::write(temp.join(".notagent/settings.json"), "{}").expect("writes");
    assert!(has_trust_requiring_project_resources(&text(&temp.path)));
    std::fs::remove_file(temp.join(".notagent/settings.json")).expect("removes");

    std::fs::create_dir_all(cwd.join(".notagent")).expect("creates");
    std::fs::write(cwd.join(".notagent/settings.json"), "{}").expect("writes");
    assert!(has_trust_requiring_project_resources(&text(&cwd)));

    std::fs::remove_dir_all(cwd.join(".notagent")).expect("removes");
    std::fs::create_dir_all(cwd.join(".agents/skills")).expect("creates");
    assert!(has_trust_requiring_project_resources(&text(&cwd)));

    match original_home {
        Some(home) => unsafe { std::env::set_var("HOME", home) },
        None => unsafe { std::env::remove_var("HOME") },
    }
}
