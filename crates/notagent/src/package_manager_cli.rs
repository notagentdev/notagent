//! Port of `packages/coding-agent/src/package-manager-cli.ts` (889 LOC).
//!
//! The `install`, `remove`/`uninstall`, `update` and `list` sub-commands.
//!
//! Deviations:
//!
//! * Class 1 — `console.log`/`console.error` go through [`ConsoleIo`] and
//!   `process.exitCode` becomes the return value, so the ported suite can read
//!   both the way the TypeScript reads its `console` spies.
//! * Class 2 — the extension pre-pass of `createCommandSettingsManager`
//!   (`loadProjectTrustExtensions`) is gone with the extension system, and so is
//!   the Windows npm quarantine (`utils/windows-self-update.ts`), which
//!   protects native npm dependencies a Rust binary does not have.
//! * Class 3 — `chalk` becomes direct ANSI (master substitution); the colours
//!   are suppressed when the stream is not a terminal, which is what chalk's
//!   `supports-color` does.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use notagent_tui::components::markdown::{Markdown, MarkdownTheme};
use notagent_tui::tui::Component;
use tokio_util::sync::CancellationToken;

use crate::cli::config_selector::ConfigSelectorOptions;
use crate::config::{
    APP_NAME, CONFIG_DIR_NAME, InstallEnv, InstallMethod, PACKAGE_NAME, SelfUpdateCommand,
    SelfUpdatePackageTarget, VERSION, detect_install_method, get_agent_dir,
    get_self_update_command, get_self_update_unavailable_instruction,
};
use crate::core::model_runtime::{CreateModelRuntimeOptions, ModelRuntime};
use crate::core::package_manager::command_runner::{CommandRunner, ProcessCommandRunner};
use crate::core::package_manager::{
    DefaultPackageManager, PackageManagerOptions, ProgressEvent, ProgressKind,
};
use crate::core::project_trust::{
    ProjectTrustContext, ResolveProjectTrustedOptions, resolve_project_trusted,
};
use crate::core::settings_manager::{SettingsManager, SettingsManagerCreateOptions};
use crate::core::source_info::SourceScope;
use crate::core::trust_manager::ProjectTrustStore;
use crate::modes::interactive::components::config_selector::{
    ConfigWriteScope, ScopedResolvedPaths,
};
use crate::utils::version_check::{
    LatestPiRelease, VersionCheckOptions, get_latest_pi_release, is_newer_package_version,
};
use notagent_ai::models::ModelsRefreshOptions;

/// `console.log` / `console.error`.
pub trait ConsoleIo: Send + Sync {
    fn log(&self, message: &str);
    fn error(&self, message: &str);
}

/// The production console.
#[derive(Debug, Clone, Copy, Default)]
pub struct StdConsole;

impl ConsoleIo for StdConsole {
    fn log(&self, message: &str) {
        println!("{message}");
    }

    fn error(&self, message: &str) {
        eprintln!("{message}");
    }
}

/// `chalk`, reduced to the colours this file uses.
#[derive(Debug, Clone, Copy)]
pub struct Paint {
    enabled: bool,
}

impl Paint {
    pub fn new(enabled: bool) -> Self {
        Self { enabled }
    }

    fn wrap(self, open: &str, close: &str, text: &str) -> String {
        if self.enabled {
            format!("{open}{text}{close}")
        } else {
            text.to_owned()
        }
    }

    pub fn red(self, text: &str) -> String {
        self.wrap("\x1b[31m", "\x1b[39m", text)
    }

    pub fn green(self, text: &str) -> String {
        self.wrap("\x1b[32m", "\x1b[39m", text)
    }

    pub fn yellow(self, text: &str) -> String {
        self.wrap("\x1b[33m", "\x1b[39m", text)
    }

    pub fn cyan(self, text: &str) -> String {
        self.wrap("\x1b[36m", "\x1b[39m", text)
    }

    pub fn dim(self, text: &str) -> String {
        self.wrap("\x1b[2m", "\x1b[22m", text)
    }

    pub fn bold(self, text: &str) -> String {
        self.wrap("\x1b[1m", "\x1b[22m", text)
    }

    pub fn italic(self, text: &str) -> String {
        self.wrap("\x1b[3m", "\x1b[23m", text)
    }

    pub fn strikethrough(self, text: &str) -> String {
        self.wrap("\x1b[9m", "\x1b[29m", text)
    }

    pub fn underline(self, text: &str) -> String {
        self.wrap("\x1b[4m", "\x1b[24m", text)
    }
}

/// What the host hands the command: where it runs, how it prints, and the two
/// seams the TypeScript reaches through globals (`process`, the spawner).
pub struct PackageCommandRuntime {
    pub cwd: String,
    pub agent_dir: String,
    pub console: Arc<dyn ConsoleIo>,
    /// Whether ANSI colours are written (chalk's `supports-color`).
    pub color: bool,
    /// Whether there is a user to ask for project trust (`process.stdin.isTTY
    /// && process.stdout.isTTY` plus the prompt itself, which lives in C's
    /// `cli/project-trust.ts`).
    pub project_trust_context: ProjectTrustContext,
    /// Terminal width for the update note (`process.stdout.columns ?? 80`).
    pub terminal_width: Option<usize>,
    /// Where the running executable lives; decides the self-update command.
    pub install_env: InstallEnv,
    /// The spawn seam of the package manager and of the self-update.
    pub command_runner: Option<Arc<dyn CommandRunner>>,
}

