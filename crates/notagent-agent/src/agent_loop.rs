use std::sync::Arc;

use notagent_ai::types::{
    AssistantMessage, Context, StopReason, TextContent, TextOrImageContent, ToolResultMessage,
    Usage,
};
use notagent_ai::utils::event_stream::EventStream;
use notagent_ai::utils::validation::validate_tool_arguments;
use serde_json::Value;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::repeat_breaker::{RepeatBreaker, RepeatNudge, handoff_refusal_text};
use crate::stream_fn::get_default_stream_fn;
use crate::types::{
    AfterToolCallContext, AgentContext, AgentEvent, AgentLoopConfig, AgentMessage, AgentTool,
    AgentToolCall, AgentToolResult, AgentToolUpdateCallback, BeforeToolCallContext, BoxFuture,
    PrepareNextTurnContext, ShouldStopAfterTurnContext, StreamFn, ToolExecutionMode,
};

/// `AgentEventSink` — every event is awaited before the loop continues.
pub type AgentEventSink = Arc<dyn Fn(AgentEvent) -> BoxFuture<'static, ()> + Send + Sync>;

/// The loop's event stream; terminated by `agent_end`.
pub type AgentEventStream = EventStream<AgentEvent, Vec<AgentMessage>>;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct AgentLoopError(pub String);

fn create_agent_stream() -> AgentEventStream {
    EventStream::new(
        |event| matches!(event, AgentEvent::AgentEnd { .. }),
        |event| match event {
            AgentEvent::AgentEnd { messages } => messages.clone(),
            _ => Vec::new(),
        },
    )
}

/// `agentLoop(prompts, context, config, signal, streamFn)`
pub fn agent_loop(
    prompts: Vec<AgentMessage>,
    context: AgentContext,
    config: AgentLoopConfig,
    signal: Option<CancellationToken>,
    stream_fn: Option<StreamFn>,
) -> AgentEventStream {
    let stream = create_agent_stream();
    let sink_stream = stream.clone();
    let emit: AgentEventSink = Arc::new(move |event| {
        let stream = sink_stream.clone();
        Box::pin(async move { stream.push(event) })
    });

    let end_stream = stream.clone();
    tokio::spawn(async move {
        let messages = run_agent_loop(prompts, context, config, emit, signal, stream_fn).await;
        end_stream.end(Some(messages));
    });
    stream
}

/// `agentLoopContinue(context, config, signal, streamFn)`
pub fn agent_loop_continue(
    context: AgentContext,
    config: AgentLoopConfig,
    signal: Option<CancellationToken>,
    stream_fn: Option<StreamFn>,
) -> Result<AgentEventStream, AgentLoopError> {
    check_continue_preconditions(&context)?;

    let stream = create_agent_stream();
    let sink_stream = stream.clone();
    let emit: AgentEventSink = Arc::new(move |event| {
        let stream = sink_stream.clone();
        Box::pin(async move { stream.push(event) })
    });

    let end_stream = stream.clone();
    tokio::spawn(async move {
        let messages = run_agent_loop_continue(context, config, emit, signal, stream_fn)
            .await
            .unwrap_or_default();
        end_stream.end(Some(messages));
    });
    Ok(stream)
}

fn check_continue_preconditions(context: &AgentContext) -> Result<(), AgentLoopError> {
    if context.messages.is_empty() {
        return Err(AgentLoopError(
            "Cannot continue: no messages in context".to_string(),
        ));
    }
    if matches!(context.messages.last(), Some(AgentMessage::Assistant(_))) {
        return Err(AgentLoopError(
            "Cannot continue from message role: assistant".to_string(),
        ));
    }
    Ok(())
}

/// `runAgentLoop(...)`
pub async fn run_agent_loop(
    prompts: Vec<AgentMessage>,
    context: AgentContext,
    config: AgentLoopConfig,
    emit: AgentEventSink,
    signal: Option<CancellationToken>,
    stream_fn: Option<StreamFn>,
) -> Vec<AgentMessage> {
    let mut new_messages = prompts.clone();
    let mut current_context = AgentContext {
        messages: context
            .messages
            .iter()
            .cloned()
            .chain(prompts.iter().cloned())
            .collect(),
        ..context
    };

    emit(AgentEvent::AgentStart).await;
    emit(AgentEvent::TurnStart).await;
    for prompt in &prompts {
        emit(AgentEvent::MessageStart {
            message: prompt.clone(),
        })
        .await;
        emit(AgentEvent::MessageEnd {
            message: prompt.clone(),
        })
        .await;
    }

    let stream_fn = stream_fn.or_else(|| get_default_stream_fn().ok());
    if let Some(stream_fn) = stream_fn {
        // Fresh prompts are answered on their own first; steering typed in the
        // meantime joins after the first completed step.
        run_loop(
            &mut current_context,
            &mut new_messages,
            config,
            signal,
            emit,
            stream_fn,
            false,
        )
        .await;
    }
    new_messages
}

