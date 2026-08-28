use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use futures::future::BoxFuture;
use notagent::core::package_manager::command_runner::{CommandOptions, CommandRunner};
use notagent::core::package_manager::{
    DefaultPackageManager, PackageKind, PackageManagerOptions, PackageUpdate, ParsedSource,
    ProgressAction, ProgressEvent, ProgressKind,
};
use notagent::core::resource_loader::{ResolvedResource, ResolvedResources};
use notagent::core::settings_manager::{
    PackageSource, PackageSourceFilter, Settings, SettingsManager, SettingsManagerCreateOptions,
};
use notagent::core::source_info::SourceScope;

// ============================================================================
// Fixture
// ============================================================================

/// The suite mutates `HOME` and `NOTAGENT_OFFLINE`, which are process-wide.
/// Rust runs tests in threads of one process, so every test holds this lock —
fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// One recorded spawn.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RecordedCall {
    command: String,
    args: Vec<String>,
    cwd: Option<String>,
}

impl RecordedCall {
    fn matches(&self, command: &str, args: &[&str]) -> bool {
        self.command == command
            && self
                .args
                .iter()
                .map(String::as_str)
                .eq(args.iter().copied())
    }
}

type RunHandler = Arc<dyn Fn(&str, &[String], &CommandOptions) -> Result<(), String> + Send + Sync>;
type CaptureHandler =
    Arc<dyn Fn(&str, &[String], &CommandOptions) -> Result<String, String> + Send + Sync>;
type SyncHandler = Arc<dyn Fn(&str, &[String]) -> Result<String, String> + Send + Sync>;

#[derive(Default)]
struct FakeRunner {
    runs: Mutex<Vec<RecordedCall>>,
    captures: Mutex<Vec<RecordedCall>>,
    syncs: Mutex<Vec<RecordedCall>>,
    run_handler: Mutex<Option<RunHandler>>,
    capture_handler: Mutex<Option<CaptureHandler>>,
    sync_handler: Mutex<Option<SyncHandler>>,
    /// Milliseconds every `run` sleeps, so concurrency is observable.
    run_delay_ms: Mutex<BTreeMap<String, u64>>,
    active: Mutex<BTreeMap<String, usize>>,
    peak: Mutex<BTreeMap<String, usize>>,
    capture_delay_ms: AtomicUsize,
}

impl FakeRunner {
    fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn on_run<F>(self: &Arc<Self>, handler: F)
    where
        F: Fn(&str, &[String], &CommandOptions) -> Result<(), String> + Send + Sync + 'static,
    {
        *self.run_handler.lock().unwrap() = Some(Arc::new(handler));
    }

    fn on_capture<F>(self: &Arc<Self>, handler: F)
    where
        F: Fn(&str, &[String], &CommandOptions) -> Result<String, String> + Send + Sync + 'static,
    {
        *self.capture_handler.lock().unwrap() = Some(Arc::new(handler));
    }

    fn on_sync<F>(self: &Arc<Self>, handler: F)
    where
        F: Fn(&str, &[String]) -> Result<String, String> + Send + Sync + 'static,
    {
        *self.sync_handler.lock().unwrap() = Some(Arc::new(handler));
    }

    fn delay_run(self: &Arc<Self>, command: &str, millis: u64) {
        self.run_delay_ms
            .lock()
            .unwrap()
            .insert(command.to_owned(), millis);
    }

    fn runs(&self) -> Vec<RecordedCall> {
        self.runs.lock().unwrap().clone()
    }

    fn captures(&self) -> Vec<RecordedCall> {
        self.captures.lock().unwrap().clone()
    }

    fn syncs(&self) -> Vec<RecordedCall> {
        self.syncs.lock().unwrap().clone()
    }

    fn peak_concurrency(&self, command: &str) -> usize {
        self.peak.lock().unwrap().get(command).copied().unwrap_or(0)
    }

    fn ran(&self, command: &str, args: &[&str]) -> bool {
        self.runs().iter().any(|call| call.matches(command, args))
    }

    fn ran_with_cwd(&self, command: &str, args: &[&str], cwd: &str) -> bool {
        self.runs()
            .iter()
            .any(|call| call.matches(command, args) && call.cwd.as_deref() == Some(cwd))
    }

    fn enter(&self, command: &str) {
        let mut active = self.active.lock().unwrap();
        let entry = active.entry(command.to_owned()).or_insert(0);
        *entry += 1;
        let current = *entry;
        drop(active);
        let mut peak = self.peak.lock().unwrap();
        let slot = peak.entry(command.to_owned()).or_insert(0);
        *slot = (*slot).max(current);
    }

    fn leave(&self, command: &str) {
        let mut active = self.active.lock().unwrap();
        if let Some(entry) = active.get_mut(command) {
            *entry -= 1;
        }
    }
}

impl CommandRunner for FakeRunner {
    fn run(
        &self,
        command: String,
        args: Vec<String>,
        options: CommandOptions,
    ) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move {
            self.runs.lock().unwrap().push(RecordedCall {
                command: command.clone(),
                args: args.clone(),
                cwd: options.cwd.clone(),
            });
            let delay = self
                .run_delay_ms
                .lock()
                .unwrap()
                .get(&command)
                .copied()
                .unwrap_or(0);
            if delay > 0 {
                self.enter(&command);
                tokio::time::sleep(Duration::from_millis(delay)).await;
                self.leave(&command);
            }
            let handler = self.run_handler.lock().unwrap().clone();
            match handler {
                Some(handler) => handler(&command, &args, &options),
                None => Ok(()),
            }
        })
    }

    fn run_capture(
        &self,
        command: String,
        args: Vec<String>,
        options: CommandOptions,
    ) -> BoxFuture<'_, Result<String, String>> {
        Box::pin(async move {
            self.captures.lock().unwrap().push(RecordedCall {
                command: command.clone(),
                args: args.clone(),
                cwd: options.cwd.clone(),
            });
            let delay = self.capture_delay_ms.load(Ordering::SeqCst);
            if delay > 0 {
                self.enter(&command);
                tokio::time::sleep(Duration::from_millis(delay as u64)).await;
                self.leave(&command);
            }
            let handler = self.capture_handler.lock().unwrap().clone();
            match handler {
                Some(handler) => handler(&command, &args, &options),
                None => Ok(String::new()),
            }
        })
    }

    fn run_sync(&self, command: &str, args: &[String]) -> Result<String, String> {
        self.syncs.lock().unwrap().push(RecordedCall {
            command: command.to_owned(),
            args: args.to_vec(),
            cwd: None,
        });
        let handler = self.sync_handler.lock().unwrap().clone();
        match handler {
            Some(handler) => handler(command, args),
            None => Err("legacy lookup unavailable".to_owned()),
        }
    }
}

struct Fixture {
    _guard: MutexGuard<'static, ()>,
    _temp: tempfile::TempDir,
    previous_home: Option<String>,
    previous_offline: Option<String>,
    temp_dir: PathBuf,
    agent_dir: PathBuf,
    settings: Arc<SettingsManager>,
    runner: Arc<FakeRunner>,
    manager: DefaultPackageManager,
}

impl Fixture {
    fn new() -> Self {
        Self::with_settings(&Settings::default())
    }

    fn with_settings(settings: &Settings) -> Self {
        let guard = env_lock().lock().unwrap_or_else(|error| error.into_inner());
        let previous_home = std::env::var("HOME").ok();
        let previous_offline = std::env::var("NOTAGENT_OFFLINE").ok();
        let temp = tempfile::tempdir().expect("tempdir");
        let temp_dir = temp.path().to_path_buf();
        let agent_dir = temp_dir.join("agent");
        std::fs::create_dir_all(&agent_dir).expect("agent dir");
        // The suite must not see the developer's own ~/.agents/skills.
        unsafe {
            std::env::set_var("HOME", &temp_dir);
            std::env::remove_var("NOTAGENT_OFFLINE");
        }

        let settings_manager = Arc::new(SettingsManager::in_memory(
            settings,
            SettingsManagerCreateOptions::default(),
        ));
        let runner = FakeRunner::new();
        let manager = DefaultPackageManager::new(PackageManagerOptions {
            cwd: temp_dir.to_string_lossy().into_owned(),
            agent_dir: agent_dir.to_string_lossy().into_owned(),
            settings_manager: settings_manager.clone(),
            command_runner: Some(runner.clone()),
        });

        Self {
            _guard: guard,
            _temp: temp,
            previous_home,
            previous_offline,
            temp_dir,
            agent_dir,
            settings: settings_manager,
            runner,
            manager,
        }
    }

    /// when it swaps the settings manager).
    fn manager_for(&self, cwd: &Path, settings: &Arc<SettingsManager>) -> DefaultPackageManager {
        DefaultPackageManager::new(PackageManagerOptions {
            cwd: cwd.to_string_lossy().into_owned(),
            agent_dir: self.agent_dir.to_string_lossy().into_owned(),
            settings_manager: settings.clone(),
            command_runner: Some(self.runner.clone()),
        })
    }

    /// leaves `HOME` alone rely on the developer's home having no
    /// `~/.agents/skills`, which this makes true by construction.
    fn isolate_home(&self) {
        let empty_home = self.temp_dir.join("empty-home");
        self.mkdir(&empty_home);
        unsafe { std::env::set_var("HOME", &empty_home) };
    }

    fn set_offline(&self) {
        unsafe { std::env::set_var("NOTAGENT_OFFLINE", "1") };
    }

    fn write(&self, path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create dir");
        }
        std::fs::write(path, content).expect("write");
    }

    fn mkdir(&self, path: &Path) {
        std::fs::create_dir_all(path).expect("create dir");
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        unsafe {
            match self.previous_home.as_ref() {
                Some(home) => std::env::set_var("HOME", home),
                None => std::env::remove_var("HOME"),
            }
            match self.previous_offline.as_ref() {
                Some(offline) => std::env::set_var("NOTAGENT_OFFLINE", offline),
                None => std::env::remove_var("NOTAGENT_OFFLINE"),
            }
        }
    }
}

fn source(value: &str) -> PackageSource {
    PackageSource::Source(value.to_owned())
}

fn filtered(filter: PackageSourceFilter) -> PackageSource {
    PackageSource::Filtered(filter)
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn is_enabled(resources: &[ResolvedResource], suffix: &str) -> bool {
    resources
        .iter()
        .any(|resource| resource.path.ends_with(suffix) && resource.enabled)
}

fn is_disabled(resources: &[ResolvedResource], suffix: &str) -> bool {
    resources
        .iter()
        .any(|resource| resource.path.ends_with(suffix) && !resource.enabled)
}

fn contains_enabled(resources: &[ResolvedResource], fragment: &str) -> bool {
    resources
        .iter()
        .any(|resource| resource.path.contains(fragment) && resource.enabled)
}

fn contains_disabled(resources: &[ResolvedResource], fragment: &str) -> bool {
    resources
        .iter()
        .any(|resource| resource.path.contains(fragment) && !resource.enabled)
}

fn has_path(resources: &[ResolvedResource], path: &Path) -> bool {
    resources
        .iter()
        .any(|resource| resource.path == path_string(path))
}

fn find_path<'a>(resources: &'a [ResolvedResource], path: &Path) -> Option<&'a ResolvedResource> {
    resources
        .iter()
        .find(|resource| resource.path == path_string(path))
}

async fn resolve(fixture: &Fixture) -> ResolvedResources {
    fixture.manager.resolve(None).await.expect("resolve")
}

const SKILL_BODY: &str = "---\nname: skill\ndescription: A skill\n---\nContent";

// ============================================================================
// resolve
// ============================================================================

#[tokio::test]
async fn returns_no_package_sourced_paths_when_no_sources_are_configured() {
    let fixture = Fixture::new();
    let result = resolve(&fixture).await;
    assert!(result.prompts.is_empty());
    assert!(result.themes.is_empty());
    assert!(result.skills.iter().all(|resource| {
        resource.metadata.source == "auto"
            && resource.metadata.origin == notagent::core::source_info::SourceOrigin::TopLevel
    }));
}

#[tokio::test]
async fn resolves_local_prompt_paths_from_settings() {
    let fixture = Fixture::new();
    let prompt_path = fixture.agent_dir.join("prompts").join("my-prompt.md");
    fixture.write(&prompt_path, "Prompt");
    fixture
        .settings
        .set_prompt_template_paths(&strings(&["prompts/my-prompt.md"]));

    let result = resolve(&fixture).await;
    assert!(is_enabled(&result.prompts, &path_string(&prompt_path)));
}

#[tokio::test]
async fn resolves_skill_paths_from_settings() {
    let fixture = Fixture::new();
    let skill_file = fixture
        .agent_dir
        .join("skills")
        .join("my-skill")
        .join("SKILL.md");
    fixture.write(&skill_file, SKILL_BODY);
    fixture.settings.set_skill_paths(&strings(&["skills"]));

    let result = resolve(&fixture).await;
    // Skills with SKILL.md are returned as file paths
    assert!(is_enabled(&result.skills, &path_string(&skill_file)));
}

