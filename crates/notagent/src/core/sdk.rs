use std::sync::Arc;

use notagent_agent::agent::{Agent, AgentOptions};
use notagent_agent::types::{AgentMessage, StreamFn, ThinkingLevel};
use notagent_ai::types::{Message, Model, TextContent, TextOrImageContent, UserContent};

use crate::core::agent_session::{
    AgentSession, AgentSessionConfig, ScopedModel, SessionModelRuntime, parse_thinking_level,
};
use crate::core::auth_guidance::format_no_models_available_message;
use crate::core::defaults::DEFAULT_THINKING_LEVEL;
use crate::core::hooks::dispatch::HookDispatcher;
use crate::core::messages::convert_to_llm;
use crate::core::model_resolver::{FindInitialModelOptions, find_initial_model};
use crate::core::model_runtime::{
    ModelRuntime, ModelsRequestTransforms, ModelsSimpleStreamOptions,
};
use crate::core::permissions::gate::PermissionGate;
use crate::core::provider_attribution::merge_provider_attribution_headers;
use crate::core::resource_loader::ResourceLoader;
use crate::core::session_manager::SessionManager;
use crate::core::settings_manager::SettingsManager;
use crate::core::tools::ToolName;

/// Inputs of [`create_agent_session`].
pub struct CreateAgentSessionOptions {
    pub cwd: String,
    pub agent_dir: String,
    pub model_runtime: Arc<ModelRuntime>,
    pub settings_manager: Arc<SettingsManager>,
    pub resource_loader: Arc<dyn ResourceLoader>,
    pub session_manager: SessionManager,
    /// Model to use. Default: from the session, then from settings, then the
    /// first available.
    pub model: Option<Model>,
    pub thinking_level: Option<ThinkingLevel>,
    /// Models available for cycling (`--models`).
    pub scoped_models: Vec<ScopedModel>,
    /// Allowlist of tool names. When set, only these are enabled.
    pub tools: Option<Vec<String>>,
    /// Denylist, applied after `tools`.
    pub exclude_tools: Option<Vec<String>>,
    /// `"all"` starts with no tools, `"builtin"` disables the built-ins only.
    pub no_tools: Option<NoTools>,
    pub hooks: Option<Arc<HookDispatcher>>,
    /// extension the app registers ahead of every other one; here the app hands
    /// the gate in and it becomes the agent's `before_tool_call`.
    pub permissions: Option<Arc<PermissionGate>>,
    pub session_start_reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoTools {
    All,
    Builtin,
}

pub struct CreateAgentSessionResult {
    pub session: Arc<AgentSession>,
    /// Warning when the session could not be restored with the model it recorded.
    pub model_fallback_message: Option<String>,
}

/// Creates an `AgentSession` with the given options.
pub async fn create_agent_session(options: CreateAgentSessionOptions) -> CreateAgentSessionResult {
    let settings_manager = Arc::clone(&options.settings_manager);
    let model_runtime = Arc::clone(&options.model_runtime);
    let mut session_manager = options.session_manager;

    let existing = session_manager.build_session_context();
    let has_existing_session = !existing.messages.is_empty();
    let has_thinking_entry = session_manager
        .get_branch(None)
        .iter()
        .any(|entry| entry.entry_type() == "thinking_level_change");

    let mut model = options.model;
    let mut model_fallback_message: Option<String> = None;

    // A resumed session names the model it was using; use it when its
    // credentials are still configured, and say so when they are not.
    if model.is_none()
        && has_existing_session
        && let Some(session_model) = existing.model.as_ref()
    {
        let restored = model_runtime.get_model(&session_model.provider, &session_model.model_id);
        if let Some(restored) = restored
            && model_runtime.has_configured_auth(&restored.provider)
        {
            model = Some(restored);
        }
        if model.is_none() {
            model_fallback_message = Some(format!(
                "Could not restore model {}/{}",
                session_model.provider, session_model.model_id
            ));
        }
    }

    if model.is_none() {
        let default_thinking = settings_manager
            .get_default_thinking_level()
            .and_then(|level| parse_thinking_level(&level));
        let default_provider = settings_manager.get_default_provider();
        let default_model_id = settings_manager.get_default_model();
        let result = find_initial_model(FindInitialModelOptions {
            cli_provider: None,
            cli_model: None,
            scoped_models: &[],
            is_continuing: has_existing_session,
            default_provider: default_provider.as_deref(),
            default_model_id: default_model_id.as_deref(),
            default_thinking_level: default_thinking,
            model_runtime: model_runtime.as_ref(),
        });
        match result {
            Ok(result) => {
                model = result.model;
                match model.as_ref() {
                    None => model_fallback_message = Some(format_no_models_available_message()),
                    Some(model) => {
                        if let Some(previous) = model_fallback_message.take() {
                            model_fallback_message =
                                Some(format!("{previous}. Using {}/{}", model.provider, model.id));
                        }
                    }
                }
            }
            Err(error) => model_fallback_message = Some(error),
        }
    }

    let mut thinking_level = options.thinking_level;
    if thinking_level.is_none() && has_existing_session {
        thinking_level = Some(if has_thinking_entry {
            parse_thinking_level(&existing.thinking_level).unwrap_or(DEFAULT_THINKING_LEVEL)
        } else {
            settings_manager
                .get_default_thinking_level()
                .and_then(|level| parse_thinking_level(&level))
                .unwrap_or(DEFAULT_THINKING_LEVEL)
        });
    }
    let mut thinking_level = thinking_level.unwrap_or_else(|| {
        settings_manager
            .get_default_thinking_level()
            .and_then(|level| parse_thinking_level(&level))
            .unwrap_or(DEFAULT_THINKING_LEVEL)
    });
    thinking_level = match model.as_ref() {
        None => ThinkingLevel::Off,
        Some(model) => clamp(model, thinking_level),
    };

    let excluded: Option<Vec<String>> = options.exclude_tools.clone();
    let allowed_tool_names = options
        .tools
        .clone()
        .or_else(|| (options.no_tools == Some(NoTools::All)).then(Vec::new));
    // Left unset when the caller named no tools, so the session derives them
    // from the active mode's shell. A fixed default here would silently outrank
    // the shell, and the first turn of a read-only mode would come with the
    // shell and the write tools the mode exists to withhold.
    let initial_active_tool_names: Option<Vec<String>> = match options.tools.clone() {
        Some(tools) => Some(
            tools
                .into_iter()
                .filter(|name| {
                    !excluded
                        .as_ref()
                        .is_some_and(|excluded| excluded.contains(name))
                })
                .collect(),
        ),
        None => options.no_tools.map(|_| Vec::new()),
    };
    // `noTools: "builtin"` keeps everything that is not a built-in; with the
    // extension system gone that is the empty set, which is what the branch
    // above already produces.
    let _ = ToolName::Read;

    let stream_fn = build_stream_fn(
        Arc::clone(&model_runtime),
        Arc::clone(&settings_manager),
        session_manager.get_session_id().to_string(),
    );

    let block_images_settings = Arc::clone(&settings_manager);
    let permissions = options.permissions.clone();
    let before_tool_call: Option<notagent_agent::types::BeforeToolCallFn> =
        permissions.map(|permissions| {
            Arc::new(
                move |context: notagent_agent::types::BeforeToolCallContext,
                      signal: Option<tokio_util::sync::CancellationToken>| {
                    let permissions = Arc::clone(&permissions);
                    Box::pin(async move {
                        let input = context.args.as_object().cloned().unwrap_or_default();
                        let block = permissions
                            .before_tool_call(
                                crate::core::permissions::hook::PermissionCall {
                                    tool_call_id: context.tool_call.id,
                                    tool_name: context.tool_call.name,
                                    input,
                                },
                                signal.as_ref(),
                            )
                            .await?;
                        Some(notagent_agent::types::BeforeToolCallResult {
                            block: Some(true),
                            reason: block.reason,
                            terminate: Some(block.terminate),
                        })
                    })
                        as notagent_agent::types::BoxFuture<
                            'static,
                            Option<notagent_agent::types::BeforeToolCallResult>,
                        >
                },
            ) as notagent_agent::types::BeforeToolCallFn
        });
    let agent = Agent::new(AgentOptions {
        system_prompt: Some(String::new()),
        model: model.clone(),
        thinking_level: Some(thinking_level),
        tools: Some(Vec::new()),
        convert_to_llm: Some(Arc::new(move |messages: Vec<AgentMessage>| {
            let settings = Arc::clone(&block_images_settings);
            Box::pin(async move { convert_with_block_images(&messages, &settings) })
        })),
        stream_fn: Some(stream_fn),
        before_tool_call,
        session_id: Some(session_manager.get_session_id().to_string()),
        steering_mode: Some(agent_queue_mode(settings_manager.get_steering_mode())),
        follow_up_mode: Some(agent_queue_mode(settings_manager.get_follow_up_mode())),
        transport: parse_transport(&settings_manager.get_transport()),
        thinking_budgets: settings_manager.get_thinking_budgets().map(|budgets| {
            notagent_ai::types::ThinkingBudgets {
                minimal: budgets.minimal,
                low: budgets.low,
                medium: budgets.medium,
                high: budgets.high,
            }
        }),
        max_retry_delay_ms: Some(
            settings_manager
                .get_provider_retry_settings()
                .max_retry_delay_ms,
        ),
        ..AgentOptions::default()
    });

    // Restore the transcript, or record the starting point of a new session so
    // resuming it can restore the same two things.
    if has_existing_session {
        agent.set_messages(existing.messages.clone());
        if !has_thinking_entry {
            let _ =
                session_manager.append_thinking_level_change(thinking_level_name(thinking_level));
        }
    } else {
        if let Some(model) = model.as_ref() {
            let _ = session_manager.append_model_change(&model.provider, &model.id);
        }
        let _ = session_manager.append_thinking_level_change(thinking_level_name(thinking_level));
    }

    let session = AgentSession::new(AgentSessionConfig {
        agent,
        session_manager,
        settings_manager,
        cwd: options.cwd,
        scoped_models: options.scoped_models,
        resource_loader: options.resource_loader,
        model_runtime: model_runtime as Arc<dyn SessionModelRuntime>,
        initial_active_tool_names,
        allowed_tool_names,
        excluded_tool_names: excluded,
        base_tools_override: None,
        hooks: options.hooks,
        permissions: options.permissions,
        session_start_reason: options.session_start_reason,
    });

    CreateAgentSessionResult {
        session,
        model_fallback_message,
    }
}

fn clamp(model: &Model, level: ThinkingLevel) -> ThinkingLevel {
    ThinkingLevel::from(notagent_ai::clamp_thinking_level(
        model,
        model_thinking_level(level),
    ))
}

fn model_thinking_level(level: ThinkingLevel) -> notagent_ai::types::ModelThinkingLevel {
    use notagent_ai::types::ModelThinkingLevel;
    match level {
        ThinkingLevel::Off => ModelThinkingLevel::Off,
        ThinkingLevel::Minimal => ModelThinkingLevel::Minimal,
        ThinkingLevel::Low => ModelThinkingLevel::Low,
        ThinkingLevel::Medium => ModelThinkingLevel::Medium,
        ThinkingLevel::High => ModelThinkingLevel::High,
        ThinkingLevel::Xhigh => ModelThinkingLevel::Xhigh,
        ThinkingLevel::Max => ModelThinkingLevel::Max,
    }
}

fn thinking_level_name(level: ThinkingLevel) -> &'static str {
    match level {
        ThinkingLevel::Off => "off",
        ThinkingLevel::Minimal => "minimal",
        ThinkingLevel::Low => "low",
        ThinkingLevel::Medium => "medium",
        ThinkingLevel::High => "high",
        ThinkingLevel::Xhigh => "xhigh",
        ThinkingLevel::Max => "max",
    }
}