impl PackageCommandRuntime {
    /// The runtime of a real CLI invocation.
    pub fn from_process() -> Self {
        use std::io::IsTerminal;
        Self {
            cwd: std::env::current_dir()
                .map(|cwd| cwd.to_string_lossy().into_owned())
                .unwrap_or_default(),
            agent_dir: get_agent_dir().to_string_lossy().into_owned(),
            console: Arc::new(StdConsole),
            color: std::io::stdout().is_terminal(),
            project_trust_context: ProjectTrustContext::default(),
            terminal_width: None,
            install_env: InstallEnv::current(),
            command_runner: None,
        }
    }

    fn paint(&self) -> Paint {
        Paint::new(self.color)
    }

    fn runner(&self) -> Arc<dyn CommandRunner> {
        self.command_runner
            .clone()
            .unwrap_or_else(|| Arc::new(ProcessCommandRunner))
    }
}

/// `PackageCommand`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageCommand {
    Install,
    Remove,
    Update,
    List,
}

/// `UpdateTarget`
#[derive(Debug, Clone, PartialEq, Eq)]
enum UpdateTarget {
    All,
    SelfUpdate,
    Extensions(Option<String>),
    Models,
}

/// `PackageCommandOptions`
#[derive(Debug, Clone, PartialEq, Eq)]
struct PackageCommandOptions {
    command: PackageCommand,
    source: Option<String>,
    update_target: Option<UpdateTarget>,
    show_extensions_skipped_note: bool,
    local: bool,
    force: bool,
    project_trust_override: Option<bool>,
    help: bool,
    invalid_option: Option<String>,
    invalid_argument: Option<String>,
    missing_option_value: Option<String>,
    conflicting_options: Option<String>,
}

/// `getPackageCommandUsage(command)`
fn get_package_command_usage(command: PackageCommand) -> String {
    match command {
        PackageCommand::Install => {
            format!("{APP_NAME} install <source> [-l] [--approve|--no-approve]")
        }
        PackageCommand::Remove => {
            format!("{APP_NAME} remove <source> [-l] [--approve|--no-approve]")
        }
        PackageCommand::Update => format!(
            "{APP_NAME} update [source|self|notagent] [--self|--extensions|--models|--all] [--extension <source>] [--approve|--no-approve] [--force]"
        ),
        PackageCommand::List => format!("{APP_NAME} list [--approve|--no-approve]"),
    }
}

/// `printPackageCommandHelp(command)`
fn print_package_command_help(command: PackageCommand, runtime: &PackageCommandRuntime) {
    let paint = runtime.paint();
    let usage = get_package_command_usage(command);
    let bold_usage = paint.bold("Usage:");
    let text = match command {
        PackageCommand::Install => format!(
            "{bold_usage}
  {usage}

Install a package and add it to settings.

Options:
  -l, --local       Install project-locally ({CONFIG_DIR_NAME}/settings.json)
  -a, --approve     Trust project-local files for this command
  -na, --no-approve Ignore project-local files for this command

Examples:
  {APP_NAME} install npm:@foo/bar
  {APP_NAME} install git:github.com/user/repo
  {APP_NAME} install git:git@github.com:user/repo
  {APP_NAME} install https://github.com/user/repo
  {APP_NAME} install ssh://git@github.com/user/repo
  {APP_NAME} install ./local/path
"
        ),
        PackageCommand::Remove => format!(
            "{bold_usage}
  {usage}

Remove a package and its source from settings.
Alias: {APP_NAME} uninstall <source> [-l]

Options:
  -l, --local       Remove from project settings ({CONFIG_DIR_NAME}/settings.json)
  -a, --approve     Trust project-local files for this command
  -na, --no-approve Ignore project-local files for this command

Examples:
  {APP_NAME} remove npm:@foo/bar
  {APP_NAME} uninstall npm:@foo/bar
"
        ),
        PackageCommand::Update => format!(
            "{bold_usage}
  {usage}

Update notagent, installed packages, or model catalogs.

Options:
  --self                  Update notagent only (default when no target is given)
  --extensions            Update installed packages only
  --models                Refresh model catalogs only
  --all                   Update notagent and installed packages
  --extension <source>    Update one package only
  -a, --approve           Trust project-local files for this command
  -na, --no-approve       Ignore project-local files for this command
  --force                 Reinstall notagent even if the current version is latest

Short forms:
  {APP_NAME} update                Update notagent only
  {APP_NAME} update --all          Update notagent and all extensions
  {APP_NAME} update --models       Refresh model catalogs only
  {APP_NAME} update <source>       Update one package
  {APP_NAME} update notagent             Update notagent only (self works as alias to notagent)
"
        ),
        PackageCommand::List => format!(
            "{bold_usage}
  {usage}

List installed packages from user and project settings.

Options:
  -a, --approve      Trust project-local files for this command
  -na, --no-approve  Ignore project-local files for this command
"
        ),
    };
    runtime.console.log(&text);
}

