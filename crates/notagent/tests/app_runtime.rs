#![allow(dead_code)]

use std::sync::Arc;

use notagent::core::agent_session::AgentSession;
use notagent::core::agent_session_runtime::{
    AgentSessionRuntime, CreateAgentSessionRuntimeResult, create_agent_session_runtime,
};
use notagent::core::agent_session_services::{
    CreateAgentSessionFromServicesOptions, CreateAgentSessionServicesOptions,
    create_agent_session_from_services, create_agent_session_services,
};
use notagent::core::auth_storage::{AuthStorage, AuthStorageData};
use notagent::core::hooks::dispatch::HookDispatcher;
use notagent::core::model_runtime::{CreateModelRuntimeOptions, ModelRuntime};
use notagent::core::models_store::InMemoryCodingAgentModelsStore;
use notagent::core::resource_loader::DefaultResourceLoaderOptions;
use notagent::core::session_manager::SessionManager;
use notagent::core::settings_manager::{SettingsManager, SettingsManagerCreateOptions};
use notagent_ai::providers::faux::{
    FauxCore, FauxProviderOptions, FauxResponseStep, faux_assistant_message, faux_provider,
    faux_text, faux_tool_call,
};
use notagent_ai::types::StopReason;
use serde_json::json;

pub fn reply(text: &str) -> FauxResponseStep {
    faux_assistant_message(vec![faux_text(text)], StopReason::Stop).into()
}

pub fn tool_call_reply(name: &str, id: &str, arguments: serde_json::Value) -> FauxResponseStep {
    faux_assistant_message(
        vec![faux_tool_call(name, arguments, Some(id.to_string()))],
        StopReason::ToolUse,
    )
    .into()
}

/// A response that ends in an error, as a provider failure reaches the session.
pub fn error_reply(message: &str) -> FauxResponseStep {
    let mut assistant = faux_assistant_message(Vec::new(), StopReason::Error);
    assistant.error_message = Some(message.to_owned());
    assistant.into()
}

pub struct HeadlessApp {
    runtime: Arc<AgentSessionRuntime>,
    faux: Arc<FauxCore>,
    temp: tempfile::TempDir,
}

impl HeadlessApp {
    pub async fn create() -> Self {
        Self::build(true, None, None, None).await
    }

    pub async fn create_with_hooks(hooks: Arc<HookDispatcher>) -> Self {
        Self::build(true, None, None, Some(hooks)).await
    }

    /// With a llama.cpp credential pointing at `base_url`, so `/llama` finds a
    /// configured server.
    pub async fn create_with_llama(base_url: &str) -> Self {
        Self::build(true, None, Some(base_url.to_owned()), None).await
    }

    /// A model whose provider has no credentials at all, so prompt preflight
    /// refuses before a request is made.
    pub async fn create_without_auth() -> Self {
        Self::build(false, None, None, None).await
    }

    /// With a provider that streams slowly enough to still be running when the
    /// next command arrives.
    pub async fn create_slow(tokens_per_second: f64) -> Self {
        Self::build(true, Some(tokens_per_second), None, None).await
    }