/// `runAgentLoopContinue(...)`
pub async fn run_agent_loop_continue(
    context: AgentContext,
    config: AgentLoopConfig,
    emit: AgentEventSink,
    signal: Option<CancellationToken>,
    stream_fn: Option<StreamFn>,
) -> Result<Vec<AgentMessage>, AgentLoopError> {
    check_continue_preconditions(&context)?;

    let mut new_messages: Vec<AgentMessage> = Vec::new();
    let mut current_context = context;

    emit(AgentEvent::AgentStart).await;
    emit(AgentEvent::TurnStart).await;

    let stream_fn = stream_fn.or_else(|| get_default_stream_fn().ok());
    if let Some(stream_fn) = stream_fn {
        // A continuation brings no fresh input of its own, so anything queued
        // rides along with the resumed request instead of waiting a step.
        run_loop(
            &mut current_context,
            &mut new_messages,
            config,
            signal,
            emit,
            stream_fn,
            true,
        )
        .await;
    }
    Ok(new_messages)
}

/// `runLoop(...)` — the shared loop body.
/// `drain_steering_first` controls whether the steering queue is polled before
/// the first request. A run started with fresh prompts defers the poll so the
/// model answers those prompts alone; every later poll happens after a
/// completed step, between two requests.
async fn run_loop(
    current_context: &mut AgentContext,
    new_messages: &mut Vec<AgentMessage>,
    initial_config: AgentLoopConfig,
    signal: Option<CancellationToken>,
    emit: AgentEventSink,
    stream_function: StreamFn,
    drain_steering_first: bool,
) {
    let mut config = initial_config;
    let mut first_turn = true;
    let mut repeat_breaker = RepeatBreaker::default();
    let mut pending_messages: Vec<AgentMessage> = match &config.get_steering_messages {
        Some(get_steering_messages) if drain_steering_first => get_steering_messages().await,
        _ => Vec::new(),
    };

    // Outer loop: runs again when follow-up messages arrive after the agent would stop.
    loop {
        let mut has_more_tool_calls = true;

        // Inner loop: tool calls and steering messages.
        while has_more_tool_calls || !pending_messages.is_empty() {
            if !first_turn {
                emit(AgentEvent::TurnStart).await;
            } else {
                first_turn = false;
            }

            if signal.as_ref().is_some_and(CancellationToken::is_cancelled) {
                finish_cancelled_turn(current_context, new_messages, &config, &emit).await;
                return;
            }

            if !pending_messages.is_empty() {
                repeat_breaker.reset();
                for message in std::mem::take(&mut pending_messages) {
                    emit(AgentEvent::MessageStart {
                        message: message.clone(),
                    })
                    .await;
                    emit(AgentEvent::MessageEnd {
                        message: message.clone(),
                    })
                    .await;
                    current_context.messages.push(message.clone());
                    new_messages.push(message);
                }
            }

            let message = stream_assistant_response(
                current_context,
                &config,
                signal.clone(),
                emit.clone(),
                stream_function.clone(),
            )
            .await;
            new_messages.push(AgentMessage::Assistant(message.clone()));

            if matches!(message.stop_reason, StopReason::Error | StopReason::Aborted) {
                emit(AgentEvent::TurnEnd {
                    message: AgentMessage::Assistant(message),
                    tool_results: Vec::new(),
                })
                .await;
                emit(AgentEvent::AgentEnd {
                    messages: new_messages.clone(),
                })
                .await;
                return;
            }

            let tool_calls: Vec<AgentToolCall> = message
                .content
                .iter()
                .filter_map(|block| match block {
                    notagent_ai::types::AssistantContent::ToolCall(tool_call) => {
                        Some(tool_call.clone())
                    }
                    _ => None,
                })
                .collect();

            let mut tool_results: Vec<ToolResultMessage> = Vec::new();
            has_more_tool_calls = false;
            // Taken for every response, so a text answer settles the handoff too.
            let handoff = repeat_breaker.take_handoff();
            if !tool_calls.is_empty() {
                let batch = if handoff {
                    reject_tool_calls(&tool_calls, |_| handoff_refusal_text(), true, emit.clone())
                        .await
                } else if message.stop_reason == StopReason::Length {
                    // A "length" stop means the arguments may be truncated, so no
                    // call in this message is safe to execute.
                    reject_tool_calls(
                        &tool_calls,
                        |tool_call| format!(
                            "Tool call \"{}\" was not executed: the response hit the output token limit, so its arguments may be truncated. Re-issue the tool call with complete arguments.",
                            tool_call.name
                        ),
                        false,
                        emit.clone(),
                    )
                    .await
                } else {
                    let nudges = repeat_breaker.observe(&tool_calls);
                    execute_tool_calls(
                        current_context,
                        &message,
                        &tool_calls,
                        &nudges,
                        &config,
                        signal.clone(),
                        emit.clone(),
                    )
                    .await
                };
                tool_results.extend(batch.messages);
                has_more_tool_calls = !batch.terminate;

                for result in &tool_results {
                    current_context
                        .messages
                        .push(AgentMessage::ToolResult(result.clone()));
                    new_messages.push(AgentMessage::ToolResult(result.clone()));
                }
            }

            emit(AgentEvent::TurnEnd {
                message: AgentMessage::Assistant(message.clone()),
                tool_results: tool_results.clone(),
            })
            .await;

            if finish_cancelled_after_turn(
                current_context,
                new_messages,
                &config,
                signal.as_ref(),
                &emit,
            )
            .await
            {
                return;
            }

            if let Some(prepare_next_turn) = &config.prepare_next_turn {
                let update = prepare_next_turn(PrepareNextTurnContext {
                    message: message.clone(),
                    tool_results: tool_results.clone(),
                    context: current_context.clone(),
                    new_messages: new_messages.clone(),
                })
                .await;
                if let Some(update) = update {
                    if let Some(context) = update.context {
                        *current_context = context;
                    }
                    if let Some(model) = update.model {
                        config.model = model;
                    }
                    if let Some(thinking_level) = update.thinking_level {
                        config.base.reasoning = thinking_level.to_reasoning();
                    }
                }
            }

            if finish_cancelled_after_turn(
                current_context,
                new_messages,
                &config,
                signal.as_ref(),
                &emit,
            )
            .await
            {
                return;
            }

            let should_stop = if let Some(should_stop_after_turn) = &config.should_stop_after_turn {
                should_stop_after_turn(ShouldStopAfterTurnContext {
                    message: message.clone(),
                    tool_results: tool_results.clone(),
                    context: current_context.clone(),
                    new_messages: new_messages.clone(),
                })
                .await
            } else {
                false
            };
            if finish_cancelled_after_turn(
                current_context,
                new_messages,
                &config,
                signal.as_ref(),
                &emit,
            )
            .await
            {
                return;
            }
            if should_stop {
                emit(AgentEvent::AgentEnd {
                    messages: new_messages.clone(),
                })
                .await;
                return;
            }

            pending_messages = match &config.get_steering_messages {
                Some(get_steering_messages) => get_steering_messages().await,
                None => Vec::new(),
            };
            if finish_cancelled_after_turn(
                current_context,
                new_messages,
                &config,
                signal.as_ref(),
                &emit,
            )
            .await
            {
                return;
            }
        }

        // The agent would stop here; check for follow-up messages.
        let follow_up_messages = match &config.get_follow_up_messages {
            Some(get_follow_up_messages) => get_follow_up_messages().await,
            None => Vec::new(),
        };
        if finish_cancelled_after_turn(
            current_context,
            new_messages,
            &config,
            signal.as_ref(),
            &emit,
        )
        .await
        {
            return;
        }
        if !follow_up_messages.is_empty() {
            pending_messages = follow_up_messages;
            continue;
        }
        break;
    }

    emit(AgentEvent::AgentEnd {
        messages: new_messages.clone(),
    })
    .await;
}