/// `parsePackageCommand(args)`
fn parse_package_command(args: &[String]) -> Option<PackageCommandOptions> {
    let (raw_command, rest) = args.split_first()?;
    let command = match raw_command.as_str() {
        "uninstall" | "remove" => PackageCommand::Remove,
        "install" => PackageCommand::Install,
        "update" => PackageCommand::Update,
        "list" => PackageCommand::List,
        _ => return None,
    };

    let mut local = false;
    let mut force = false;
    let mut project_trust_override: Option<bool> = None;
    let mut help = false;
    let mut invalid_option: Option<String> = None;
    let mut invalid_argument: Option<String> = None;
    let mut missing_option_value: Option<String> = None;
    let mut conflicting_options: Option<String> = None;
    let mut source: Option<String> = None;
    let mut self_flag = false;
    let mut extensions_flag = false;
    let mut models_flag = false;
    let mut all_flag = false;
    let mut extension_flag_source: Option<String> = None;

    let mut index = 0;
    while index < rest.len() {
        let arg = rest[index].as_str();
        index += 1;

        if arg == "-h" || arg == "--help" {
            help = true;
            continue;
        }

        // Each flag is valid for a subset of the commands; anywhere else it is
        // reported as an unknown option (first one wins).
        let flag_for = |flag: &mut bool, allowed: bool| {
            if allowed {
                *flag = true;
                None
            } else {
                Some(arg.to_owned())
            }
        };

        let flag_result = match arg {
            "-l" | "--local" => Some(flag_for(
                &mut local,
                matches!(command, PackageCommand::Install | PackageCommand::Remove),
            )),
            "--self" => Some(flag_for(&mut self_flag, command == PackageCommand::Update)),
            "--extensions" => Some(flag_for(
                &mut extensions_flag,
                command == PackageCommand::Update,
            )),
            "--models" => Some(flag_for(
                &mut models_flag,
                command == PackageCommand::Update,
            )),
            "--all" => Some(flag_for(&mut all_flag, command == PackageCommand::Update)),
            "--force" => Some(flag_for(&mut force, command == PackageCommand::Update)),
            _ => None,
        };
        if let Some(rejected) = flag_result {
            if let Some(rejected) = rejected {
                invalid_option = invalid_option.or(Some(rejected));
            }
            continue;
        }

        match arg {
            "--approve" | "-a" => {
                project_trust_override = Some(true);
                continue;
            }
            "--no-approve" | "-na" => {
                project_trust_override = Some(false);
                continue;
            }
            "--extension" => {
                if command != PackageCommand::Update {
                    invalid_option = invalid_option.or_else(|| Some(arg.to_owned()));
                    continue;
                }
                match rest.get(index) {
                    None => {
                        missing_option_value =
                            missing_option_value.or_else(|| Some(arg.to_owned()));
                    }
                    Some(value) if value.starts_with('-') => {
                        missing_option_value =
                            missing_option_value.or_else(|| Some(arg.to_owned()));
                    }
                    Some(value) => {
                        if extension_flag_source.is_some() {
                            conflicting_options = conflicting_options.or_else(|| {
                                Some("--extension can only be provided once".to_owned())
                            });
                        } else {
                            extension_flag_source = Some(value.clone());
                        }
                        index += 1;
                    }
                }
                continue;
            }
            _ => {}
        }

        if arg.starts_with('-') {
            invalid_option = invalid_option.or_else(|| Some(arg.to_owned()));
            continue;
        }

        if source.is_none() {
            source = Some(arg.to_owned());
        } else {
            invalid_argument = invalid_argument.or_else(|| Some(arg.to_owned()));
        }
    }

    let mut update_target: Option<UpdateTarget> = None;
    let mut show_extensions_skipped_note = false;
    if command == PackageCommand::Update {
        if all_flag
            && (self_flag || extensions_flag || models_flag || extension_flag_source.is_some())
        {
            conflicting_options = conflicting_options.or_else(|| {
                Some(
                    "--all cannot be combined with --self, --extensions, --models, or --extension"
                        .to_owned(),
                )
            });
        }
        if all_flag && source.is_some() {
            conflicting_options = conflicting_options
                .or_else(|| Some("--all cannot be combined with a positional source".to_owned()));
        }

        if models_flag {
            if self_flag || extensions_flag || all_flag || extension_flag_source.is_some() {
                conflicting_options = conflicting_options.or_else(|| {
                    Some(
                        "--models cannot be combined with --self, --extensions, --all, or --extension"
                            .to_owned(),
                    )
                });
            }
            if source.is_some() {
                conflicting_options = conflicting_options.or_else(|| {
                    Some("--models cannot be combined with a positional source".to_owned())
                });
            }
            update_target = Some(UpdateTarget::Models);
        } else if let Some(extension_source) = extension_flag_source.clone() {
            if self_flag || extensions_flag || all_flag {
                conflicting_options = conflicting_options.or_else(|| {
                    Some(
                        "--extension cannot be combined with --self, --extensions, or --all"
                            .to_owned(),
                    )
                });
            }
            if source.is_some() {
                conflicting_options = conflicting_options.or_else(|| {
                    Some("--extension cannot be combined with a positional source".to_owned())
                });
            }
            update_target = Some(UpdateTarget::Extensions(Some(extension_source)));
        } else if let Some(positional) = source.clone() {
            let source_is_self = positional == "self" || positional == "notagent";
            if source_is_self {
                update_target = Some(if extensions_flag {
                    UpdateTarget::All
                } else {
                    UpdateTarget::SelfUpdate
                });
            } else {
                if extensions_flag || self_flag || all_flag {
                    conflicting_options = conflicting_options.or_else(|| {
                        Some(
                            "positional update targets cannot be combined with --self, --extensions, or --all"
                                .to_owned(),
                        )
                    });
                }
                update_target = Some(UpdateTarget::Extensions(Some(positional)));
            }
        // Two branches in the TypeScript (`--all`, and `--self --extensions`),
        // one here — they set the same target.
        } else if all_flag || (self_flag && extensions_flag) {
            update_target = Some(UpdateTarget::All);
        } else if self_flag {
            update_target = Some(UpdateTarget::SelfUpdate);
        } else if extensions_flag {
            update_target = Some(UpdateTarget::Extensions(None));
        } else {
            update_target = Some(UpdateTarget::SelfUpdate);
            show_extensions_skipped_note = true;
        }
    }

    Some(PackageCommandOptions {
        command,
        source,
        update_target,
        show_extensions_skipped_note,
        local,
        force,
        project_trust_override,
        help,
        invalid_option,
        invalid_argument,
        missing_option_value,
        conflicting_options,
    })
}

