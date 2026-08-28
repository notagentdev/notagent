use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures::future::BoxFuture;
use notagent::core::project_trust::{
    ProjectTrustContext, ResolveProjectTrustedOptions, resolve_project_trusted,
};
use notagent::core::settings_manager::DefaultProjectTrust;
use notagent::core::trust_manager::{ProjectTrustStore, get_project_trust_options};

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let directory = tempfile::Builder::new()
            .prefix("trust-test-")
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
fn stores_decisions_and_inherits_from_parent_directories() {
    let temp = TempDir::new();
    let agent_dir = temp.create("agent");
    let store = ProjectTrustStore::new(&text(&agent_dir));
    let parent_dir = temp.join("trusted-parent");
    let child_dir = parent_dir.join("project");
    std::fs::create_dir_all(&child_dir).expect("creates");

    assert_eq!(store.get(&text(&child_dir)).expect("reads"), None);
    store.set(&text(&parent_dir), Some(true)).expect("writes");
    assert_eq!(store.get(&text(&child_dir)).expect("reads"), Some(true));
    store.set(&text(&child_dir), Some(false)).expect("writes");
    assert_eq!(store.get(&text(&child_dir)).expect("reads"), Some(false));
    store.set(&text(&child_dir), None).expect("writes");
    assert_eq!(store.get(&text(&child_dir)).expect("reads"), Some(true));
}

#[test]
fn offers_the_parent_folder_and_the_session_only_answers() {
    let temp = TempDir::new();
    let cwd = temp.create("project");
    let options = get_project_trust_options(&text(&cwd), true);
    let labels: Vec<&str> = options.iter().map(|option| option.label.as_str()).collect();
    assert_eq!(labels.len(), 5);
    assert_eq!(labels[0], "Trust");
    assert!(labels[1].starts_with("Trust parent folder ("));
    assert_eq!(labels[2], "Trust (this session only)");
    assert_eq!(labels[3], "Do not trust");
    assert_eq!(labels[4], "Do not trust (this session only)");
    // Trusting the parent clears whatever the folder itself said, so the
    // inherited decision is the one that applies.
    assert_eq!(options[1].updates.len(), 2);
    assert_eq!(options[1].updates[1].decision, None);
    // A session-only answer records nothing at all.
    assert!(options[2].updates.is_empty());
    assert!(options[2].trusted);
}

#[test]
fn omits_the_session_only_answers_when_they_are_not_wanted() {
    let temp = TempDir::new();
    let cwd = temp.create("project");
    let labels: Vec<String> = get_project_trust_options(&text(&cwd), false)
        .into_iter()
        .map(|option| option.label)
        .collect();
    assert_eq!(labels.len(), 3);
    assert!(
        !labels
            .iter()
            .any(|label| label.contains("this session only"))
    );
}

fn resolve(
    cwd: &str,
    store: &ProjectTrustStore,
    trust_override: Option<bool>,
    default_project_trust: Option<DefaultProjectTrust>,
    context: ProjectTrustContext,
) -> bool {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("runtime");
    runtime
        .block_on(resolve_project_trusted(ResolveProjectTrustedOptions {
            cwd,
            trust_store: store,
            trust_override,
            default_project_trust,
            project_trust_context: context,
        }))
        .expect("resolves")
}

#[test]
fn an_explicit_override_decides_without_asking_anything() {
    let temp = TempDir::new();
    let store = ProjectTrustStore::new(&text(&temp.create("agent")));
    let cwd = text(&temp.create("project"));
    assert!(resolve(
        &cwd,
        &store,
        Some(true),
        None,
        ProjectTrustContext::default()
    ));
    assert!(!resolve(
        &cwd,
        &store,
        Some(false),
        None,
        ProjectTrustContext::default()
    ));
}

#[test]
fn a_project_without_trust_requiring_resources_is_trusted() {
    let temp = TempDir::new();
    let store = ProjectTrustStore::new(&text(&temp.create("agent")));
    let cwd = text(&temp.create("project"));
    assert!(resolve(
        &cwd,
        &store,
        None,
        Some(DefaultProjectTrust::Never),
        ProjectTrustContext::default()
    ));
}

#[test]
fn a_recorded_decision_is_used_before_the_default_and_the_prompt() {
    let temp = TempDir::new();
    let store = ProjectTrustStore::new(&text(&temp.create("agent")));
    let project = temp.create("project");
    std::fs::create_dir_all(project.join(".notagent")).expect("creates");
    std::fs::write(project.join(".notagent/settings.json"), "{}").expect("writes");
    let cwd = text(&project);

    store.set(&cwd, Some(false)).expect("writes");
    assert!(!resolve(
        &cwd,
        &store,
        None,
        Some(DefaultProjectTrust::Always),
        ProjectTrustContext::default()
    ));
    store.set(&cwd, Some(true)).expect("writes");
    assert!(resolve(
        &cwd,
        &store,
        None,
        Some(DefaultProjectTrust::Never),
        ProjectTrustContext::default()
    ));
}

#[test]
fn without_a_ui_an_undecided_project_is_not_trusted() {
    let temp = TempDir::new();
    let store = ProjectTrustStore::new(&text(&temp.create("agent")));
    let project = temp.create("project");
    std::fs::create_dir_all(project.join(".notagent")).expect("creates");
    std::fs::write(project.join(".notagent/settings.json"), "{}").expect("writes");
    let cwd = text(&project);
    assert!(!resolve(
        &cwd,
        &store,
        None,
        Some(DefaultProjectTrust::Ask),
        ProjectTrustContext::default()
    ));
}

#[test]
fn the_prompt_answer_is_recorded_and_returned() {
    let temp = TempDir::new();
    let store = ProjectTrustStore::new(&text(&temp.create("agent")));
    let project = temp.create("project");
    std::fs::create_dir_all(project.join(".notagent")).expect("creates");
    std::fs::write(project.join(".notagent/settings.json"), "{}").expect("writes");
    let cwd = text(&project);

    let context = ProjectTrustContext {
        has_ui: true,
        select: Some(Arc::new(|prompt: String, labels: Vec<String>| {
            assert!(prompt.starts_with("Trust project folder?"));
            let chosen = labels[0].clone();
            Box::pin(async move { Some(chosen) }) as BoxFuture<'static, Option<String>>
        })),
    };
    assert!(resolve(
        &cwd,
        &store,
        None,
        Some(DefaultProjectTrust::Ask),
        context
    ));
    assert_eq!(store.get(&cwd).expect("reads"), Some(true));
}

#[test]
fn a_dismissed_prompt_leaves_the_project_untrusted_and_unrecorded() {
    let temp = TempDir::new();
    let store = ProjectTrustStore::new(&text(&temp.create("agent")));
    let project = temp.create("project");
    std::fs::create_dir_all(project.join(".notagent")).expect("creates");
    std::fs::write(project.join(".notagent/settings.json"), "{}").expect("writes");
    let cwd = text(&project);

    let context = ProjectTrustContext {
        has_ui: true,
        select: Some(Arc::new(|_prompt, _labels| {
            Box::pin(async { None }) as BoxFuture<'static, Option<String>>
        })),
    };
    assert!(!resolve(
        &cwd,
        &store,
        None,
        Some(DefaultProjectTrust::Ask),
        context
    ));
    assert_eq!(store.get(&cwd).expect("reads"), None);
}
