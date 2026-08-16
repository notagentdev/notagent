//! Port of `packages/coding-agent/src/modes/rpc/rpc-mode.ts`.
//!
//! Headless operation over stdin and stdout: commands in, responses and events
//! out, one JSON record per line. This is how another program embeds the agent.
//!
//! Two details carry the protocol. `prompt` answers as soon as preflight
//! accepted the message — a queued prompt is a success even though its run is
//! still going — and every other command answers after it finished. And the
//! agent waits for standard output to drain between events, so a slow reader
//! slows the run down instead of filling memory.
//!
//! Deviation (class 2): the extension UI bridge of the TypeScript (dialogs,
//! widgets, editor control over the wire) is gone with the extension system
//! (`plans/facts/extension-boundary.md` §3), and with it the
//! `extension_ui_request`/`extension_ui_response` records and the shutdown
//! handler an extension could install. What remains is the command surface.

use std::sync::Arc;

use serde_json::{Map, Value, json};
use tokio::io::AsyncReadExt;

use notagent_agent::types::QueueMode;

use crate::core::agent_session::{
    AgentSession, ContextUsage, PromptOptions, SessionStats, SessionTokenTotals,
};
use crate::core::agent_session_runtime::{AgentSessionRuntime, ForkPosition};
use crate::core::bash_executor::BashResult;
use crate::core::output_guard::{
    flush_raw_stdout, take_over_stdout, wait_for_raw_stdout_backpressure, write_raw_stdout,
};
use crate::modes::json_event::{compaction_result_to_json, to_json_event};
use crate::modes::rpc::jsonl::{JsonlLineSplitter, serialize_json_line};
use crate::modes::rpc::rpc_types::*;
use crate::utils::shell::kill_tracked_detached_children;

/// Where a record goes. The protocol writes to standard output; the ported
/// suites pass a sink instead, as the TypeScript tests mock the output guard.
pub type RpcOutputSink = Arc<dyn Fn(&Value) + Send + Sync>;

fn stdout_sink() -> RpcOutputSink {
    Arc::new(|value: &Value| write_raw_stdout(&serialize_json_line(value)))
}

fn queue_mode(mode: QueueMode) -> RpcQueueMode {
    match mode {
        QueueMode::All => RpcQueueMode::All,
        QueueMode::OneAtATime => RpcQueueMode::OneAtATime,
    }
}

fn bash_result_to_json(result: &BashResult) -> Value {
    let mut map = Map::new();
    map.insert("output".to_owned(), Value::from(result.output.clone()));
    map.insert(
        "exitCode".to_owned(),
        result.exit_code.map(Value::from).unwrap_or(Value::Null),
    );
    map.insert("cancelled".to_owned(), Value::from(result.cancelled));
    map.insert("truncated".to_owned(), Value::from(result.truncated));
    if let Some(full_output_path) = &result.full_output_path {
        map.insert(
            "fullOutputPath".to_owned(),
            Value::from(full_output_path.clone()),
        );
    }
    Value::Object(map)
}

fn token_totals_to_json(tokens: &SessionTokenTotals) -> Value {
    json!({
        "input": tokens.input,
        "output": tokens.output,
        "cacheRead": tokens.cache_read,
        "cacheWrite": tokens.cache_write,
        "total": tokens.total,
    })
}

fn context_usage_to_json(usage: &ContextUsage) -> Value {
    json!({
        "tokens": usage.tokens,
        "contextWindow": usage.context_window,
        "percent": usage.percent,
    })
}

fn session_stats_to_json(stats: &SessionStats) -> Value {
    let mut map = Map::new();
    map.insert(
        "sessionFile".to_owned(),
        stats
            .session_file
            .clone()
            .map(Value::from)
            .unwrap_or(Value::Null),
    );
    map.insert(
        "sessionId".to_owned(),
        Value::from(stats.session_id.clone()),
    );
    map.insert("userMessages".to_owned(), Value::from(stats.user_messages));
    map.insert(
        "assistantMessages".to_owned(),
        Value::from(stats.assistant_messages),
    );
    map.insert("toolCalls".to_owned(), Value::from(stats.tool_calls));
    map.insert("toolResults".to_owned(), Value::from(stats.tool_results));
    map.insert(
        "totalMessages".to_owned(),
        Value::from(stats.total_messages),
    );
    map.insert("tokens".to_owned(), token_totals_to_json(&stats.tokens));
    map.insert("cost".to_owned(), Value::from(stats.cost));
    if let Some(context_usage) = &stats.context_usage {
        map.insert(
            "contextUsage".to_owned(),
            context_usage_to_json(context_usage),
        );
    }
    Value::Object(map)
}