/// `updateTargetIncludesSelf(target)`
fn update_target_includes_self(target: &UpdateTarget) -> bool {
    matches!(target, UpdateTarget::All | UpdateTarget::SelfUpdate)
}

/// `updateTargetIncludesExtensions(target)`
fn update_target_includes_extensions(target: &UpdateTarget) -> bool {
    matches!(target, UpdateTarget::All | UpdateTarget::Extensions(_))
}

/// `reportSettingsErrors(settingsManager, context)`
fn report_settings_errors(
    settings_manager: &SettingsManager,
    context: &str,
    runtime: &PackageCommandRuntime,
) {
    let paint = runtime.paint();
    for error in settings_manager.drain_errors() {
        let scope = match error.scope {
            crate::core::settings_manager::SettingsScope::Global => "global",
            crate::core::settings_manager::SettingsScope::Project => "project",
        };
        runtime.console.error(&paint.yellow(&format!(
            "Warning ({context}, {scope} settings): {}",
            error.message
        )));
    }
}

/// `refreshModelCatalogs(agentDir)`
async fn refresh_model_catalogs(
    agent_dir: &str,
    runtime: &PackageCommandRuntime,
) -> Result<(), String> {
    let token = CancellationToken::new();
    let timeout_token = token.clone();
    let timeout = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(15)).await;
        timeout_token.cancel();
    });

    let result = async {
        let model_runtime = ModelRuntime::create(CreateModelRuntimeOptions {
            auth_path: Some(join(agent_dir, "auth.json")),
            models_path: Some(Some(join(agent_dir, "models.json"))),
            allow_model_network: Some(false),
            signal: Some(token.clone()),
            ..CreateModelRuntimeOptions::default()
        })
        .await?;
        // `handlePackageCommand(args, { extensionFactories })` (`main.ts:663`)
        // hands the built-in extensions to the package command as well, so the
        // llama.cpp catalog is part of `update --models` there too.
        let llama = crate::core::llama::provider::create_llama_provider();
        let _ = model_runtime.register_native_provider(std::sync::Arc::clone(&llama.provider)
            as std::sync::Arc<dyn notagent_ai::models::Provider>);
        let result = model_runtime
            .refresh(ModelsRefreshOptions {
                allow_network: Some(true),
                force: Some(true),
                signal: Some(token.clone()),
                providers: None,
            })
            .await;
        if result.aborted {
            return Err("Model catalog refresh timed out.".to_owned());
        }
        if !result.errors.is_empty() {
            let details = result
                .errors
                .iter()
                .map(|(provider, error)| format!("{provider}: {error}"))
                .collect::<Vec<_>>()
                .join("; ");
            return Err(format!("Could not refresh model catalogs: {details}"));
        }
        Ok(())
    }
    .await;
    timeout.abort();
    result?;

    runtime
        .console
        .log(&runtime.paint().green("Model catalogs refreshed"));
    Ok(())
}

/// `printSelfUpdateUnavailable(npmCommand, updatePackageTarget)`
fn print_self_update_unavailable(
    runtime: &PackageCommandRuntime,
    npm_command: Option<&[String]>,
    target: &SelfUpdatePackageTarget,
) {
    runtime.console.error(&format!(
        "error: {APP_NAME} cannot self-update this installation."
    ));
    match get_self_update_unavailable_instruction(
        &runtime.install_env,
        PACKAGE_NAME,
        npm_command,
        target,
    ) {
        Ok(instruction) => runtime.console.error(&instruction),
        Err(error) => runtime.console.error(&error),
    }

    if let Some(entrypoint) = runtime.install_env.entrypoint.as_ref() {
        runtime.console.error("");
        runtime.console.error(&format!(
            "Location of notagent executable: {}",
            entrypoint.display()
        ));
    }
}

