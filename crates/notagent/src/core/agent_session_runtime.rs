use std::path::Path;
use std::sync::{Arc, Mutex};

use notagent_agent::types::BoxFuture;

use crate::core::agent_session::AgentSession;
use crate::core::agent_session_services::{AgentSessionRuntimeDiagnostic, AgentSessionServices};
use crate::core::session_cwd::{MissingSessionCwdError, assert_session_cwd_exists};
use crate::core::session_manager::SessionManager;
use crate::utils::paths::{current_dir, resolve_path_default};

/// Why a session is being replaced. Reaches the user's `SessionEnd` hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplacementReason {
    New,
    Resume,
    Fork,
    Quit,
}

impl ReplacementReason {
    pub fn as_str(self) -> &'static str {
        match self {
            ReplacementReason::New => "new",
            ReplacementReason::Resume => "resume",
            ReplacementReason::Fork => "fork",
            ReplacementReason::Quit => "quit",
        }
    }
}

/// What a runtime creation produced.
pub struct CreateAgentSessionRuntimeResult {
    pub session: Arc<AgentSession>,
    pub services: Arc<AgentSessionServices>,
    pub diagnostics: Vec<AgentSessionRuntimeDiagnostic>,
    pub model_fallback_message: Option<String>,
}

/// Builds a full runtime for a target directory and session manager.
/// The factory closes over the process-global fixed inputs, rebuilds the
/// cwd-bound services, resolves the session options against them and finally
/// creates the session.
pub type CreateAgentSessionRuntimeFactory = Arc<
    dyn Fn(
            CreateAgentSessionRuntimeInput,
        ) -> BoxFuture<'static, Result<CreateAgentSessionRuntimeResult, String>>
        + Send
        + Sync,
>;

pub struct CreateAgentSessionRuntimeInput {
    pub cwd: String,
    pub agent_dir: String,
    pub session_manager: SessionManager,
    pub reason: ReplacementReason,
    pub previous_session_file: Option<String>,
}

/// `/import` was given a path that does not exist.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("File not found: {0}")]
pub struct SessionImportFileNotFoundError(pub String);

/// Called after a replacement, with the session that took over.
pub type RebindSession = Arc<dyn Fn(Arc<AgentSession>) -> BoxFuture<'static, ()> + Send + Sync>;

/// Runs after the shutdown hooks and before the outgoing session is
/// invalidated, for host-owned UI teardown that must not yield.
pub type BeforeSessionInvalidate = Arc<dyn Fn() + Send + Sync>;

struct RuntimeState {
    session: Arc<AgentSession>,
    services: Arc<AgentSessionServices>,
    diagnostics: Vec<AgentSessionRuntimeDiagnostic>,
    model_fallback_message: Option<String>,
}

/// Why opening a session file failed.
/// first, which lost the issue the dialog needs. The variant carries it, and
/// `to_string()` still produces the message the other callers print.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SessionOpenError {
    #[error("{0}")]
    MissingCwd(#[from] MissingSessionCwdError),
    #[error("{0}")]
    Failed(String),
}

pub struct AgentSessionRuntime {
    state: Mutex<RuntimeState>,
    create_runtime: CreateAgentSessionRuntimeFactory,
    rebind_session: Mutex<Option<RebindSession>>,
    before_session_invalidate: Mutex<Option<BeforeSessionInvalidate>>,
}

impl AgentSessionRuntime {
    pub fn new(
        result: CreateAgentSessionRuntimeResult,
        create_runtime: CreateAgentSessionRuntimeFactory,
    ) -> Self {
        Self {
            state: Mutex::new(RuntimeState {
                session: result.session,
                services: result.services,
                diagnostics: result.diagnostics,
                model_fallback_message: result.model_fallback_message,
            }),
            create_runtime,
            rebind_session: Mutex::new(None),
            before_session_invalidate: Mutex::new(None),
        }
    }

    pub fn session(&self) -> Arc<AgentSession> {
        Arc::clone(&self.state.lock().expect("poisoned").session)
    }

    pub fn services(&self) -> Arc<AgentSessionServices> {
        Arc::clone(&self.state.lock().expect("poisoned").services)
    }

    pub fn cwd(&self) -> String {
        self.state.lock().expect("poisoned").services.cwd.clone()
    }

    pub fn diagnostics(&self) -> Vec<AgentSessionRuntimeDiagnostic> {
        self.state.lock().expect("poisoned").diagnostics.clone()
    }

    pub fn model_fallback_message(&self) -> Option<String> {
        self.state
            .lock()
            .expect("poisoned")
            .model_fallback_message
            .clone()
    }

    pub fn set_rebind_session(&self, rebind: Option<RebindSession>) {
        *self.rebind_session.lock().expect("poisoned") = rebind;
    }

    pub fn set_before_session_invalidate(&self, before: Option<BeforeSessionInvalidate>) {
        *self.before_session_invalidate.lock().expect("poisoned") = before;
    }

