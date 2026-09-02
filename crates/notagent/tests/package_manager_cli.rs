mod support;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use futures::future::BoxFuture;
use notagent::config::InstallEnv;
use notagent::core::package_manager::command_runner::{CommandOptions, CommandRunner};
use notagent::core::project_trust::ProjectTrustContext;
use notagent::core::trust_manager::ProjectTrustStore;
use notagent::package_manager_cli::{ConsoleIo, PackageCommandRuntime, handle_package_command};
use notagent::utils::version_check::set_latest_version_url_for_tests;
use support::{CannedResponse, TestServer};

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[derive(Default)]
struct RecordingConsole {
    stdout: Mutex<Vec<String>>,
    stderr: Mutex<Vec<String>>,
}

impl RecordingConsole {
    fn stdout(&self) -> String {
        self.stdout.lock().unwrap().join("\n")
    }

    fn stderr(&self) -> String {
        self.stderr.lock().unwrap().join("\n")
    }
}

impl ConsoleIo for RecordingConsole {
    fn log(&self, message: &str) {
        self.stdout.lock().unwrap().push(message.to_owned());
    }

    fn error(&self, message: &str) {
        self.stderr.lock().unwrap().push(message.to_owned());
    }
}

/// Records what the command would have spawned.
#[derive(Default)]
struct RecordingRunner {
    runs: Mutex<Vec<(String, Vec<String>)>>,
}

impl RecordingRunner {
    fn runs(&self) -> Vec<(String, Vec<String>)> {
        self.runs.lock().unwrap().clone()
    }
}

impl CommandRunner for RecordingRunner {
    fn run(
        &self,
        command: String,
        args: Vec<String>,
        _options: CommandOptions,
    ) -> BoxFuture<'_, Result<(), String>> {
        self.runs.lock().unwrap().push((command, args));
        Box::pin(async { Ok(()) })
    }

    fn run_capture(
        &self,
        _command: String,
        _args: Vec<String>,
        _options: CommandOptions,
    ) -> BoxFuture<'_, Result<String, String>> {
        Box::pin(async { Ok(String::new()) })
    }

    fn run_sync(&self, _command: &str, _args: &[String]) -> Result<String, String> {
        Err("no global package manager in tests".to_owned())
    }
}

struct Fixture {
    _guard: MutexGuard<'static, ()>,
    _temp: tempfile::TempDir,
    previous_home: Option<String>,
    previous_offline: Option<String>,
    agent_dir: PathBuf,
    project_dir: PathBuf,
    package_dir: PathBuf,
    console: Arc<RecordingConsole>,
    runner: Arc<RecordingRunner>,
    runtime: PackageCommandRuntime,
}

impl Fixture {
    fn new() -> Self {
        let guard = env_lock().lock().unwrap_or_else(|error| error.into_inner());
        let previous_home = std::env::var("HOME").ok();
        let previous_offline = std::env::var("NOTAGENT_OFFLINE").ok();
        let temp = tempfile::tempdir().expect("tempdir");
        let agent_dir = temp.path().join("agent");
        let project_dir = temp.path().join("project");
        let package_dir = temp.path().join("local-package");
        for dir in [&agent_dir, &project_dir, &package_dir] {
            std::fs::create_dir_all(dir).expect("create dir");
        }
        unsafe {
            std::env::set_var("HOME", temp.path());
            std::env::remove_var("NOTAGENT_OFFLINE");
        }

        let console = Arc::new(RecordingConsole::default());
        let runner = Arc::new(RecordingRunner::default());
        let runtime = PackageCommandRuntime {
            cwd: project_dir.to_string_lossy().into_owned(),
            agent_dir: agent_dir.to_string_lossy().into_owned(),
            console: console.clone(),
            color: false,
            project_trust_context: ProjectTrustContext::default(),
            terminal_width: Some(80),
            install_env: InstallEnv {
                package_dir: temp.path().join("install"),
                exec_path: temp.path().join("install").join("notagent"),
                entrypoint: Some(temp.path().join("install").join("notagent")),
                standalone_binary: true,
            },
            command_runner: Some(runner.clone()),
        };

        Self {
            _guard: guard,
            _temp: temp,
            previous_home,
            previous_offline,
            agent_dir,
            project_dir,
            package_dir,
            console,
            runner,
            runtime,
        }
    }

    async fn run(&self, args: &[&str]) -> Option<i32> {
        let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
        handle_package_command(&args, &self.runtime).await
    }