/// `SELF_UPDATE_NOTE_MARKDOWN_THEME`
fn self_update_note_markdown_theme(paint: Paint) -> MarkdownTheme {
    use std::rc::Rc;
    MarkdownTheme {
        heading: Rc::new(move |text: &str| paint.bold(&paint.yellow(text))),
        link: Rc::new(move |text: &str| paint.cyan(text)),
        link_url: Rc::new(move |text: &str| paint.dim(text)),
        code: Rc::new(move |text: &str| paint.yellow(text)),
        code_block: Rc::new(move |text: &str| paint.dim(text)),
        code_block_border: Rc::new(move |text: &str| paint.dim(text)),
        quote: Rc::new(move |text: &str| paint.dim(text)),
        quote_border: Rc::new(move |text: &str| paint.dim(text)),
        hr: Rc::new(move |text: &str| paint.dim(text)),
        list_bullet: Rc::new(move |text: &str| paint.yellow(text)),
        bold: Rc::new(move |text: &str| paint.bold(text)),
        italic: Rc::new(move |text: &str| paint.italic(text)),
        strikethrough: Rc::new(move |text: &str| paint.strikethrough(text)),
        underline: Rc::new(move |text: &str| paint.underline(text)),
        highlight_code: None,
        code_block_indent: None,
    }
}

/// `printSelfUpdateNote(note)`
fn print_self_update_note(note: &str, runtime: &PackageCommandRuntime) {
    let trimmed_note = note.trim();
    if trimmed_note.is_empty() {
        return;
    }

    let paint = runtime.paint();
    runtime.console.log("");
    runtime
        .console
        .log(&paint.bold(&paint.yellow("Update note")));
    let width = runtime.terminal_width.unwrap_or(80).max(20);
    let rendered = Markdown::new(
        trimmed_note,
        0,
        0,
        self_update_note_markdown_theme(paint),
        None,
        None,
    )
    .render(width)
    .iter()
    .map(|line| line.trim_end().to_owned())
    .collect::<Vec<_>>()
    .join("\n");
    runtime.console.log(&rendered);
    runtime.console.log("");
}

/// `SelfUpdatePlan`
struct SelfUpdatePlan {
    package_name: String,
    install_spec: String,
    version: String,
    should_run: bool,
    note: Option<String>,
}

/// `getSelfUpdatePlan(force)`
async fn get_self_update_plan(
    force: bool,
    runtime: &PackageCommandRuntime,
) -> Result<SelfUpdatePlan, String> {
    let latest_release: Option<LatestPiRelease> = get_latest_pi_release(
        VERSION,
        VersionCheckOptions {
            retry: true,
            ..VersionCheckOptions::default()
        },
    )
    .await
    .map_err(|error| format!("Could not determine latest {APP_NAME} version: {error}"))?;
    let Some(latest_release) = latest_release else {
        return Err(format!("Could not determine latest {APP_NAME} version."));
    };

    let package_name = latest_release
        .package_name
        .clone()
        .unwrap_or_else(|| PACKAGE_NAME.to_owned());
    let install_spec = format!("{package_name}@{}", latest_release.version);
    if force
        || package_name != PACKAGE_NAME
        || is_newer_package_version(&latest_release.version, VERSION)
    {
        return Ok(SelfUpdatePlan {
            package_name,
            install_spec,
            version: latest_release.version,
            note: latest_release.note,
            should_run: true,
        });
    }

    runtime.console.log(
        &runtime
            .paint()
            .green(&format!("{APP_NAME} is already up to date (v{VERSION})")),
    );
    Ok(SelfUpdatePlan {
        package_name,
        install_spec,
        version: latest_release.version,
        should_run: false,
        note: None,
    })
}

/// `runSelfUpdate(command)`
async fn run_self_update(
    command: &SelfUpdateCommand,
    runtime: &PackageCommandRuntime,
) -> Result<(), String> {
    runtime.console.log(
        &runtime
            .paint()
            .dim(&format!("Updating {APP_NAME} with {}...", command.display)),
    );
    let runner = runtime.runner();
    let steps = command.steps.clone().unwrap_or_else(|| {
        vec![crate::config::SelfUpdateCommandStep {
            command: command.command.clone(),
            args: command.args.clone(),
            display: command.display.clone(),
        }]
    });
    for step in steps {
        runner
            .run(
                step.command.clone(),
                step.args.clone(),
                crate::core::package_manager::command_runner::CommandOptions::default(),
            )
            .await
            .map_err(|error| format!("{}: {error}", step.display))?;
    }
    Ok(())
}