fn agent_queue_mode(
    mode: crate::core::settings_manager::QueueMode,
) -> notagent_agent::types::QueueMode {
    match mode {
        crate::core::settings_manager::QueueMode::All => notagent_agent::types::QueueMode::All,
        crate::core::settings_manager::QueueMode::OneAtATime => {
            notagent_agent::types::QueueMode::OneAtATime
        }
    }
}

/// Defence in depth for `blockImages`: the setting is read per call, so a
/// mid-session change takes effect, and an image that got past the tools is
/// still replaced before the provider sees it.
fn convert_with_block_images(
    messages: &[AgentMessage],
    settings_manager: &SettingsManager,
) -> Vec<Message> {
    let converted = convert_to_llm(messages);
    if !settings_manager.get_block_images() {
        return converted;
    }
    converted
        .into_iter()
        .map(|message| match message {
            Message::User(mut user) => {
                user.content = strip_images(user.content);
                Message::User(user)
            }
            Message::ToolResult(mut result) => {
                result.content = strip_image_blocks(result.content);
                Message::ToolResult(result)
            }
            other => other,
        })
        .collect()
}

const BLOCKED_IMAGE_TEXT: &str = "Image reading is disabled.";

fn strip_images(content: UserContent) -> UserContent {
    match content {
        UserContent::Blocks(blocks) => UserContent::Blocks(strip_image_blocks(blocks)),
        text => text,
    }
}