#[tokio::test]
async fn auto_discovers_root_markdown_skills_from_notagent_skill_dirs() {
    let fixture = Fixture::new();
    let skill_file = fixture.agent_dir.join("skills").join("single-file.md");
    fixture.write(&skill_file, SKILL_BODY);

    let result = resolve(&fixture).await;
    assert!(is_enabled(&result.skills, &path_string(&skill_file)));
}

#[tokio::test]
async fn resolves_project_paths_relative_to_the_notagent_dir() {
    let fixture = Fixture::new();
    let prompt_path = fixture
        .temp_dir
        .join(".notagent")
        .join("prompts")
        .join("project.md");
    fixture.write(&prompt_path, "Project prompt");
    fixture
        .settings
        .set_project_prompt_template_paths(&strings(&["prompts/project.md"]))
        .expect("project settings");

    let result = resolve(&fixture).await;
    assert!(is_enabled(&result.prompts, &path_string(&prompt_path)));
}

#[tokio::test]
async fn auto_discovers_user_prompts_with_overrides() {
    let fixture = Fixture::new();
    let prompt_path = fixture.agent_dir.join("prompts").join("auto.md");
    fixture.write(&prompt_path, "Auto prompt");
    fixture
        .settings
        .set_prompt_template_paths(&strings(&["!prompts/auto.md"]));

    let result = resolve(&fixture).await;
    assert!(is_disabled(&result.prompts, &path_string(&prompt_path)));
}

#[cfg(unix)]
#[tokio::test]
async fn resolves_symlinked_user_and_project_resources_once() {
    let fixture = Fixture::new();
    let shared_dir = fixture.temp_dir.join("shared-resources");
    let shared_skills = shared_dir.join("skills");
    let shared_prompts = shared_dir.join("prompts");
    let shared_themes = shared_dir.join("themes");
    fixture.write(
        &shared_skills.join("shared-skill").join("SKILL.md"),
        SKILL_BODY,
    );
    fixture.write(&shared_prompts.join("shared.md"), "Shared prompt");
    fixture.write(
        &shared_themes.join("shared.json"),
        r#"{"name":"shared-theme"}"#,
    );

    fixture.mkdir(&fixture.temp_dir.join(".notagent"));
    for (target, link) in [
        (&shared_skills, fixture.agent_dir.join("skills")),
        (&shared_prompts, fixture.agent_dir.join("prompts")),
        (&shared_themes, fixture.agent_dir.join("themes")),
        (
            &shared_skills,
            fixture.temp_dir.join(".notagent").join("skills"),
        ),
        (
            &shared_prompts,
            fixture.temp_dir.join(".notagent").join("prompts"),
        ),
        (
            &shared_themes,
            fixture.temp_dir.join(".notagent").join("themes"),
        ),
    ] {
        std::os::unix::fs::symlink(target, link).expect("symlink");
    }

    let result = resolve(&fixture).await;
    assert_eq!(
        (
            result.skills.len(),
            result.prompts.len(),
            result.themes.len()
        ),
        (1, 1, 1)
    );
    // Project auto-discovered has higher precedence than user auto-discovered,
    // so the surviving entry should be scoped to project.
    assert_eq!(result.skills[0].metadata.scope, SourceScope::Project);
    assert_eq!(result.prompts[0].metadata.scope, SourceScope::Project);
    assert_eq!(result.themes[0].metadata.scope, SourceScope::Project);
}

#[tokio::test]
async fn auto_discovers_project_prompts_with_overrides() {
    let fixture = Fixture::new();
    let prompt_path = fixture
        .temp_dir
        .join(".notagent")
        .join("prompts")
        .join("is.md");
    fixture.write(&prompt_path, "Is prompt");
    fixture
        .settings
        .set_project_prompt_template_paths(&strings(&["!prompts/is.md"]))
        .expect("project settings");

    let result = resolve(&fixture).await;
    assert!(is_disabled(&result.prompts, &path_string(&prompt_path)));
}

// ============================================================================
// auto-discovered skill metadata
// ============================================================================

#[tokio::test]
async fn uses_the_agent_dir_as_base_dir_for_user_notagent_skills() {
    let fixture = Fixture::new();
    let skill_path = fixture
        .agent_dir
        .join("skills")
        .join("user-notagent")
        .join("SKILL.md");
    fixture.write(&skill_path, SKILL_BODY);

    let result = resolve(&fixture).await;
    let skill = find_path(&result.skills, &skill_path).expect("skill");
    assert_eq!(skill.metadata.source, "auto");
    assert_eq!(skill.metadata.scope, SourceScope::User);
    assert_eq!(
        skill.metadata.base_dir.as_deref(),
        Some(path_string(&fixture.agent_dir).as_str())
    );
}

#[tokio::test]
async fn uses_the_project_notagent_dir_as_base_dir_for_project_skills() {
    let fixture = Fixture::new();
    let project_base_dir = fixture.temp_dir.join(".notagent");
    let skill_path = project_base_dir
        .join("skills")
        .join("project-notagent")
        .join("SKILL.md");
    fixture.write(&skill_path, SKILL_BODY);

    let result = resolve(&fixture).await;
    let skill = find_path(&result.skills, &skill_path).expect("skill");
    assert_eq!(skill.metadata.source, "auto");
    assert_eq!(skill.metadata.scope, SourceScope::Project);
    assert_eq!(
        skill.metadata.base_dir.as_deref(),
        Some(path_string(&project_base_dir).as_str())
    );
}

#[tokio::test]
async fn uses_the_home_agents_dir_as_base_dir_for_user_agents_skills() {
    let fixture = Fixture::new();
    let agents_base_dir = fixture.temp_dir.join(".agents");
    let skill_path = agents_base_dir
        .join("skills")
        .join("user-agents")
        .join("SKILL.md");
    fixture.write(&skill_path, SKILL_BODY);

    let result = resolve(&fixture).await;
    let skill = find_path(&result.skills, &skill_path).expect("skill");
    assert_eq!(skill.metadata.source, "auto");
    assert_eq!(skill.metadata.scope, SourceScope::User);
    assert_eq!(
        skill.metadata.base_dir.as_deref(),
        Some(path_string(&agents_base_dir).as_str())
    );
}

#[tokio::test]
async fn uses_each_project_agents_dir_as_its_own_base_dir() {
    let fixture = Fixture::new();
    let repo_root = fixture.temp_dir.join("repo");
    let nested_cwd = repo_root.join("packages").join("feature");
    fixture.mkdir(&nested_cwd);
    fixture.mkdir(&repo_root.join(".git"));

    let repo_agents_base_dir = repo_root.join(".agents");
    let repo_skill = repo_agents_base_dir
        .join("skills")
        .join("repo")
        .join("SKILL.md");
    fixture.write(&repo_skill, SKILL_BODY);

    let package_agents_base_dir = repo_root.join("packages").join(".agents");
    let package_skill = package_agents_base_dir
        .join("skills")
        .join("package")
        .join("SKILL.md");
    fixture.write(&package_skill, SKILL_BODY);

    let manager = fixture.manager_for(&nested_cwd, &fixture.settings);
    let result = manager.resolve(None).await.expect("resolve");

    let resolved_repo_skill = find_path(&result.skills, &repo_skill).expect("repo skill");
    let resolved_package_skill = find_path(&result.skills, &package_skill).expect("package skill");
    assert_eq!(resolved_repo_skill.metadata.source, "auto");
    assert_eq!(resolved_repo_skill.metadata.scope, SourceScope::Project);
    assert_eq!(
        resolved_repo_skill.metadata.base_dir.as_deref(),
        Some(path_string(&repo_agents_base_dir).as_str())
    );
    assert_eq!(resolved_package_skill.metadata.source, "auto");
    assert_eq!(resolved_package_skill.metadata.scope, SourceScope::Project);
    assert_eq!(
        resolved_package_skill.metadata.base_dir.as_deref(),
        Some(path_string(&package_agents_base_dir).as_str())
    );
}

// ============================================================================
// .agents/skills auto-discovery
// ============================================================================

#[tokio::test]
async fn scans_agents_skills_from_cwd_up_to_the_git_repo_root() {
    let fixture = Fixture::new();
    fixture.isolate_home();
    let repo_root = fixture.temp_dir.join("repo");
    let nested_cwd = repo_root.join("packages").join("feature");
    fixture.mkdir(&nested_cwd);
    fixture.mkdir(&repo_root.join(".git"));

    let above_repo_skill = fixture
        .temp_dir
        .join(".agents")
        .join("skills")
        .join("above-repo")
        .join("SKILL.md");
    fixture.write(&above_repo_skill, SKILL_BODY);
    let repo_root_skill = repo_root
        .join(".agents")
        .join("skills")
        .join("repo-root")
        .join("SKILL.md");
    fixture.write(&repo_root_skill, SKILL_BODY);
    let nested_skill = repo_root
        .join("packages")
        .join(".agents")
        .join("skills")
        .join("nested")
        .join("SKILL.md");
    fixture.write(&nested_skill, SKILL_BODY);

    let manager = fixture.manager_for(&nested_cwd, &fixture.settings);
    let result = manager.resolve(None).await.expect("resolve");
    assert!(is_enabled(&result.skills, &path_string(&repo_root_skill)));
    assert!(is_enabled(&result.skills, &path_string(&nested_skill)));
    assert!(!has_path(&result.skills, &above_repo_skill));
}

#[tokio::test]
async fn scans_agents_skills_up_to_the_filesystem_root_outside_a_repo() {
    let fixture = Fixture::new();
    let non_repo_root = fixture.temp_dir.join("non-repo");
    let nested_cwd = non_repo_root.join("a").join("b");
    fixture.mkdir(&nested_cwd);

    let root_skill = non_repo_root
        .join(".agents")
        .join("skills")
        .join("root")
        .join("SKILL.md");
    fixture.write(&root_skill, SKILL_BODY);
    let middle_skill = non_repo_root
        .join("a")
        .join(".agents")
        .join("skills")
        .join("middle")
        .join("SKILL.md");
    fixture.write(&middle_skill, SKILL_BODY);

    let manager = fixture.manager_for(&nested_cwd, &fixture.settings);
    let result = manager.resolve(None).await.expect("resolve");
    assert!(is_enabled(&result.skills, &path_string(&root_skill)));
    assert!(is_enabled(&result.skills, &path_string(&middle_skill)));
}

#[tokio::test]
async fn ignores_root_markdown_files_in_agents_skills() {
    let fixture = Fixture::new();
    let agents_skills_dir = fixture.temp_dir.join(".agents").join("skills");
    let root_skill = agents_skills_dir.join("root-file.md");
    let nested_skill = agents_skills_dir.join("nested-skill").join("SKILL.md");
    fixture.write(&root_skill, SKILL_BODY);
    fixture.write(&nested_skill, SKILL_BODY);

    let work_dir = fixture.temp_dir.join("work");
    fixture.mkdir(&work_dir);
    let manager = fixture.manager_for(&work_dir, &fixture.settings);
    let result = manager.resolve(None).await.expect("resolve");
    assert!(!has_path(&result.skills, &root_skill));
    assert!(is_enabled(&result.skills, &path_string(&nested_skill)));
}

#[tokio::test]
async fn keeps_home_agents_skills_user_scoped_below_home_outside_a_repo() {
    let fixture = Fixture::new();
    let cwd = fixture.temp_dir.join("scratch").join("nested");
    fixture.mkdir(&cwd);
    let home_skill = fixture
        .temp_dir
        .join(".agents")
        .join("skills")
        .join("home-skill")
        .join("SKILL.md");
    fixture.write(&home_skill, SKILL_BODY);

    let local_agent_dir = fixture.temp_dir.join(".notagent").join("agent");
    fixture.mkdir(&local_agent_dir);
    let settings = Arc::new(SettingsManager::in_memory(
        &Settings::default(),
        SettingsManagerCreateOptions::default(),
    ));
    let manager = DefaultPackageManager::new(PackageManagerOptions {
        cwd: path_string(&cwd),
        agent_dir: path_string(&local_agent_dir),
        settings_manager: settings,
        command_runner: Some(fixture.runner.clone()),
    });

    let result = manager.resolve(None).await.expect("resolve");
    let matching: Vec<&ResolvedResource> = result
        .skills
        .iter()
        .filter(|resource| resource.path == path_string(&home_skill))
        .collect();
    assert_eq!(matching.len(), 1);
    assert!(matching[0].enabled);
    assert_eq!(matching[0].metadata.scope, SourceScope::User);
    assert_eq!(matching[0].metadata.source, "auto");
}