/// Subscriptions of the current session; replaced whenever the session is.
struct Bindings {
    events: Option<crate::core::agent_session::ListenerHandle>,
    backpressure: Option<Box<dyn FnOnce() + Send>>,
}

impl Bindings {
    fn clear(&mut self) {
        self.events = None;
        if let Some(unsubscribe) = self.backpressure.take() {
            unsubscribe();
        }
    }
}

/// The whole mutable state of one RPC run.
pub struct RpcState {
    runtime_host: Arc<AgentSessionRuntime>,
    sink: RpcOutputSink,
    bindings: std::sync::Mutex<Bindings>,
    shutting_down: std::sync::atomic::AtomicBool,
}

impl RpcState {
    /// One RPC run over an existing runtime. `sink` replaces standard output.
    pub fn new(runtime_host: Arc<AgentSessionRuntime>, sink: Option<RpcOutputSink>) -> Arc<Self> {
        Arc::new(RpcState {
            runtime_host,
            sink: sink.unwrap_or_else(stdout_sink),
            bindings: std::sync::Mutex::new(Bindings {
                events: None,
                backpressure: None,
            }),
            shutting_down: std::sync::atomic::AtomicBool::new(false),
        })
    }

    fn output(&self, value: &Value) {
        (self.sink)(value);
    }

    fn session(&self) -> Arc<AgentSession> {
        self.runtime_host.session()
    }

    /// Subscribes to the current session: events out, backpressure in.
    pub fn rebind_session(&self) {
        let session = self.session();
        let sink = Arc::clone(&self.sink);
        let mut bindings = self.bindings.lock().expect("poisoned");
        bindings.clear();
        bindings.events = Some(session.subscribe(Arc::new(move |event| {
            sink(&to_json_event(&event));
        })));
        let unsubscribe = session.agent().subscribe(Arc::new(move |_event, _signal| {
            Box::pin(async move {
                wait_for_raw_stdout_backpressure().await;
            })
        }));
        bindings.backpressure = Some(Box::new(unsubscribe));
    }

    async fn shutdown(&self, exit_code: i32, drain: bool) -> ! {
        if self
            .shutting_down
            .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            std::process::exit(exit_code);
        }
        self.bindings.lock().expect("poisoned").clear();
        self.runtime_host.dispose().await;
        if drain {
            flush_raw_stdout().await;
        }
        std::process::exit(exit_code);
    }
}

/// Runs the agent in RPC mode. Returns only through process exit.
pub async fn run_rpc_mode(runtime_host: Arc<AgentSessionRuntime>) -> ! {
    take_over_stdout();
    let state = RpcState::new(Arc::clone(&runtime_host), None);

    {
        let rebind_state = Arc::clone(&state);
        runtime_host.set_rebind_session(Some(Arc::new(move |_session| {
            let rebind_state = Arc::clone(&rebind_state);
            Box::pin(async move {
                rebind_state.rebind_session();
            })
        })));
    }
    state.rebind_session();

    // SIGTERM skips the drain: the sender wants the process gone, and a reader
    // that stopped reading would make the flush wait forever.
    let signal_state = Arc::clone(&state);
    tokio::spawn(async move {
        let Ok(mut terminate) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        else {
            return;
        };
        let Ok(mut hangup) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
        else {
            return;
        };
        let (exit_code, drain) = tokio::select! {
            _ = terminate.recv() => (143, false),
            _ = hangup.recv() => (129, true),
        };
        kill_tracked_detached_children();
        signal_state.shutdown(exit_code, drain).await;
    });

    let mut stdin = tokio::io::stdin();
    let mut splitter = JsonlLineSplitter::new();
    let mut buffer = [0u8; 8192];
    loop {
        let read = match stdin.read(&mut buffer).await {
            Ok(0) | Err(_) => {
                // End of input is the client hanging up.
                let mut lines: Vec<String> = Vec::new();
                splitter.end(|line| lines.push(line));
                for line in lines {
                    handle_input_line(&state, &line).await;
                }
                state.shutdown(0, true).await;
            }
            Ok(read) => read,
        };
        let mut lines: Vec<String> = Vec::new();
        splitter.push(&buffer[..read], |line| lines.push(line));
        for line in lines {
            handle_input_line(&state, &line).await;
        }
    }
}

