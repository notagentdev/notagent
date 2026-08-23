//! Port of `packages/coding-agent/test/suite/harness.ts`.
//!
//! A whole session wired to the faux provider: the scripted responses are the
//! only thing the tests have to arrange, and everything below the session is
//! real — a real agent, a real session manager, real settings.
//!
//! Deviation (class 1): the model runtime is a small stand-in rather than the
//! real one, because `model-runtime.ts` is workstream B's (interface request
//! C-13). It answers exactly the six questions the session asks it, from the
//! faux provider's model list.

#![allow(dead_code)]

use std::sync::{Arc, Mutex};

use notagent::core::agent_session::{
    AgentSession, AgentSessionConfig, AgentSessionEvent, SessionAuth, SessionModelRuntime,
};
use notagent::core::diagnostics::ResourceDiagnostic;
use notagent::core::prompt_templates::PromptTemplate;
use notagent::core::resource_loader::{
    ContextFile, ResourceExtensionPaths, ResourceLoader, ResourceLoaderReloadOptions,
};
use notagent::core::session_manager::SessionManager;
use notagent::core::settings_manager::{SettingsManager, SettingsManagerCreateOptions};
use notagent::core::skills::Skill;
use notagent::modes::interactive::theme::theme::Theme;
use notagent_agent::agent::{Agent, AgentOptions};
use notagent_agent::types::{AgentMessage, AgentTool, BoxFuture, StreamFn};
use notagent_ai::providers::faux::{
    FauxCore, FauxModelDefinition, FauxProviderOptions, FauxResponseStep, faux_provider,
};
use notagent_ai::types::{AssistantContent, Model, TextOrImageContent, UserContent};

/// A resource loader that answers with nothing, which is what the suite needs:
/// the sessions under test bring their own tools and system prompt.
#[derive(Default)]
pub struct EmptyResourceLoader {
    pub system_prompt: Option<String>,
}

impl ResourceLoader for EmptyResourceLoader {
    fn get_skills(&self) -> (Vec<Skill>, Vec<ResourceDiagnostic>) {
        (Vec::new(), Vec::new())
    }
    fn get_prompts(&self) -> (Vec<PromptTemplate>, Vec<ResourceDiagnostic>) {
        (Vec::new(), Vec::new())
    }
    fn get_themes(&self) -> (Vec<Theme>, Vec<ResourceDiagnostic>) {
        (Vec::new(), Vec::new())
    }
    fn get_agents_files(&self) -> Vec<ContextFile> {
        Vec::new()
    }
    fn get_system_prompt(&self) -> Option<String> {
        self.system_prompt.clone()
    }
    fn get_system_prompt_source(&self) -> Option<String> {
        None
    }
    fn get_append_system_prompt(&self) -> Vec<String> {
        Vec::new()
    }
    fn get_append_system_prompt_sources(&self) -> Vec<String> {
        Vec::new()
    }
    fn extend_resources(&self, _paths: ResourceExtensionPaths) {}
    fn reload<'a>(&'a self, _options: ResourceLoaderReloadOptions) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }
}

/// The seven questions the session asks the model runtime, answered from the faux
/// provider's model list.
pub struct FauxModelRuntime {
    pub models: Vec<Model>,
    pub with_configured_auth: bool,
}

impl SessionModelRuntime for FauxModelRuntime {
    fn get_auth<'a>(
        &'a self,
        _model: &'a Model,
    ) -> BoxFuture<'a, Result<Option<SessionAuth>, String>> {
        let configured = self.with_configured_auth;
        Box::pin(async move {
            Ok(configured.then(|| SessionAuth {
                api_key: Some("faux-key".to_string()),
                ..SessionAuth::default()
            }))
        })
    }

    fn has_configured_auth(&self, _provider: &str) -> bool {
        self.with_configured_auth
    }

    fn check_auth<'a>(&'a self, _provider: &'a str) -> BoxFuture<'a, bool> {
        let configured = self.with_configured_auth;
        Box::pin(async move { configured })
    }

    fn is_using_oauth(&self, _provider: &str) -> bool {
        false
    }

    fn is_using_subscription(&self, _provider: &str) -> bool {
        false
    }

    fn get_available_snapshot(&self) -> Vec<Model> {
        self.models.clone()
    }

    fn get_model(&self, provider: &str, id: &str) -> Option<Model> {
        self.models
            .iter()
            .find(|model| model.provider == provider && model.id == id)
            .cloned()
    }
}