#[cfg(unix)]
#[tokio::test]
async fn dedupes_user_skills_when_the_agent_skills_dir_is_a_symlink() {
    let fixture = Fixture::new();
    let agents_skills_dir = fixture.temp_dir.join(".agents").join("skills");
    fixture.mkdir(&agents_skills_dir);
    std::os::unix::fs::symlink(&agents_skills_dir, fixture.agent_dir.join("skills"))
        .expect("symlink");

    let skill_path = agents_skills_dir.join("foo").join("SKILL.md");
    fixture.write(&skill_path, SKILL_BODY);

    let result = resolve(&fixture).await;
    let foo_skills: Vec<&ResolvedResource> = result
        .skills
        .iter()
        .filter(|resource| resource.path.ends_with("foo/SKILL.md"))
        .collect();
    assert_eq!(foo_skills.len(), 1);
}

// ============================================================================
// ignore files
// ============================================================================

#[tokio::test]
async fn respects_gitignore_in_skill_directories() {
    let fixture = Fixture::new();
    let skills_dir = fixture.agent_dir.join("skills");
    fixture.write(&skills_dir.join(".gitignore"), "venv\n__pycache__\n");
    fixture.write(&skills_dir.join("good-skill").join("SKILL.md"), SKILL_BODY);
    fixture.write(
        &skills_dir.join("venv").join("bad-skill").join("SKILL.md"),
        SKILL_BODY,
    );
    fixture.settings.set_skill_paths(&strings(&["skills"]));

    let result = resolve(&fixture).await;
    assert!(contains_enabled(&result.skills, "good-skill"));
    assert!(!contains_enabled(&result.skills, "venv"));
}

#[tokio::test]
async fn does_not_apply_a_parent_gitignore_to_notagent_auto_discovery() {
    let fixture = Fixture::new();
    fixture.write(&fixture.temp_dir.join(".gitignore"), ".notagent\n");
    let skill_path = fixture
        .temp_dir
        .join(".notagent")
        .join("skills")
        .join("auto-skill")
        .join("SKILL.md");
    fixture.write(&skill_path, SKILL_BODY);

    let result = resolve(&fixture).await;
    assert!(is_enabled(&result.skills, &path_string(&skill_path)));
}

// ============================================================================
// package resources
//
// which is gone with the extension system (class 2). The same collection runs
// for a local package in the settings, so the cases move there.
// ============================================================================

#[tokio::test]
async fn handles_directories_with_a_notagent_manifest() {
    let fixture = Fixture::new();
    let package_dir = fixture.temp_dir.join("my-package");
    fixture.write(
        &package_dir.join("package.json"),
        r#"{"name":"my-package","notagent":{"skills":["./skills"]}}"#,
    );
    let skill = package_dir.join("skills").join("my-skill").join("SKILL.md");
    fixture.write(&skill, SKILL_BODY);

    fixture
        .settings
        .set_packages(&[source(&path_string(&package_dir))]);
    let result = resolve(&fixture).await;
    // Skills with SKILL.md are returned as file paths
    assert!(is_enabled(&result.skills, &path_string(&skill)));
}

#[tokio::test]
async fn keeps_manifest_entries_with_a_leading_tilde_package_relative() {
    let fixture = Fixture::new();
    let package_dir = fixture.temp_dir.join("tilde-manifest-package");
    let direct_skill = package_dir
        .join("~skills")
        .join("direct-skill")
        .join("SKILL.md");
    let slash_skill = package_dir
        .join("~")
        .join("skills")
        .join("slash-skill")
        .join("SKILL.md");
    fixture.write(&direct_skill, SKILL_BODY);
    fixture.write(&slash_skill, SKILL_BODY);
    fixture.write(
        &package_dir.join("package.json"),
        r#"{"name":"tilde-manifest-package","notagent":{"skills":["~skills","~/skills"]}}"#,
    );

    fixture
        .settings
        .set_packages(&[source(&path_string(&package_dir))]);
    let result = resolve(&fixture).await;
    assert!(is_enabled(&result.skills, &path_string(&direct_skill)));
    assert!(is_enabled(&result.skills, &path_string(&slash_skill)));
}

#[tokio::test]
async fn handles_directories_with_the_auto_discovery_layout() {
    let fixture = Fixture::new();
    let package_dir = fixture.temp_dir.join("auto-pkg");
    fixture.write(&package_dir.join("themes").join("dark.json"), "{}");
    fixture.write(&package_dir.join("prompts").join("main.md"), "Prompt");

    fixture
        .settings
        .set_packages(&[source(&path_string(&package_dir))]);
    let result = resolve(&fixture).await;
    assert!(is_enabled(&result.themes, "dark.json"));
    assert!(is_enabled(&result.prompts, "main.md"));
}

#[tokio::test]
async fn stops_recursing_when_a_package_skill_directory_contains_a_skill_file() {
    let fixture = Fixture::new();
    let package_dir = fixture.temp_dir.join("skill-root-pkg");
    let root_skill = package_dir
        .join("skills")
        .join("root-skill")
        .join("SKILL.md");
    let nested_skill = package_dir
        .join("skills")
        .join("root-skill")
        .join("nested-skill")
        .join("SKILL.md");
    fixture.write(&root_skill, SKILL_BODY);
    fixture.write(&nested_skill, SKILL_BODY);

    fixture
        .settings
        .set_packages(&[source(&path_string(&package_dir))]);
    let result = resolve(&fixture).await;
    assert!(is_enabled(&result.skills, &path_string(&root_skill)));
    assert!(!has_path(&result.skills, &nested_skill));
}

// ============================================================================
// progress callback
// ============================================================================

#[tokio::test]
async fn emits_no_progress_events_for_a_local_package() {
    let fixture = Fixture::new();
    let events: Arc<Mutex<Vec<ProgressEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    fixture
        .manager
        .set_progress_callback(Some(Arc::new(move |event: &ProgressEvent| {
            sink.lock().unwrap().push(event.clone());
        })));

    let package_dir = fixture.temp_dir.join("local-pkg");
    fixture.write(&package_dir.join("prompts").join("main.md"), "Prompt");
    fixture
        .settings
        .set_packages(&[source(&path_string(&package_dir))]);

    // Local paths don't trigger install progress
    resolve(&fixture).await;
    assert!(events.lock().unwrap().is_empty());
}

// ============================================================================
// command spawning
// ============================================================================

#[cfg(unix)]
#[test]
fn preserves_argv_entries_containing_spaces() {
    use notagent::core::package_manager::command_runner::ProcessCommandRunner;
    let value_with_space = "C:\\Users\\A B\\.notagent\\npm";
    let output = ProcessCommandRunner
        .run_sync("/bin/echo", &[value_with_space.to_owned()])
        .expect("echo");
    assert_eq!(output, value_with_space);
}

// ============================================================================
// npmCommand
// ============================================================================

fn npm_command_settings(command: &[&str]) -> Settings {
    Settings {
        npm_command: Some(strings(command)),
        ..Settings::default()
    }
}

#[tokio::test]
async fn uses_the_npm_command_argv_for_npm_installs() {
    let fixture = Fixture::with_settings(&npm_command_settings(&[
        "mise", "exec", "node@20", "--", "npm",
    ]));

    fixture
        .manager
        .install("npm:@scope/pkg", false)
        .await
        .expect("install");

    let npm_root = path_string(&fixture.agent_dir.join("npm"));
    assert!(fixture.runner.ran(
        "mise",
        &[
            "exec",
            "node@20",
            "--",
            "npm",
            "install",
            "@scope/pkg",
            "--prefix",
            &npm_root,
            "--legacy-peer-deps",
        ]
    ));
}

#[tokio::test]
async fn passes_legacy_peer_deps_when_uninstalling_npm_packages() {
    let fixture = Fixture::new();
    fixture.mkdir(&fixture.agent_dir.join("npm"));

    fixture
        .manager
        .remove("npm:@scope/pkg", false)
        .await
        .expect("remove");

    let npm_root = path_string(&fixture.agent_dir.join("npm"));
    assert!(fixture.runner.ran(
        "npm",
        &[
            "uninstall",
            "@scope/pkg",
            "--prefix",
            &npm_root,
            "--legacy-peer-deps"
        ]
    ));
}

#[tokio::test]
async fn uses_bun_cwd_for_npm_package_installs() {
    let fixture = Fixture::with_settings(&npm_command_settings(&[
        "mise", "exec", "bun@1", "--", "bun",
    ]));

    fixture
        .manager
        .install("npm:@scope/pkg", false)
        .await
        .expect("install");

    let npm_root = path_string(&fixture.agent_dir.join("npm"));
    assert!(fixture.runner.ran(
        "mise",
        &[
            "exec",
            "bun@1",
            "--",
            "bun",
            "install",
            "@scope/pkg",
            "--cwd",
            &npm_root,
            "--omit=peer",
        ]
    ));
}

#[tokio::test]
async fn installs_git_package_dependencies_with_omit_dev() {
    let fixture = Fixture::new();
    let target_dir = fixture
        .agent_dir
        .join("git")
        .join("github.com")
        .join("user")
        .join("repo");
    let clone_target = target_dir.clone();
    fixture.runner.on_run(move |command, args, _options| {
        if command == "git" && args.first().is_some_and(|arg| arg == "clone") {
            std::fs::create_dir_all(&clone_target).expect("target dir");
            std::fs::write(
                clone_target.join("package.json"),
                r#"{"name":"repo","version":"1.0.0"}"#,
            )
            .expect("package.json");
        }
        Ok(())
    });

    fixture
        .manager
        .install("git:github.com/user/repo", false)
        .await
        .expect("install");

    assert!(fixture.runner.ran_with_cwd(
        "npm",
        &["install", "--omit=dev"],
        &path_string(&target_dir)
    ));
}

#[tokio::test]
async fn removes_a_newly_created_checkout_when_git_clone_fails() {
    let fixture = Fixture::new();
    let target_dir = fixture
        .agent_dir
        .join("git")
        .join("github.com")
        .join("user")
        .join("repo");
    let clone_target = target_dir.clone();
    fixture.runner.on_run(move |command, args, _options| {
        if command == "git" && args.first().is_some_and(|arg| arg == "clone") {
            std::fs::create_dir_all(&clone_target).expect("target dir");
            return Err("simulated git clone failure".to_owned());
        }
        Ok(())
    });

    let error = fixture
        .manager
        .install("git:github.com/user/repo", false)
        .await
        .expect_err("clone failure");
    assert_eq!(error.0, "simulated git clone failure");
    assert!(!target_dir.exists());
}

#[tokio::test]
async fn removes_a_newly_cloned_checkout_when_dependency_installation_fails() {
    let fixture = Fixture::new();
    let target_dir = fixture
        .agent_dir
        .join("git")
        .join("github.com")
        .join("user")
        .join("repo");
    let clone_target = target_dir.clone();
    fixture.runner.on_run(move |command, args, _options| {
        if command == "git" && args.first().is_some_and(|arg| arg == "clone") {
            std::fs::create_dir_all(&clone_target).expect("target dir");
            std::fs::write(
                clone_target.join("package.json"),
                r#"{"name":"repo","version":"1.0.0"}"#,
            )
            .expect("package.json");
            return Ok(());
        }
        if command == "npm" {
            return Err("simulated dependency install failure".to_owned());
        }
        Ok(())
    });

    let error = fixture
        .manager
        .install("git:github.com/user/repo", false)
        .await
        .expect_err("dependency failure");
    assert_eq!(error.0, "simulated dependency install failure");
    assert!(!target_dir.exists());
}

#[tokio::test]
async fn reconciles_an_existing_git_checkout_to_a_pinned_ref_during_install() {
    let fixture = Fixture::new();
    let target_dir = fixture
        .agent_dir
        .join("git")
        .join("github.com")
        .join("user")
        .join("repo");
    fixture.write(
        &target_dir.join("package.json"),
        r#"{"name":"repo","version":"1.0.0"}"#,
    );
    fixture.runner.on_capture(|_command, args, _options| {
        if args[0] == "rev-parse" && args[1] == "HEAD" {
            return Ok("old-head".to_owned());
        }
        if args[0] == "rev-parse" && args[1] == "FETCH_HEAD^{commit}" {
            return Ok("new-head".to_owned());
        }
        Err(format!("Unexpected capture args: {}", args.join(" ")))
    });

    fixture
        .manager
        .install("git:github.com/user/repo@v2", false)
        .await
        .expect("install");

    let cwd = path_string(&target_dir);
    assert!(
        fixture
            .runner
            .ran_with_cwd("git", &["fetch", "origin", "v2"], &cwd)
    );
    assert!(
        fixture
            .runner
            .ran_with_cwd("git", &["reset", "--hard", "FETCH_HEAD^{commit}"], &cwd)
    );
    assert!(fixture.runner.ran_with_cwd("git", &["clean", "-fdx"], &cwd));
    assert!(
        fixture
            .runner
            .ran_with_cwd("npm", &["install", "--omit=dev"], &cwd)
    );
}