async fn finish_cancelled_after_turn(
    context: &mut AgentContext,
    new_messages: &mut Vec<AgentMessage>,
    config: &AgentLoopConfig,
    signal: Option<&CancellationToken>,
    emit: &AgentEventSink,
) -> bool {
    if !signal.is_some_and(CancellationToken::is_cancelled) {
        return false;
    }
    emit(AgentEvent::TurnStart).await;
    finish_cancelled_turn(context, new_messages, config, emit).await;
    true
}

fn create_aborted_assistant_message(
    config: &AgentLoopConfig,
    partial: Option<&AssistantMessage>,
) -> AssistantMessage {
    if let Some(partial) = partial {
        return AssistantMessage {
            stop_reason: StopReason::Aborted,
            deferred: None,
            error_message: Some("Operation aborted".to_string()),
            raw_stop_reason: None,
            end_turn: None,
            timestamp: now_ms(),
            ..partial.clone()
        };
    }

    AssistantMessage {
        content: Vec::new(),
        api: config.model.api.clone(),
        provider: config.model.provider.clone(),
        model: config.model.id.clone(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: StopReason::Aborted,
        deferred: None,
        error_message: Some("Operation aborted".to_string()),
        raw_stop_reason: None,
        end_turn: None,
        timestamp: now_ms(),
    }
}

async fn finish_cancelled_turn(
    context: &mut AgentContext,
    new_messages: &mut Vec<AgentMessage>,
    config: &AgentLoopConfig,
    emit: &AgentEventSink,
) {
    let message = create_aborted_assistant_message(config, None);
    finish_assistant_message(context, &message, false, emit).await;
    new_messages.push(AgentMessage::Assistant(message.clone()));
    emit(AgentEvent::TurnEnd {
        message: AgentMessage::Assistant(message),
        tool_results: Vec::new(),
    })
    .await;
    emit(AgentEvent::AgentEnd {
        messages: new_messages.clone(),
    })
    .await;
}

async fn finish_cancelled_response(
    context: &mut AgentContext,
    config: &AgentLoopConfig,
    added_partial: bool,
    emit: &AgentEventSink,
) -> AssistantMessage {
    let partial = added_partial.then(|| context.messages.last()).flatten();
    let partial = partial.and_then(|message| match message {
        AgentMessage::Assistant(message) => Some(message),
        _ => None,
    });
    let message = create_aborted_assistant_message(config, partial);
    finish_assistant_message(context, &message, added_partial, emit).await;
    message
}

/// `streamAssistantResponse(...)` — the only place that converts to LLM messages.
async fn stream_assistant_response(
    context: &mut AgentContext,
    config: &AgentLoopConfig,
    signal: Option<CancellationToken>,
    emit: AgentEventSink,
    stream_function: StreamFn,
) -> AssistantMessage {
    if signal.as_ref().is_some_and(CancellationToken::is_cancelled) {
        return finish_cancelled_response(context, config, false, &emit).await;
    }

    if let Some(prepare_context) = &config.prepare_context {
        let prepared = prepare_context(context.messages.clone(), signal.clone()).await;
        if signal.as_ref().is_some_and(CancellationToken::is_cancelled) {
            return finish_cancelled_response(context, config, false, &emit).await;
        }
        match prepared {
            Ok(messages) => context.messages = messages,
            Err(error) => {
                let mut message = create_aborted_assistant_message(config, None);
                message.stop_reason = StopReason::Error;
                message.error_message = Some(error.to_string());
                finish_assistant_message(context, &message, false, &emit).await;
                return message;
            }
        }
    }

    let mut messages = context.messages.clone();
    if let Some(transform_context) = &config.transform_context {
        messages = transform_context(messages, signal.clone()).await;
    }
    let llm_messages = (config.convert_to_llm)(messages).await;

    if signal.as_ref().is_some_and(CancellationToken::is_cancelled) {
        return finish_cancelled_response(context, config, false, &emit).await;
    }

    let llm_context = Context {
        system_prompt: Some(context.system_prompt.clone()),
        messages: llm_messages,
        tools: context
            .tools
            .as_ref()
            .map(|tools| tools.iter().map(|tool| tool.to_tool()).collect()),
    };

    // Resolved per turn because OAuth tokens expire during long tool phases.
    let resolved_api_key = match &config.get_api_key {
        Some(get_api_key) => get_api_key(config.model.provider.clone()).await,
        None => None,
    }
    .or_else(|| config.base.base.base.api_key.clone());

    if signal.as_ref().is_some_and(CancellationToken::is_cancelled) {
        return finish_cancelled_response(context, config, false, &emit).await;
    }

    let mut options = config.base.clone();
    options.base.base.api_key = resolved_api_key;
    options.base.base.signal = signal.clone();

    let request = stream_function(config.model.clone(), llm_context, Some(options));
    let response = match signal.as_ref() {
        Some(signal) => {
            tokio::select! {
                biased;
                () = signal.cancelled() => {
                    return finish_cancelled_response(context, config, false, &emit).await;
                }
                response = request => response,
            }
        }
        None => request.await,
    };

    let mut added_partial = false;
    let mut streaming = false;
    loop {
        let event = match signal.as_ref() {
            Some(signal) => {
                tokio::select! {
                    biased;
                    () = signal.cancelled() => {
                        return finish_cancelled_response(context, config, added_partial, &emit).await;
                    }
                    event = response.next() => event,
                }
            }
            None => response.next().await,
        };
        let Some(event) = event else {
            break;
        };
        use notagent_ai::types::AssistantMessageEvent as Event;
        match &event {
            Event::Start { partial } => {
                context
                    .messages
                    .push(AgentMessage::Assistant(partial.clone()));
                added_partial = true;
                streaming = true;
                emit(AgentEvent::MessageStart {
                    message: AgentMessage::Assistant(partial.clone()),
                })
                .await;
            }
            Event::TextStart { partial, .. }
            | Event::TextDelta { partial, .. }
            | Event::TextEnd { partial, .. }
            | Event::ThinkingStart { partial, .. }
            | Event::ThinkingDelta { partial, .. }
            | Event::ThinkingEnd { partial, .. }
            | Event::ToolcallStart { partial, .. }
            | Event::ToolcallDelta { partial, .. }
            | Event::ToolcallEnd { partial, .. } => {
                if streaming {
                    let index = context.messages.len() - 1;
                    context.messages[index] = AgentMessage::Assistant(partial.clone());
                    emit(AgentEvent::MessageUpdate {
                        message: AgentMessage::Assistant(partial.clone()),
                        assistant_message_event: event.clone(),
                    })
                    .await;
                }
            }
            Event::Done { .. } | Event::Error { .. } => {
                let final_message = response.result().await;
                finish_assistant_message(context, &final_message, added_partial, &emit).await;
                return final_message;
            }
        }
    }

    let final_message = response.result().await;
    finish_assistant_message(context, &final_message, added_partial, &emit).await;
    final_message
}

async fn finish_assistant_message(
    context: &mut AgentContext,
    final_message: &AssistantMessage,
    added_partial: bool,
    emit: &AgentEventSink,
) {
    if added_partial {
        let index = context.messages.len() - 1;
        context.messages[index] = AgentMessage::Assistant(final_message.clone());
    } else {
        context
            .messages
            .push(AgentMessage::Assistant(final_message.clone()));
        emit(AgentEvent::MessageStart {
            message: AgentMessage::Assistant(final_message.clone()),
        })
        .await;
    }
    emit(AgentEvent::MessageEnd {
        message: AgentMessage::Assistant(final_message.clone()),
    })
    .await;
}

/// Result of one tool batch.
struct ExecutedToolCallBatch {
    messages: Vec<ToolResultMessage>,
    terminate: bool,
}

const CANCELLED_TOOL_GRACE_MS: u64 = 2_000;

/// `createErrorToolResult(message)`
fn create_error_tool_result(message: impl Into<String>) -> AgentToolResult {
    AgentToolResult {
        content: vec![TextOrImageContent::Text(TextContent::new(message))],
        details: Some(Value::Object(Default::default())),
        usage: None,
        added_tool_names: None,
        terminate: None,
    }
}

fn create_aborted_tool_result(tool_name: &str) -> AgentToolResult {
    create_error_tool_result(format!(
        "The user manually interrupted \"{tool_name}\" (and anything else running at the same time). This was a deliberate user action, not a system error, timeout, or capacity limit. Do not retry automatically or guess at the cause — wait for the user's next instruction."
    ))
}

fn create_grace_timeout_tool_result(tool_name: &str) -> AgentToolResult {
    create_error_tool_result(format!(
        "Tool \"{tool_name}\" aborted by grace timeout ({CANCELLED_TOOL_GRACE_MS}ms)"
    ))
}

fn normalize_cancelled_tool_failure(
    result: (AgentToolResult, bool),
    signal: &CancellationToken,
    tool_name: &str,
) -> (AgentToolResult, bool) {
    if signal.is_cancelled() && result.1 {
        (create_aborted_tool_result(tool_name), true)
    } else {
        result
    }
}

/// One finalized tool call.
#[derive(Clone)]
struct FinalizedToolCall {
    tool_call: AgentToolCall,
    result: AgentToolResult,
    is_error: bool,
}

fn should_terminate_tool_batch(finalized: &[FinalizedToolCall]) -> bool {
    !finalized.is_empty()
        && finalized
            .iter()
            .all(|entry| entry.result.terminate == Some(true))
}

/// `createToolResultMessage(finalized)`
fn create_tool_result_message(finalized: &FinalizedToolCall, timestamp: i64) -> ToolResultMessage {
    ToolResultMessage {
        tool_call_id: finalized.tool_call.id.clone(),
        tool_name: finalized.tool_call.name.clone(),
        content: finalized.result.content.clone(),
        details: finalized.result.details.clone(),
        usage: finalized.result.usage,
        added_tool_names: finalized
            .result
            .added_tool_names
            .clone()
            .filter(|names| !names.is_empty()),
        is_error: finalized.is_error,
        timestamp,
    }
}

async fn emit_tool_execution_end(finalized: &FinalizedToolCall, emit: &AgentEventSink) {
    emit(AgentEvent::ToolExecutionEnd {
        tool_call_id: finalized.tool_call.id.clone(),
        tool_name: finalized.tool_call.name.clone(),
        result: finalized.result.clone(),
        is_error: finalized.is_error,
    })
    .await;
}

async fn emit_tool_result_message(message: &ToolResultMessage, emit: &AgentEventSink) {
    emit(AgentEvent::MessageStart {
        message: AgentMessage::ToolResult(message.clone()),
    })
    .await;
    emit(AgentEvent::MessageEnd {
        message: AgentMessage::ToolResult(message.clone()),
    })
    .await;
}

/// Closes every call of a message as an error without running any of them.
async fn reject_tool_calls(
    tool_calls: &[AgentToolCall],
    reason: impl Fn(&AgentToolCall) -> String,
    terminate: bool,
    emit: AgentEventSink,
) -> ExecutedToolCallBatch {
    let mut messages = Vec::new();
    for tool_call in tool_calls {
        emit(AgentEvent::ToolExecutionStart {
            tool_call_id: tool_call.id.clone(),
            tool_name: tool_call.name.clone(),
            args: Value::Object(tool_call.arguments.clone()),
        })
        .await;
        let finalized = FinalizedToolCall {
            tool_call: tool_call.clone(),
            result: create_error_tool_result(reason(tool_call)),
            is_error: true,
        };
        emit_tool_execution_end(&finalized, &emit).await;
        let message = create_tool_result_message(&finalized, now_ms());
        emit_tool_result_message(&message, &emit).await;
        messages.push(message);
    }
    ExecutedToolCallBatch {
        messages,
        terminate,
    }
}

/// Appends the repeat reminder a call earned, if any. Applied before the call's
/// end is emitted, so listeners and the session record see the result the model
/// does.
fn apply_nudge(finalized: &mut FinalizedToolCall, nudge: Option<RepeatNudge>) {
    if let Some(nudge) = nudge {
        nudge.apply(&mut finalized.result);
    }
}

fn nudge_at(nudges: &[Option<RepeatNudge>], index: usize) -> Option<RepeatNudge> {
    nudges.get(index).copied().flatten()
}

/// `executeToolCalls(...)` — picks the execution mode.
async fn execute_tool_calls(
    current_context: &AgentContext,
    assistant_message: &AssistantMessage,
    tool_calls: &[AgentToolCall],
    nudges: &[Option<RepeatNudge>],
    config: &AgentLoopConfig,
    signal: Option<CancellationToken>,
    emit: AgentEventSink,
) -> ExecutedToolCallBatch {
    let has_sequential_tool_call = tool_calls.iter().any(|tool_call| {
        current_context
            .tools
            .as_ref()
            .and_then(|tools| tools.iter().find(|tool| tool.name() == tool_call.name))
            .and_then(|tool| tool.execution_mode())
            == Some(ToolExecutionMode::Sequential)
    });
    if config.tool_execution == Some(ToolExecutionMode::Sequential) || has_sequential_tool_call {
        execute_tool_calls_sequential(
            current_context,
            assistant_message,
            tool_calls,
            nudges,
            config,
            signal,
            emit,
        )
        .await
    } else {
        execute_tool_calls_parallel(
            current_context,
            assistant_message,
            tool_calls,
            nudges,
            config,
            signal,
            emit,
        )
        .await
    }
}

async fn execute_tool_calls_sequential(
    current_context: &AgentContext,
    assistant_message: &AssistantMessage,
    tool_calls: &[AgentToolCall],
    nudges: &[Option<RepeatNudge>],
    config: &AgentLoopConfig,
    signal: Option<CancellationToken>,
    emit: AgentEventSink,
) -> ExecutedToolCallBatch {
    let mut finalized_calls = Vec::new();
    let mut messages = Vec::new();

    for (index, tool_call) in tool_calls.iter().enumerate() {
        emit(AgentEvent::ToolExecutionStart {
            tool_call_id: tool_call.id.clone(),
            tool_name: tool_call.name.clone(),
            args: Value::Object(tool_call.arguments.clone()),
        })
        .await;

        let preparation = prepare_tool_call(
            current_context,
            assistant_message,
            tool_call,
            config,
            signal.clone(),
        )
        .await;
        let mut finalized = match preparation {
            PreparedToolCall::Immediate { result, is_error } => FinalizedToolCall {
                tool_call: tool_call.clone(),
                result,
                is_error,
            },
            PreparedToolCall::Prepared { tool, args } => {
                let executed = execute_prepared_tool_call_with_grace(
                    &tool,
                    tool_call,
                    args.clone(),
                    signal.clone(),
                    emit.clone(),
                )
                .await;
                finalize_executed_tool_call(
                    current_context,
                    assistant_message,
                    tool_call,
                    args,
                    executed,
                    config,
                    signal.clone(),
                )
                .await
            }
        };
        apply_nudge(&mut finalized, nudge_at(nudges, index));

        emit_tool_execution_end(&finalized, &emit).await;
        let message = create_tool_result_message(&finalized, now_ms());
        emit_tool_result_message(&message, &emit).await;
        finalized_calls.push(finalized);
        messages.push(message);
    }

    ExecutedToolCallBatch {
        terminate: should_terminate_tool_batch(&finalized_calls),
        messages,
    }
}

async fn execute_tool_calls_parallel(
    current_context: &AgentContext,
    assistant_message: &AssistantMessage,
    tool_calls: &[AgentToolCall],
    nudges: &[Option<RepeatNudge>],
    config: &AgentLoopConfig,
    signal: Option<CancellationToken>,
    emit: AgentEventSink,
) -> ExecutedToolCallBatch {
    // Phase 1: preflight, strictly sequential, with an abort check after every call.
    let mut immediate: Vec<(usize, FinalizedToolCall)> = Vec::new();
    let mut tasks: JoinSet<(usize, FinalizedToolCall)> = JoinSet::new();

    for (index, tool_call) in tool_calls.iter().enumerate() {
        emit(AgentEvent::ToolExecutionStart {
            tool_call_id: tool_call.id.clone(),
            tool_name: tool_call.name.clone(),
            args: Value::Object(tool_call.arguments.clone()),
        })
        .await;

        let preparation = prepare_tool_call(
            current_context,
            assistant_message,
            tool_call,
            config,
            signal.clone(),
        )
        .await;
        match preparation {
            PreparedToolCall::Immediate { result, is_error } => {
                let mut finalized = FinalizedToolCall {
                    tool_call: tool_call.clone(),
                    result,
                    is_error,
                };
                apply_nudge(&mut finalized, nudge_at(nudges, index));
                emit_tool_execution_end(&finalized, &emit).await;
                immediate.push((index, finalized));
                continue;
            }
            PreparedToolCall::Prepared { tool, args } => {
                // Phase 2: the actual executions run as real tokio tasks.
                let tool_call = tool_call.clone();
                let context = current_context.clone();
                let assistant_message = assistant_message.clone();
                let config = config.clone();
                let signal = signal.clone();
                let emit = emit.clone();
                let nudge = nudge_at(nudges, index);
                tasks.spawn(async move {
                    let executed = execute_prepared_tool_call_with_grace(
                        &tool,
                        &tool_call,
                        args.clone(),
                        signal.clone(),
                        emit.clone(),
                    )
                    .await;
                    let mut finalized = finalize_executed_tool_call(
                        &context,
                        &assistant_message,
                        &tool_call,
                        args,
                        executed,
                        &config,
                        signal,
                    )
                    .await;
                    apply_nudge(&mut finalized, nudge);
                    emit_tool_execution_end(&finalized, &emit).await;
                    (index, finalized)
                });
            }
        }
    }

    let mut ordered: Vec<(usize, FinalizedToolCall)> = immediate;
    join_tool_calls(&mut tasks, &mut ordered).await;
    for (index, tool_call) in tool_calls.iter().enumerate() {
        if ordered
            .iter()
            .any(|(finalized_index, _)| *finalized_index == index)
        {
            continue;
        }
        let result = if signal.as_ref().is_some_and(CancellationToken::is_cancelled) {
            create_aborted_tool_result(&tool_call.name)
        } else {
            create_error_tool_result(format!(
                "Tool \"{}\" failed before producing a result",
                tool_call.name
            ))
        };
        let mut finalized = FinalizedToolCall {
            tool_call: tool_call.clone(),
            result,
            is_error: true,
        };
        apply_nudge(&mut finalized, nudge_at(nudges, index));
        emit_tool_execution_end(&finalized, &emit).await;
        ordered.push((index, finalized));
    }
    // Phase 3: result messages in assistant source order.
    ordered.sort_by_key(|(index, _)| *index);

    let mut messages = Vec::new();
    let finalized_calls: Vec<FinalizedToolCall> = ordered
        .into_iter()
        .map(|(_, finalized)| finalized)
        .collect();
    for finalized in &finalized_calls {
        let message = create_tool_result_message(finalized, now_ms());
        emit_tool_result_message(&message, &emit).await;
        messages.push(message);
    }

    ExecutedToolCallBatch {
        terminate: should_terminate_tool_batch(&finalized_calls),
        messages,
    }
}

async fn join_tool_calls(
    tasks: &mut JoinSet<(usize, FinalizedToolCall)>,
    ordered: &mut Vec<(usize, FinalizedToolCall)>,
) {
    while let Some(joined) = tasks.join_next().await {
        if let Ok(entry) = joined {
            ordered.push(entry);
        }
    }
}

/// Outcome of the preflight phase.
enum PreparedToolCall {
    Prepared {
        tool: Arc<dyn AgentTool>,
        args: Value,
    },
    Immediate {
        result: AgentToolResult,
        is_error: bool,
    },
}

/// `prepareToolCall(...)` — lookup, argument preparation, validation, beforeToolCall.
async fn prepare_tool_call(
    current_context: &AgentContext,
    assistant_message: &AssistantMessage,
    tool_call: &AgentToolCall,
    config: &AgentLoopConfig,
    signal: Option<CancellationToken>,
) -> PreparedToolCall {
    let tool = current_context
        .tools
        .as_ref()
        .and_then(|tools| tools.iter().find(|tool| tool.name() == tool_call.name))
        .cloned();
    let Some(tool) = tool else {
        return PreparedToolCall::Immediate {
            result: create_error_tool_result(format!("Tool {} not found", tool_call.name)),
            is_error: true,
        };
    };

    let prepared_arguments = tool.prepare_arguments(Value::Object(tool_call.arguments.clone()));
    let prepared_call = notagent_ai::types::ToolCall {
        arguments: prepared_arguments.as_object().cloned().unwrap_or_default(),
        ..tool_call.clone()
    };
    let validated_args = match validate_tool_arguments(&tool.to_tool(), &prepared_call) {
        Ok(args) => args,
        Err(error) => {
            return PreparedToolCall::Immediate {
                result: create_error_tool_result(error.to_string()),
                is_error: true,
            };
        }
    };

    if let Some(before_tool_call) = &config.before_tool_call {
        let before_result = before_tool_call(
            BeforeToolCallContext {
                assistant_message: assistant_message.clone(),
                tool_call: tool_call.clone(),
                args: validated_args.clone(),
                context: current_context.clone(),
            },
            signal.clone(),
        )
        .await;
        if signal.as_ref().is_some_and(CancellationToken::is_cancelled) {
            return PreparedToolCall::Immediate {
                result: create_aborted_tool_result(&tool_call.name),
                is_error: true,
            };
        }
        if let Some(before_result) = before_result
            && before_result.block == Some(true)
        {
            let mut result = create_error_tool_result(
                before_result
                    .reason
                    .unwrap_or_else(|| "Tool execution was blocked".to_string()),
            );
            if before_result.terminate == Some(true) {
                result.terminate = Some(true);
            }
            return PreparedToolCall::Immediate {
                result,
                is_error: true,
            };
        }
    }

    if signal.as_ref().is_some_and(CancellationToken::is_cancelled) {
        return PreparedToolCall::Immediate {
            result: create_aborted_tool_result(&tool_call.name),
            is_error: true,
        };
    }

    PreparedToolCall::Prepared {
        tool,
        args: validated_args,
    }
}

/// `executePreparedToolCall(...)` — update events are delivered while the tool
/// runs, and all of them before the call's end.
/// The callback a tool gets is synchronous, so it cannot await the sink itself.
/// Collecting the updates and emitting them after `execute` returned delivered
/// every progress report together with the result, which left a running command
/// looking silent until it finished. They go through a channel instead that is
/// drained alongside the execution.
async fn execute_prepared_tool_call(
    tool: &Arc<dyn AgentTool>,
    tool_call: &AgentToolCall,
    args: Value,
    signal: Option<CancellationToken>,
    emit: AgentEventSink,
) -> (AgentToolResult, bool) {
    let (update_sender, mut update_receiver) = tokio::sync::mpsc::unbounded_channel();
    let accepting_updates = Arc::new(std::sync::atomic::AtomicBool::new(true));

    let update_flag = Arc::clone(&accepting_updates);
    let update_call = tool_call.clone();
    let on_update: AgentToolUpdateCallback = Arc::new(move |partial_result| {
        if !update_flag.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        let _ = update_sender.send(AgentEvent::ToolExecutionUpdate {
            tool_call_id: update_call.id.clone(),
            tool_name: update_call.name.clone(),
            args: Value::Object(update_call.arguments.clone()),
            partial_result,
        });
    });

    let execution = tool.execute(&tool_call.id, args, signal, Some(on_update));
    tokio::pin!(execution);
    let outcome = loop {
        tokio::select! {
            biased;
            Some(event) = update_receiver.recv() => emit(event).await,
            outcome = &mut execution => break outcome,
        }
    };
    // Late updates are refused, the ones already sent still go out in order.
    accepting_updates.store(false, std::sync::atomic::Ordering::SeqCst);
    while let Ok(event) = update_receiver.try_recv() {
        emit(event).await;
    }

    match outcome {
        Ok(result) => (result, false),
        Err(error) => (create_error_tool_result(error.to_string()), true),
    }
}

async fn execute_prepared_tool_call_with_grace(
    tool: &Arc<dyn AgentTool>,
    tool_call: &AgentToolCall,
    args: Value,
    signal: Option<CancellationToken>,
    emit: AgentEventSink,
) -> (AgentToolResult, bool) {
    if signal.as_ref().is_some_and(CancellationToken::is_cancelled) {
        return (create_aborted_tool_result(&tool_call.name), true);
    }

    let execution = execute_prepared_tool_call(tool, tool_call, args, signal.clone(), emit);
    tokio::pin!(execution);

    let Some(signal) = signal else {
        return execution.await;
    };

    tokio::select! {
        biased;
        () = signal.cancelled() => {
            match tokio::time::timeout(
                std::time::Duration::from_millis(CANCELLED_TOOL_GRACE_MS),
                &mut execution,
            )
            .await
            {
                Ok(result) => normalize_cancelled_tool_failure(result, &signal, &tool_call.name),
                Err(_) => (create_grace_timeout_tool_result(&tool_call.name), true),
            }
        }
        result = &mut execution => {
            normalize_cancelled_tool_failure(result, &signal, &tool_call.name)
        },
    }
}

/// `finalizeExecutedToolCall(...)` — applies the afterToolCall overrides field by field.
async fn finalize_executed_tool_call(
    current_context: &AgentContext,
    assistant_message: &AssistantMessage,
    tool_call: &AgentToolCall,
    args: Value,
    executed: (AgentToolResult, bool),
    config: &AgentLoopConfig,
    signal: Option<CancellationToken>,
) -> FinalizedToolCall {
    let (mut result, mut is_error) = executed;

    if let Some(after_tool_call) = &config.after_tool_call {
        let after_result = after_tool_call(
            AfterToolCallContext {
                assistant_message: assistant_message.clone(),
                tool_call: tool_call.clone(),
                args,
                result: result.clone(),
                is_error,
                context: current_context.clone(),
            },
            signal,
        )
        .await;
        if let Some(after_result) = after_result {
            result = AgentToolResult {
                content: after_result.content.unwrap_or(result.content),
                details: after_result.details.or(result.details),
                usage: after_result.usage.or(result.usage),
                terminate: after_result.terminate.or(result.terminate),
                added_tool_names: result.added_tool_names,
            };
            if let Some(after_is_error) = after_result.is_error {
                is_error = after_is_error;
            }
        }
    }

    FinalizedToolCall {
        tool_call: tool_call.clone(),
        result,
        is_error,
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time before the Unix epoch")
        .as_millis() as i64
}