#[derive(Default)]
pub struct HarnessOptions {
    pub models: Option<Vec<FauxModelDefinition>>,
    pub system_prompt: Option<String>,
    pub tools: Option<Vec<Arc<dyn AgentTool>>>,
    pub initial_active_tool_names: Option<Vec<String>>,
    pub allowed_tool_names: Option<Vec<String>>,
    pub excluded_tool_names: Option<Vec<String>>,
    pub with_configured_auth: Option<bool>,
    pub settings: Option<serde_json::Value>,
    /// Slows the faux provider down, for cases that need a request to still be
    /// in flight while something else is asserted.
    pub tokens_per_second: Option<f64>,
    /// The user's hooks, for the cases that assert a dispatch point.
    pub hooks: Option<Arc<notagent::core::hooks::dispatch::HookDispatcher>>,
    /// The permission chain, for the cases that assert a call was gated —
    /// including the ones that borrow a tool over the lending endpoint.
    pub permissions: Option<Arc<notagent::core::permissions::gate::PermissionGate>>,
    /// Context window the model RUNTIME reports, diverging from the session's
    /// model snapshot — the shape of a dynamic provider whose server shrank
    /// its window after the model was selected.
    pub runtime_window_override: Option<u64>,
}

pub struct Harness {
    pub session: Arc<AgentSession>,
    pub settings_manager: Arc<SettingsManager>,
    pub faux: Arc<FauxCore>,
    pub models: Vec<Model>,
    pub events: Arc<Mutex<Vec<AgentSessionEvent>>>,
    pub temp: tempfile::TempDir,
    _subscription: notagent::core::agent_session::ListenerHandle,
}

impl Harness {
    /// Wait until the run started by a spawned `prompt` is in flight.
    ///
    /// Interface request B-16: without a deadline the loop spins forever when
    /// the spawned run finished before the first look — `#[tokio::test]` gives
    /// a current-thread scheduler, so the whole turn can run inside the first
    /// `await`. Cases that need the window use a slowed faux provider
    /// (`tokens_per_second`); the deadline turns a regression into a failure
    /// with a message instead of a hanging `check.sh`.
    pub async fn wait_until_streaming(&self) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !self.session.is_streaming() {
            assert!(
                std::time::Instant::now() < deadline,
                "the run never became visible as streaming; it most likely finished before the                  first look — slow the faux provider down (`tokens_per_second`)"
            );
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
    }

    pub fn model(&self) -> Model {
        self.models[0].clone()
    }

    pub fn set_responses(&self, responses: Vec<FauxResponseStep>) {
        self.faux.set_responses(responses);
    }

    pub fn append_responses(&self, responses: Vec<FauxResponseStep>) {
        self.faux.append_responses(responses);
    }

    pub fn pending_response_count(&self) -> usize {
        self.faux.pending_response_count()
    }

    pub fn events(&self) -> Vec<AgentSessionEvent> {
        self.events.lock().expect("poisoned").clone()
    }

    pub fn user_texts(&self) -> Vec<String> {
        self.session
            .messages()
            .iter()
            .filter_map(|message| match message {
                AgentMessage::User(message) => Some(user_content_text(&message.content)),
                _ => None,
            })
            .collect()
    }

