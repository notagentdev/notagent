//! Port of `packages/coding-agent/src/main.ts`.
//!
//! The entry point: parse the arguments, decide which mode the process is in,
//! build exactly the services that mode needs, and hand over.
//!
//! Two things happen earlier than one might expect, for the same reason. Hooks
//! are loaded before anything else runs, because `SessionStart` is one of them
//! and a hook that missed the event it was declared for is worse than no hook.
//! And the permission gate is built before the session it belongs to, so it sits
//! ahead of everything else that could approve a call — its dialog is bound
//! later, once there is a screen to ask on.
//!
//! Deviation (class 1): Rust reserves `src/main.rs` for the binary root, so the
//! module of `src/main.ts` lives in `src/main_app.rs`; `src/cli.rs` carries the
//! process setup of `src/cli.ts` and `src/bin/notagent.rs` is the binary root.
//!
//! Deviation (class 2): the extension factories, the extension flag values and
//! the extension load diagnostics are gone with the extension system
//! (`plans/facts/extension-boundary.md` §3). Permissions and hooks, which
//! TypeScript installs as two hidden inline extensions, are built here as native
//! objects and handed to the session.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use notagent_ai::models::models_are_equal;
use notagent_ai::types::ImageContent;

use crate::cli::args::{
    Args, ListModels, OutputMode, StartMode, help_text, parse_args, unknown_flags_error,
};
use crate::cli::auth_check::{
    AuthCheckReason, AuthCheckResult, AuthCheckStatus, auth_check_result_to_json,
    check_provider_auth, create_auth_check_model_runtime, get_provider_credential,
};
use crate::cli::auth_command::{
    AuthCommandError, AuthCommandKind, get_auth_command_name, get_auth_command_usage,
    is_auth_command_help, parse_auth_command, print_auth_command_help,
};
use crate::cli::credential_print::resolve_credential_for_print;
use crate::cli::file_processor::process_file_arguments;
use crate::cli::initial_message::{InitialMessageInput, build_initial_message};
use crate::cli::list_models::list_models;
use crate::cli::session_picker::{SessionChoice, select_session};
use crate::cli::startup_ui::{
    should_run_first_time_setup, show_first_time_setup, show_startup_selector,
};
use crate::config::{VERSION, expand_tilde_path, get_agent_dir};
use crate::core::agent_session::{AgentSession, ScopedModel as SessionScopedModel};
use crate::core::agent_session_runtime::{
    CreateAgentSessionRuntimeFactory, CreateAgentSessionRuntimeResult, create_agent_session_runtime,
};
use crate::core::agent_session_services::{
    AgentSessionRuntimeDiagnostic, CreateAgentSessionFromServicesOptions,
    CreateAgentSessionServicesOptions, DiagnosticLevel, create_agent_session_from_services,
    create_agent_session_services,
};
use crate::core::auth_guidance::format_no_models_available_message;
use crate::core::auth_storage::{AuthStorage, ReadOnlyAuthStorage};
use crate::core::export_html::{ExportOptions, export_from_file};
use crate::core::hooks::dispatch::{HookDispatcher, create_approval_observer};
use crate::core::hooks::load_hooks;
use crate::core::hooks::payload::HookSessionContext;
use crate::core::hooks::runtime::{HookReportLevel, HookReporter, HookRuntime, HookRuntimeOptions};
use crate::core::model_resolver::{resolve_cli_model, resolve_model_scope};
use crate::core::model_runtime::{CreateModelRuntimeOptions, ModelRuntime};
use crate::core::output_guard::{
    console_log, restore_stdout, stdin_is_tty, stdout_is_tty, stdout_write, take_over_stdout,
};
use crate::core::permissions::coordinator::ApprovalPresenter;
use crate::core::permissions::gate::{PermissionGate, PermissionGateOptions};
use crate::core::permissions::hook::PermissionSessionState;
use crate::core::project_trust::{
    AppMode, ProjectTrustContext, ResolveProjectTrustedOptions, resolve_project_trusted,
};
use crate::core::sdk::NoTools;
use crate::core::session_cwd::{
    format_missing_session_cwd_error, format_missing_session_cwd_prompt,
    get_missing_session_cwd_issue,
};
use crate::core::session_manager::{NewSessionOptions, SessionManager, assert_valid_session_id};
use crate::core::settings_manager::{SettingsManager, SettingsManagerCreateOptions};
use crate::core::timings::{print_timings, reset_timings, time};
use crate::core::trust_manager::{ProjectTrustStore, has_trust_requiring_project_resources};
use crate::migrations::run_migrations;
use notagent_tui::tui::run_until;

use crate::modes::interactive::interactive_mode::{
    InteractiveModeHandle, InteractiveModeOptions, create_interactive_mode,
};
use crate::modes::interactive::theme::theme::{init_theme, stop_theme_watcher};
use crate::modes::print_mode::{PrintModeOptions, PrintOutputMode, run_print_mode};
use crate::modes::rpc::rpc_mode::run_rpc_mode;
use crate::package_manager_cli::{
    PackageCommandRuntime, handle_config_command, handle_package_command,
};
use crate::utils::abort::timeout_signal;
use crate::utils::chalk::{dim, red, yellow};
use crate::utils::paths::{
    current_dir, is_local_path, normalize_path_default, resolve_path_default,
};

/// Run a startup dialog on this thread.
///
/// Deviation (class 1): the TUI is `!Send` like its TypeScript original, and the
/// binary runs on a multi-threaded runtime; a `LocalSet` gives the dialog the
/// single-threaded context Node's event loop provides for free.
async fn run_dialog<F: Future>(dialog: F) -> F::Output {
    tokio::task::LocalSet::new().run_until(dialog).await
}

/// Wall-clock ceiling for the model-runtime and model-scope work at startup.
const STARTUP_TIMEOUT_MS: u64 = 15_000;

fn is_truthy_env_flag(value: Option<&str>) -> bool {
    match value {
        None | Some("") => false,
        Some(value) => {
            value == "1" || value.to_lowercase() == "true" || value.to_lowercase() == "yes"
        }
    }
}

fn env_flag(name: &str) -> bool {
    is_truthy_env_flag(std::env::var(name).ok().as_deref())
}

fn collect_settings_diagnostics(
    settings_manager: &SettingsManager,
    context: &str,
) -> Vec<AgentSessionRuntimeDiagnostic> {
    settings_manager
        .drain_errors()
        .into_iter()
        .map(|error| AgentSessionRuntimeDiagnostic {
            level: DiagnosticLevel::Warning,
            message: format!(
                "({context}, {} settings) {}",
                match error.scope {
                    crate::core::settings_manager::SettingsScope::Global => "global",
                    crate::core::settings_manager::SettingsScope::Project => "project",
                },
                error.message
            ),
        })
        .collect()
}