#[tokio::test]
async fn reconciles_an_existing_git_checkout_to_its_update_target_without_a_ref() {
    let fixture = Fixture::new();
    let target_dir = fixture
        .agent_dir
        .join("git")
        .join("github.com")
        .join("user")
        .join("repo");
    fixture.mkdir(&target_dir);
    fixture.runner.on_capture(|_command, args, _options| {
        if args[0] == "rev-parse" && args[1] == "--abbrev-ref" {
            return Ok("origin/main".to_owned());
        }
        if args[0] == "rev-parse" && args[1] == "HEAD" {
            return Ok("old-head".to_owned());
        }
        if args[0] == "rev-parse" && args[1] == "@{upstream}" {
            return Ok("new-head".to_owned());
        }
        if args[0] == "rev-parse" && args[1] == "@{upstream}^{commit}" {
            return Ok("new-head".to_owned());
        }
        Err(format!("Unexpected capture args: {}", args.join(" ")))
    });

    fixture
        .manager
        .install("git:github.com/user/repo", false)
        .await
        .expect("install");

    let cwd = path_string(&target_dir);
    assert!(fixture.runner.ran_with_cwd(
        "git",
        &[
            "fetch",
            "--prune",
            "--no-tags",
            "origin",
            "+refs/heads/main:refs/remotes/origin/main",
        ],
        &cwd
    ));
    assert!(
        fixture
            .runner
            .ran_with_cwd("git", &["reset", "--hard", "@{upstream}^{commit}"], &cwd)
    );
    assert!(fixture.runner.ran_with_cwd("git", &["clean", "-fdx"], &cwd));
}

#[tokio::test]
async fn uses_a_plain_install_for_git_dependencies_when_npm_command_is_configured() {
    let fixture = Fixture::with_settings(&npm_command_settings(&["pnpm"]));
    let target_dir = fixture
        .agent_dir
        .join("git")
        .join("github.com")
        .join("user")
        .join("repo");
    let clone_target = target_dir.clone();
    fixture.runner.on_run(move |command, args, _options| {
        if command == "git" && args.first().is_some_and(|arg| arg == "clone") {
            std::fs::create_dir_all(&clone_target).expect("target dir");
            std::fs::write(
                clone_target.join("package.json"),
                r#"{"name":"repo","version":"1.0.0"}"#,
            )
            .expect("package.json");
        }
        Ok(())
    });

    fixture
        .manager
        .install("git:github.com/user/repo", false)
        .await
        .expect("install");

    assert!(
        fixture
            .runner
            .ran_with_cwd("pnpm", &["install"], &path_string(&target_dir))
    );
}

#[tokio::test]
async fn updates_git_package_dependencies_with_omit_dev() {
    let fixture = Fixture::new();
    let target_dir = fixture
        .temp_dir
        .join(".notagent")
        .join("git")
        .join("github.com")
        .join("user")
        .join("repo");
    fixture.write(
        &target_dir.join("package.json"),
        r#"{"name":"repo","version":"1.0.0"}"#,
    );
    fixture
        .settings
        .set_project_packages(&[source("git:github.com/user/repo")])
        .expect("project settings");
    fixture.runner.on_capture(|_command, args, _options| {
        if args[0] == "rev-parse" && args[1] == "--abbrev-ref" && args[2] == "@{upstream}" {
            return Ok("origin/main".to_owned());
        }
        if args[0] == "rev-parse" && (args[1] == "@{upstream}" || args[1] == "@{upstream}^{commit}")
        {
            return Ok("remote-head".to_owned());
        }
        if args[0] == "rev-parse" && args[1] == "HEAD" {
            return Ok("local-head".to_owned());
        }
        Err(format!("Unexpected capture args: {}", args.join(" ")))
    });

    fixture
        .manager
        .update(Some("git:github.com/user/repo"))
        .await
        .expect("update");

    assert!(fixture.runner.ran_with_cwd(
        "npm",
        &["install", "--omit=dev"],
        &path_string(&target_dir)
    ));
}

#[tokio::test]
async fn repairs_missing_git_dependencies_when_the_checkout_is_current() {
    let fixture = Fixture::new();
    let target_dir = fixture
        .agent_dir
        .join("git")
        .join("github.com")
        .join("user")
        .join("repo");
    fixture.write(
        &target_dir.join("package.json"),
        r#"{"name":"repo","version":"1.0.0","dependencies":{"dependency":"1.0.0"}}"#,
    );
    fixture
        .settings
        .set_packages(&[source("git:github.com/user/repo")]);
    fixture.runner.on_capture(|_command, args, _options| {
        if args[0] == "rev-parse" && args[1] == "--abbrev-ref" {
            return Ok("origin/main".to_owned());
        }
        Ok("current-head".to_owned())
    });

    fixture
        .manager
        .update(Some("git:github.com/user/repo"))
        .await
        .expect("update");

    let cwd = path_string(&target_dir);
    assert!(
        fixture
            .runner
            .ran_with_cwd("npm", &["install", "--omit=dev"], &cwd)
    );
    assert!(!fixture.runner.ran_with_cwd("git", &["clean", "-fdx"], &cwd));
}

#[tokio::test]
async fn repairs_deleted_git_dependencies_when_cleaning_fails() {
    let fixture = Fixture::new();
    let target_dir = fixture
        .agent_dir
        .join("git")
        .join("github.com")
        .join("user")
        .join("repo");
    fixture.write(
        &target_dir.join("package.json"),
        r#"{"name":"repo","version":"1.0.0","dependencies":{"dependency":"1.0.0"}}"#,
    );
    fixture
        .settings
        .set_packages(&[source("git:github.com/user/repo")]);
    fixture.runner.on_capture(|_command, args, _options| {
        if args[0] == "rev-parse" && args[1] == "--abbrev-ref" {
            return Ok("origin/main".to_owned());
        }
        if args[1] == "HEAD" {
            return Ok("old-head".to_owned());
        }
        Ok("new-head".to_owned())
    });
    fixture.runner.on_run(|_command, args, _options| {
        if args[0] == "clean" {
            return Err("simulated clean failure".to_owned());
        }
        Ok(())
    });

    let error = fixture
        .manager
        .update(Some("git:github.com/user/repo"))
        .await
        .expect_err("clean failure");
    assert_eq!(error.0, "simulated clean failure");
    assert!(fixture.runner.ran_with_cwd(
        "npm",
        &["install", "--omit=dev"],
        &path_string(&target_dir)
    ));
}

#[tokio::test]
async fn uses_a_plain_install_through_npm_command_argv_when_updating_git_dependencies() {
    let fixture = Fixture::with_settings(&npm_command_settings(&[
        "mise", "exec", "node@20", "--", "pnpm",
    ]));
    let target_dir = fixture
        .temp_dir
        .join(".notagent")
        .join("git")
        .join("github.com")
        .join("user")
        .join("repo");
    fixture.write(
        &target_dir.join("package.json"),
        r#"{"name":"repo","version":"1.0.0"}"#,
    );
    fixture
        .settings
        .set_project_packages(&[source("git:github.com/user/repo")])
        .expect("project settings");
    fixture.runner.on_capture(|_command, args, _options| {
        if args[0] == "rev-parse" && args[1] == "--abbrev-ref" && args[2] == "@{upstream}" {
            return Ok("origin/main".to_owned());
        }
        if args[0] == "rev-parse" && (args[1] == "@{upstream}" || args[1] == "@{upstream}^{commit}")
        {
            return Ok("remote-head".to_owned());
        }
        if args[0] == "rev-parse" && args[1] == "HEAD" {
            return Ok("local-head".to_owned());
        }
        Err(format!("Unexpected capture args: {}", args.join(" ")))
    });

    fixture
        .manager
        .update(Some("git:github.com/user/repo"))
        .await
        .expect("update");

    assert!(fixture.runner.ran_with_cwd(
        "mise",
        &["exec", "node@20", "--", "pnpm", "install"],
        &path_string(&target_dir)
    ));
}

#[tokio::test]
async fn invalidates_the_cached_global_npm_root_when_the_npm_command_changes() {
    let fixture = Fixture::with_settings(&npm_command_settings(&[
        "mise", "exec", "node@20", "--", "npm",
    ]));
    let root20 = fixture
        .temp_dir
        .join("node20")
        .join("lib")
        .join("node_modules");
    let root22 = fixture
        .temp_dir
        .join("node22")
        .join("lib")
        .join("node_modules");
    fixture.mkdir(&root20.join("@scope").join("pkg"));

    let (root20_string, root22_string) = (path_string(&root20), path_string(&root22));
    fixture.runner.on_sync(move |command, args| {
        assert_eq!(command, "mise");
        match args[1].as_str() {
            "node@20" => Ok(root20_string.clone()),
            "node@22" => Ok(root22_string.clone()),
            other => Err(format!("unexpected args {other}")),
        }
    });

    assert_eq!(
        fixture
            .manager
            .get_installed_path("npm:@scope/pkg", SourceScope::User)
            .expect("installed path"),
        Some(path_string(&root20.join("@scope").join("pkg")))
    );
    assert!(
        fixture.runner.syncs()[0].matches("mise", &["exec", "node@20", "--", "npm", "root", "-g"])
    );

    fixture
        .settings
        .set_npm_command(Some(&strings(&["mise", "exec", "node@22", "--", "npm"])));

    assert_eq!(
        fixture
            .manager
            .get_installed_path("npm:@scope/pkg", SourceScope::User)
            .expect("installed path"),
        None
    );
    assert!(
        fixture.runner.syncs()[1].matches("mise", &["exec", "node@22", "--", "npm", "root", "-g"])
    );
}

#[tokio::test]
async fn installs_user_npm_packages_into_the_managed_npm_root() {
    let mut settings = npm_command_settings(&["pnpm"]);
    settings.packages = Some(vec![source("npm:pnpm-pkg")]);
    let fixture = Fixture::with_settings(&settings);

    let package_path = fixture
        .agent_dir
        .join("npm")
        .join("node_modules")
        .join("pnpm-pkg");
    let npm_root = path_string(&fixture.agent_dir.join("npm"));
    let install_target = package_path.clone();
    fixture.runner.on_run(move |command, args, _options| {
        assert_eq!(command, "pnpm");
        assert_eq!(
            args,
            strings(&[
                "install",
                "pnpm-pkg",
                "--prefix",
                &npm_root,
                "--config.auto-install-peers=false",
                "--config.strict-peer-dependencies=false",
                "--config.strict-dep-builds=false",
            ])
        );
        std::fs::create_dir_all(install_target.join("prompts")).expect("package dir");
        std::fs::write(
            install_target.join("package.json"),
            r#"{"name":"pnpm-pkg","version":"1.0.0"}"#,
        )
        .expect("package.json");
        std::fs::write(install_target.join("prompts").join("index.md"), "Prompt").expect("prompt");
        Ok(())
    });

    let first = resolve(&fixture).await;
    let second = resolve(&fixture).await;
    let prompt_path = path_string(&package_path.join("prompts").join("index.md"));
    assert!(is_enabled(&first.prompts, &prompt_path));
    assert!(is_enabled(&second.prompts, &prompt_path));
    assert_eq!(fixture.runner.runs().len(), 1);
    assert_eq!(
        fixture
            .manager
            .get_installed_path("npm:pnpm-pkg", SourceScope::User)
            .expect("installed path"),
        Some(path_string(&package_path))
    );
}

#[tokio::test]
async fn loads_legacy_pnpm_global_package_paths_from_the_list_output() {
    let mut settings = npm_command_settings(&["pnpm"]);
    settings.packages = Some(vec![source("npm:pnpm-pkg")]);
    let fixture = Fixture::with_settings(&settings);

    let pnpm_root = fixture.temp_dir.join("pnpm").join("global").join("v11");
    let package_path = pnpm_root
        .join("20-hash")
        .join("node_modules")
        .join("pnpm-pkg");
    fixture.write(
        &package_path.join("package.json"),
        r#"{"name":"pnpm-pkg","version":"1.0.0"}"#,
    );
    fixture.write(&package_path.join("prompts").join("index.md"), "Prompt");

    let listing = format!(
        r#"[{{"path":"{}","dependencies":{{"pnpm-pkg":{{"version":"1.0.0","path":"{}"}}}}}}]"#,
        path_string(&pnpm_root),
        path_string(&package_path)
    );
    fixture.runner.on_sync(move |command, args| {
        assert_eq!(command, "pnpm");
        if args.join(" ") == "list -g --depth 0 --json" {
            return Ok(listing.clone());
        }
        Err(format!("unexpected args {}", args.join(" ")))
    });

    let result = resolve(&fixture).await;
    assert!(is_enabled(
        &result.prompts,
        &path_string(&package_path.join("prompts").join("index.md"))
    ));
    assert!(fixture.runner.runs().is_empty());
    assert_eq!(
        fixture
            .manager
            .get_installed_path("npm:pnpm-pkg", SourceScope::User)
            .expect("installed path"),
        Some(path_string(&package_path))
    );
}

