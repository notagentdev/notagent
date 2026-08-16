//! Port of `packages/coding-agent/src/core/agent-session-services.ts`.
//!
//! The services bound to one working directory: the model runtime, the settings
//! and the resource loader. They are rebuilt whenever the effective cwd changes,
//! which is why they are separate from the session — the session's own options
//! (model, tools) have to be resolved against them first.
//!
//! Non-fatal problems are returned as diagnostics rather than printed or fatal:
//! the app layer decides whether a warning is shown and whether an error should
//! stop startup.
//!
//! Deviation (class 2): the extension flag values and the pending provider
//! registrations of the TypeScript are gone with the extension system
//! (`plans/facts/extension-boundary.md` §3). The one registration that was
//! productive — the native llama.cpp provider of the built-in llama extension
//! (`main.ts:641`, `extensions/index.ts:4`) — happens natively here, at the
//! point where `agent-session-services.ts:170-181` drains
//! `pendingNativeProviderRegistrations`: after the resources are loaded and
//! before the first refresh.

use std::sync::Arc;

use crate::config::get_agent_dir;
use crate::core::llama::provider::{LlamaProviderController, create_llama_provider};
use crate::core::model_runtime::{CreateModelRuntimeOptions, ModelRuntime};
use crate::core::resource_loader::{
    DefaultResourceLoader, DefaultResourceLoaderOptions, ResourceLoader,
    ResourceLoaderReloadOptions,
};
use crate::core::sdk::{
    CreateAgentSessionOptions, CreateAgentSessionResult, NoTools, create_agent_session,
};
use crate::core::session_manager::SessionManager;
use crate::core::settings_manager::{SettingsManager, SettingsManagerCreateOptions};
use crate::utils::paths::{current_dir, resolve_path_default};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticLevel {
    Info,
    Warning,
    Error,
}

/// A non-fatal problem found while creating services or a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSessionRuntimeDiagnostic {
    pub level: DiagnosticLevel,
    pub message: String,
}

/// Inputs of [`create_agent_session_services`].
#[derive(Default)]
pub struct CreateAgentSessionServicesOptions {
    pub cwd: String,
    pub agent_dir: Option<String>,
    pub settings_manager: Option<Arc<SettingsManager>>,
    pub model_runtime: Option<Arc<ModelRuntime>>,
    /// Resource loader options minus the three the services fill in.
    pub resource_loader_options: Option<DefaultResourceLoaderOptions>,
    pub resource_loader_reload_options: Option<ResourceLoaderReloadOptions>,
}

/// A coherent set of cwd-bound services. Infrastructure only — the session is
/// created separately, once its own options have been resolved against these.
pub struct AgentSessionServices {
    pub cwd: String,
    pub agent_dir: String,
    pub model_runtime: Arc<ModelRuntime>,
    pub settings_manager: Arc<SettingsManager>,
    pub resource_loader: Arc<dyn ResourceLoader>,
    /// The controller of the natively registered llama.cpp provider; `/llama`
    /// writes the router catalog through it.
    pub llama: Arc<LlamaProviderController>,
    pub diagnostics: Vec<AgentSessionRuntimeDiagnostic>,
}