fn report_diagnostics(diagnostics: &[AgentSessionRuntimeDiagnostic]) {
    for diagnostic in diagnostics {
        let message = match diagnostic.level {
            DiagnosticLevel::Error => red(&format!("Error: {}", diagnostic.message)),
            DiagnosticLevel::Warning => yellow(&format!("Warning: {}", diagnostic.message)),
            DiagnosticLevel::Info => dim(&diagnostic.message),
        };
        eprintln!("{message}");
    }
}

fn resolve_app_mode(parsed: &Args, stdin_is_tty: bool, stdout_is_tty: bool) -> AppMode {
    match parsed.mode {
        Some(OutputMode::Rpc) => AppMode::Rpc,
        Some(OutputMode::Json) => AppMode::Json,
        _ => {
            if parsed.print || !stdin_is_tty || !stdout_is_tty {
                AppMode::Print
            } else {
                AppMode::Interactive
            }
        }
    }
}

fn to_print_output_mode(app_mode: AppMode) -> PrintOutputMode {
    match app_mode {
        AppMode::Json => PrintOutputMode::Json,
        _ => PrintOutputMode::Text,
    }
}

/// `--help` and `--list-models` print and exit without a session; nothing of the
/// protocol modes applies to them, so standard output stays theirs.
fn is_plain_runtime_metadata_command(parsed: &Args) -> bool {
    !parsed.print && parsed.mode.is_none() && (parsed.help || parsed.list_models.is_some())
}

/// Runs `notagent auth …`. `None` means the arguments were not an auth command.
async fn run_auth_command(args: &[String]) -> Option<i32> {
    if is_auth_command_help(args) {
        print_auth_command_help();
        return Some(0);
    }

    let command = match parse_auth_command(args) {
        Ok(None) => return None,
        Ok(Some(command)) => command,
        Err(AuthCommandError(message)) => {
            eprintln!("{}", red(&format!("Error: {message}")));
            return Some(1);
        }
    };

    let parsed = parse_args(&command.args);
    if !parsed.unknown_flags.is_empty() {
        let option = parsed
            .unknown_flags
            .keys()
            .next()
            .cloned()
            .unwrap_or_default();
        eprintln!(
            "{}",
            red(&format!(
                "Unknown option --{option} for \"{}\".",
                get_auth_command_name(command.kind)
            ))
        );
        eprintln!(
            "{}",
            dim(&format!(
                "Use \"{} --help\" or \"{}\".",
                crate::config::APP_NAME,
                get_auth_command_usage(command.kind)
            ))
        );
        return Some(1);
    }

    let failure_code = if command.kind == AuthCommandKind::Check {
        2
    } else {
        1
    };
    let result = run_auth_command_inner(&parsed, &command).await;
    match result {
        Ok(exit_code) => Some(exit_code),
        Err(AuthCommandError(message)) => {
            eprintln!("{}", red(&format!("Error: {message}")));
            Some(failure_code)
        }
    }
}

async fn run_auth_command_inner(
    parsed: &Args,
    command: &crate::cli::auth_command::AuthCommand,
) -> Result<i32, AuthCommandError> {
    if !parsed.diagnostics.is_empty() {
        return Err(AuthCommandError(
            parsed
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.message.clone())
                .collect::<Vec<_>>()
                .join("\n"),
        ));
    }

    let signal = timeout_signal(STARTUP_TIMEOUT_MS);

    if command.kind != AuthCommandKind::Check {
        let model_runtime = ModelRuntime::create(CreateModelRuntimeOptions {
            allow_model_network: Some(false),
            signal: Some(signal.clone()),
            ..CreateModelRuntimeOptions::default()
        })
        .await
        .map_err(AuthCommandError)?;
        let credential = resolve_credential_for_print(
            parsed,
            &model_runtime,
            command.kind,
            command.min_expiry_ms,
            Some(signal),
        )
        .await?;
        stdout_write(&format!("{credential}\n"));
        return Ok(0);
    }

    let requested = crate::cli::auth_command::validate_auth_command_args(parsed, command.kind)?;
    let mut credential: Option<String> = None;
    let result = check_with_credentials(parsed, command, &mut credential)
        .await
        .unwrap_or_else(|| AuthCheckResult {
            status: AuthCheckStatus::Invalid,
            provider: requested
                .provider
                .clone()
                .or(requested.model.clone())
                .unwrap_or_default(),
            reason: Some(AuthCheckReason::InvalidState),
            auth_type: None,
        });

    let output = if command.json {
        auth_check_result_to_json(&result, credential.as_deref()).to_string()
    } else {
        credential
            .clone()
            .unwrap_or_else(|| result.status.as_str().to_owned())
    };
    stdout_write(&format!("{output}\n"));
    Ok(match result.status {
        AuthCheckStatus::Ready => 0,
        AuthCheckStatus::NotReady => 1,
        AuthCheckStatus::Invalid => 2,
    })
}

/// The `auth check` body. `None` stands for the TypeScript `catch` that turns
/// any failure below into `invalid`.
async fn check_with_credentials(
    parsed: &Args,
    command: &crate::cli::auth_command::AuthCommand,
    credential: &mut Option<String>,
) -> Option<AuthCheckResult> {
    let credentials: Arc<dyn notagent_ai::auth::types::CredentialStore> = if command.no_refresh {
        Arc::new(ReadOnlyAuthStorage::create_default().ok()?)
    } else {
        Arc::new(AuthStorage::create_default().ok()?)
    };
    let model_runtime = create_auth_check_model_runtime(Arc::clone(&credentials))
        .await
        .ok()?;
    let mut result = check_provider_auth(parsed, &model_runtime, !command.no_refresh)
        .await
        .ok()?;
    if command.credentials && result.status == AuthCheckStatus::Ready {
        *credential = get_provider_credential(
            &result.provider,
            &model_runtime,
            credentials.as_ref(),
            !command.no_refresh,
        )
        .await;
        if credential.is_none() {
            result = AuthCheckResult {
                status: AuthCheckStatus::NotReady,
                provider: result.provider,
                reason: Some(AuthCheckReason::CredentialNotAvailable),
                auth_type: None,
            };
        }
    }
    Some(result)
}