#[tokio::test]
async fn resolves_wrapped_pnpm_global_package_paths_from_the_list_output() {
    let fixture = Fixture::with_settings(&npm_command_settings(&[
        "mise", "exec", "node@20", "--", "pnpm",
    ]));
    let pnpm_root = fixture.temp_dir.join("pnpm").join("global").join("v11");
    let package_path = pnpm_root
        .join("20-hash")
        .join("node_modules")
        .join("pnpm-pkg");
    fixture.mkdir(&package_path);

    let listing = format!(
        r#"[{{"path":"{}","dependencies":{{"pnpm-pkg":{{"path":"{}"}}}}}}]"#,
        path_string(&pnpm_root),
        path_string(&package_path)
    );
    fixture.runner.on_sync(move |command, args| {
        assert_eq!(command, "mise");
        if args.join(" ") == "exec node@20 -- pnpm list -g --depth 0 --json" {
            return Ok(listing.clone());
        }
        Err(format!("unexpected args {}", args.join(" ")))
    });

    assert_eq!(
        fixture
            .manager
            .get_installed_path("npm:pnpm-pkg", SourceScope::User)
            .expect("installed path"),
        Some(path_string(&package_path))
    );
}

#[tokio::test]
async fn ignores_malformed_legacy_pnpm_global_package_lists() {
    let fixture = Fixture::with_settings(&npm_command_settings(&["pnpm"]));
    fixture
        .runner
        .on_sync(|_command, _args| Ok("not json".to_owned()));

    assert_eq!(
        fixture
            .manager
            .get_installed_path("npm:pnpm-pkg", SourceScope::User)
            .expect("installed path"),
        None
    );
}

// ============================================================================
// source parsing
// ============================================================================

#[tokio::test]
async fn emits_progress_events_on_an_install_attempt() {
    let fixture = Fixture::new();
    let events: Arc<Mutex<Vec<ProgressEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    fixture
        .manager
        .set_progress_callback(Some(Arc::new(move |event: &ProgressEvent| {
            sink.lock().unwrap().push(event.clone());
        })));
    fixture
        .runner
        .on_run(|_command, _args, _options| Err("simulated npm install failure".to_owned()));

    let error = fixture
        .manager
        .install("npm:nonexistent-package@1.0.0", false)
        .await
        .expect_err("install failure");
    assert_eq!(error.0, "simulated npm install failure");

    let events = events.lock().unwrap();
    assert!(
        events
            .iter()
            .any(|event| event.kind == ProgressKind::Start
                && event.action == ProgressAction::Install)
    );
    assert!(events.iter().any(|event| event.kind == ProgressKind::Error));
}

#[tokio::test]
async fn recognizes_github_urls_without_a_git_prefix() {
    let fixture = Fixture::new();
    fixture
        .runner
        .on_run(|_command, _args, _options| Err("simulated git clone failure".to_owned()));
    let source_url = "https://github.com/nonexistent/repo";

    let error = fixture
        .manager
        .install(source_url, false)
        .await
        .expect_err("clone failure");
    assert_eq!(error.0, "simulated git clone failure");

    let runs = fixture.runner.runs();
    assert!(runs.iter().any(|call| {
        call.command == "git" && call.args[0] == "clone" && call.args[1] == source_url
    }));
}

#[test]
fn parses_package_source_types_from_the_docs_examples() {
    let fixture = Fixture::new();
    let parse_npm = |source: &str| match fixture.manager.parse_source(source) {
        ParsedSource::Npm(npm) => npm,
        other => panic!("Expected npm source: {source} ({other:?})"),
    };

    assert!(parse_npm("npm:@scope/pkg@1.2.3").pinned);
    assert!(!parse_npm("npm:@scope/pkg@^1.2.3").pinned);
    assert!(!parse_npm("npm:pkg").pinned);

    for git_source in [
        "git:github.com/user/repo@v1",
        "https://github.com/user/repo@v1",
        "git:git@github.com:user/repo@v1",
        "ssh://git@github.com/user/repo@v1",
    ] {
        assert!(matches!(
            fixture.manager.parse_source(git_source),
            ParsedSource::Git(_)
        ));
    }

    for local_source in [
        "/absolute/path/to/package",
        "./relative/path/to/package",
        "../relative/path/to/package",
    ] {
        assert!(matches!(
            fixture.manager.parse_source(local_source),
            ParsedSource::Local(_)
        ));
    }
}

#[test]
fn never_parses_dot_relative_paths_as_git() {
    let fixture = Fixture::new();
    for path in ["./packages/agent-timers", "../packages/agent-timers"] {
        match fixture.manager.parse_source(path) {
            ParsedSource::Local(parsed) => assert_eq!(parsed, path),
            other => panic!("expected a local source, got {other:?}"),
        }
    }
}

/// "HTTPS git URL parsing" block; `utils/git.rs` pins the parser itself.
#[test]
fn parses_every_supported_git_spelling() {
    let fixture = Fixture::new();
    let git = |source: &str| match fixture.manager.parse_source(source) {
        ParsedSource::Git(git) => git,
        other => panic!("expected a git source for {source}, got {other:?}"),
    };

    let https = git("https://github.com/user/repo");
    assert_eq!(https.host, "github.com");
    assert_eq!(https.path, "user/repo");
    assert!(!https.pinned);

    let ssh = git("ssh://git@github.com/user/repo");
    assert_eq!(ssh.repo, "ssh://git@github.com/user/repo");
    assert_eq!(ssh.path, "user/repo");

    let scp = git("git:git@github.com:user/repo");
    assert_eq!(scp.repo, "git@github.com:user/repo");
    assert_eq!(scp.host, "github.com");
    assert!(!scp.pinned);

    let scp_with_ref = git("git:git@github.com:user/repo@v1.0.0");
    assert_eq!(scp_with_ref.ref_name.as_deref(), Some("v1.0.0"));
    assert!(scp_with_ref.pinned);

    assert_eq!(git("git:https://github.com/user/repo").path, "user/repo");
    assert_eq!(git("git:github.com/user/repo").host, "github.com");
    assert_eq!(git("https://github.com/user/repo.git").path, "user/repo");
    assert_eq!(git("https://gitlab.com/user/repo").host, "gitlab.com");
    assert_eq!(git("https://bitbucket.org/user/repo").host, "bitbucket.org");
    assert_eq!(git("https://codeberg.org/user/repo").host, "codeberg.org");

    let with_ref = git("https://github.com/user/repo@v1.2.3");
    assert_eq!(with_ref.ref_name.as_deref(), Some("v1.2.3"));
    assert!(with_ref.pinned);
    assert_eq!(
        git("https://github.com/user/repo@feature/branch")
            .ref_name
            .as_deref(),
        Some("feature/branch")
    );

    // Without the git: prefix these stay local paths.
    for local in ["github.com/user/repo", "git@github.com:user/repo"] {
        assert!(matches!(
            fixture.manager.parse_source(local),
            ParsedSource::Local(_)
        ));
    }
}

// ============================================================================
// git install paths
// ============================================================================

#[test]
fn rejects_paths_outside_the_git_install_roots() {
    let fixture = Fixture::new();
    let traversal_source = notagent::utils::git::GitSource {
        repo: "git@evil.example:../../victim/repo".to_owned(),
        host: "evil.example".to_owned(),
        path: "../../victim/repo".to_owned(),
        ref_name: None,
        pinned: false,
    };

    for scope in [
        SourceScope::User,
        SourceScope::Project,
        SourceScope::Temporary,
    ] {
        let error = fixture
            .manager
            .get_git_install_path(&traversal_source, scope)
            .expect_err("traversal rejected");
        assert!(error.0.contains("outside package install root"));
    }
}

// ============================================================================
// temporary install paths
// ============================================================================

#[test]
fn places_temporary_npm_packages_under_the_agent_temp_folder() {
    let fixture = Fixture::new();
    let ParsedSource::Npm(source) = fixture.manager.parse_source("npm:left-pad") else {
        panic!("Expected npm source");
    };

    let install_path = fixture
        .manager
        .get_npm_install_path(&source, SourceScope::Temporary)
        .expect("install path");
    let temp_root = fixture.agent_dir.join("tmp").join("extensions");

    assert!(install_path.ends_with("node_modules/left-pad"));
    assert!(install_path.starts_with(&path_string(&temp_root)));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&temp_root)
            .expect("temp root")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700);
    }
}

// ============================================================================
// settings source normalization
// ============================================================================

#[test]
fn stores_global_local_packages_relative_to_the_agent_settings_base() {
    let fixture = Fixture::new();
    let package_dir = fixture.temp_dir.join("packages").join("local-global-pkg");
    fixture.mkdir(&package_dir.join("prompts"));

    assert!(
        fixture
            .manager
            .add_source_to_settings("./packages/local-global-pkg", false)
            .expect("add")
    );

    let settings = fixture.settings.get_global_settings();
    let expected = pathdiff(&fixture.agent_dir, &package_dir);
    assert_eq!(settings.packages, Some(vec![source(&expected)]));
}

#[test]
fn stores_project_local_packages_relative_to_the_notagent_settings_base() {
    let fixture = Fixture::new();
    let package_dir = fixture.temp_dir.join("project-local-pkg");
    fixture.mkdir(&package_dir.join("prompts"));

    assert!(
        fixture
            .manager
            .add_source_to_settings("./project-local-pkg", true)
            .expect("add")
    );

    let settings = fixture.settings.get_project_settings();
    let expected = pathdiff(&fixture.temp_dir.join(".notagent"), &package_dir);
    assert_eq!(settings.packages, Some(vec![source(&expected)]));
}

fn pathdiff(from: &Path, to: &Path) -> String {
    let from: Vec<_> = from.components().collect();
    let to: Vec<_> = to.components().collect();
    let common = from
        .iter()
        .zip(to.iter())
        .take_while(|(left, right)| left == right)
        .count();
    let mut parts: Vec<String> =
        std::iter::repeat_n("..".to_owned(), from.len() - common).collect();
    parts.extend(
        to[common..]
            .iter()
            .map(|component| component.as_os_str().to_string_lossy().into_owned()),
    );
    parts.join("/")
}

#[test]
fn removes_local_package_entries_using_equivalent_path_forms() {
    let fixture = Fixture::new();
    let package_dir = fixture.temp_dir.join("remove-local-pkg");
    fixture.mkdir(&package_dir.join("prompts"));

    fixture
        .manager
        .add_source_to_settings("./remove-local-pkg", false)
        .expect("add");
    assert!(
        fixture
            .manager
            .remove_source_from_settings(&format!("{}/", path_string(&package_dir)), false)
            .expect("remove")
    );
    assert_eq!(
        fixture
            .settings
            .get_global_settings()
            .packages
            .unwrap_or_default()
            .len(),
        0
    );
}

#[test]
fn returns_false_when_adding_the_same_git_source_with_the_same_ref() {
    let fixture = Fixture::new();
    assert!(
        fixture
            .manager
            .add_source_to_settings("git:github.com/user/repo@v1", false)
            .expect("add")
    );
    assert!(
        !fixture
            .manager
            .add_source_to_settings("git:github.com/user/repo@v1", false)
            .expect("add again")
    );
    assert_eq!(
        fixture.settings.get_global_settings().packages,
        Some(vec![source("git:github.com/user/repo@v1")])
    );
}

#[test]
fn updates_the_ref_when_adding_the_same_git_source_with_a_different_ref() {
    let fixture = Fixture::new();
    fixture
        .manager
        .add_source_to_settings("git:github.com/user/repo@v1", false)
        .expect("add");
    assert!(
        fixture
            .manager
            .add_source_to_settings("git:github.com/user/repo@v2", false)
            .expect("update")
    );
    assert_eq!(
        fixture.settings.get_global_settings().packages,
        Some(vec![source("git:github.com/user/repo@v2")])
    );
}

#[test]
fn preserves_package_filters_when_replacing_a_package_source_ref() {
    let fixture = Fixture::new();
    let filter = PackageSourceFilter {
        source: "git:github.com/user/repo@v1".to_owned(),
        skills: Some(Vec::new()),
        prompts: Some(strings(&["prompts/review.md"])),
        themes: Some(strings(&["themes/dark.json"])),
        ..PackageSourceFilter::default()
    };
    fixture.settings.set_packages(&[filtered(filter.clone())]);

    assert!(
        fixture
            .manager
            .add_source_to_settings("git:github.com/user/repo@v2", false)
            .expect("update")
    );
    assert_eq!(
        fixture.settings.get_global_settings().packages,
        Some(vec![filtered(PackageSourceFilter {
            source: "git:github.com/user/repo@v2".to_owned(),
            ..filter
        })])
    );
}