/// Handles one input line, including the parse-error answer.
pub async fn handle_input_line(state: &Arc<RpcState>, line: &str) {
    let parsed: Value = match serde_json::from_str(line) {
        Ok(parsed) => parsed,
        Err(error) => {
            state.output(
                &RpcResponse::error(None, "parse", format!("Failed to parse command: {error}"))
                    .to_value(),
            );
            wait_for_raw_stdout_backpressure().await;
            return;
        }
    };

    let command = RpcCommandEnvelope::from_value(parsed);
    match handle_command(state, &command).await {
        Ok(Some(response)) => {
            state.output(&response.to_value());
            wait_for_raw_stdout_backpressure().await;
        }
        Ok(None) => {}
        Err(message) => {
            state.output(
                &RpcResponse::error(command.id.clone(), &command.command_type, message).to_value(),
            );
            wait_for_raw_stdout_backpressure().await;
        }
    }
}

/// Handles one command. `Ok(None)` means the answer follows asynchronously.
async fn handle_command(
    state: &Arc<RpcState>,
    command: &RpcCommandEnvelope,
) -> Result<Option<RpcResponse>, String> {
    let id = command.id.clone();
    let name = command.command_type.as_str();
    let session = state.session();
    let success = |data: Option<Value>| Ok(Some(RpcResponse::success(id.clone(), name, data)));

    match name {
        // =================================================================
        // Prompting
        // =================================================================
        "prompt" => {
            let payload: RpcPromptCommand = command.parse()?;
            // The authoritative response goes out as soon as preflight accepted
            // the prompt; the run itself keeps going in the background.
            let response_id = id.clone();
            let succeeded = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let preflight_succeeded = Arc::clone(&succeeded);
            let preflight_sink = Arc::clone(&state.sink);
            let error_sink = Arc::clone(&state.sink);
            let options = PromptOptions {
                images: payload.images.unwrap_or_default(),
                streaming_behavior: payload.streaming_behavior.map(Into::into),
                preflight_result: Some(Arc::new(move |did_succeed| {
                    if did_succeed {
                        preflight_succeeded.store(true, std::sync::atomic::Ordering::SeqCst);
                        preflight_sink(
                            &RpcResponse::success(response_id.clone(), "prompt", None).to_value(),
                        );
                    }
                })),
                ..PromptOptions::default()
            };
            let message = payload.message.clone();
            let error_id = id.clone();
            tokio::spawn(async move {
                if let Err(error) = session.prompt(&message, options).await
                    && !succeeded.load(std::sync::atomic::Ordering::SeqCst)
                {
                    error_sink(&RpcResponse::error(error_id, "prompt", error).to_value());
                }
            });
            Ok(None)
        }

        "steer" => {
            let payload: RpcMessageCommand = command.parse()?;
            session.steer(&payload.message, &payload.images.unwrap_or_default());
            success(None)
        }

        "follow_up" => {
            let payload: RpcMessageCommand = command.parse()?;
            session.follow_up(&payload.message, &payload.images.unwrap_or_default());
            success(None)
        }

        "abort" => {
            session.abort().await;
            success(None)
        }

        "new_session" => {
            let payload: RpcNewSessionCommand = command.parse()?;
            state
                .runtime_host
                .new_session(payload.parent_session.as_deref())
                .await?;
            success(Some(json!({ "cancelled": false })))
        }

        // =================================================================
        // State
        // =================================================================
        "get_state" => {
            let state_value = RpcSessionState {
                model: session.model(),
                thinking_level: session.thinking_level(),
                is_streaming: session.is_streaming(),
                is_compacting: session.is_compacting(),
                steering_mode: queue_mode(session.steering_mode()),
                follow_up_mode: queue_mode(session.follow_up_mode()),
                session_file: session.session_file(),
                session_id: session.session_id(),
                session_name: session.session_name(),
                auto_compaction_enabled: session.auto_compaction_enabled(),
                message_count: session.messages().len(),
                pending_message_count: session.pending_message_count(),
            };
            success(Some(
                serde_json::to_value(&state_value).map_err(|error| error.to_string())?,
            ))
        }

        // =================================================================
        // Model
        // =================================================================
        "set_model" => {
            let payload: RpcSetModelCommand = command.parse()?;
            let model = session
                .model_runtime()
                .get_available_snapshot()
                .into_iter()
                .find(|model| model.provider == payload.provider && model.id == payload.model_id);
            let Some(model) = model else {
                return Ok(Some(RpcResponse::error(
                    id,
                    name,
                    format!("Model not found: {}/{}", payload.provider, payload.model_id),
                )));
            };
            let value = serde_json::to_value(&model).map_err(|error| error.to_string())?;
            session.set_model(model).await?;
            success(Some(value))
        }

        "cycle_model" => match session.cycle_model(true) {
            None => success(Some(Value::Null)),
            Some(result) => success(Some(json!({
                "model": serde_json::to_value(&result.model).map_err(|error| error.to_string())?,
                "thinkingLevel": serde_json::to_value(result.thinking_level)
                    .map_err(|error| error.to_string())?,
                "isScoped": result.is_scoped,
            }))),
        },

        "get_available_models" => {
            let models = session.model_runtime().get_available_snapshot();
            success(Some(json!({
                "models": serde_json::to_value(&models).map_err(|error| error.to_string())?,
            })))
        }

        // =================================================================
        // Thinking
        // =================================================================
        "set_thinking_level" => {
            let payload: RpcSetThinkingLevelCommand = command.parse()?;
            session.set_thinking_level(payload.level);
            success(None)
        }

        "cycle_thinking_level" => match session.cycle_thinking_level() {
            None => success(Some(Value::Null)),
            Some(level) => success(Some(json!({
                "level": serde_json::to_value(level).map_err(|error| error.to_string())?,
            }))),
        },

        "get_available_thinking_levels" => {
            let levels = session.get_available_thinking_levels();
            success(Some(json!({
                "levels": serde_json::to_value(&levels).map_err(|error| error.to_string())?,
            })))
        }

        // =================================================================
        // Queue modes
        // =================================================================
        "set_steering_mode" => {
            let payload: RpcSetQueueModeCommand = command.parse()?;
            session.set_steering_mode(match payload.mode {
                RpcQueueMode::All => QueueMode::All,
                RpcQueueMode::OneAtATime => QueueMode::OneAtATime,
            });
            success(None)
        }

        "set_follow_up_mode" => {
            let payload: RpcSetQueueModeCommand = command.parse()?;
            session.set_follow_up_mode(match payload.mode {
                RpcQueueMode::All => QueueMode::All,
                RpcQueueMode::OneAtATime => QueueMode::OneAtATime,
            });
            success(None)
        }

        // =================================================================
        // Compaction
        // =================================================================
        "compact" => {
            let payload: RpcCompactCommand = command.parse()?;
            let result = session
                .compact(payload.custom_instructions.as_deref())
                .await?;
            success(Some(compaction_result_to_json(&result)))
        }

        "set_auto_compaction" => {
            let payload: RpcSetEnabledCommand = command.parse()?;
            session.set_auto_compaction_enabled(payload.enabled);
            success(None)
        }

        // =================================================================
        // Retry
        // =================================================================
        "set_auto_retry" => {
            let payload: RpcSetEnabledCommand = command.parse()?;
            session.set_auto_retry_enabled(payload.enabled);
            success(None)
        }

        "abort_retry" => {
            session.abort_retry();
            success(None)
        }

        // =================================================================
        // Bash
        // =================================================================
        "bash" => {
            let payload: RpcBashCommand = command.parse()?;
            let result = session
                .execute_bash(
                    &payload.command,
                    None,
                    crate::core::agent_session::ExecuteBashOptions {
                        exclude_from_context: payload.exclude_from_context.unwrap_or(false),
                        id: id.clone(),
                        operations: None,
                    },
                )
                .await;
            success(Some(bash_result_to_json(&result)))
        }

        "abort_bash" => {
            session.abort_bash();
            success(None)
        }

        // =================================================================
        // Session
        // =================================================================
        "get_session_stats" => success(Some(session_stats_to_json(&session.get_session_stats()))),

        "export_html" => Ok(Some(RpcResponse::error(
            id,
            name,
            "HTML export is not available in this build",
        ))),

        "switch_session" => {
            let payload: RpcSwitchSessionCommand = command.parse()?;
            state
                .runtime_host
                .switch_session(&payload.session_path, None)
                .await
                .map_err(|error| error.to_string())?;
            success(Some(json!({ "cancelled": false })))
        }

        "fork" => {
            let payload: RpcForkCommand = command.parse()?;
            let selected_text = state
                .runtime_host
                .fork(&payload.entry_id, ForkPosition::Before)
                .await?;
            success(Some(json!({
                "text": selected_text.unwrap_or_default(),
                "cancelled": false,
            })))
        }

        "clone" => {
            let leaf_id =
                session.with_session_manager(|manager| manager.get_leaf_id().map(str::to_owned));
            let Some(leaf_id) = leaf_id else {
                return Ok(Some(RpcResponse::error(
                    id,
                    name,
                    "Cannot clone session: no current entry selected",
                )));
            };
            state.runtime_host.fork(&leaf_id, ForkPosition::At).await?;
            success(Some(json!({ "cancelled": false })))
        }

        "get_fork_messages" => {
            let messages: Vec<Value> = session
                .get_user_messages_for_forking()
                .into_iter()
                .map(|(entry_id, text)| json!({ "entryId": entry_id, "text": text }))
                .collect();
            success(Some(json!({ "messages": messages })))
        }

        "get_entries" => {
            let payload: RpcGetEntriesCommand = command.parse()?;
            let (entries, leaf_id) = session.with_session_manager(|manager| {
                (
                    manager.get_entries(),
                    manager.get_leaf_id().map(str::to_owned),
                )
            });
            let entries = match payload.since.as_deref() {
                None => entries,
                Some(since) => {
                    let Some(index) = entries.iter().position(|entry| entry.id() == since) else {
                        return Ok(Some(RpcResponse::error(
                            id,
                            name,
                            format!("Entry not found: {since}"),
                        )));
                    };
                    entries[index + 1..].to_vec()
                }
            };
            success(Some(json!({
                "entries": serde_json::to_value(&entries).map_err(|error| error.to_string())?,
                "leafId": leaf_id,
            })))
        }

        "get_tree" => {
            let (tree, leaf_id) = session.with_session_manager(|manager| {
                (manager.get_tree(), manager.get_leaf_id().map(str::to_owned))
            });
            success(Some(json!({
                "tree": session_tree_to_json(&tree),
                "leafId": leaf_id,
            })))
        }

        "get_last_assistant_text" => success(Some(json!({
            "text": session.get_last_assistant_text(),
        }))),

        "set_session_name" => {
            let payload: RpcSetSessionNameCommand = command.parse()?;
            let session_name = payload.name.trim();
            if session_name.is_empty() {
                return Ok(Some(RpcResponse::error(
                    id,
                    name,
                    "Session name cannot be empty",
                )));
            }
            session.set_session_name(session_name);
            success(None)
        }

        // =================================================================
        // Messages
        // =================================================================
        "get_messages" => success(Some(json!({
            "messages": serde_json::to_value(session.messages())
                .map_err(|error| error.to_string())?,
        }))),

        // =================================================================
        // Commands available for invocation via prompt
        // =================================================================
        "get_commands" => {
            let mut commands: Vec<RpcSlashCommand> = Vec::new();
            for template in session.prompt_templates() {
                commands.push(RpcSlashCommand {
                    name: template.name.clone(),
                    description: Some(template.description.clone()),
                    source: "prompt",
                    source_info: template.source_info.clone(),
                });
            }
            for skill in session.resource_loader().get_skills().0 {
                commands.push(RpcSlashCommand {
                    name: format!("skill:{}", skill.name),
                    description: Some(skill.description.clone()),
                    source: "skill",
                    source_info: skill.source_info.clone(),
                });
            }
            success(Some(json!({
                "commands": serde_json::to_value(&commands).map_err(|error| error.to_string())?,
            })))
        }

        unknown => Ok(Some(RpcResponse::error(
            id,
            unknown,
            format!("Unknown command: {unknown}"),
        ))),
    }
}

fn session_tree_to_json(nodes: &[crate::core::session_manager::SessionTreeNode]) -> Value {
    Value::Array(
        nodes
            .iter()
            .map(|node| {
                let mut map = Map::new();
                map.insert(
                    "entry".to_owned(),
                    serde_json::to_value(&node.entry).unwrap_or(Value::Null),
                );
                map.insert("children".to_owned(), session_tree_to_json(&node.children));
                if let Some(label) = &node.label {
                    map.insert("label".to_owned(), Value::from(label.clone()));
                }
                if let Some(label_timestamp) = &node.label_timestamp {
                    map.insert(
                        "labelTimestamp".to_owned(),
                        Value::from(label_timestamp.clone()),
                    );
                }
                Value::Object(map)
            })
            .collect(),
    )
}