async fn prepare_initial_message(
    parsed: &mut Args,
    auto_resize_images: bool,
    stdin_content: Option<String>,
) -> (Option<String>, Option<Vec<ImageContent>>) {
    if parsed.file_args.is_empty() {
        let result = build_initial_message(
            parsed,
            InitialMessageInput {
                stdin_content,
                ..InitialMessageInput::default()
            },
        );
        return (result.initial_message, result.initial_images);
    }

    let file_args = parsed.file_args.clone();
    let processed = process_file_arguments(&file_args, auto_resize_images).await;
    let result = build_initial_message(
        parsed,
        InitialMessageInput {
            file_text: Some(processed.text),
            file_images: Some(processed.images),
            stdin_content,
        },
    );
    (result.initial_message, result.initial_images)
}

/// What a session argument resolved to.
enum ResolvedSession {
    /// A direct file path.
    Path(String),
    /// Found in the current project.
    Local(String),
    /// Found in a different project.
    Global {
        path: String,
        cwd: String,
    },
    NotFound(String),
}

async fn find_local_session_by_exact_id(
    session_id: &str,
    cwd: &str,
    session_dir: Option<&str>,
) -> Option<String> {
    SessionManager::list(cwd, session_dir, None)
        .await
        .into_iter()
        .find(|session| session.id == session_id)
        .map(|session| session.path)
}

/// A session argument is a path if it looks like one; otherwise it is matched as
/// an id prefix, first in this project and then across all of them.
async fn resolve_session_path(
    session_arg: &str,
    cwd: &str,
    session_dir: Option<&str>,
) -> ResolvedSession {
    if session_arg.contains('/') || session_arg.contains('\\') || session_arg.ends_with(".jsonl") {
        return ResolvedSession::Path(
            resolve_path_default(session_arg, cwd).unwrap_or_else(|_| session_arg.to_owned()),
        );
    }

    let local_sessions = SessionManager::list(cwd, session_dir, None).await;
    let local_match = local_sessions
        .iter()
        .find(|session| session.id == session_arg)
        .or_else(|| {
            local_sessions
                .iter()
                .find(|session| session.id.starts_with(session_arg))
        });
    if let Some(local_match) = local_match {
        return ResolvedSession::Local(local_match.path.clone());
    }

    let all_sessions = SessionManager::list_all(session_dir, None).await;
    let global_match = all_sessions
        .iter()
        .find(|session| session.id == session_arg)
        .or_else(|| {
            all_sessions
                .iter()
                .find(|session| session.id.starts_with(session_arg))
        });
    if let Some(global_match) = global_match {
        return ResolvedSession::Global {
            path: global_match.path.clone(),
            cwd: global_match.cwd.clone(),
        };
    }

    ResolvedSession::NotFound(session_arg.to_owned())
}

/// `y`/`yes` on standard input, anything else is a no.
fn prompt_confirm(message: &str) -> bool {
    stdout_write(&format!("{message} [y/N] "));
    let mut answer = String::new();
    if std::io::stdin().read_line(&mut answer).is_err() {
        return false;
    }
    let answer = answer.trim().to_lowercase();
    answer == "y" || answer == "yes"
}

/// Flag combinations that cannot be honoured together, as fatal errors.
fn validate_fork_flags(parsed: &Args) -> Result<(), String> {
    if parsed.fork.is_none() {
        return Ok(());
    }
    let conflicting: Vec<&str> = [
        parsed.session.is_some().then_some("--session"),
        parsed.continue_session.then_some("--continue"),
        parsed.resume.then_some("--resume"),
        parsed.no_session.then_some("--no-session"),
    ]
    .into_iter()
    .flatten()
    .collect();
    if conflicting.is_empty() {
        return Ok(());
    }
    Err(format!(
        "Error: --fork cannot be combined with {}",
        conflicting.join(", ")
    ))
}

fn validate_session_id_flags(parsed: &Args) -> Result<(), String> {
    let Some(session_id) = parsed.session_id.as_deref() else {
        return Ok(());
    };
    let conflicting: Vec<&str> = [
        parsed.session.is_some().then_some("--session"),
        parsed.continue_session.then_some("--continue"),
        parsed.resume.then_some("--resume"),
    ]
    .into_iter()
    .flatten()
    .collect();
    if !conflicting.is_empty() {
        return Err(format!(
            "Error: --session-id cannot be combined with {}",
            conflicting.join(", ")
        ));
    }
    assert_valid_session_id(session_id).map_err(|message| format!("Error: {message}"))
}

fn new_session_options(session_id: Option<&str>) -> Option<NewSessionOptions> {
    session_id.map(|id| NewSessionOptions {
        id: Some(id.to_owned()),
        ..NewSessionOptions::default()
    })
}

/// Opens, forks or creates the session the flags asked for.
async fn create_session_manager(
    parsed: &Args,
    cwd: &str,
    session_dir: Option<&str>,
    settings_manager: &SettingsManager,
) -> Result<SessionManager, String> {
    if parsed.no_session || parsed.help || parsed.list_models.is_some() {
        return SessionManager::in_memory(
            Some(cwd),
            new_session_options(parsed.session_id.as_deref()),
        )
        .map_err(|error| format!("Error: {error}"));
    }

    if let Some(fork) = parsed.fork.as_deref() {
        if let Some(session_id) = parsed.session_id.as_deref()
            && find_local_session_by_exact_id(session_id, cwd, session_dir)
                .await
                .is_some()
        {
            return Err(format!("Session already exists with id '{session_id}'"));
        }

        return match resolve_session_path(fork, cwd, session_dir).await {
            ResolvedSession::Path(path)
            | ResolvedSession::Local(path)
            | ResolvedSession::Global { path, .. } => SessionManager::fork_from(
                &path,
                cwd,
                session_dir,
                new_session_options(parsed.session_id.as_deref()),
            )
            .map_err(|error| format!("Error: {error}")),
            ResolvedSession::NotFound(arg) => Err(format!("No session found matching '{arg}'")),
        };
    }

    if let Some(session) = parsed.session.as_deref() {
        return match resolve_session_path(session, cwd, session_dir).await {
            ResolvedSession::Path(path) | ResolvedSession::Local(path) => {
                SessionManager::open(&path, session_dir, None)
                    .map_err(|error| format!("Error: {error}"))
            }
            ResolvedSession::Global {
                path,
                cwd: session_cwd,
            } => {
                console_log(&yellow(&format!(
                    "Session found in different project: {session_cwd}"
                )));
                if !prompt_confirm("Fork this session into current directory?") {
                    console_log(&dim("Aborted."));
                    return Err(String::new());
                }
                SessionManager::fork_from(&path, cwd, session_dir, None)
                    .map_err(|error| format!("Error: {error}"))
            }
            ResolvedSession::NotFound(arg) => Err(format!("No session found matching '{arg}'")),
        };
    }

    if parsed.resume {
        let choice = run_dialog(select_session(cwd, session_dir, settings_manager)).await;
        // `stopThemeWatcher()` in the `finally` of the TypeScript version: the
        // picker's theme watcher must not outlive the dialog.
        stop_theme_watcher();
        return match choice {
            SessionChoice::Selected(path) => SessionManager::open(&path, session_dir, None)
                .map_err(|error| format!("Error: {error}")),
            SessionChoice::Cancelled => {
                console_log(&dim("No session selected"));
                Err(String::new())
            }
            // `process.exit(0)` — Ctrl+C in the picker leaves without a word.
            SessionChoice::Exit => Err(String::new()),
        };
    }

    if parsed.continue_session {
        return SessionManager::continue_recent(cwd, session_dir)
            .map_err(|error| format!("Error: {error}"));
    }

    if let Some(session_id) = parsed.session_id.as_deref() {
        if let Some(path) = find_local_session_by_exact_id(session_id, cwd, session_dir).await {
            return SessionManager::open(&path, session_dir, None)
                .map_err(|error| format!("Error: {error}"));
        }
        eprintln!(
            "{}",
            yellow(&format!(
                "Warning: No project session found with id '{session_id}'; creating a new session with that id."
            ))
        );
    }

    SessionManager::create(
        cwd,
        session_dir,
        new_session_options(parsed.session_id.as_deref()),
    )
    .map_err(|error| format!("Error: {error}"))
}