// ============================================================================
// package deduplication
// ============================================================================

#[tokio::test]
async fn dedupes_the_same_local_package_in_global_and_project_settings() {
    let fixture = Fixture::new();
    let package_dir = fixture.temp_dir.join("shared-pkg");
    fixture.write(&package_dir.join("prompts").join("shared.md"), "Shared");

    fixture
        .settings
        .set_packages(&[source(&path_string(&package_dir))]);
    fixture
        .settings
        .set_project_packages(&[source(&path_string(&package_dir))])
        .expect("project settings");

    let result = resolve(&fixture).await;
    let shared: Vec<&ResolvedResource> = result
        .prompts
        .iter()
        .filter(|resource| resource.path.contains("shared-pkg"))
        .collect();
    assert_eq!(shared.len(), 1);
    assert_eq!(shared[0].metadata.scope, SourceScope::Project);
}

#[tokio::test]
async fn keeps_two_different_packages() {
    let fixture = Fixture::new();
    let first_dir = fixture.temp_dir.join("pkg1");
    let second_dir = fixture.temp_dir.join("pkg2");
    fixture.write(&first_dir.join("prompts").join("from-pkg1.md"), "One");
    fixture.write(&second_dir.join("prompts").join("from-pkg2.md"), "Two");

    fixture
        .settings
        .set_packages(&[source(&path_string(&first_dir))]);
    fixture
        .settings
        .set_project_packages(&[source(&path_string(&second_dir))])
        .expect("project settings");

    let result = resolve(&fixture).await;
    assert!(contains_enabled(&result.prompts, "pkg1"));
    assert!(contains_enabled(&result.prompts, "pkg2"));
}

#[test]
fn normalizes_every_supported_git_url_format_to_one_identity() {
    let fixture = Fixture::new();
    let identity = |source: &str| {
        fixture
            .manager
            .get_package_identity(source, None)
            .expect("identity")
    };

    for url in [
        "https://github.com/user/repo",
        "https://github.com/user/repo.git",
        "https://github.com/user/repo@v1.0.0",
        "ssh://git@github.com/user/repo",
        "git:https://github.com/user/repo",
        "git:github.com/user/repo",
        "git:git@github.com:user/repo",
        "git:git@github.com:user/repo.git",
        "git:git@github.com:user/repo@v1.0.0",
    ] {
        assert_eq!(identity(url), "git:github.com/user/repo", "for {url}");
    }

    assert_ne!(
        identity("https://github.com/user/repo1"),
        identity("git:git@github.com:user/repo2")
    );
}

// ============================================================================
// pattern filtering in top-level arrays
// ============================================================================

#[tokio::test]
async fn excludes_prompts_with_a_bang_pattern() {
    let fixture = Fixture::new();
    let prompts_dir = fixture.agent_dir.join("prompts");
    fixture.write(&prompts_dir.join("keep.md"), "Keep");
    fixture.write(&prompts_dir.join("remove.md"), "Remove");
    fixture
        .settings
        .set_prompt_template_paths(&strings(&["prompts", "!**/remove.md"]));

    let result = resolve(&fixture).await;
    assert!(is_enabled(&result.prompts, "keep.md"));
    assert!(is_disabled(&result.prompts, "remove.md"));
}

#[tokio::test]
async fn filters_themes_with_glob_patterns() {
    let fixture = Fixture::new();
    let themes_dir = fixture.agent_dir.join("themes");
    for name in ["dark.json", "light.json", "funky.json"] {
        fixture.write(&themes_dir.join(name), "{}");
    }
    fixture
        .settings
        .set_theme_paths(&strings(&["themes", "!funky.json"]));

    let result = resolve(&fixture).await;
    assert!(is_enabled(&result.themes, "dark.json"));
    assert!(is_enabled(&result.themes, "light.json"));
    assert!(is_disabled(&result.themes, "funky.json"));
}

#[tokio::test]
async fn filters_prompts_with_an_exclusion_pattern() {
    let fixture = Fixture::new();
    let prompts_dir = fixture.agent_dir.join("prompts");
    fixture.write(&prompts_dir.join("review.md"), "Review code");
    fixture.write(&prompts_dir.join("explain.md"), "Explain code");
    fixture
        .settings
        .set_prompt_template_paths(&strings(&["prompts", "!explain.md"]));

    let result = resolve(&fixture).await;
    assert!(is_enabled(&result.prompts, "review.md"));
    assert!(is_disabled(&result.prompts, "explain.md"));
}

#[tokio::test]
async fn filters_skills_with_an_exclusion_pattern() {
    let fixture = Fixture::new();
    let skills_dir = fixture.agent_dir.join("skills");
    fixture.write(&skills_dir.join("good-skill").join("SKILL.md"), SKILL_BODY);
    fixture.write(&skills_dir.join("bad-skill").join("SKILL.md"), SKILL_BODY);
    fixture
        .settings
        .set_skill_paths(&strings(&["skills", "!**/bad-skill"]));

    let result = resolve(&fixture).await;
    assert!(contains_enabled(&result.skills, "good-skill"));
    assert!(contains_disabled(&result.skills, "bad-skill"));
}

#[tokio::test]
async fn works_without_patterns() {
    let fixture = Fixture::new();
    let prompt_path = fixture.agent_dir.join("prompts").join("my-prompt.md");
    fixture.write(&prompt_path, "Prompt");
    fixture
        .settings
        .set_prompt_template_paths(&strings(&["prompts/my-prompt.md"]));

    let result = resolve(&fixture).await;
    assert!(is_enabled(&result.prompts, &path_string(&prompt_path)));
}

// ============================================================================
// pattern filtering in the notagent manifest
// ============================================================================

#[tokio::test]
async fn supports_glob_patterns_in_manifest_entries() {
    let fixture = Fixture::new();
    let package_dir = fixture.temp_dir.join("manifest-pkg");
    fixture.write(&package_dir.join("prompts").join("local.md"), "Local");
    fixture.write(
        &package_dir
            .join("node_modules")
            .join("dep")
            .join("prompts")
            .join("remote.md"),
        "Remote",
    );
    fixture.write(
        &package_dir
            .join("node_modules")
            .join("dep")
            .join("prompts")
            .join("skip.md"),
        "Skip",
    );
    fixture.write(
        &package_dir.join("package.json"),
        r#"{"name":"manifest-pkg","notagent":{"prompts":["prompts","node_modules/dep/prompts","!**/skip.md"]}}"#,
    );
    fixture
        .settings
        .set_packages(&[source(&path_string(&package_dir))]);

    let result = resolve(&fixture).await;
    assert!(is_enabled(&result.prompts, "local.md"));
    assert!(is_enabled(&result.prompts, "remote.md"));
    assert!(
        !result
            .prompts
            .iter()
            .any(|resource| resource.path.ends_with("skip.md"))
    );
}

#[tokio::test]
async fn supports_glob_patterns_in_manifest_skills() {
    let fixture = Fixture::new();
    let package_dir = fixture.temp_dir.join("skill-manifest-pkg");
    fixture.write(
        &package_dir
            .join("skills")
            .join("good-skill")
            .join("SKILL.md"),
        SKILL_BODY,
    );
    fixture.write(
        &package_dir
            .join("skills")
            .join("bad-skill")
            .join("SKILL.md"),
        SKILL_BODY,
    );
    fixture.write(
        &package_dir.join("package.json"),
        r#"{"name":"skill-manifest-pkg","notagent":{"skills":["skills","!**/bad-skill"]}}"#,
    );
    fixture
        .settings
        .set_packages(&[source(&path_string(&package_dir))]);

    let result = resolve(&fixture).await;
    assert!(contains_enabled(&result.skills, "good-skill"));
    assert!(
        !result
            .skills
            .iter()
            .any(|resource| resource.path.contains("bad-skill"))
    );
}

#[tokio::test]
async fn expands_positive_glob_manifest_entries_before_collecting_skills() {
    let fixture = Fixture::new();
    let package_dir = fixture.temp_dir.join("skill-manifest-glob-pkg");
    fixture.write(
        &package_dir
            .join("plugins")
            .join("pdf-to-markdown")
            .join("skills")
            .join("pdf-to-markdown")
            .join("SKILL.md"),
        SKILL_BODY,
    );
    fixture.write(
        &package_dir
            .join("plugins")
            .join("nutrient-dws")
            .join("skills")
            .join("document-processor-api")
            .join("SKILL.md"),
        SKILL_BODY,
    );
    fixture.write(
        &package_dir.join("package.json"),
        r#"{"name":"skill-manifest-glob-pkg","notagent":{"skills":["./plugins/*/skills"]}}"#,
    );
    fixture
        .settings
        .set_packages(&[source(&path_string(&package_dir))]);

    let result = resolve(&fixture).await;
    assert!(contains_enabled(&result.skills, "pdf-to-markdown"));
    assert!(contains_enabled(&result.skills, "document-processor-api"));
}

// ============================================================================
// pattern filtering in package filters
// ============================================================================

fn prompt_filter(package_dir: &Path, prompts: &[&str]) -> PackageSource {
    filtered(PackageSourceFilter {
        source: path_string(package_dir),
        skills: Some(Vec::new()),
        prompts: Some(strings(prompts)),
        themes: Some(Vec::new()),
        ..PackageSourceFilter::default()
    })
}

#[tokio::test]
async fn applies_user_filters_on_top_of_manifest_filters() {
    // Manifest excludes baz.md, user excludes bar.md — both are gone.
    let fixture = Fixture::new();
    let package_dir = fixture.temp_dir.join("layered-pkg");
    for name in ["foo.md", "bar.md", "baz.md"] {
        fixture.write(&package_dir.join("prompts").join(name), "Prompt");
    }
    fixture.write(
        &package_dir.join("package.json"),
        r#"{"name":"layered-pkg","notagent":{"prompts":["prompts","!**/baz.md"]}}"#,
    );
    fixture
        .settings
        .set_packages(&[prompt_filter(&package_dir, &["!**/bar.md"])]);

    let result = resolve(&fixture).await;
    assert!(is_enabled(&result.prompts, "foo.md"));
    assert!(is_disabled(&result.prompts, "bar.md"));
    assert!(
        !result
            .prompts
            .iter()
            .any(|resource| resource.path.ends_with("baz.md"))
    );
}

#[tokio::test]
async fn excludes_package_resources_with_a_bang_pattern() {
    let fixture = Fixture::new();
    let package_dir = fixture.temp_dir.join("pattern-pkg");
    for name in ["foo.md", "bar.md", "baz.md"] {
        fixture.write(&package_dir.join("prompts").join(name), "Prompt");
    }
    fixture
        .settings
        .set_packages(&[prompt_filter(&package_dir, &["!**/baz.md"])]);

    let result = resolve(&fixture).await;
    assert!(is_enabled(&result.prompts, "foo.md"));
    assert!(is_enabled(&result.prompts, "bar.md"));
    assert!(is_disabled(&result.prompts, "baz.md"));
}

#[tokio::test]
async fn filters_themes_from_a_package() {
    let fixture = Fixture::new();
    let package_dir = fixture.temp_dir.join("theme-pkg");
    fixture.write(&package_dir.join("themes").join("nice.json"), "{}");
    fixture.write(&package_dir.join("themes").join("ugly.json"), "{}");
    fixture
        .settings
        .set_packages(&[filtered(PackageSourceFilter {
            source: path_string(&package_dir),
            skills: Some(Vec::new()),
            prompts: Some(Vec::new()),
            themes: Some(strings(&["!ugly.json"])),
            ..PackageSourceFilter::default()
        })]);

    let result = resolve(&fixture).await;
    assert!(is_enabled(&result.themes, "nice.json"));
    assert!(is_disabled(&result.themes, "ugly.json"));
}

#[tokio::test]
async fn combines_include_and_exclude_patterns() {
    let fixture = Fixture::new();
    let package_dir = fixture.temp_dir.join("combo-pkg");
    for name in ["alpha.md", "beta.md", "gamma.md"] {
        fixture.write(&package_dir.join("prompts").join(name), "Prompt");
    }
    fixture.settings.set_packages(&[prompt_filter(
        &package_dir,
        &["**/alpha.md", "**/beta.md", "!**/beta.md"],
    )]);

    let result = resolve(&fixture).await;
    assert!(is_enabled(&result.prompts, "alpha.md"));
    assert!(is_disabled(&result.prompts, "beta.md"));
    assert!(is_disabled(&result.prompts, "gamma.md"));
}