fn strip_image_blocks(blocks: Vec<TextOrImageContent>) -> Vec<TextOrImageContent> {
    if !blocks
        .iter()
        .any(|block| matches!(block, TextOrImageContent::Image(_)))
    {
        return blocks;
    }
    let replaced: Vec<TextOrImageContent> = blocks
        .into_iter()
        .map(|block| match block {
            TextOrImageContent::Image(_) => {
                TextOrImageContent::Text(TextContent::new(BLOCKED_IMAGE_TEXT))
            }
            text => text,
        })
        .collect();
    // Consecutive placeholders collapse to one, so a message of ten images does
    // not become ten identical lines.
    let mut deduped: Vec<TextOrImageContent> = Vec::with_capacity(replaced.len());
    for block in replaced {
        let is_placeholder = matches!(
            &block,
            TextOrImageContent::Text(text) if text.text == BLOCKED_IMAGE_TEXT
        );
        let previous_is_placeholder = matches!(
            deduped.last(),
            Some(TextOrImageContent::Text(text)) if text.text == BLOCKED_IMAGE_TEXT
        );
        if is_placeholder && previous_is_placeholder {
            continue;
        }
        deduped.push(block);
    }
    deduped
}

/// The provider path: retry budgets, timeouts and attribution headers, resolved
/// per request so a settings change takes effect without a restart.
fn build_stream_fn(
    model_runtime: Arc<ModelRuntime>,
    settings_manager: Arc<SettingsManager>,
    default_session_id: String,
) -> StreamFn {
    Arc::new(move |model, context, options| {
        let model_runtime = Arc::clone(&model_runtime);
        let settings_manager = Arc::clone(&settings_manager);
        let default_session_id = default_session_id.clone();
        Box::pin(async move {
            let provider_retry = settings_manager.get_provider_retry_settings();
            let http_idle_timeout_ms = settings_manager.get_http_idle_timeout_ms().unwrap_or(0);
            // An SDK reads `timeout: 0` as "time out immediately", not as "no
            // timeout"; the largest int32 is the way to say the latter.
            let effective_timeout_ms = if http_idle_timeout_ms == 0 {
                2_147_483_647
            } else {
                http_idle_timeout_ms
            };

            let mut request = options.unwrap_or_default();
            if request.base.base.timeout_ms.is_none() {
                request.base.base.timeout_ms =
                    Some(provider_retry.timeout_ms.unwrap_or(effective_timeout_ms));
            }
            if request.base.websocket_connect_timeout_ms.is_none() {
                request.base.websocket_connect_timeout_ms = settings_manager
                    .get_websocket_connect_timeout_ms()
                    .unwrap_or(None);
            }
            if request.base.base.max_retries.is_none() {
                request.base.base.max_retries =
                    provider_retry.max_retries.map(|value| value as u32);
            }
            if request.base.base.max_retry_delay_ms.is_none() {
                request.base.base.max_retry_delay_ms = Some(provider_retry.max_retry_delay_ms);
            }

            let session_id = request
                .base
                .session_id
                .clone()
                .unwrap_or(default_session_id);
            let attribution_model = model.clone();
            let attribution_settings = Arc::clone(&settings_manager);
            let transforms = ModelsRequestTransforms {
                transform_headers: Some(Arc::new(move |headers| {
                    let merged = merge_provider_attribution_headers(
                        &attribution_model,
                        &attribution_settings,
                        Some(&session_id),
                        &[Some(headers)],
                    );
                    Box::pin(async move { merged.unwrap_or_default() })
                })),
            };

            model_runtime.stream_simple(
                &model,
                &context,
                Some(ModelsSimpleStreamOptions {
                    options: request,
                    transforms,
                }),
            )
        })
    })
}

/// The transport setting is a string in settings.json and an enum here; an
fn parse_transport(value: &str) -> Option<notagent_ai::types::Transport> {
    use notagent_ai::types::Transport;
    match value {
        "sse" => Some(Transport::Sse),
        "websocket" => Some(Transport::Websocket),
        "websocket-cached" => Some(Transport::WebsocketCached),
        "auto" => Some(Transport::Auto),
        _ => None,
    }
}