/// `createCommandSettingsManager(options)`
///
/// Deviation (class 2): the extension pre-pass that let extensions answer the
/// trust question is gone; everything else — saved trust, `--approve`,
/// `defaultProjectTrust`, the prompt — is unchanged.
async fn create_command_settings_manager(
    runtime: &PackageCommandRuntime,
    project_trust_override: Option<bool>,
    use_saved_project_trust_only: bool,
) -> (Arc<SettingsManager>, Vec<String>) {
    let settings_manager = Arc::new(SettingsManager::create(
        Path::new(&runtime.cwd),
        Some(Path::new(&runtime.agent_dir)),
        SettingsManagerCreateOptions {
            project_trusted: Some(false),
        },
    ));
    let mut project_trust_warnings: Vec<String> = Vec::new();
    let trust_store = ProjectTrustStore::new(&runtime.agent_dir);

    if use_saved_project_trust_only {
        let saved_project_trusted = trust_store.get(&runtime.cwd).ok().flatten() == Some(true);
        settings_manager
            .set_project_trusted(project_trust_override.unwrap_or(saved_project_trusted));
        return (settings_manager, project_trust_warnings);
    }

    let project_trusted = resolve_project_trusted(ResolveProjectTrustedOptions {
        cwd: &runtime.cwd,
        trust_store: &trust_store,
        trust_override: project_trust_override,
        default_project_trust: Some(settings_manager.get_default_project_trust()),
        project_trust_context: runtime.project_trust_context.clone(),
    })
    .await
    .unwrap_or_else(|error| {
        project_trust_warnings.push(error.0);
        false
    });
    settings_manager.set_project_trusted(project_trusted);
    (settings_manager, project_trust_warnings)
}

fn report_project_trust_warnings(warnings: &[String], runtime: &PackageCommandRuntime) {
    let paint = runtime.paint();
    for warning in warnings {
        runtime
            .console
            .error(&paint.yellow(&format!("Warning: {warning}")));
    }
}

fn join(base: &str, part: &str) -> String {
    PathBuf::from(base)
        .join(part)
        .to_string_lossy()
        .into_owned()
}

const CONFIG_COMMAND_USAGE_SUFFIX: &str = " config [-l] [--approve|--no-approve]";

fn config_command_usage() -> String {
    format!("{APP_NAME}{CONFIG_COMMAND_USAGE_SUFFIX}")
}

/// `printConfigCommandHelp()`
fn print_config_command_help(runtime: &PackageCommandRuntime) {
    let paint = runtime.paint();
    runtime.console.log(&format!(
        "{}\n  {}\n\nOpen the resource configuration TUI to enable or disable package resources.\nWithout -l, starts in global settings (~/{CONFIG_DIR_NAME}/agent/settings.json).\nPress Tab in the TUI to switch between global and project-local modes.\n\nOptions:\n  -l, --local       Edit project overrides ({CONFIG_DIR_NAME}/settings.json)\n  -a, --approve     Trust project-local files for this command with -l\n  -na, --no-approve Ignore project-local files for this command with -l\n",
        paint.bold("Usage:"),
        config_command_usage(),
    ));
}

/// `handleConfigCommand(args, runtimeOptions)`
///
/// `None` means the arguments are not the config command (the TypeScript's
/// `false`); otherwise the process exit code. TypeScript ends the successful
/// run with `process.exit(0)` from inside the dialog callback; the port returns
/// the code so the caller's render loop unwinds first (deviation class 1).
pub async fn handle_config_command(
    args: &[String],
    runtime: &PackageCommandRuntime,
) -> Option<i32> {
    let (command, rest) = args.split_first()?;
    if command != "config" {
        return None;
    }
    let paint = runtime.paint();
    let console = runtime.console.clone();

    if rest.iter().any(|arg| arg == "-h" || arg == "--help") {
        print_config_command_help(runtime);
        return Some(0);
    }

    let mut local = false;
    let mut project_trust_override: Option<bool> = None;
    for arg in rest {
        match arg.as_str() {
            "-l" | "--local" => local = true,
            "-a" | "--approve" => project_trust_override = Some(true),
            "-na" | "--no-approve" => project_trust_override = Some(false),
            arg if arg.starts_with('-') => {
                console.error(&paint.red(&format!("Unknown option {arg} for \"config\".")));
                console.error(&paint.dim(&format!(
                    "Use \"{APP_NAME} --help\" or \"{}\".",
                    config_command_usage()
                )));
                return Some(1);
            }
            arg => {
                console.error(&paint.red(&format!("Unexpected argument {arg}.")));
                console.error(&paint.dim(&format!("Usage: {}", config_command_usage())));
                return Some(1);
            }
        }
    }

    let (settings_manager, project_trust_warnings) =
        create_command_settings_manager(runtime, project_trust_override, false).await;
    report_project_trust_warnings(&project_trust_warnings, runtime);
    if local && !settings_manager.is_project_trusted() {
        console.error(
            &paint.red("Project is not trusted. Use --approve to modify local resource config."),
        );
        return Some(1);
    }
    report_settings_errors(&settings_manager, "config command", runtime);

    let global_settings_manager = Arc::new(SettingsManager::create(
        Path::new(&runtime.cwd),
        Some(Path::new(&runtime.agent_dir)),
        SettingsManagerCreateOptions {
            project_trusted: Some(false),
        },
    ));
    let global_resolved_paths = DefaultPackageManager::new(PackageManagerOptions {
        cwd: runtime.cwd.clone(),
        agent_dir: runtime.agent_dir.clone(),
        settings_manager: global_settings_manager,
        command_runner: runtime.command_runner.clone(),
    })
    .resolve(None)
    .await
    .unwrap_or_default();
    let project_resolved_paths = if settings_manager.is_project_trusted() {
        DefaultPackageManager::new(PackageManagerOptions {
            cwd: runtime.cwd.clone(),
            agent_dir: runtime.agent_dir.clone(),
            settings_manager: Arc::clone(&settings_manager),
            command_runner: runtime.command_runner.clone(),
        })
        .resolve(None)
        .await
        .unwrap_or_default()
    } else {
        global_resolved_paths.clone()
    };

    let project_mode_available = settings_manager.is_project_trusted();
    crate::cli::config_selector::select_config(ConfigSelectorOptions {
        resolved_paths: ScopedResolvedPaths {
            global: global_resolved_paths,
            project: project_resolved_paths,
        },
        settings_manager,
        cwd: &runtime.cwd,
        agent_dir: &runtime.agent_dir,
        write_scope: if local {
            ConfigWriteScope::Project
        } else {
            ConfigWriteScope::Global
        },
        project_mode_available,
    })
    .await;
    Some(0)
}