#[tokio::test]
async fn works_with_direct_paths_in_package_filters() {
    let fixture = Fixture::new();
    let package_dir = fixture.temp_dir.join("direct-pkg");
    fixture.write(&package_dir.join("prompts").join("one.md"), "One");
    fixture.write(&package_dir.join("prompts").join("two.md"), "Two");
    fixture
        .settings
        .set_packages(&[prompt_filter(&package_dir, &["prompts/one.md"])]);

    let result = resolve(&fixture).await;
    assert!(is_enabled(&result.prompts, "one.md"));
    assert!(is_disabled(&result.prompts, "two.md"));
}

#[tokio::test]
async fn resolves_autoload_disabled_project_entries_as_deltas_over_global_packages() {
    let fixture = Fixture::new();
    let package_dir = fixture
        .agent_dir
        .join("npm")
        .join("node_modules")
        .join("notagent-tools");
    fixture.write(
        &package_dir.join("package.json"),
        r#"{"name":"notagent-tools","version":"1.0.0"}"#,
    );
    fixture.write(&package_dir.join("prompts").join("foo.md"), "Foo");
    fixture.write(&package_dir.join("prompts").join("bar.md"), "Bar");
    fixture
        .settings
        .set_packages(&[source("npm:notagent-tools")]);
    fixture
        .settings
        .set_project_packages(&[filtered(PackageSourceFilter {
            source: "npm:notagent-tools".to_owned(),
            autoload: Some(false),
            prompts: Some(strings(&["-prompts/foo.md"])),
            ..PackageSourceFilter::default()
        })])
        .expect("project settings");
    fixture
        .runner
        .on_run(|_command, _args, _options| Err("unexpected install".to_owned()));

    let result = resolve(&fixture).await;
    assert!(fixture.runner.runs().is_empty());

    let foo = find_path(&result.prompts, &package_dir.join("prompts").join("foo.md"))
        .expect("foo prompt");
    assert!(!foo.enabled);
    assert_eq!(foo.metadata.scope, SourceScope::Project);
    let bar = find_path(&result.prompts, &package_dir.join("prompts").join("bar.md"))
        .expect("bar prompt");
    assert!(bar.enabled);
    assert_eq!(bar.metadata.scope, SourceScope::User);
}

#[tokio::test]
async fn resolves_autoload_disabled_entries_as_positive_only_without_a_global_package() {
    let fixture = Fixture::new();
    let package_dir = fixture.temp_dir.join("positive-only-pkg");
    fixture.write(&package_dir.join("prompts").join("foo.md"), "Foo");
    fixture.write(&package_dir.join("prompts").join("bar.md"), "Bar");
    fixture.write(
        &package_dir.join("skills").join("foo").join("SKILL.md"),
        SKILL_BODY,
    );
    fixture
        .settings
        .set_project_packages(&[filtered(PackageSourceFilter {
            source: pathdiff(&fixture.temp_dir.join(".notagent"), &package_dir),
            autoload: Some(false),
            prompts: Some(strings(&["+prompts/foo.md"])),
            ..PackageSourceFilter::default()
        })])
        .expect("project settings");

    let result = resolve(&fixture).await;
    assert_eq!(
        result
            .prompts
            .iter()
            .map(|resource| resource.path.clone())
            .collect::<Vec<_>>(),
        vec![path_string(&package_dir.join("prompts").join("foo.md"))]
    );
    assert!(result.skills.is_empty());
}

// ============================================================================
// force-include and force-exclude patterns
// ============================================================================

#[tokio::test]
async fn force_includes_prompts_after_an_exclusion() {
    let fixture = Fixture::new();
    let prompts_dir = fixture.agent_dir.join("prompts");
    for name in ["keep.md", "excluded.md", "force-back.md"] {
        fixture.write(&prompts_dir.join(name), "Prompt");
    }
    // Exclude all, then force-include one back
    fixture.settings.set_prompt_template_paths(&strings(&[
        "prompts",
        "!prompts/*.md",
        "+prompts/force-back.md",
    ]));

    let result = resolve(&fixture).await;
    assert!(is_disabled(&result.prompts, "keep.md"));
    assert!(is_disabled(&result.prompts, "excluded.md"));
    assert!(is_enabled(&result.prompts, "force-back.md"));
}

#[tokio::test]
async fn force_include_overrides_exclude_in_package_filters() {
    let fixture = Fixture::new();
    let package_dir = fixture.temp_dir.join("force-pkg");
    for name in ["alpha.md", "beta.md", "gamma.md"] {
        fixture.write(&package_dir.join("prompts").join(name), "Prompt");
    }
    fixture.settings.set_packages(&[prompt_filter(
        &package_dir,
        &["!**/*.md", "+prompts/beta.md"],
    )]);

    let result = resolve(&fixture).await;
    assert!(is_disabled(&result.prompts, "alpha.md"));
    assert!(is_enabled(&result.prompts, "beta.md"));
    assert!(is_disabled(&result.prompts, "gamma.md"));
}

#[tokio::test]
async fn force_includes_multiple_resources() {
    let fixture = Fixture::new();
    let package_dir = fixture.temp_dir.join("multi-force-pkg");
    for name in ["skill-a", "skill-b", "skill-c"] {
        fixture.write(
            &package_dir.join("skills").join(name).join("SKILL.md"),
            SKILL_BODY,
        );
    }
    fixture
        .settings
        .set_packages(&[filtered(PackageSourceFilter {
            source: path_string(&package_dir),
            skills: Some(strings(&["!**/*", "+skills/skill-a", "+skills/skill-c"])),
            prompts: Some(Vec::new()),
            themes: Some(Vec::new()),
            ..PackageSourceFilter::default()
        })]);

    let result = resolve(&fixture).await;
    assert!(contains_enabled(&result.skills, "skill-a"));
    assert!(contains_disabled(&result.skills, "skill-b"));
    assert!(contains_enabled(&result.skills, "skill-c"));
}

#[tokio::test]
async fn force_includes_after_a_specific_exclusion() {
    let fixture = Fixture::new();
    let prompts_dir = fixture.agent_dir.join("prompts");
    fixture.write(&prompts_dir.join("a.md"), "A");
    fixture.write(&prompts_dir.join("b.md"), "B");
    // Specifically exclude b.md, then force it back
    fixture.settings.set_prompt_template_paths(&strings(&[
        "prompts",
        "!prompts/b.md",
        "+prompts/b.md",
    ]));

    let result = resolve(&fixture).await;
    assert!(is_enabled(&result.prompts, "a.md"));
    assert!(is_enabled(&result.prompts, "b.md"));
}

#[tokio::test]
async fn handles_force_include_in_manifest_patterns() {
    let fixture = Fixture::new();
    let package_dir = fixture.temp_dir.join("manifest-force-pkg");
    for name in ["one.md", "two.md", "three.md"] {
        fixture.write(&package_dir.join("prompts").join(name), "Prompt");
    }
    fixture.write(
        &package_dir.join("package.json"),
        r#"{"name":"manifest-force-pkg","notagent":{"prompts":["prompts","!**/two.md","+prompts/two.md"]}}"#,
    );
    fixture
        .settings
        .set_packages(&[source(&path_string(&package_dir))]);

    let result = resolve(&fixture).await;
    assert!(is_enabled(&result.prompts, "one.md"));
    assert!(is_enabled(&result.prompts, "two.md"));
    assert!(is_enabled(&result.prompts, "three.md"));
}

#[tokio::test]
async fn force_includes_themes() {
    let fixture = Fixture::new();
    let themes_dir = fixture.agent_dir.join("themes");
    for name in ["dark.json", "light.json", "special.json"] {
        fixture.write(&themes_dir.join(name), "{}");
    }
    fixture.settings.set_theme_paths(&strings(&[
        "themes",
        "!themes/*.json",
        "+themes/special.json",
    ]));

    let result = resolve(&fixture).await;
    assert!(is_disabled(&result.themes, "dark.json"));
    assert!(is_disabled(&result.themes, "light.json"));
    assert!(is_enabled(&result.themes, "special.json"));
}

#[tokio::test]
async fn force_excludes_top_level_resources() {
    let fixture = Fixture::new();
    let prompts_dir = fixture.agent_dir.join("prompts");
    fixture.write(&prompts_dir.join("alpha.md"), "Alpha");
    fixture.write(&prompts_dir.join("beta.md"), "Beta");
    fixture.settings.set_prompt_template_paths(&strings(&[
        "prompts",
        "+prompts/alpha.md",
        "-prompts/alpha.md",
    ]));

    let result = resolve(&fixture).await;
    assert!(is_disabled(&result.prompts, "alpha.md"));
    assert!(is_enabled(&result.prompts, "beta.md"));
}

#[tokio::test]
async fn force_excludes_in_package_filters() {
    let fixture = Fixture::new();
    let package_dir = fixture.temp_dir.join("force-exclude-pkg");
    fixture.write(&package_dir.join("prompts").join("alpha.md"), "Alpha");
    fixture.write(&package_dir.join("prompts").join("beta.md"), "Beta");
    fixture.settings.set_packages(&[prompt_filter(
        &package_dir,
        &["prompts/*.md", "+prompts/alpha.md", "-prompts/alpha.md"],
    )]);

    let result = resolve(&fixture).await;
    assert!(is_disabled(&result.prompts, "alpha.md"));
    assert!(is_enabled(&result.prompts, "beta.md"));
}

// ============================================================================
// offline mode and network timeouts
// ============================================================================