    /// Settles the current session and takes it out of service.
    async fn teardown_current(&self, reason: ReplacementReason) {
        let session = self.session();
        // Settle any active response first, so the aborted turn — tool results
        // included — is persisted to the outgoing session before it is replaced.
        session.abort().await;
        session.shutdown_hooks(reason.as_str()).await;
        if let Some(before) = self
            .before_session_invalidate
            .lock()
            .expect("poisoned")
            .clone()
        {
            before();
        }
        session.dispose();
    }

    fn apply(&self, result: CreateAgentSessionRuntimeResult) {
        let mut state = self.state.lock().expect("poisoned");
        state.session = result.session;
        state.services = result.services;
        state.diagnostics = result.diagnostics;
        state.model_fallback_message = result.model_fallback_message;
    }

    async fn finish_replacement(&self) {
        let rebind = self.rebind_session.lock().expect("poisoned").clone();
        if let Some(rebind) = rebind {
            rebind(self.session()).await;
        }
    }

    async fn replace_with(
        &self,
        cwd: String,
        session_manager: SessionManager,
        reason: ReplacementReason,
        previous_session_file: Option<String>,
    ) -> Result<(), String> {
        let agent_dir = self.services().agent_dir.clone();
        self.teardown_current(reason).await;
        let result = (self.create_runtime)(CreateAgentSessionRuntimeInput {
            cwd,
            agent_dir,
            session_manager,
            reason,
            previous_session_file,
        })
        .await?;
        self.apply(result);
        self.finish_replacement().await;
        Ok(())
    }

    /// Opens a different session file.
    pub async fn switch_session(
        &self,
        session_path: &str,
        cwd_override: Option<&str>,
    ) -> Result<(), SessionOpenError> {
        let previous_session_file = self.session().session_file();
        let session_manager = SessionManager::open(session_path, None, cwd_override)
            .map_err(|error| SessionOpenError::Failed(error.to_string()))?;
        assert_session_cwd_exists(&session_manager, &self.cwd())?;
        let cwd = session_manager.get_cwd().to_string();
        self.replace_with(
            cwd,
            session_manager,
            ReplacementReason::Resume,
            previous_session_file,
        )
        .await
        .map_err(SessionOpenError::Failed)
    }

    /// Starts a fresh session in the same directory.
    pub async fn new_session(&self, parent_session: Option<&str>) -> Result<(), String> {
        let previous_session_file = self.session().session_file();
        let (session_dir, is_persisted, cwd) = self.session().with_session_manager(|manager| {
            (
                manager.get_session_dir().to_string(),
                manager.is_persisted(),
                manager.get_cwd().to_string(),
            )
        });
        let mut session_manager = if is_persisted {
            SessionManager::create(&cwd, Some(&session_dir), None)
                .map_err(|error| error.to_string())?
        } else {
            SessionManager::in_memory(Some(&cwd), None).map_err(|error| error.to_string())?
        };
        if let Some(parent_session) = parent_session {
            session_manager
                .new_session(Some(crate::core::session_manager::NewSessionOptions {
                    parent_session: Some(parent_session.to_string()),
                    ..Default::default()
                }))
                .map_err(|error| error.to_string())?;
        }

        self.replace_with(
            self.cwd(),
            session_manager,
            ReplacementReason::New,
            previous_session_file,
        )
        .await
    }

    /// Forks at `entry_id`.
    /// `Before` puts the selected user message back into the editor and forks at
    /// its parent; `At` keeps everything up to and including it.
    pub async fn fork(
        &self,
        entry_id: &str,
        position: ForkPosition,
    ) -> Result<Option<String>, String> {
        let session = self.session();
        let previous_session_file = session.session_file();

        let (target_leaf_id, selected_text) = session.with_session_manager(|manager| {
            let Some(entry) = manager.get_entry(entry_id) else {
                return Err("Invalid entry ID for forking".to_string());
            };
            match position {
                ForkPosition::At => Ok((Some(entry.id().to_string()), None)),
                ForkPosition::Before => {
                    let crate::core::session_manager::SessionEntry::Message(message) = entry else {
                        return Err("Invalid entry ID for forking".to_string());
                    };
                    if message.message.get("role").and_then(|role| role.as_str()) != Some("user") {
                        return Err("Invalid entry ID for forking".to_string());
                    }
                    let text = message
                        .message
                        .get("content")
                        .map(user_text)
                        .unwrap_or_default();
                    Ok((message.parent_id.clone(), Some(text)))
                }
            }
        })?;

        let (is_persisted, session_dir, current_session_file, cwd) =
            session.with_session_manager(|manager| {
                (
                    manager.is_persisted(),
                    manager.get_session_dir().to_string(),
                    manager.get_session_file().map(str::to_string),
                    manager.get_cwd().to_string(),
                )
            });

        let session_manager = if is_persisted {
            let current_session_file = current_session_file
                .ok_or_else(|| "Persisted session is missing a session file".to_string())?;
            match target_leaf_id.as_deref() {
                None => {
                    let mut manager = SessionManager::create(&cwd, Some(&session_dir), None)
                        .map_err(|error| error.to_string())?;
                    manager
                        .new_session(Some(crate::core::session_manager::NewSessionOptions {
                            parent_session: Some(current_session_file),
                            ..Default::default()
                        }))
                        .map_err(|error| error.to_string())?;
                    manager
                }
                Some(leaf) => {
                    if !Path::new(&current_session_file).exists() {
                        return Err(
                            "This session has not been saved yet. Wait for the first assistant response before cloning or forking it."
                                .to_string(),
                        );
                    }
                    let mut manager =
                        SessionManager::open(&current_session_file, Some(&session_dir), None)
                            .map_err(|error| error.to_string())?;
                    manager
                        .create_branched_session(leaf)
                        .map_err(|error| error.to_string())?
                        .ok_or_else(|| "Failed to create forked session".to_string())?;
                    manager
                }
            }
        } else {
            // An in-memory session forks in place.
            let mut manager =
                SessionManager::in_memory(Some(&cwd), None).map_err(|error| error.to_string())?;
            match target_leaf_id.as_deref() {
                None => {
                    manager
                        .new_session(Some(crate::core::session_manager::NewSessionOptions {
                            parent_session: previous_session_file.clone(),
                            ..Default::default()
                        }))
                        .map_err(|error| error.to_string())?;
                }
                Some(leaf) => {
                    manager
                        .create_branched_session(leaf)
                        .map_err(|error| error.to_string())?;
                }
            }
            manager
        };

        let next_cwd = session_manager.get_cwd().to_string();
        self.replace_with(
            next_cwd,
            session_manager,
            ReplacementReason::Fork,
            previous_session_file,
        )
        .await?;
        Ok(selected_text)
    }