/// `handlePackageCommand(args, runtimeOptions)`
///
/// `None` means the arguments are not a package command (the TypeScript's
/// `false`); otherwise the process exit code (`0` where the TypeScript leaves
/// `process.exitCode` untouched).
pub async fn handle_package_command(
    args: &[String],
    runtime: &PackageCommandRuntime,
) -> Option<i32> {
    let options = parse_package_command(args)?;
    let paint = runtime.paint();
    let console = runtime.console.clone();

    if options.help {
        print_package_command_help(options.command, runtime);
        return Some(0);
    }

    let usage = get_package_command_usage(options.command);
    let command_name = match options.command {
        PackageCommand::Install => "install",
        PackageCommand::Remove => "remove",
        PackageCommand::Update => "update",
        PackageCommand::List => "list",
    };

    if let Some(invalid_option) = options.invalid_option.as_ref() {
        console.error(&paint.red(&format!(
            "Unknown option {invalid_option} for \"{command_name}\"."
        )));
        console.error(&paint.dim(&format!("Use \"{APP_NAME} --help\" or \"{usage}\".")));
        return Some(1);
    }

    if let Some(missing_option_value) = options.missing_option_value.as_ref() {
        console.error(&paint.red(&format!("Missing value for {missing_option_value}.")));
        console.error(&paint.dim(&format!("Usage: {usage}")));
        return Some(1);
    }

    if let Some(invalid_argument) = options.invalid_argument.as_ref() {
        console.error(&paint.red(&format!("Unexpected argument {invalid_argument}.")));
        console.error(&paint.dim(&format!("Usage: {usage}")));
        return Some(1);
    }

    if let Some(conflicting_options) = options.conflicting_options.as_ref() {
        console.error(&paint.red(conflicting_options));
        console.error(&paint.dim(&format!("Usage: {usage}")));
        return Some(1);
    }

    let source = options.source.clone();
    if matches!(
        options.command,
        PackageCommand::Install | PackageCommand::Remove
    ) && source.is_none()
    {
        console.error(&paint.red(&format!("Missing {command_name} source.")));
        console.error(&paint.dim(&format!("Usage: {usage}")));
        return Some(1);
    }

    if options.command == PackageCommand::Update
        && options.update_target == Some(UpdateTarget::Models)
    {
        return Some(
            match refresh_model_catalogs(&runtime.agent_dir, runtime).await {
                Ok(()) => 0,
                Err(message) => {
                    console.error(&paint.red(&format!("Error: {message}")));
                    1
                }
            },
        );
    }

    let writes_project_package_config = matches!(
        options.command,
        PackageCommand::Install | PackageCommand::Remove
    ) && options.local;
    let (settings_manager, project_trust_warnings) = create_command_settings_manager(
        runtime,
        options.project_trust_override,
        options.command == PackageCommand::Update,
    )
    .await;
    report_project_trust_warnings(&project_trust_warnings, runtime);
    if !settings_manager.is_project_trusted() && writes_project_package_config {
        console.error(
            &paint.red("Project is not trusted. Use --approve to modify local package config."),
        );
        return Some(1);
    }
    report_settings_errors(&settings_manager, "package command", runtime);
    let self_update_npm_command = settings_manager.get_global_settings().npm_command.clone();

    let package_manager = DefaultPackageManager::new(PackageManagerOptions {
        cwd: runtime.cwd.clone(),
        agent_dir: runtime.agent_dir.clone(),
        settings_manager: settings_manager.clone(),
        command_runner: Some(runtime.runner()),
    });

    let progress_console = console.clone();
    package_manager.set_progress_callback(Some(Arc::new(move |event: &ProgressEvent| {
        if event.kind == ProgressKind::Start
            && let Some(message) = event.message.as_ref()
        {
            progress_console.log(&paint.dim(message));
        }
    })));

    match options.command {
        PackageCommand::Install => {
            let source = source.expect("checked above");
            match package_manager
                .install_and_persist(&source, options.local)
                .await
            {
                Ok(()) => {
                    console.log(&paint.green(&format!("Installed {source}")));
                    Some(0)
                }
                Err(error) => Some(report_command_error(&error.0, runtime)),
            }
        }

        PackageCommand::Remove => {
            let source = source.expect("checked above");
            match package_manager
                .remove_and_persist(&source, options.local)
                .await
            {
                Ok(true) => {
                    console.log(&paint.green(&format!("Removed {source}")));
                    Some(0)
                }
                Ok(false) => {
                    console.error(&paint.red(&format!("No matching package found for {source}")));
                    Some(1)
                }
                Err(error) => Some(report_command_error(&error.0, runtime)),
            }
        }

        PackageCommand::List => {
            let configured_packages = match package_manager.list_configured_packages() {
                Ok(packages) => packages,
                Err(error) => return Some(report_command_error(&error.0, runtime)),
            };
            let user_packages: Vec<_> = configured_packages
                .iter()
                .filter(|package| package.scope == SourceScope::User)
                .collect();
            let project_packages: Vec<_> = configured_packages
                .iter()
                .filter(|package| package.scope == SourceScope::Project)
                .collect();

            if configured_packages.is_empty() {
                console.log(&paint.dim("No packages installed."));
                return Some(0);
            }

            let format_package = |package: &crate::core::package_manager::ConfiguredPackage| {
                let display = if package.filtered {
                    format!("{} (filtered)", package.source)
                } else {
                    package.source.clone()
                };
                console.log(&format!("  {display}"));
                if let Some(installed_path) = package.installed_path.as_ref() {
                    console.log(&paint.dim(&format!("    {installed_path}")));
                }
            };

            if !user_packages.is_empty() {
                console.log(&paint.bold("User packages:"));
                for package in &user_packages {
                    format_package(package);
                }
            }

            if !project_packages.is_empty() {
                if !user_packages.is_empty() {
                    console.log("");
                }
                console.log(&paint.bold("Project packages:"));
                for package in &project_packages {
                    format_package(package);
                }
            }

            Some(0)
        }

        PackageCommand::Update => {
            let target = options
                .update_target
                .clone()
                .unwrap_or(UpdateTarget::SelfUpdate);
            if options.show_extensions_skipped_note {
                console.log(&paint.dim(&format!(
                    "Extensions are skipped. Run {APP_NAME} update --extensions to update extensions."
                )));
            }
            if update_target_includes_extensions(&target) {
                let update_source = match &target {
                    UpdateTarget::Extensions(source) => source.clone(),
                    _ => None,
                };
                if let Err(error) = package_manager.update(update_source.as_deref()).await {
                    return Some(report_command_error(&error.0, runtime));
                }
                match update_source.as_ref() {
                    Some(update_source) => {
                        console.log(&paint.green(&format!("Updated {update_source}")));
                    }
                    None => console.log(&paint.green("Updated packages")),
                }
            }
            if update_target_includes_self(&target) {
                let self_update_plan = match get_self_update_plan(options.force, runtime).await {
                    Ok(plan) => plan,
                    Err(message) => return Some(report_command_error(&message, runtime)),
                };
                if !self_update_plan.should_run {
                    return Some(0);
                }
                let install_method = detect_install_method(&runtime.install_env);
                if cfg!(windows)
                    && install_method != InstallMethod::Npm
                    && install_method != InstallMethod::Pnpm
                {
                    console.error(&paint.red(&format!(
                        "{APP_NAME} self-update on Windows is only supported for npm and pnpm installs."
                    )));
                    console.error(&paint.dim(&format!(
                        // `binary` instead of the TypeScript's `bun-binary`,
                        // the name `config.rs` gives the standalone install.
                        "Detected install method: {}. Update {APP_NAME} manually.",
                        install_method.as_str()
                    )));
                    return Some(1);
                }
                let self_update_target = SelfUpdatePackageTarget {
                    package_name: self_update_plan.package_name.clone(),
                    install_spec: self_update_plan.install_spec.clone(),
                };
                let self_update_command = match get_self_update_command(
                    &runtime.install_env,
                    PACKAGE_NAME,
                    self_update_npm_command.as_deref(),
                    &self_update_target,
                ) {
                    Ok(Some(command)) => command,
                    Ok(None) | Err(_) => {
                        print_self_update_unavailable(
                            runtime,
                            self_update_npm_command.as_deref(),
                            &self_update_target,
                        );
                        return Some(1);
                    }
                };
                if let Some(note) = self_update_plan.note.as_ref() {
                    print_self_update_note(note, runtime);
                }
                if let Err(message) = run_self_update(&self_update_command, runtime).await {
                    console.error(&paint.red(&format!("Error: {message}")));
                    if install_method == InstallMethod::Pnpm {
                        console.error(&paint.yellow(
                            "If pnpm reports missing package versions, its cached registry metadata may be stale.",
                        ));
                        console.error(&paint.yellow(&format!(
                            "Run `pnpm store prune` and retry `{APP_NAME} update --self`."
                        )));
                    }
                    console.error(&paint.dim(&format!(
                        "If this keeps failing, run this command yourself: {}",
                        self_update_command.display
                    )));
                    return Some(1);
                }
                console.log(&paint.green(&format!(
                    "Updated {APP_NAME} from {VERSION} to {}",
                    self_update_plan.version
                )));
            }
            Some(0)
        }
    }
}

/// The `catch` around the command switch.
fn report_command_error(message: &str, runtime: &PackageCommandRuntime) -> i32 {
    runtime
        .console
        .error(&runtime.paint().red(&format!("Error: {message}")));
    1
}