    fn write(&self, path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create dir");
        }
        std::fs::write(path, content).expect("write");
    }

    fn global_settings(&self) -> serde_json::Value {
        let content =
            std::fs::read_to_string(self.agent_dir.join("settings.json")).expect("global settings");
        serde_json::from_str(&content).expect("settings json")
    }

    fn project_settings(&self) -> serde_json::Value {
        let content =
            std::fs::read_to_string(self.project_dir.join(".notagent").join("settings.json"))
                .expect("project settings");
        serde_json::from_str(&content).expect("settings json")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        set_latest_version_url_for_tests(None);
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

fn packages(settings: &serde_json::Value) -> Vec<String> {
    settings
        .get("packages")
        .and_then(serde_json::Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

// ============================================================================

#[tokio::test]
async fn persists_global_relative_local_package_paths_relative_to_settings_json() {
    let fixture = Fixture::new();
    let relative_package_dir = fixture.project_dir.join("packages").join("local-package");
    std::fs::create_dir_all(&relative_package_dir).expect("package dir");

    assert_eq!(
        fixture.run(&["install", "./packages/local-package"]).await,
        Some(0)
    );

    let stored = packages(&fixture.global_settings());
    assert_eq!(stored.len(), 1);
    let resolved_from_settings =
        std::fs::canonicalize(fixture.agent_dir.join(&stored[0])).expect("stored path");
    assert_eq!(
        resolved_from_settings,
        std::fs::canonicalize(&relative_package_dir).expect("package dir")
    );
}

#[tokio::test]
async fn removes_local_packages_using_a_path_with_a_trailing_slash() {
    let fixture = Fixture::new();
    let with_slash = format!("{}/", fixture.package_dir.to_string_lossy());

    assert_eq!(fixture.run(&["install", &with_slash]).await, Some(0));
    assert_eq!(packages(&fixture.global_settings()).len(), 1);

    assert_eq!(fixture.run(&["remove", &with_slash]).await, Some(0));
    assert_eq!(packages(&fixture.global_settings()).len(), 0);
}

#[tokio::test]
async fn skips_untrusted_project_package_settings() {
    let fixture = Fixture::new();
    fixture.write(
        &fixture.project_dir.join(".notagent").join("settings.json"),
        r#"{"packages":["npm:@project/pkg"]}"#,
    );

    assert_eq!(fixture.run(&["list"]).await, Some(0));

    let stdout = fixture.console.stdout();
    assert!(stdout.contains("No packages installed."));
    assert!(!stdout.contains("Project packages:"));
}

#[tokio::test]
async fn uses_remembered_project_trust_for_list() {
    let fixture = Fixture::new();
    fixture.write(
        &fixture.project_dir.join(".notagent").join("settings.json"),
        r#"{"packages":["npm:@project/pkg"]}"#,
    );
    ProjectTrustStore::new(&fixture.runtime.agent_dir)
        .set(&fixture.runtime.cwd, Some(true))
        .expect("trust store");

    assert_eq!(fixture.run(&["list"]).await, Some(0));

    let stdout = fixture.console.stdout();
    assert!(stdout.contains("Project packages:"));
    assert!(stdout.contains("npm:@project/pkg"));
    assert!(!stdout.contains("No packages installed."));
}

#[tokio::test]
async fn overrides_remembered_trust_for_list_with_no_approve() {
    let fixture = Fixture::new();
    fixture.write(
        &fixture.project_dir.join(".notagent").join("settings.json"),
        r#"{"packages":["npm:@project/pkg"]}"#,
    );
    ProjectTrustStore::new(&fixture.runtime.agent_dir)
        .set(&fixture.runtime.cwd, Some(true))
        .expect("trust store");

    assert_eq!(fixture.run(&["list", "--no-approve"]).await, Some(0));

    let stdout = fixture.console.stdout();
    assert!(stdout.contains("No packages installed."));
    assert!(!stdout.contains("Project packages:"));
}

#[tokio::test]
async fn approves_project_trust_for_list_with_approve() {
    let fixture = Fixture::new();
    fixture.write(
        &fixture.project_dir.join(".notagent").join("settings.json"),
        r#"{"packages":["npm:@project/pkg"]}"#,
    );

    assert_eq!(fixture.run(&["list", "--approve"]).await, Some(0));

    let stdout = fixture.console.stdout();
    assert!(stdout.contains("Project packages:"));
    assert!(stdout.contains("npm:@project/pkg"));
    assert!(!stdout.contains("No packages installed."));
}

#[tokio::test]
async fn uses_default_project_trust_for_list() {
    let fixture = Fixture::new();
    fixture.write(
        &fixture.agent_dir.join("settings.json"),
        r#"{"defaultProjectTrust":"always"}"#,
    );
    fixture.write(
        &fixture.project_dir.join(".notagent").join("settings.json"),
        r#"{"packages":["npm:@project/pkg"]}"#,
    );

    assert_eq!(fixture.run(&["list"]).await, Some(0));

    let stdout = fixture.console.stdout();
    assert!(stdout.contains("Project packages:"));
    assert!(stdout.contains("npm:@project/pkg"));
}

#[tokio::test]
async fn lets_the_trust_store_override_the_default_project_trust() {
    let fixture = Fixture::new();
    fixture.write(
        &fixture.agent_dir.join("settings.json"),
        r#"{"defaultProjectTrust":"always"}"#,
    );
    fixture.write(
        &fixture.project_dir.join(".notagent").join("settings.json"),
        r#"{"packages":["npm:@project/pkg"]}"#,
    );
    ProjectTrustStore::new(&fixture.runtime.agent_dir)
        .set(&fixture.runtime.cwd, Some(false))
        .expect("trust store");

    assert_eq!(fixture.run(&["list"]).await, Some(0));

    let stdout = fixture.console.stdout();
    assert!(stdout.contains("No packages installed."));
    assert!(!stdout.contains("Project packages:"));
}

#[tokio::test]
async fn does_not_ask_for_project_trust_during_update() {
    let fixture = Fixture::new();
    fixture.write(
        &fixture.agent_dir.join("settings.json"),
        r#"{"defaultProjectTrust":"always"}"#,
    );
    fixture.write(
        &fixture.project_dir.join(".notagent").join("settings.json"),
        r#"{"packages":["npm:fake-package"]}"#,
    );

    assert_eq!(fixture.run(&["update", "--extensions"]).await, Some(0));

    // Untrusted (update reads only the saved decision), so the project package
    // is never touched — `defaultProjectTrust: always` does not apply here.
    assert!(fixture.runner.runs().is_empty());
}

#[tokio::test]
async fn uses_saved_project_trust_during_update() {
    let fixture = Fixture::new();
    let installed_path = fixture
        .project_dir
        .join(".notagent")
        .join("npm")
        .join("node_modules")
        .join("fake-package");
    fixture.write(
        &installed_path.join("package.json"),
        r#"{"name":"fake-package","version":"1.0.0"}"#,
    );
    fixture.write(
        &fixture.project_dir.join(".notagent").join("settings.json"),
        r#"{"packages":["npm:fake-package"]}"#,
    );
    ProjectTrustStore::new(&fixture.runtime.agent_dir)
        .set(&fixture.runtime.cwd, Some(true))
        .expect("trust store");

    assert_eq!(fixture.run(&["update", "--extensions"]).await, Some(0));

    let runs = fixture.runner.runs();
    assert!(
        runs.iter()
            .any(|(command, args)| command == "npm" && args[0] == "install"),
        "{runs:?}"
    );
}

#[tokio::test]
async fn blocks_local_package_changes_when_the_project_is_untrusted() {
    let fixture = Fixture::new();
    fixture.write(
        &fixture.project_dir.join(".notagent").join("settings.json"),
        "{}",
    );

    assert_eq!(
        fixture.run(&["install", "-l", "./local-package"]).await,
        Some(1)
    );
    assert!(
        fixture
            .console
            .stderr()
            .contains("Project is not trusted. Use --approve to modify local package config.")
    );
}

#[tokio::test]
async fn allows_a_local_package_install_to_initialize_fresh_project_settings() {
    let fixture = Fixture::new();
    let package_dir = fixture.package_dir.to_string_lossy().into_owned();

    assert_eq!(fixture.run(&["install", "-l", &package_dir]).await, Some(0));

    let stored = packages(&fixture.project_settings());
    assert_eq!(stored.len(), 1);
    assert_eq!(
        std::fs::canonicalize(fixture.project_dir.join(".notagent").join(&stored[0]))
            .expect("stored path"),
        std::fs::canonicalize(&fixture.package_dir).expect("package dir")
    );
}

#[tokio::test]
async fn shows_the_install_subcommand_help() {
    let fixture = Fixture::new();

    assert_eq!(fixture.run(&["install", "--help"]).await, Some(0));

    let stdout = fixture.console.stdout();
    assert!(stdout.contains("Usage:"));
    assert!(stdout.contains("notagent install <source> [-l]"));
    assert!(fixture.console.stderr().is_empty());
}

#[tokio::test]
async fn rejects_update_models_combined_with_another_update_target() {
    let fixture = Fixture::new();

    assert_eq!(
        fixture.run(&["update", "--models", "--self"]).await,
        Some(1)
    );
    assert!(
        fixture
            .console
            .stderr()
            .contains("--models cannot be combined with --self")
    );
    assert!(fixture.runner.runs().is_empty());
}

#[tokio::test]
async fn shows_a_friendly_error_for_unknown_install_options() {
    let fixture = Fixture::new();

    assert_eq!(fixture.run(&["install", "--unknown"]).await, Some(1));

    let stderr = fixture.console.stderr();
    assert!(stderr.contains("Unknown option --unknown for \"install\"."));
    assert!(stderr.contains(
        "Use \"notagent --help\" or \"notagent install <source> [-l] [--approve|--no-approve]\"."
    ));
}

#[tokio::test]
async fn shows_a_friendly_error_for_a_missing_install_source() {
    let fixture = Fixture::new();

    assert_eq!(fixture.run(&["install"]).await, Some(1));

    let stderr = fixture.console.stderr();
    assert!(stderr.contains("Missing install source."));
    assert!(stderr.contains("Usage: notagent install <source> [-l]"));
    assert!(!stderr.contains("at "));
}

#[tokio::test]
async fn suggests_the_configured_source_when_the_update_input_omits_the_npm_prefix() {
    let fixture = Fixture::new();
    fixture.write(
        &fixture.agent_dir.join("settings.json"),
        r#"{"packages":["npm:notagent-formatter"]}"#,
    );

    assert_eq!(
        fixture.run(&["update", "notagent-formatter"]).await,
        Some(1)
    );

    assert!(
        fixture
            .console
            .stderr()
            .contains("Did you mean npm:notagent-formatter?")
    );
    assert!(
        !fixture
            .console
            .stdout()
            .contains("Updated notagent-formatter")
    );
    assert!(packages(&fixture.global_settings()).contains(&"npm:notagent-formatter".to_owned()));
}

/// The offline half of the self-update path: `getLatestPiRelease` returns
/// nothing, so the plan cannot be built and the command reports why. The
/// comment).
#[tokio::test]
async fn reports_that_the_latest_version_could_not_be_determined_when_offline() {
    let fixture = Fixture::new();
    unsafe { std::env::set_var("NOTAGENT_OFFLINE", "1") };

    assert_eq!(fixture.run(&["update", "--self"]).await, Some(1));

    assert!(
        fixture
            .console
            .stderr()
            .contains("Could not determine latest notagent version."),
        "{}",
        fixture.console.stderr()
    );
    assert!(fixture.runner.runs().is_empty());
}

#[tokio::test]
async fn a_homebrew_self_update_prints_the_command_without_running_it() {
    let mut fixture = Fixture::new();
    let server =
        TestServer::start(Vec::new(), CannedResponse::json(r#"{"version":"999.0.0"}"#)).await;
    set_latest_version_url_for_tests(Some(format!("{}/api/latest-version.json", server.base_url)));
    fixture.runtime.install_env = InstallEnv {
        package_dir: PathBuf::from("/opt/homebrew/Caskroom/notagent/0.1.46"),
        exec_path: PathBuf::from("/opt/homebrew/Caskroom/notagent/0.1.46/notagent"),
        entrypoint: Some(PathBuf::from("/opt/homebrew/bin/notagent")),
        standalone_binary: true,
    };

    assert_eq!(fixture.run(&["update", "--self"]).await, Some(0));
    assert!(
        fixture
            .console
            .stdout()
            .contains("Run: brew upgrade notagent"),
        "{}",
        fixture.console.stdout()
    );
    assert!(fixture.runner.runs().is_empty());
}

#[tokio::test]
async fn is_not_a_package_command_for_unrelated_arguments() {
    let fixture = Fixture::new();
    assert_eq!(fixture.run(&["--version"]).await, None);
    assert_eq!(fixture.run(&[]).await, None);
}

#[tokio::test]
async fn notes_that_extensions_are_skipped_without_an_update_target() {
    let fixture = Fixture::new();
    unsafe { std::env::set_var("NOTAGENT_OFFLINE", "1") };

    fixture.run(&["update"]).await;

    assert!(fixture.console.stdout().contains(
        "Extensions are skipped. Run notagent update --extensions to update extensions."
    ));
}
