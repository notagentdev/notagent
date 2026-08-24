//! The low-level agent loop.
//!
//! 1:1 port of `packages/agent/src/agent-loop.ts` (796 LOC). Works on `AgentMessage`
//! throughout and converts to `Message` only at the LLM boundary.
//!
//! Parallel tool execution uses a `JoinSet` (master plan) while keeping the TS
//! semantics exactly: preflight runs sequentially with an abort check after every call,
//! `tool_execution_end` fires in completion order, and the tool-result messages are
//! emitted afterwards in assistant source order.

use std::sync::Arc;

use notagent_ai::types::{
    AssistantMessage, Context, StopReason, TextContent, TextOrImageContent, ToolResultMessage,
};
use notagent_ai::utils::event_stream::EventStream;
use notagent_ai::utils::validation::validate_tool_arguments;
use serde_json::Value;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

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

/// Error of [`agent_loop_continue`] (TS throws).
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
///
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

            if !pending_messages.is_empty() {
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
            if !tool_calls.is_empty() {
                // A "length" stop means the arguments may be truncated, so no call in
                // this message is safe to execute.
                let batch = if message.stop_reason == StopReason::Length {
                    fail_tool_calls_from_truncated_message(&tool_calls, emit.clone()).await
                } else {
                    execute_tool_calls(
                        current_context,
                        &message,
                        &tool_calls,
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

            if let Some(should_stop_after_turn) = &config.should_stop_after_turn
                && should_stop_after_turn(ShouldStopAfterTurnContext {
                    message: message.clone(),
                    tool_results: tool_results.clone(),
                    context: current_context.clone(),
                    new_messages: new_messages.clone(),
                })
                .await
            {
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
        }

        // The agent would stop here; check for follow-up messages.
        let follow_up_messages = match &config.get_follow_up_messages {
            Some(get_follow_up_messages) => get_follow_up_messages().await,
            None => Vec::new(),
        };
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

/// `streamAssistantResponse(...)` — the only place that converts to LLM messages.
async fn stream_assistant_response(
    context: &mut AgentContext,
    config: &AgentLoopConfig,
    signal: Option<CancellationToken>,
    emit: AgentEventSink,
    stream_function: StreamFn,
) -> AssistantMessage {
    let mut messages = context.messages.clone();
    if let Some(transform_context) = &config.transform_context {
        messages = transform_context(messages, signal.clone()).await;
    }
    let llm_messages = (config.convert_to_llm)(messages).await;

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

    let mut options = config.base.clone();
    options.base.base.api_key = resolved_api_key;
    options.base.base.signal = signal;

    let response = stream_function(config.model.clone(), llm_context, Some(options)).await;

    let mut added_partial = false;
    let mut streaming = false;
    while let Some(event) = response.next().await {
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

/// `failToolCallsFromTruncatedMessage(toolCalls, emit)`
async fn fail_tool_calls_from_truncated_message(
    tool_calls: &[AgentToolCall],
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
            result: create_error_tool_result(format!(
                "Tool call \"{}\" was not executed: the response hit the output token limit, so its arguments may be truncated. Re-issue the tool call with complete arguments.",
                tool_call.name
            )),
            is_error: true,
        };
        emit_tool_execution_end(&finalized, &emit).await;
        let message = create_tool_result_message(&finalized, now_ms());
        emit_tool_result_message(&message, &emit).await;
        messages.push(message);
    }
    ExecutedToolCallBatch {
        messages,
        terminate: false,
    }
}

/// `executeToolCalls(...)` — picks the execution mode.
async fn execute_tool_calls(
    current_context: &AgentContext,
    assistant_message: &AssistantMessage,
    tool_calls: &[AgentToolCall],
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
    config: &AgentLoopConfig,
    signal: Option<CancellationToken>,
    emit: AgentEventSink,
) -> ExecutedToolCallBatch {
    let mut finalized_calls = Vec::new();
    let mut messages = Vec::new();

    for tool_call in tool_calls {
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
        let finalized = match preparation {
            PreparedToolCall::Immediate { result, is_error } => FinalizedToolCall {
                tool_call: tool_call.clone(),
                result,
                is_error,
            },
            PreparedToolCall::Prepared { tool, args } => {
                let executed = execute_prepared_tool_call(
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

        emit_tool_execution_end(&finalized, &emit).await;
        let message = create_tool_result_message(&finalized, now_ms());
        emit_tool_result_message(&message, &emit).await;
        finalized_calls.push(finalized);
        messages.push(message);

        if signal.as_ref().is_some_and(CancellationToken::is_cancelled) {
            break;
        }
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
                let finalized = FinalizedToolCall {
                    tool_call: tool_call.clone(),
                    result,
                    is_error,
                };
                emit_tool_execution_end(&finalized, &emit).await;
                immediate.push((index, finalized));
                if signal.as_ref().is_some_and(CancellationToken::is_cancelled) {
                    break;
                }
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
                tasks.spawn(async move {
                    let executed = execute_prepared_tool_call(
                        &tool,
                        &tool_call,
                        args.clone(),
                        signal.clone(),
                        emit.clone(),
                    )
                    .await;
                    let finalized = finalize_executed_tool_call(
                        &context,
                        &assistant_message,
                        &tool_call,
                        args,
                        executed,
                        &config,
                        signal,
                    )
                    .await;
                    // `tool_execution_end` fires in completion order, as in TS.
                    emit_tool_execution_end(&finalized, &emit).await;
                    (index, finalized)
                });
            }
        }
        if signal.as_ref().is_some_and(CancellationToken::is_cancelled) {
            break;
        }
    }

    let mut ordered: Vec<(usize, FinalizedToolCall)> = immediate;
    while let Some(joined) = tasks.join_next().await {
        if let Ok(entry) = joined {
            ordered.push(entry);
        }
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
                result: create_error_tool_result("Operation aborted"),
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
            result: create_error_tool_result("Operation aborted"),
            is_error: true,
        };
    }

    PreparedToolCall::Prepared {
        tool,
        args: validated_args,
    }
}

/// `executePreparedToolCall(...)` — update events are collected and awaited afterwards.
async fn execute_prepared_tool_call(
    tool: &Arc<dyn AgentTool>,
    tool_call: &AgentToolCall,
    args: Value,
    signal: Option<CancellationToken>,
    emit: AgentEventSink,
) -> (AgentToolResult, bool) {
    let updates: Arc<std::sync::Mutex<Vec<AgentEvent>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let accepting_updates = Arc::new(std::sync::atomic::AtomicBool::new(true));

    let update_sink = Arc::clone(&updates);
    let update_flag = Arc::clone(&accepting_updates);
    let update_call = tool_call.clone();
    let on_update: AgentToolUpdateCallback = Arc::new(move |partial_result| {
        if !update_flag.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        update_sink
            .lock()
            .expect("update sink poisoned")
            .push(AgentEvent::ToolExecutionUpdate {
                tool_call_id: update_call.id.clone(),
                tool_name: update_call.name.clone(),
                args: Value::Object(update_call.arguments.clone()),
                partial_result,
            });
    });

    let outcome = tool
        .execute(&tool_call.id, args, signal, Some(on_update))
        .await;
    // `acceptingUpdates = false` before the collected updates are awaited.
    accepting_updates.store(false, std::sync::atomic::Ordering::SeqCst);
    let collected = std::mem::take(&mut *updates.lock().expect("update sink poisoned"));
    for event in collected {
        emit(event).await;
    }

    match outcome {
        Ok(result) => (result, false),
        Err(error) => (create_error_tool_result(error.to_string()), true),
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