    async fn build(
        with_auth: bool,
        tokens_per_second: Option<f64>,
        llama_base_url: Option<String>,
        hooks: Option<Arc<HookDispatcher>>,
    ) -> Self {
        let temp = tempfile::tempdir().expect("temp dir");
        let cwd = temp.path().join("project");
        let agent_dir = temp.path().join("agent");
        std::fs::create_dir_all(&cwd).expect("cwd");
        std::fs::create_dir_all(&agent_dir).expect("agent dir");
        let cwd = cwd.to_string_lossy().into_owned();
        let agent_dir = agent_dir.to_string_lossy().into_owned();

        let mut stored = if with_auth {
            json!({ "faux": { "type": "api_key", "key": "faux-key" } })
                .as_object()
                .cloned()
                .unwrap_or_else(AuthStorageData::new)
        } else {
            AuthStorageData::new()
        };
        if let Some(base_url) = llama_base_url {
            stored.insert(
                "llama.cpp".to_owned(),
                json!({ "type": "api_key", "key": "local", "env": { "LLAMA_BASE_URL": base_url } }),
            );
        }
        let credentials = Arc::new(AuthStorage::in_memory(stored));
        let model_runtime = ModelRuntime::create(CreateModelRuntimeOptions {
            credentials: Some(credentials),
            models_path: Some(None),
            models_store: Some(Arc::new(InMemoryCodingAgentModelsStore::new())),
            allow_model_network: Some(false),
            refresh_on_create: Some(false),
            ..CreateModelRuntimeOptions::default()
        })
        .await
        .expect("model runtime");

        let faux = faux_provider(FauxProviderOptions {
            tokens_per_second,
            ..FauxProviderOptions::default()
        });
        faux.core.set_responses(Vec::new());
        model_runtime
            .register_native_provider(faux.provider)
            .expect("register faux provider");
        let model = if with_auth {
            model_runtime
                .get_models(Some("faux"))
                .into_iter()
                .next()
                .expect("faux model")
        } else {
            // A provider nothing knows: `checkAuth` finds no credential and the
            // session refuses the prompt with the no-API-key guidance.
            notagent_ai::types::Model {
                id: "fake-model".to_owned(),
                name: "Fake Model".to_owned(),
                api: "openai-completions".to_owned(),
                provider: "fake-provider".to_owned(),
                base_url: "https://example.invalid".to_owned(),
                reasoning: false,
                thinking_level_map: None,
                input: Vec::new(),
                cost: notagent_ai::types::ModelCost {
                    input: 0.0,
                    output: 0.0,
                    cache_read: 0.0,
                    cache_write: 0.0,
                    tiers: None,
                },
                context_window: 0,
                max_tokens: 0,
                sampling_params: None,
                headers: None,
                compat: None,
            }
        };

        let settings_manager = Arc::new(SettingsManager::create(
            std::path::Path::new(&cwd),
            Some(std::path::Path::new(&agent_dir)),
            SettingsManagerCreateOptions::default(),
        ));

        let factory_cwd = cwd.clone();
        let factory_agent_dir = agent_dir.clone();
        let factory_model_runtime = Arc::clone(&model_runtime);
        let factory_settings = Arc::clone(&settings_manager);
        let factory_model = model.clone();
        let factory_hooks = hooks;
        let create_runtime: notagent::core::agent_session_runtime::CreateAgentSessionRuntimeFactory =
            Arc::new(move |input| {
                let cwd = factory_cwd.clone();
                let agent_dir = factory_agent_dir.clone();
                let model_runtime = Arc::clone(&factory_model_runtime);
                let settings_manager = Arc::clone(&factory_settings);
                let model = factory_model.clone();
                let hooks = factory_hooks.clone();
                Box::pin(async move {
                    let services =
                        create_agent_session_services(CreateAgentSessionServicesOptions {
                            cwd: cwd.clone(),
                            agent_dir: Some(agent_dir.clone()),
                            settings_manager: Some(settings_manager),
                            model_runtime: Some(model_runtime),
                            resource_loader_options: Some(DefaultResourceLoaderOptions {
                                cwd,
                                agent_dir,
                                no_skills: true,
                                no_prompt_templates: true,
                                no_themes: true,
                                no_context_files: true,
                                ..DefaultResourceLoaderOptions::default()
                            }),
                            ..CreateAgentSessionServicesOptions::default()
                        })
                        .await?;
                    let created = create_agent_session_from_services(
                        &services,
                        CreateAgentSessionFromServicesOptions {
                            session_manager: input.session_manager,
                            model: Some(model),
                            thinking_level: None,
                            scoped_models: Vec::new(),
                            tools: None,
                            exclude_tools: None,
                            no_tools: None,
                            hooks,
                            permissions: None,
                            session_start_reason: input.reason.as_str().to_owned(),
                        },
                    )
                    .await;
                    Ok(CreateAgentSessionRuntimeResult {
                        session: created.session,
                        services: Arc::new(services),
                        diagnostics: Vec::new(),
                        model_fallback_message: created.model_fallback_message,
                    })
                })
                    as futures::future::BoxFuture<
                        'static,
                        Result<CreateAgentSessionRuntimeResult, String>,
                    >
            });

        let session_manager = SessionManager::create(&cwd, None, None).expect("session manager");
        let runtime = create_agent_session_runtime(
            create_runtime,
            cwd.clone(),
            agent_dir.clone(),
            session_manager,
        )
        .await
        .expect("runtime");

        HeadlessApp {
            runtime: Arc::new(runtime),
            faux: Arc::clone(&faux.core),
            temp,
        }
    }

    pub fn runtime(&self) -> Arc<AgentSessionRuntime> {
        Arc::clone(&self.runtime)
    }

    pub fn session(&self) -> Arc<AgentSession> {
        self.runtime.session()
    }

    pub fn faux(&self) -> &FauxCore {
        &self.faux
    }

    /// The project directory of this run.
    pub fn cwd(&self) -> String {
        self.temp
            .path()
            .join("project")
            .to_string_lossy()
            .into_owned()
    }

    /// The agent directory of this run (`~/.notagent/agent` stands in here).
    pub fn agent_dir(&self) -> String {
        self.temp
            .path()
            .join("agent")
            .to_string_lossy()
            .into_owned()
    }

    /// A path inside the project directory of this run.
    pub fn path(&self, name: &str) -> String {
        self.temp
            .path()
            .join("project")
            .join(name)
            .to_string_lossy()
            .into_owned()
    }
}