/// Creates the cwd-bound services, plus whatever went wrong on the way.
pub async fn create_agent_session_services(
    options: CreateAgentSessionServicesOptions,
) -> Result<AgentSessionServices, String> {
    let cwd = resolve_path_default(&options.cwd, &current_dir()).unwrap_or(options.cwd.clone());
    let agent_dir = match options.agent_dir.as_deref() {
        Some(agent_dir) => resolve_path_default(agent_dir, &current_dir())
            .unwrap_or_else(|_| agent_dir.to_string()),
        None => get_agent_dir().to_string_lossy().into_owned(),
    };

    let model_runtime = match options.model_runtime {
        Some(model_runtime) => model_runtime,
        None => {
            ModelRuntime::create(CreateModelRuntimeOptions {
                auth_path: Some(
                    std::path::Path::new(&agent_dir)
                        .join("auth.json")
                        .to_string_lossy()
                        .into_owned(),
                ),
                models_path: Some(Some(
                    std::path::Path::new(&agent_dir)
                        .join("models.json")
                        .to_string_lossy()
                        .into_owned(),
                )),
                ..CreateModelRuntimeOptions::default()
            })
            .await?
        }
    };

    let settings_manager = options.settings_manager.unwrap_or_else(|| {
        Arc::new(SettingsManager::create(
            std::path::Path::new(&cwd),
            Some(std::path::Path::new(&agent_dir)),
            SettingsManagerCreateOptions::default(),
        ))
    });

    let resource_loader = Arc::new(DefaultResourceLoader::new(DefaultResourceLoaderOptions {
        cwd: cwd.clone(),
        agent_dir: agent_dir.clone(),
        settings_manager: Some(Arc::clone(&settings_manager)),
        ..options.resource_loader_options.unwrap_or_default()
    }));
    resource_loader
        .reload(options.resource_loader_reload_options.unwrap_or_default())
        .await;

    // `registerNativeProvider(provider)` of the llama extension's factory.
    let llama = Arc::new(create_llama_provider());
    let mut diagnostics: Vec<AgentSessionRuntimeDiagnostic> = Vec::new();
    if let Err(error) = model_runtime.register_native_provider(
        Arc::clone(&llama.provider) as Arc<dyn notagent_ai::models::Provider>
    ) {
        diagnostics.push(AgentSessionRuntimeDiagnostic {
            level: DiagnosticLevel::Error,
            message: format!("Extension \"<inline:llama.cpp>\" error: {error}"),
        });
    }

    // The model catalogs are refreshed without the network here: startup must
    // not wait on a provider, and the runtime refreshes again in the background.
    model_runtime
        .refresh(notagent_ai::models::ModelsRefreshOptions {
            allow_network: Some(false),
            ..notagent_ai::models::ModelsRefreshOptions::default()
        })
        .await;

    Ok(AgentSessionServices {
        cwd,
        agent_dir,
        model_runtime,
        settings_manager,
        resource_loader,
        llama,
        diagnostics,
    })
}

/// Inputs of [`create_agent_session_from_services`].
pub struct CreateAgentSessionFromServicesOptions {
    pub session_manager: SessionManager,
    pub model: Option<notagent_ai::types::Model>,
    pub thinking_level: Option<notagent_agent::types::ThinkingLevel>,
    pub scoped_models: Vec<crate::core::agent_session::ScopedModel>,
    pub tools: Option<Vec<String>>,
    pub exclude_tools: Option<Vec<String>>,
    pub no_tools: Option<NoTools>,
    pub hooks: Option<Arc<crate::core::hooks::dispatch::HookDispatcher>>,
    pub permissions: Option<Arc<crate::core::permissions::gate::PermissionGate>>,
    pub session_start_reason: String,
}

/// Creates a session from services that already exist.
///
/// Keeping the two apart is what lets a caller resolve model, thinking level and
/// tools against the target directory before the session is constructed.
pub async fn create_agent_session_from_services(
    services: &AgentSessionServices,
    options: CreateAgentSessionFromServicesOptions,
) -> CreateAgentSessionResult {
    create_agent_session(CreateAgentSessionOptions {
        cwd: services.cwd.clone(),
        agent_dir: services.agent_dir.clone(),
        model_runtime: Arc::clone(&services.model_runtime),
        settings_manager: Arc::clone(&services.settings_manager),
        resource_loader: Arc::clone(&services.resource_loader),
        session_manager: options.session_manager,
        model: options.model,
        thinking_level: options.thinking_level,
        scoped_models: options.scoped_models,
        tools: options.tools,
        exclude_tools: options.exclude_tools,
        no_tools: options.no_tools,
        hooks: options.hooks,
        permissions: options.permissions,
        session_start_reason: options.session_start_reason,
    })
    .await
}