    /// Copies a session JSONL into the session directory and switches to it.
    pub async fn import_from_jsonl(
        &self,
        input_path: &str,
        cwd_override: Option<&str>,
    ) -> Result<(), SessionOpenError> {
        let resolved = resolve_path_default(input_path, &current_dir())
            .map_err(|error| SessionOpenError::Failed(error.to_string()))?;
        if !Path::new(&resolved).exists() {
            return Err(SessionOpenError::Failed(
                SessionImportFileNotFoundError(resolved).to_string(),
            ));
        }

        let session_dir = self
            .session()
            .with_session_manager(|manager| manager.get_session_dir().to_string());
        if !Path::new(&session_dir).exists() {
            std::fs::create_dir_all(&session_dir)
                .map_err(|error| SessionOpenError::Failed(error.to_string()))?;
        }

        let file_name = Path::new(&resolved)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let destination = Path::new(&session_dir).join(&file_name);
        let destination_text = destination.to_string_lossy().into_owned();

        let previous_session_file = self.session().session_file();
        if destination_text != resolved {
            std::fs::copy(&resolved, &destination)
                .map_err(|error| SessionOpenError::Failed(error.to_string()))?;
        }

        let session_manager =
            SessionManager::open(&destination_text, Some(&session_dir), cwd_override)
                .map_err(|error| SessionOpenError::Failed(error.to_string()))?;
        assert_session_cwd_exists(&session_manager, &self.cwd())?;
        let cwd = session_manager.get_cwd().to_string();
        self.replace_with(
            cwd,
            session_manager,
            ReplacementReason::Resume,
            previous_session_file,
        )
        .await
        .map_err(SessionOpenError::Failed)
    }

    /// Shuts the runtime down for good.
    pub async fn dispose(&self) {
        let session = self.session();
        session
            .shutdown_hooks(ReplacementReason::Quit.as_str())
            .await;
        // Every MCP server this session started is a process it owns; the
        session.shutdown_mcp_servers().await;
        if let Some(before) = self
            .before_session_invalidate
            .lock()
            .expect("poisoned")
            .clone()
        {
            before();
        }
        session.dispose();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ForkPosition {
    /// Fork at the parent of the selected entry; its text goes to the editor.
    #[default]
    Before,
    /// Fork at the selected entry, keeping it.
    At,
}

fn user_text(content: &serde_json::Value) -> String {
    match content {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Array(blocks) => blocks
            .iter()
            .filter(|block| block.get("type").and_then(|value| value.as_str()) == Some("text"))
            .filter_map(|block| block.get("text").and_then(|value| value.as_str()))
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// Creates the first runtime from a factory and an initial session target. The
/// same factory is kept for the later `/new`, `/resume`, `/fork` and import
/// flows.
pub async fn create_agent_session_runtime(
    create_runtime: CreateAgentSessionRuntimeFactory,
    cwd: String,
    agent_dir: String,
    session_manager: SessionManager,
) -> Result<AgentSessionRuntime, String> {
    assert_session_cwd_exists(&session_manager, &cwd)
        .map_err(|error: MissingSessionCwdError| error.to_string())?;
    let result = create_runtime(CreateAgentSessionRuntimeInput {
        cwd,
        agent_dir,
        session_manager,
        reason: ReplacementReason::New,
        previous_session_file: None,
    })
    .await?;
    Ok(AgentSessionRuntime::new(result, create_runtime))
}