#[tokio::test]
async fn updates_npm_range_packages_using_the_configured_spec() {
    let fixture = Fixture::new();
    let installed_path = fixture
        .temp_dir
        .join(".notagent")
        .join("npm")
        .join("node_modules")
        .join("example");
    fixture.write(
        &installed_path.join("package.json"),
        r#"{"name":"example","version":"1.0.0"}"#,
    );
    fixture
        .settings
        .set_project_packages(&[source("npm:example@^1.0.0")])
        .expect("project settings");
    fixture
        .runner
        .on_capture(|_command, _args, _options| Ok(r#"["1.0.0","1.2.0"]"#.to_owned()));

    fixture
        .manager
        .update(Some("npm:example"))
        .await
        .expect("update");

    let captures = fixture.runner.captures();
    assert!(captures.iter().any(|call| {
        call.matches("npm", &["view", "example@^1.0.0", "version", "--json"])
            && call.cwd.as_deref() == Some(path_string(&fixture.temp_dir).as_str())
    }));
    let npm_root = path_string(&fixture.temp_dir.join(".notagent").join("npm"));
    assert!(fixture.runner.ran(
        "npm",
        &[
            "install",
            "example@^1.0.0",
            "--prefix",
            &npm_root,
            "--legacy-peer-deps"
        ]
    ));
}

#[tokio::test]
async fn skips_a_project_npm_update_when_the_installed_version_is_latest() {
    let fixture = Fixture::new();
    let installed_path = fixture
        .temp_dir
        .join(".notagent")
        .join("npm")
        .join("node_modules")
        .join("example");
    fixture.write(
        &installed_path.join("package.json"),
        r#"{"name":"example","version":"1.3.1"}"#,
    );
    fixture
        .settings
        .set_project_packages(&[source("npm:example@^1.0.0")])
        .expect("project settings");
    fixture
        .runner
        .on_capture(|_command, _args, _options| Ok(r#"["1.0.0","1.3.1","1.0.2"]"#.to_owned()));

    fixture
        .manager
        .update(Some("npm:example"))
        .await
        .expect("update");

    assert_eq!(fixture.runner.captures().len(), 1);
    assert!(fixture.runner.runs().is_empty());
}

#[tokio::test]
async fn migrates_legacy_user_npm_installs_into_the_managed_root_during_update() {
    let fixture = Fixture::new();
    let legacy_root = fixture.temp_dir.join("legacy-global").join("node_modules");
    let legacy_path = legacy_root.join("legacy-pkg");
    let managed_path = fixture
        .agent_dir
        .join("npm")
        .join("node_modules")
        .join("legacy-pkg");
    fixture.write(
        &legacy_path.join("package.json"),
        r#"{"name":"legacy-pkg","version":"1.0.0"}"#,
    );
    fixture.settings.set_packages(&[source("npm:legacy-pkg")]);

    let legacy_root_string = path_string(&legacy_root);
    fixture
        .runner
        .on_sync(move |_command, _args| Ok(legacy_root_string.clone()));
    let npm_root = path_string(&fixture.agent_dir.join("npm"));
    let install_target = managed_path.clone();
    fixture.runner.on_run(move |command, args, _options| {
        assert_eq!(command, "npm");
        assert_eq!(
            args,
            strings(&[
                "install",
                "legacy-pkg@latest",
                "--prefix",
                &npm_root,
                "--legacy-peer-deps"
            ])
        );
        std::fs::create_dir_all(&install_target).expect("managed dir");
        std::fs::write(
            install_target.join("package.json"),
            r#"{"name":"legacy-pkg","version":"1.0.0"}"#,
        )
        .expect("package.json");
        Ok(())
    });

    assert_eq!(
        fixture
            .manager
            .get_installed_path("npm:legacy-pkg", SourceScope::User)
            .expect("installed path"),
        Some(path_string(&legacy_path))
    );

    fixture
        .manager
        .update(Some("npm:legacy-pkg"))
        .await
        .expect("update");

    assert!(fixture.runner.captures().is_empty());
    assert_eq!(fixture.runner.runs().len(), 1);
    assert_eq!(
        fixture
            .manager
            .get_installed_path("npm:legacy-pkg", SourceScope::User)
            .expect("installed path"),
        Some(path_string(&managed_path))
    );
}

#[tokio::test]
async fn batches_npm_updates_per_scope_and_runs_git_updates_in_parallel() {
    let fixture = Fixture::new();
    let user_npm = fixture.agent_dir.join("npm").join("node_modules");
    let project_npm = fixture
        .temp_dir
        .join(".notagent")
        .join("npm")
        .join("node_modules");
    for (root, name) in [
        (&user_npm, "user-old"),
        (&user_npm, "user-current"),
        (&user_npm, "user-unknown"),
        (&project_npm, "project-old"),
        (&project_npm, "project-current"),
    ] {
        fixture.write(
            &root.join(name).join("package.json"),
            &format!(r#"{{"name":"{name}","version":"1.0.0"}}"#),
        );
    }
    // through the fake runner, so their checkouts have to exist.
    let git_root = fixture
        .agent_dir
        .join("git")
        .join("github.com")
        .join("example");
    for name in ["user-repo-a", "user-repo-b", "user-repo-pinned"] {
        fixture.mkdir(&git_root.join(name));
    }
    fixture.mkdir(
        &fixture
            .temp_dir
            .join(".notagent")
            .join("git")
            .join("github.com")
            .join("example")
            .join("project-repo-a"),
    );

    fixture.settings.set_packages(&[
        source("npm:user-old"),
        source("npm:user-current"),
        source("npm:user-unknown"),
        source("npm:user-pinned@1.0.0"),
        source("git:github.com/example/user-repo-a"),
        source("git:github.com/example/user-repo-b"),
        source("git:github.com/example/user-repo-pinned@v1"),
    ]);
    fixture
        .settings
        .set_project_packages(&[
            source("npm:project-old"),
            source("npm:project-current"),
            source("npm:project-missing"),
            source("git:github.com/example/project-repo-a"),
        ])
        .expect("project settings");

    fixture
        .runner
        .on_capture(|_command, args, _options| match args[0].as_str() {
            "view" => match args[1].as_str() {
                "user-old" | "project-old" => Ok(r#""2.0.0""#.to_owned()),
                "user-current" | "project-current" => Ok(r#""1.0.0""#.to_owned()),
                "user-unknown" => Err("registry unavailable".to_owned()),
                other => Err(format!("Unexpected package lookup: {other}")),
            },
            // Every checkout is already at its target, so no reset follows.
            "rev-parse" => Ok("same-head".to_owned()),
            other => Err(format!("Unexpected capture: {other}")),
        });
    fixture.runner.delay_run("npm", 20);
    fixture.runner.delay_run("git", 20);

    fixture.manager.update(None).await.expect("update");

    let view_calls = fixture
        .runner
        .captures()
        .iter()
        .filter(|call| call.args[0] == "view")
        .count();
    assert_eq!(view_calls, 5);

    let npm_runs: Vec<_> = fixture
        .runner
        .runs()
        .into_iter()
        .filter(|call| call.command == "npm")
        .collect();
    assert_eq!(npm_runs.len(), 2);
    let user_root = path_string(&fixture.agent_dir.join("npm"));
    let project_root = path_string(&fixture.temp_dir.join(".notagent").join("npm"));
    assert!(npm_runs.iter().any(|call| call.matches(
        "npm",
        &[
            "install",
            "user-old@latest",
            "user-unknown@latest",
            "--prefix",
            &user_root,
            "--legacy-peer-deps",
        ]
    )));
    assert!(npm_runs.iter().any(|call| call.matches(
        "npm",
        &[
            "install",
            "project-old@latest",
            "project-missing@latest",
            "--prefix",
            &project_root,
            "--legacy-peer-deps",
        ]
    )));

    let git_fetches = fixture
        .runner
        .runs()
        .iter()
        .filter(|call| call.command == "git" && call.args[0] == "fetch")
        .count();
    assert_eq!(git_fetches, 4);
    assert!(fixture.runner.peak_concurrency("npm") > 1);
    assert!(fixture.runner.peak_concurrency("git") > 1);
}

#[tokio::test]
async fn suggests_npm_source_prefixes_for_update_lookups() {
    let fixture = Fixture::new();
    fixture
        .settings
        .set_project_packages(&[source("npm:example")])
        .expect("project settings");

    let error = fixture
        .manager
        .update(Some("example"))
        .await
        .expect_err("no match");
    assert_eq!(
        error.0,
        "No matching package found for example. Did you mean npm:example?"
    );
}

#[tokio::test]
async fn suggests_git_source_prefixes_for_update_lookups() {
    let fixture = Fixture::new();
    fixture
        .settings
        .set_project_packages(&[source("git:github.com/example/repo")])
        .expect("project settings");

    let error = fixture
        .manager
        .update(Some("github.com/example/repo"))
        .await
        .expect_err("no match");
    assert_eq!(
        error.0,
        "No matching package found for github.com/example/repo. Did you mean git:github.com/example/repo?"
    );
}

#[tokio::test]
async fn skips_installing_missing_package_sources_when_offline() {
    let fixture = Fixture::new();
    fixture.set_offline();
    fixture
        .settings
        .set_project_packages(&[
            source("npm:missing-package"),
            source("git:github.com/example/missing-repo"),
        ])
        .expect("project settings");

    let result = resolve(&fixture).await;
    let all_resources = [result.skills, result.prompts, result.themes].concat();
    assert!(!all_resources.iter().any(|resource| {
        resource.metadata.origin == notagent::core::source_info::SourceOrigin::Package
    }));
    assert!(fixture.runner.runs().is_empty());
}

#[tokio::test]
async fn does_not_run_npm_view_during_resolve_for_installed_unpinned_packages() {
    let fixture = Fixture::new();
    fixture.set_offline();
    let installed_path = fixture
        .temp_dir
        .join(".notagent")
        .join("npm")
        .join("node_modules")
        .join("example");
    fixture.write(
        &installed_path.join("package.json"),
        r#"{"name":"example","version":"1.0.0"}"#,
    );
    fixture.write(&installed_path.join("prompts").join("index.md"), "Prompt");
    fixture
        .settings
        .set_project_packages(&[source("npm:example@^1.0.0")])
        .expect("project settings");

    let result = resolve(&fixture).await;
    assert!(is_enabled(&result.prompts, "prompts/index.md"));
    assert!(fixture.runner.captures().is_empty());
}

#[tokio::test]
async fn reinstalls_pinned_npm_packages_when_the_installed_version_differs() {
    let fixture = Fixture::new();
    let installed_path = fixture
        .temp_dir
        .join(".notagent")
        .join("npm")
        .join("node_modules")
        .join("example");
    fixture.write(
        &installed_path.join("package.json"),
        r#"{"name":"example","version":"1.0.0"}"#,
    );
    fixture
        .settings
        .set_project_packages(&[source("npm:example@2.0.0")])
        .expect("project settings");

    resolve(&fixture).await;

    let installs: Vec<_> = fixture
        .runner
        .runs()
        .into_iter()
        .filter(|call| call.command == "npm" && call.args[0] == "install")
        .collect();
    assert_eq!(installs.len(), 1);
    assert_eq!(installs[0].args[1], "example@2.0.0");
}

#[tokio::test]
async fn does_not_check_package_updates_when_offline() {
    let fixture = Fixture::new();
    fixture.set_offline();

    let updates = fixture
        .manager
        .check_for_available_updates()
        .await
        .expect("updates");
    assert_eq!(updates, Vec::<PackageUpdate>::new());
    assert!(fixture.runner.captures().is_empty());
}

#[tokio::test]
async fn reports_updates_for_installed_unpinned_npm_packages() {
    let fixture = Fixture::new();
    let installed_path = fixture
        .temp_dir
        .join(".notagent")
        .join("npm")
        .join("node_modules")
        .join("example");
    fixture.write(
        &installed_path.join("package.json"),
        r#"{"name":"example","version":"1.0.0"}"#,
    );
    fixture
        .settings
        .set_project_packages(&[source("npm:example")])
        .expect("project settings");
    fixture
        .runner
        .on_capture(|_command, _args, _options| Ok(r#""1.2.3""#.to_owned()));

    let updates = fixture
        .manager
        .check_for_available_updates()
        .await
        .expect("updates");
    assert_eq!(
        updates,
        vec![PackageUpdate {
            source: "npm:example".to_owned(),
            display_name: "example".to_owned(),
            kind: PackageKind::Npm,
            scope: SourceScope::Project,
        }]
    );
}

#[tokio::test]
async fn skips_pinned_packages_when_checking_for_updates() {
    let fixture = Fixture::new();
    let installed_npm_path = fixture
        .temp_dir
        .join(".notagent")
        .join("npm")
        .join("node_modules")
        .join("example");
    fixture.write(
        &installed_npm_path.join("package.json"),
        r#"{"name":"example","version":"1.0.0"}"#,
    );
    let ParsedSource::Git(parsed_git) = fixture
        .manager
        .parse_source("git:github.com/example/repo@v1")
    else {
        panic!("expected a git source");
    };
    let installed_git_path = fixture
        .manager
        .get_git_install_path(&parsed_git, SourceScope::Project)
        .expect("git install path");
    fixture.mkdir(Path::new(&installed_git_path));

    fixture
        .settings
        .set_project_packages(&[
            source("npm:example@1.0.0"),
            source("git:github.com/example/repo@v1"),
        ])
        .expect("project settings");

    let updates = fixture
        .manager
        .check_for_available_updates()
        .await
        .expect("updates");
    assert_eq!(updates, Vec::<PackageUpdate>::new());
    assert!(fixture.runner.captures().is_empty());
}

#[tokio::test]
async fn uses_npm_view_to_fetch_the_latest_version() {
    let fixture = Fixture::new();
    fixture
        .runner
        .on_capture(|_command, _args, _options| Ok(r#""1.2.3""#.to_owned()));

    let latest = fixture
        .manager
        .get_latest_npm_version("example", None)
        .await
        .expect("latest");
    assert_eq!(latest, "1.2.3");
    let captures = fixture.runner.captures();
    assert_eq!(captures.len(), 1);
    assert!(captures[0].matches("npm", &["view", "example", "version", "--json"]));
    assert_eq!(
        captures[0].cwd.as_deref(),
        Some(path_string(&fixture.temp_dir).as_str())
    );
}

#[tokio::test]
async fn uses_the_npm_command_argv_for_npm_update_checks() {
    let fixture = Fixture::with_settings(&npm_command_settings(&[
        "mise", "exec", "node@20", "--", "npm",
    ]));
    fixture
        .runner
        .on_capture(|_command, _args, _options| Ok(r#""1.2.3""#.to_owned()));

    let latest = fixture
        .manager
        .get_latest_npm_version("@scope/pkg", None)
        .await
        .expect("latest");
    assert_eq!(latest, "1.2.3");
    assert!(fixture.runner.captures()[0].matches(
        "mise",
        &[
            "exec",
            "node@20",
            "--",
            "npm",
            "view",
            "@scope/pkg",
            "version",
            "--json"
        ]
    ));
}

/// pins that the capture resolves on `close`, not on `exit`, so no output is
/// guarantee; it is checked against a real process (class 1).
#[cfg(unix)]
#[tokio::test]
async fn waits_for_the_process_before_resolving_captured_stdout() {
    use notagent::core::package_manager::command_runner::ProcessCommandRunner;
    let output = ProcessCommandRunner
        .run_capture(
            "/bin/sh".to_owned(),
            strings(&["-c", "printf 'abc123\\n'"]),
            CommandOptions::default(),
        )
        .await
        .expect("capture");
    assert_eq!(output, "abc123");
}

#[cfg(unix)]
#[tokio::test]
async fn reports_a_failing_captured_command_with_its_exit_code() {
    use notagent::core::package_manager::command_runner::ProcessCommandRunner;
    let error = ProcessCommandRunner
        .run_capture(
            "/bin/sh".to_owned(),
            strings(&["-c", "echo boom >&2; exit 3"]),
            CommandOptions::default(),
        )
        .await
        .expect_err("failure");
    assert!(error.contains("failed with code 3"), "{error}");
    assert!(error.contains("boom"), "{error}");
}