    pub fn assistant_texts(&self) -> Vec<String> {
        self.session
            .messages()
            .iter()
            .filter_map(|message| match message {
                AgentMessage::Assistant(message) => Some(
                    message
                        .content
                        .iter()
                        .filter_map(|block| match block {
                            AssistantContent::Text(text) => Some(text.text.clone()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n"),
                ),
                _ => None,
            })
            .collect()
    }

    pub fn custom_messages(&self, custom_type: &str) -> Vec<AgentMessage> {
        self.session
            .messages()
            .into_iter()
            .filter(|message| {
                matches!(message, AgentMessage::Custom(custom) if custom.custom_type == custom_type)
            })
            .collect()
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.session.dispose();
    }
}

pub fn user_content_text(content: &UserContent) -> String {
    match content {
        UserContent::Text(text) => text.clone(),
        UserContent::Blocks(blocks) => blocks
            .iter()
            .filter_map(|block| match block {
                TextOrImageContent::Text(text) => Some(text.text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

pub fn create_harness(options: HarnessOptions) -> Harness {
    let temp = tempfile::tempdir().expect("temp dir");
    let cwd = temp.path().to_string_lossy().into_owned();

    let faux = faux_provider(FauxProviderOptions {
        models: options.models.clone(),
        tokens_per_second: options.tokens_per_second,
        ..FauxProviderOptions::default()
    });
    faux.core.set_responses(Vec::new());
    let models: Vec<Model> = faux.core.models.clone();
    let model = faux.core.get_model(None).expect("default faux model");
    let with_configured_auth = options.with_configured_auth.unwrap_or(true);

    let session_manager = SessionManager::in_memory(Some(&cwd), None).expect("session manager");
    let settings: notagent::core::settings_manager::Settings = options
        .settings
        .clone()
        .map(|settings| serde_json::from_value(settings).expect("settings"))
        .unwrap_or_default();
    let settings_manager = Arc::new(SettingsManager::in_memory(
        &settings,
        SettingsManagerCreateOptions::default(),
    ));

    let core = Arc::clone(&faux.core);
    let stream_fn: StreamFn = Arc::new(move |request_model, context, request_options| {
        let core = Arc::clone(&core);
        Box::pin(async move { core.stream(&request_model, &context, request_options) })
    });

    let agent = Agent::new(AgentOptions {
        model: Some(model.clone()),
        system_prompt: Some(
            options
                .system_prompt
                .clone()
                .unwrap_or_else(|| "You are a test assistant.".to_string()),
        ),
        tools: Some(Vec::new()),
        convert_to_llm: Some(Arc::new(|messages| {
            Box::pin(async move { notagent::core::messages::convert_to_llm(&messages) })
        })),
        get_api_key: Some(Arc::new(move |_provider| {
            Box::pin(async move { with_configured_auth.then(|| "faux-key".to_string()) })
        })),
        stream_fn: Some(stream_fn),
        ..AgentOptions::default()
    });

    let session = AgentSession::new(AgentSessionConfig {
        agent,
        session_manager,
        settings_manager: Arc::clone(&settings_manager),
        cwd,
        scoped_models: Vec::new(),
        resource_loader: Arc::new(EmptyResourceLoader::default()),
        model_runtime: Arc::new(FauxModelRuntime {
            models: match options.runtime_window_override {
                Some(window) => models
                    .iter()
                    .cloned()
                    .map(|mut model| {
                        model.context_window = window;
                        model
                    })
                    .collect(),
                None => models.clone(),
            },
            with_configured_auth,
        }),
        initial_active_tool_names: options.initial_active_tool_names.clone(),
        allowed_tool_names: options.allowed_tool_names.clone(),
        excluded_tool_names: options.excluded_tool_names.clone(),
        base_tools_override: options.tools.clone(),
        hooks: options.hooks.clone(),
        permissions: options.permissions.clone(),
        session_start_reason: "startup".to_string(),
    });

    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&events);
    let subscription = session.subscribe(Arc::new(move |event| {
        sink.lock().expect("poisoned").push(event);
    }));

    Harness {
        session,
        settings_manager,
        faux: Arc::clone(&faux.core),
        models,
        events,
        temp,
        _subscription: subscription,
    }
}