/// What the CLI flags say about model, thinking level and tools.
struct SessionOptions {
    model: Option<notagent_ai::types::Model>,
    thinking_level: Option<notagent_agent::types::ThinkingLevel>,
    scoped_models: Vec<SessionScopedModel>,
    tools: Option<Vec<String>>,
    exclude_tools: Option<Vec<String>>,
    no_tools: Option<NoTools>,
}

fn build_session_options(
    parsed: &Args,
    scoped_models: &[crate::core::model_resolver::ScopedModel],
    has_existing_session: bool,
    model_runtime: &ModelRuntime,
    settings_manager: &SettingsManager,
) -> (SessionOptions, bool, Vec<AgentSessionRuntimeDiagnostic>) {
    let mut options = SessionOptions {
        model: None,
        thinking_level: None,
        scoped_models: Vec::new(),
        tools: None,
        exclude_tools: None,
        no_tools: None,
    };
    let mut diagnostics: Vec<AgentSessionRuntimeDiagnostic> = Vec::new();
    let mut cli_thinking_from_model = false;

    // `--provider <name> --model <pattern>` and `--model <provider>/<pattern>`.
    if let Some(cli_model) = parsed.model.as_deref() {
        let resolved = resolve_cli_model(
            parsed.provider.as_deref(),
            Some(cli_model),
            parsed.thinking,
            model_runtime,
        );
        if let Some(warning) = resolved.warning {
            diagnostics.push(AgentSessionRuntimeDiagnostic {
                level: DiagnosticLevel::Warning,
                message: warning,
            });
        }
        if let Some(error) = resolved.error {
            diagnostics.push(AgentSessionRuntimeDiagnostic {
                level: DiagnosticLevel::Error,
                message: error,
            });
        }
        if let Some(model) = resolved.model {
            options.model = Some(model);
            // `--model <pattern>:<thinking>` is a shorthand; an explicit
            // `--thinking` still wins, applied below.
            if parsed.thinking.is_none()
                && let Some(thinking_level) = resolved.thinking_level
            {
                options.thinking_level = Some(thinking_level);
                cli_thinking_from_model = true;
            }
        }
    }

    if options.model.is_none() && !scoped_models.is_empty() && !has_existing_session {
        let saved_provider = settings_manager.get_default_provider();
        let saved_model_id = settings_manager.get_default_model();
        let saved_model = match (saved_provider, saved_model_id) {
            (Some(provider), Some(model_id)) => model_runtime.get_model(&provider, &model_id),
            _ => None,
        };
        let saved_in_scope = saved_model.as_ref().and_then(|saved_model| {
            scoped_models
                .iter()
                .find(|scoped| models_are_equal(Some(&scoped.model), Some(saved_model)))
        });
        let chosen = saved_in_scope.unwrap_or(&scoped_models[0]);
        options.model = Some(chosen.model.clone());
        if parsed.thinking.is_none()
            && let Some(thinking_level) = chosen.thinking_level
        {
            options.thinking_level = Some(thinking_level);
        }
    }

    // An explicit `--thinking` wins over the scoped models' levels.
    if let Some(thinking) = parsed.thinking {
        options.thinking_level = Some(thinking);
    }

    // The cycling list keeps its level unset unless the pattern named one:
    // unset means "inherit the session's level" while cycling.
    options.scoped_models = scoped_models
        .iter()
        .map(|scoped| SessionScopedModel {
            model: scoped.model.clone(),
            thinking_level: scoped.thinking_level,
        })
        .collect();

    if parsed.no_tools {
        options.no_tools = Some(NoTools::All);
    } else if parsed.no_builtin_tools {
        options.no_tools = Some(NoTools::Builtin);
    }
    options.tools = parsed.tools.clone();
    options.exclude_tools = parsed.exclude_tools.clone();

    (options, cli_thinking_from_model, diagnostics)
}

fn resolve_cli_paths(cwd: &str, paths: Option<&Vec<String>>) -> Vec<String> {
    paths
        .map(|paths| {
            paths
                .iter()
                .map(|value| {
                    if is_local_path(value) {
                        resolve_path_default(value, cwd).unwrap_or_else(|_| value.clone())
                    } else {
                        value.clone()
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The whole run. Returns the process exit code.
pub async fn main(args: Vec<String>) -> i32 {
    reset_timings();

    // Permission enforcement is bound late: the mode lives on the session and
    // the dialog on the interactive mode, and neither exists yet. The gate is
    // built now so it sits ahead of everything else that could approve a call.
    let permission_session: Arc<Mutex<Option<Arc<AgentSession>>>> = Arc::new(Mutex::new(None));
    let permission_present: Arc<Mutex<Option<ApprovalPresenter>>> = Arc::new(Mutex::new(None));
    let hook_report: Arc<Mutex<Option<HookReporter>>> = Arc::new(Mutex::new(None));

    let hook_declarations = load_hooks(&[
        get_agent_dir(),
        Path::new(&current_dir()).join(crate::config::CONFIG_DIR_NAME),
    ]);
    let context_session = Arc::clone(&permission_session);
    let report_target = Arc::clone(&hook_report);
    let hook_runtime = Arc::new(HookRuntime::new(HookRuntimeOptions {
        hooks: hook_declarations.hooks,
        diagnostics: hook_declarations.diagnostics,
        context: Arc::new(move || {
            let session = context_session.lock().expect("poisoned").clone();
            HookSessionContext {
                session_id: session
                    .as_ref()
                    .map(|session| session.session_id())
                    .unwrap_or_default(),
                transcript_path: session.as_ref().and_then(|session| session.session_file()),
                cwd: current_dir(),
            }
        }),
        report: Some(Arc::new(move |message, level| {
            let reporter = report_target.lock().expect("poisoned").clone();
            match reporter {
                Some(reporter) => reporter(message, level),
                // Before the UI exists there is nowhere else to put this, and a
                // hook failure at startup is exactly the one worth seeing.
                None => eprintln!(
                    "{}: {message}",
                    match level {
                        HookReportLevel::Error => "Error",
                        _ => "Warning",
                    }
                ),
            }
        })),
        signal: None,
    }));

    let state_session = Arc::clone(&permission_session);
    let present_target = Arc::clone(&permission_present);
    let decide_runtime = Arc::clone(&hook_runtime);
    let permissions = Arc::new(PermissionGate::new(PermissionGateOptions {
        state: Arc::new(move || {
            let session = state_session.lock().expect("poisoned").clone();
            let mode = session.as_ref().and_then(|session| session.active_mode());
            PermissionSessionState {
                mode_id: mode.as_ref().map(|mode| mode.id.clone()),
                shell: mode.as_ref().map(|mode| mode.shell),
                // Before a session exists, assume the supervised level rather
                // than the permissive one.
                approval: mode
                    .as_ref()
                    .map(|mode| mode.approval)
                    .unwrap_or(crate::core::modes::shells::ApprovalLevel::Manual),
                cwd: current_dir(),
            }
        }),
        // With no dialog there is nobody to ask, so an ask becomes a refusal.
        // That is why a non-interactive run needs --auto or --yolo to do
        // anything a policy would have asked about; read-only work still runs
        // unhindered, because nothing asks about it.
        present: Arc::new(move |request| {
            let presenter = present_target.lock().expect("poisoned").clone();
            Box::pin(async move {
                match presenter {
                    Some(presenter) => presenter(request).await,
                    None => crate::core::permissions::request::ApprovalAnswer::Deny,
                }
            })
        }),
        policies: Vec::new(),
        // PreToolUse runs from here rather than from the hook dispatcher, so one
        // run serves both purposes: telling a supervisor a tool was attempted,
        // and filling the user-authored slots of the policy chain.
        decide: Some(Arc::new(move |tool_name, input| {
            let runtime = Arc::clone(&decide_runtime);
            Box::pin(async move { runtime.decide(&tool_name, &input).await.verdict })
        })),
    }));
    let hooks = Arc::new(HookDispatcher::new(Arc::clone(&hook_runtime)));

    let offline_mode = args.iter().any(|arg| arg == "--offline") || env_flag("NOTAGENT_OFFLINE");
    if offline_mode {
        unsafe {
            std::env::set_var("NOTAGENT_OFFLINE", "1");
            std::env::set_var("NOTAGENT_SKIP_VERSION_CHECK", "1");
        }
    }

    if let Some(exit_code) = run_auth_command(&args).await {
        return exit_code;
    }

    let mut cwd = current_dir();
    let agent_dir = get_agent_dir().to_string_lossy().into_owned();

    if let Some(exit_code) =
        handle_package_command(&args, &PackageCommandRuntime::from_process()).await
    {
        return exit_code;
    }
    // `handleConfigCommand` opens the resource configuration TUI. Like the
    // startup dialogs it is `!Send`, so it runs inside a `LocalSet`.
    if let Some(exit_code) = run_dialog(handle_config_command(
        &args,
        &PackageCommandRuntime::from_process(),
    ))
    .await
    {
        return exit_code;
    }

    let parsed = parse_args(&args);
    let mut parsed = parsed;
    if !parsed.diagnostics.is_empty() {
        for diagnostic in &parsed.diagnostics {
            let (label, paint): (&str, fn(&str) -> String) = match diagnostic.level {
                crate::cli::args::DiagnosticLevel::Error => ("Error", red),
                crate::cli::args::DiagnosticLevel::Warning => ("Warning", yellow),
            };
            eprintln!("{}", paint(&format!("{label}: {}", diagnostic.message)));
        }
        if parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.level == crate::cli::args::DiagnosticLevel::Error)
        {
            return 1;
        }
    }
    // An unknown long flag has nobody left to claim it now that extensions are
    // gone (`plans/facts/extension-boundary.md` §6).
    if let Some(message) = unknown_flags_error(&parsed) {
        eprintln!("{}", red(&format!("Error: {message}")));
        return 1;
    }
    time("parseArgs");

    if parsed.version {
        console_log(VERSION);
        return 0;
    }

    if let Some(export) = parsed.export.as_deref() {
        let output_path = parsed.messages.first().cloned();
        match export_from_file(
            export,
            ExportOptions {
                output_path,
                theme_name: None,
                tool_renderer: None,
            },
        ) {
            Ok(result) => {
                console_log(&format!("Exported to: {result}"));
                return 0;
            }
            Err(error) => {
                eprintln!("{}", red(&format!("Error: {error}")));
                return 1;
            }
        }
    }

    let mut app_mode = resolve_app_mode(&parsed, stdin_is_tty(), stdout_is_tty());

    let should_take_over_stdout =
        app_mode != AppMode::Interactive && !is_plain_runtime_metadata_command(&parsed);
    if should_take_over_stdout {
        take_over_stdout();
    }

    if parsed.mode == Some(OutputMode::Rpc) && !parsed.file_args.is_empty() {
        eprintln!(
            "{}",
            red("Error: @file arguments are not supported in RPC mode")
        );
        return 1;
    }

    if let Err(message) = validate_fork_flags(&parsed) {
        eprintln!("{}", red(&message));
        return 1;
    }
    if let Err(message) = validate_session_id_flags(&parsed) {
        eprintln!("{}", red(&message));
        return 1;
    }

    let migrations = run_migrations(Path::new(&cwd));
    let migrated_providers = migrations.migrated_auth_providers;
    time("runMigrations");

    let startup_settings_manager = Arc::new(SettingsManager::create(
        Path::new(&cwd),
        Some(Path::new(&agent_dir)),
        SettingsManagerCreateOptions::default(),
    ));
    report_diagnostics(&collect_settings_diagnostics(
        &startup_settings_manager,
        "startup session lookup",
    ));

    // Experimental first-time setup: theme choice and analytics opt-in. It runs
    // before any runtime service is created so the chosen settings apply
    // everywhere.
    if app_mode == AppMode::Interactive
        && !parsed.help
        && parsed.list_models.is_none()
        && should_run_first_time_setup(None)
    {
        run_dialog(show_first_time_setup(&startup_settings_manager)).await;
        time("firstTimeSetup");
    }

    // Decide the final runtime cwd before creating cwd-bound services:
    // `--session` and `--resume` may select a session from another project, so
    // project-local settings, resources and models must be resolved only after
    // the target cwd is known.
    let env_session_dir = std::env::var(crate::config::env_session_dir()).ok();
    let session_dir = parsed
        .session_dir
        .as_deref()
        .and_then(|value| normalize_path_default(value).ok())
        .or_else(|| {
            env_session_dir
                .as_deref()
                .filter(|value| !value.is_empty())
                .map(|value| expand_tilde_path(value).to_string_lossy().into_owned())
        })
        .or_else(|| startup_settings_manager.get_session_dir());

    let mut session_manager = match create_session_manager(
        &parsed,
        &cwd,
        session_dir.as_deref(),
        &startup_settings_manager,
    )
    .await
    {
        Ok(session_manager) => session_manager,
        Err(message) => {
            if message.is_empty() {
                return 0;
            }
            eprintln!("{}", red(&message));
            return 1;
        }
    };

    if let Some(issue) = get_missing_session_cwd_issue(&session_manager, &cwd) {
        // Interactively the user is offered the fallback directory; every other
        // mode reports and stops.
        if app_mode != AppMode::Interactive {
            eprintln!("{}", red(&format_missing_session_cwd_error(&issue)));
            return 1;
        }
        let chosen = run_dialog(show_startup_selector(
            &startup_settings_manager,
            &format_missing_session_cwd_prompt(&issue),
            vec!["Continue".to_owned(), "Cancel".to_owned()],
        ))
        .await;
        match chosen.as_deref() {
            Some("Continue") => {
                cwd = issue.fallback_cwd.clone();
            }
            _ => {
                eprintln!("{}", red(&format_missing_session_cwd_error(&issue)));
                return 1;
            }
        }
    }

    if let Some(name) = parsed.name.as_deref() {
        let name = name.trim();
        if name.is_empty() {
            eprintln!("{}", red("Error: --name requires a non-empty value"));
            return 1;
        }
        let _ = session_manager.append_session_info(name);
    }
    time("createSessionManager");

    let trust_store = Arc::new(ProjectTrustStore::new(&agent_dir));
    let trust_prompt_mode = if parsed.help || parsed.list_models.is_some() {
        AppMode::Print
    } else {
        app_mode
    };
    let project_trust_by_cwd: Arc<Mutex<HashMap<String, bool>>> =
        Arc::new(Mutex::new(HashMap::new()));

    let resolved_skill_paths = resolve_cli_paths(&cwd, parsed.skills.as_ref());
    let resolved_prompt_template_paths = resolve_cli_paths(&cwd, parsed.prompt_templates.as_ref());
    let resolved_theme_paths = resolve_cli_paths(&cwd, parsed.themes.as_ref());

    let factory_parsed = parsed.clone();
    let factory_hooks = Arc::clone(&hooks);
    let factory_permissions = Arc::clone(&permissions);
    let factory_trust_store = Arc::clone(&trust_store);
    let factory_startup_settings = Arc::clone(&startup_settings_manager);
    let factory_trust_by_cwd = Arc::clone(&project_trust_by_cwd);
    let create_runtime: CreateAgentSessionRuntimeFactory = Arc::new(move |input| {
        let parsed = factory_parsed.clone();
        let hooks = Arc::clone(&factory_hooks);
        let permissions = Arc::clone(&factory_permissions);
        let trust_store = Arc::clone(&factory_trust_store);
        let startup_settings_manager = Arc::clone(&factory_startup_settings);
        let project_trust_by_cwd = Arc::clone(&factory_trust_by_cwd);
        let skill_paths = resolved_skill_paths.clone();
        let prompt_template_paths = resolved_prompt_template_paths.clone();
        let theme_paths = resolved_theme_paths.clone();
        Box::pin(async move {
            build_runtime(
                input,
                parsed,
                hooks,
                permissions,
                trust_store,
                startup_settings_manager,
                project_trust_by_cwd,
                trust_prompt_mode,
                skill_paths,
                prompt_template_paths,
                theme_paths,
            )
            .await
        }) as BoxFuture<'static, Result<CreateAgentSessionRuntimeResult, String>>
    });
    time("createRuntime");

    let runtime_cwd = session_manager.get_cwd().to_owned();
    let runtime = match create_agent_session_runtime(
        create_runtime,
        runtime_cwd,
        agent_dir.clone(),
        session_manager,
    )
    .await
    {
        Ok(runtime) => Arc::new(runtime),
        Err(message) => {
            eprintln!("{}", red(&format!("Error: {message}")));
            return 1;
        }
    };
    time("createAgentSessionRuntime");

    // Bound for every mode, not only the interactive one: a print run has a
    // session too, and a hook that received an empty session id could not tell
    // two concurrent runs apart.
    *permission_session.lock().expect("poisoned") = Some(runtime.session());
    let services = runtime.services();
    let session = runtime.session();
    let settings_manager = Arc::clone(&services.settings_manager);
    let model_runtime = Arc::clone(&services.model_runtime);

    if parsed.help {
        console_log(&help_text());
        return 0;
    }

    if let Some(list_models_arg) = parsed.list_models.clone() {
        let search_pattern = match &list_models_arg {
            ListModels::All => None,
            ListModels::Search(pattern) => Some(pattern.as_str()),
        };
        list_models(
            &model_runtime,
            search_pattern,
            Some(timeout_signal(STARTUP_TIMEOUT_MS)),
        )
        .await;
        return 0;
    }

    // Piped stdin, except in RPC mode where stdin carries the protocol.
    let mut stdin_content: Option<String> = None;
    if app_mode != AppMode::Rpc {
        stdin_content = read_piped_stdin().await;
        if stdin_content.is_some() && app_mode == AppMode::Interactive {
            app_mode = AppMode::Print;
        }
    }
    time("readPipedStdin");

    let (initial_message, initial_images) = prepare_initial_message(
        &mut parsed,
        settings_manager.get_image_auto_resize(),
        stdin_content,
    )
    .await;
    time("prepareInitialMessage");
    init_theme(
        settings_manager.get_theme().as_deref(),
        app_mode == AppMode::Interactive,
    );
    time("initTheme");

    time("resolveModelScope");
    let diagnostics = runtime.diagnostics();
    report_diagnostics(&diagnostics);
    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.level == DiagnosticLevel::Error)
    {
        return 1;
    }
    time("createAgentSession");

    if app_mode != AppMode::Interactive && session.model().is_none() {
        eprintln!("{}", red(&format_no_models_available_message()));
        return 1;
    }

    let startup_benchmark = env_flag("NOTAGENT_STARTUP_BENCHMARK");
    if startup_benchmark && app_mode != AppMode::Interactive {
        eprintln!(
            "{}",
            red("Error: NOTAGENT_STARTUP_BENCHMARK only supports interactive mode")
        );
        return 1;
    }

    // RPC refreshes catalogs here in the background; the interactive mode starts
    // its refresh after the TUI is up.
    if !offline_mode && app_mode == AppMode::Rpc {
        let refresh_runtime = Arc::clone(&model_runtime);
        let signal = timeout_signal(STARTUP_TIMEOUT_MS);
        tokio::spawn(async move {
            refresh_runtime
                .refresh(notagent_ai::models::ModelsRefreshOptions {
                    signal: Some(signal),
                    ..notagent_ai::models::ModelsRefreshOptions::default()
                })
                .await;
        });
    }

    if let Some(start_mode) = parsed.start_mode {
        session.set_mode(match start_mode {
            StartMode::Auto => "auto",
            StartMode::Yolo => "yolo",
        });
    }

    // The approval observer records what the user answered, so a PreToolUse hook
    // sees the same decision the chain reached.
    permissions.observe(Some(create_approval_observer(Arc::clone(&hook_runtime))));

    match app_mode {
        AppMode::Rpc => {
            // Nothing binds a reporter outside the interactive mode, so these
            // reach standard error. A rejected declaration has to be visible.
            hook_runtime.report_diagnostics();
            print_timings();
            run_rpc_mode(runtime).await
        }
        AppMode::Interactive => {
            hook_runtime.report_diagnostics();
            print_timings();
            let session_cwd = session.with_session_manager(|manager| manager.get_cwd().to_owned());
            let auto_trust_on_reload_cwd = (parsed.project_trust_override.is_none()
                && !has_trust_requiring_project_resources(&session_cwd))
            .then_some(session_cwd);
            let InteractiveModeHandle {
                mut renderer,
                mut pump,
                run,
            } = create_interactive_mode(
                Arc::clone(&runtime),
                InteractiveModeOptions {
                    migrated_providers,
                    model_fallback_message: runtime.model_fallback_message(),
                    auto_trust_on_reload_cwd,
                    initial_message,
                    initial_images: initial_images.unwrap_or_default(),
                    initial_messages: parsed.messages.clone(),
                    verbose: parsed.verbose,
                    tui_mode: parsed.tui_mode,
                    terminal: None,
                },
            );
            // The render loop belongs to the caller (interface request A-20):
            // `run_until` renders and pumps stdin while the mode runs.
            let exit_code = run_until(renderer.as_mut(), pump.as_mut(), run).await;
            stop_theme_watcher();
            restore_stdout();
            exit_code
        }
        AppMode::Print | AppMode::Json => {
            hook_runtime.report_diagnostics();
            print_timings();
            let exit_code = run_print_mode(
                Arc::clone(&runtime),
                PrintModeOptions {
                    mode: Some(to_print_output_mode(app_mode)),
                    messages: parsed.messages.clone(),
                    initial_message,
                    initial_images,
                },
            )
            .await;
            restore_stdout();
            exit_code
        }
    }
}

/// The trust prompt, as far as the current mode can show one.
///
/// `createProjectTrustContext` in `cli/project-trust.ts`: only the interactive
/// mode has a screen, every other mode answers "no dialog".
///
/// Deviation (class 1): the dialog is `!Send` and the callback is not, so the
/// selector runs on a blocking thread with its own single-threaded runtime.
/// Node needs no equivalent because it has one event loop for everything; the
/// terminal is still touched by one dialog at a time.
fn trust_prompt_context(
    mode: AppMode,
    settings_manager: Arc<SettingsManager>,
) -> ProjectTrustContext {
    if mode != AppMode::Interactive {
        return ProjectTrustContext {
            has_ui: false,
            select: None,
        };
    }
    ProjectTrustContext {
        has_ui: true,
        select: Some(Arc::new(move |title: String, options: Vec<String>| {
            let settings_manager = Arc::clone(&settings_manager);
            Box::pin(async move {
                tokio::task::spawn_blocking(move || {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .ok()?;
                    let local = tokio::task::LocalSet::new();
                    local.block_on(
                        &runtime,
                        show_startup_selector(&settings_manager, &title, options),
                    )
                })
                .await
                .ok()
                .flatten()
            }) as BoxFuture<'static, Option<String>>
        })),
    }
}

/// Reads piped standard input, or `None` when it is a terminal.
async fn read_piped_stdin() -> Option<String> {
    if stdin_is_tty() {
        return None;
    }
    use tokio::io::AsyncReadExt;
    let mut data = String::new();
    if tokio::io::stdin().read_to_string(&mut data).await.is_err() {
        return None;
    }
    let trimmed = data.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

/// Builds one runtime: services for the target directory, the model scope, the
/// session options resolved against them, and finally the session.
#[allow(clippy::too_many_arguments)]
async fn build_runtime(
    input: crate::core::agent_session_runtime::CreateAgentSessionRuntimeInput,
    parsed: Args,
    hooks: Arc<HookDispatcher>,
    permissions: Arc<PermissionGate>,
    trust_store: Arc<ProjectTrustStore>,
    startup_settings_manager: Arc<SettingsManager>,
    project_trust_by_cwd: Arc<Mutex<HashMap<String, bool>>>,
    trust_prompt_mode: AppMode,
    skill_paths: Vec<String>,
    prompt_template_paths: Vec<String>,
    theme_paths: Vec<String>,
) -> Result<CreateAgentSessionRuntimeResult, String> {
    let cwd = input.cwd.clone();
    let cached_project_trust = project_trust_by_cwd
        .lock()
        .expect("poisoned")
        .get(&cwd)
        .copied();
    let has_trust_requiring_resources = has_trust_requiring_project_resources(&cwd);
    let should_resolve_project_trust = parsed.project_trust_override.is_none()
        && cached_project_trust.is_none()
        && has_trust_requiring_resources;
    let project_trusted = if should_resolve_project_trust {
        false
    } else {
        cached_project_trust
            .or(parsed.project_trust_override)
            .unwrap_or_else(|| {
                !has_trust_requiring_resources || trust_store.get(&cwd).ok().flatten() == Some(true)
            })
    };

    let runtime_settings_manager = Arc::new(SettingsManager::create(
        Path::new(&cwd),
        Some(Path::new(&input.agent_dir)),
        SettingsManagerCreateOptions {
            project_trusted: Some(project_trusted),
        },
    ));

    let reload_options = if should_resolve_project_trust {
        let trust_cwd = cwd.clone();
        let trust_store = Arc::clone(&trust_store);
        let startup_settings_manager = Arc::clone(&startup_settings_manager);
        let project_trust_by_cwd = Arc::clone(&project_trust_by_cwd);
        let trust_override = parsed.project_trust_override;
        Some(crate::core::resource_loader::ResourceLoaderReloadOptions {
            resolve_project_trust: Some(Arc::new(move || {
                let trust_cwd = trust_cwd.clone();
                let trust_store = Arc::clone(&trust_store);
                let startup_settings_manager = Arc::clone(&startup_settings_manager);
                let project_trust_by_cwd = Arc::clone(&project_trust_by_cwd);
                Box::pin(async move {
                    let trusted = resolve_project_trusted(ResolveProjectTrustedOptions {
                        cwd: &trust_cwd,
                        trust_store: &trust_store,
                        trust_override,
                        default_project_trust: Some(
                            startup_settings_manager.get_default_project_trust(),
                        ),
                        project_trust_context: trust_prompt_context(
                            trust_prompt_mode,
                            Arc::clone(&startup_settings_manager),
                        ),
                    })
                    .await
                    .unwrap_or(false);
                    project_trust_by_cwd
                        .lock()
                        .expect("poisoned")
                        .insert(trust_cwd, trusted);
                    trusted
                }) as BoxFuture<'static, bool>
            })),
        })
    } else {
        None
    };

    let services = create_agent_session_services(CreateAgentSessionServicesOptions {
        cwd: cwd.clone(),
        agent_dir: Some(input.agent_dir.clone()),
        settings_manager: Some(Arc::clone(&runtime_settings_manager)),
        resource_loader_options: Some(crate::core::resource_loader::DefaultResourceLoaderOptions {
            cwd: cwd.clone(),
            agent_dir: input.agent_dir.clone(),
            additional_skill_paths: skill_paths,
            additional_prompt_template_paths: prompt_template_paths,
            additional_theme_paths: theme_paths,
            no_skills: parsed.no_skills,
            no_prompt_templates: parsed.no_prompt_templates,
            no_themes: parsed.no_themes,
            no_context_files: parsed.no_context_files,
            system_prompt: parsed.system_prompt.clone(),
            append_system_prompt: parsed.append_system_prompt.clone(),
            ..crate::core::resource_loader::DefaultResourceLoaderOptions::default()
        }),
        resource_loader_reload_options: reload_options,
        ..CreateAgentSessionServicesOptions::default()
    })
    .await?;

    let settings_manager = Arc::clone(&services.settings_manager);
    let model_runtime = Arc::clone(&services.model_runtime);
    let mut diagnostics: Vec<AgentSessionRuntimeDiagnostic> = services.diagnostics.clone();
    diagnostics.extend(collect_settings_diagnostics(
        &settings_manager,
        "runtime creation",
    ));

    let model_patterns = parsed
        .models
        .clone()
        .or_else(|| settings_manager.get_enabled_models());
    let scope = match model_patterns {
        Some(patterns) if !patterns.is_empty() => {
            resolve_model_scope(
                &patterns,
                model_runtime.as_ref(),
                Some(notagent_ai::auth::types::AuthOperationOptions {
                    signal: Some(timeout_signal(STARTUP_TIMEOUT_MS)),
                }),
            )
            .await
        }
        _ => crate::core::model_resolver::ResolveModelScopeResult::default(),
    };
    for diagnostic in &scope.diagnostics {
        diagnostics.push(AgentSessionRuntimeDiagnostic {
            level: DiagnosticLevel::Warning,
            message: diagnostic.message.clone(),
        });
    }

    let has_existing_session = !input
        .session_manager
        .build_session_context()
        .messages
        .is_empty();
    let (session_options, cli_thinking_from_model, option_diagnostics) = build_session_options(
        &parsed,
        &scope.scoped_models,
        has_existing_session,
        &model_runtime,
        &settings_manager,
    );
    diagnostics.extend(option_diagnostics);

    if let Some(api_key) = parsed.api_key.as_deref() {
        match session_options.model.as_ref() {
            None => diagnostics.push(AgentSessionRuntimeDiagnostic {
                level: DiagnosticLevel::Error,
                message: "--api-key requires a model to be specified via --model, --provider/--model, or --models".to_owned(),
            }),
            Some(model) => {
                let _ = model_runtime
                    .set_runtime_api_key(&model.provider, api_key, None)
                    .await;
            }
        }
    }

    let created = create_agent_session_from_services(
        &services,
        CreateAgentSessionFromServicesOptions {
            session_manager: input.session_manager,
            model: session_options.model,
            thinking_level: session_options.thinking_level,
            scoped_models: session_options.scoped_models,
            tools: session_options.tools,
            exclude_tools: session_options.exclude_tools,
            no_tools: session_options.no_tools,
            hooks: Some(hooks),
            permissions: Some(permissions),
            session_start_reason: input.reason.as_str().to_owned(),
        },
    )
    .await;

    let cli_thinking_override = parsed.thinking.is_some() || cli_thinking_from_model;
    if created.session.model().is_some() && cli_thinking_override {
        created
            .session
            .set_thinking_level(created.session.thinking_level());
    }

    Ok(CreateAgentSessionRuntimeResult {
        session: created.session,
        services: Arc::new(services),
        diagnostics,
        model_fallback_message: created.model_fallback_message,
    })
}
