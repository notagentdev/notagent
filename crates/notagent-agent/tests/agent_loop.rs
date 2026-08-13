//! Port of the core cases of `packages/agent/test/agent-loop.test.ts` (1 607 LOC)
//! plus the parallelism proof the workstream plan requires.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use notagent_agent::agent_loop::{agent_loop, agent_loop_continue};
use notagent_agent::types::*;
use notagent_ai::types::*;
use notagent_ai::utils::event_stream::create_assistant_message_event_stream;
use serde_json::{Value, json};

fn model() -> Model {
    Model {
        id: "mock".to_string(),
        name: "mock".to_string(),
        api: "openai-responses".to_string(),
        provider: "openai".to_string(),
        base_url: "https://example.invalid".to_string(),
        reasoning: false,
        thinking_level_map: None,
        input: vec![Modality::Text],
        cost: ModelCost::default(),
        context_window: 8192,
        max_tokens: 2048,
        sampling_params: None,
        headers: None,
        compat: None,
    }
}

fn assistant_message(content: Vec<AssistantContent>, stop_reason: StopReason) -> AssistantMessage {
    AssistantMessage {
        content,
        api: "openai-responses".to_string(),
        provider: "openai".to_string(),
        model: "mock".to_string(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: 0,
    }
}

fn user_message(text: &str) -> AgentMessage {
    AgentMessage::User(UserMessage {
        content: UserContent::Text(text.to_string()),
        timestamp: 0,
    })
}

fn tool_call(id: &str, name: &str, arguments: Value) -> AssistantContent {
    AssistantContent::ToolCall(ToolCall {
        id: id.to_string(),
        name: name.to_string(),
        arguments: arguments.as_object().cloned().unwrap_or_default(),
        ..Default::default()
    })
}

/// `identityConverter` — passes the three LLM roles through.
fn identity_converter() -> ConvertToLlmFn {
    Arc::new(|messages: Vec<AgentMessage>| {
        Box::pin(async move {
            messages
                .iter()
                .filter_map(AgentMessage::as_llm_message)
                .collect()
        })
    })
}

/// A stream function that returns the queued messages, one per turn.
fn scripted_stream_fn(messages: Vec<AssistantMessage>) -> (StreamFn, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let scripted = Arc::new(messages);
    let stream_fn: StreamFn = Arc::new(move |_model, _context, _options| {
        let index = counter.fetch_add(1, Ordering::SeqCst);
        let scripted = Arc::clone(&scripted);
        Box::pin(async move {
            let stream = create_assistant_message_event_stream();
            let message = scripted.get(index).cloned().unwrap_or_else(|| {
                assistant_message(
                    vec![AssistantContent::Text(TextContent::new("done"))],
                    StopReason::Stop,
                )
            });
            let reason = match message.stop_reason {
                StopReason::ToolUse => DoneReason::ToolUse,
                StopReason::Length => DoneReason::Length,
                _ => DoneReason::Stop,
            };
            stream.push(AssistantMessageEvent::Done { reason, message });
            stream
        })
    });
    (stream_fn, calls)
}

fn base_config(stream_fn_model: Model) -> AgentLoopConfig {
    AgentLoopConfig {
        base: SimpleStreamOptions::default(),
        model: stream_fn_model,
        convert_to_llm: identity_converter(),
        transform_context: None,
        get_api_key: None,
        should_stop_after_turn: None,
        prepare_next_turn: None,
        get_steering_messages: None,
        get_follow_up_messages: None,
        tool_execution: None,
        before_tool_call: None,
        after_tool_call: None,
    }
}

async fn collect(
    stream: notagent_agent::agent_loop::AgentEventStream,
) -> (Vec<AgentEvent>, Vec<AgentMessage>) {
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        events.push(event);
    }
    let messages = stream.result().await;
    (events, messages)
}

fn event_names(events: &[AgentEvent]) -> Vec<&'static str> {
    events.iter().map(AgentEvent::type_name).collect()
}

/// A tool whose behaviour the test controls.
struct TestTool {
    name: String,
    parameters: Value,
    execution_mode: Option<ToolExecutionMode>,
    delay: Duration,
    result: AgentToolResult,
    fail: bool,
    started: Arc<AtomicUsize>,
    concurrent: Arc<AtomicUsize>,
    max_concurrent: Arc<AtomicUsize>,
}

impl TestTool {
    fn new(name: &str) -> Self {
        TestTool {
            name: name.to_string(),
            parameters: json!({"type": "object", "properties": {"value": {"type": "string"}}, "required": []}),
            execution_mode: None,
            delay: Duration::ZERO,
            result: AgentToolResult {
                content: vec![TextOrImageContent::Text(TextContent::new("ok"))],
                details: Some(json!({})),
                ..Default::default()
            },
            fail: false,
            started: Arc::new(AtomicUsize::new(0)),
            concurrent: Arc::new(AtomicUsize::new(0)),
            max_concurrent: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl AgentTool for TestTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        "test tool"
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    fn label(&self) -> &str {
        "Test"
    }

    fn execution_mode(&self) -> Option<ToolExecutionMode> {
        self.execution_mode
    }

    fn execute<'a>(
        &'a self,
        _tool_call_id: &'a str,
        _params: Value,
        _signal: Option<tokio_util::sync::CancellationToken>,
        on_update: Option<AgentToolUpdateCallback>,
    ) -> BoxFuture<'a, Result<AgentToolResult, ToolExecutionError>> {
        Box::pin(async move {
            self.started.fetch_add(1, Ordering::SeqCst);
            let running = self.concurrent.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_concurrent.fetch_max(running, Ordering::SeqCst);
            if let Some(on_update) = &on_update {
                on_update(AgentToolResult {
                    content: vec![TextOrImageContent::Text(TextContent::new("partial"))],
                    ..Default::default()
                });
            }
            if !self.delay.is_zero() {
                tokio::time::sleep(self.delay).await;
            }
            self.concurrent.fetch_sub(1, Ordering::SeqCst);
            if self.fail {
                return Err(ToolExecutionError::new("tool failed"));
            }
            Ok(self.result.clone())
        })
    }
}

#[tokio::test]
async fn emits_the_documented_event_sequence() {
    let context = AgentContext {
        system_prompt: "You are helpful.".to_string(),
        messages: vec![],
        tools: Some(vec![]),
    };
    let (stream_fn, _) = scripted_stream_fn(vec![assistant_message(
        vec![AssistantContent::Text(TextContent::new("Hi there!"))],
        StopReason::Stop,
    )]);

    let stream = agent_loop(
        vec![user_message("Hello")],
        context,
        base_config(model()),
        None,
        Some(stream_fn),
    );
    let (events, messages) = collect(stream).await;

    assert_eq!(messages.len(), 2);
    assert!(matches!(messages[0], AgentMessage::User(_)));
    assert!(matches!(messages[1], AgentMessage::Assistant(_)));
    assert_eq!(
        event_names(&events),
        [
            "agent_start",
            "turn_start",
            "message_start",
            "message_end",
            "message_start",
            "message_end",
            "turn_end",
            "agent_end"
        ]
    );
}

#[tokio::test]
async fn executes_tool_calls_and_appends_their_results() {
    let tool = Arc::new(TestTool::new("echo"));
    let context = AgentContext {
        system_prompt: String::new(),
        messages: vec![],
        tools: Some(vec![tool.clone() as Arc<dyn AgentTool>]),
    };
    let (stream_fn, calls) = scripted_stream_fn(vec![
        assistant_message(
            vec![tool_call("call_1", "echo", json!({"value": "x"}))],
            StopReason::ToolUse,
        ),
        assistant_message(
            vec![AssistantContent::Text(TextContent::new("done"))],
            StopReason::Stop,
        ),
    ]);

    let stream = agent_loop(
        vec![user_message("run")],
        context,
        base_config(model()),
        None,
        Some(stream_fn),
    );
    let (events, messages) = collect(stream).await;

    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "the loop continues after tool results"
    );
    assert_eq!(tool.started.load(Ordering::SeqCst), 1);
    let names = event_names(&events);
    assert!(names.contains(&"tool_execution_start"));
    assert!(
        names.contains(&"tool_execution_update"),
        "update events are forwarded"
    );
    assert!(names.contains(&"tool_execution_end"));
    assert!(matches!(messages[2], AgentMessage::ToolResult(_)));
}

#[tokio::test]
async fn does_not_execute_tool_calls_from_a_length_truncated_message() {
    let tool = Arc::new(TestTool::new("echo"));
    let context = AgentContext {
        system_prompt: String::new(),
        messages: vec![],
        tools: Some(vec![tool.clone() as Arc<dyn AgentTool>]),
    };
    let (stream_fn, _) = scripted_stream_fn(vec![
        assistant_message(
            vec![tool_call("call_1", "echo", json!({}))],
            StopReason::Length,
        ),
        assistant_message(
            vec![AssistantContent::Text(TextContent::new("recovered"))],
            StopReason::Stop,
        ),
    ]);

    let stream = agent_loop(
        vec![user_message("run")],
        context,
        base_config(model()),
        None,
        Some(stream_fn),
    );
    let (_, messages) = collect(stream).await;

    assert_eq!(
        tool.started.load(Ordering::SeqCst),
        0,
        "truncated arguments must not be executed"
    );
    let AgentMessage::ToolResult(result) = &messages[2] else {
        panic!("tool result")
    };
    assert!(result.is_error);
    let TextOrImageContent::Text(text) = &result.content[0] else {
        panic!("text")
    };
    assert!(
        text.text.contains("hit the output token limit"),
        "{}",
        text.text
    );
}

#[tokio::test]
async fn reports_unknown_tools_as_error_results() {
    let context = AgentContext {
        system_prompt: String::new(),
        messages: vec![],
        tools: Some(vec![]),
    };
    let (stream_fn, _) = scripted_stream_fn(vec![
        assistant_message(
            vec![tool_call("call_1", "missing", json!({}))],
            StopReason::ToolUse,
        ),
        assistant_message(
            vec![AssistantContent::Text(TextContent::new("ok"))],
            StopReason::Stop,
        ),
    ]);

    let stream = agent_loop(
        vec![user_message("run")],
        context,
        base_config(model()),
        None,
        Some(stream_fn),
    );
    let (_, messages) = collect(stream).await;

    let AgentMessage::ToolResult(result) = &messages[2] else {
        panic!("tool result")
    };
    assert!(result.is_error);
    let TextOrImageContent::Text(text) = &result.content[0] else {
        panic!("text")
    };
    assert_eq!(text.text, "Tool missing not found");
}

#[tokio::test]
async fn emits_tool_execution_end_in_completion_order_and_results_in_source_order() {
    let slow = Arc::new(TestTool {
        delay: Duration::from_millis(60),
        ..TestTool::new("slow")
    });
    let fast = Arc::new(TestTool {
        delay: Duration::from_millis(5),
        ..TestTool::new("fast")
    });
    let context = AgentContext {
        system_prompt: String::new(),
        messages: vec![],
        tools: Some(vec![
            slow.clone() as Arc<dyn AgentTool>,
            fast.clone() as Arc<dyn AgentTool>,
        ]),
    };
    let (stream_fn, _) = scripted_stream_fn(vec![
        assistant_message(
            vec![
                tool_call("call_slow", "slow", json!({})),
                tool_call("call_fast", "fast", json!({})),
            ],
            StopReason::ToolUse,
        ),
        assistant_message(
            vec![AssistantContent::Text(TextContent::new("done"))],
            StopReason::Stop,
        ),
    ]);

    let stream = agent_loop(
        vec![user_message("run")],
        context,
        base_config(model()),
        None,
        Some(stream_fn),
    );
    let (events, messages) = collect(stream).await;

    let ends: Vec<String> = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::ToolExecutionEnd { tool_call_id, .. } => Some(tool_call_id.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        ends,
        ["call_fast", "call_slow"],
        "ends follow completion order"
    );

    let results: Vec<String> = messages
        .iter()
        .filter_map(|message| match message {
            AgentMessage::ToolResult(result) => Some(result.tool_call_id.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        results,
        ["call_slow", "call_fast"],
        "results follow assistant source order"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn executes_tools_in_parallel() {
    // The proof the plan asks for: three tools with 150 ms latency each must finish in
    // clearly less than their sum.
    let tools: Vec<Arc<TestTool>> = (0..3)
        .map(|index| {
            Arc::new(TestTool {
                delay: Duration::from_millis(150),
                ..TestTool::new(&format!("t{index}"))
            })
        })
        .collect();
    let context = AgentContext {
        system_prompt: String::new(),
        messages: vec![],
        tools: Some(
            tools
                .iter()
                .map(|tool| tool.clone() as Arc<dyn AgentTool>)
                .collect(),
        ),
    };
    let calls: Vec<AssistantContent> = (0..3)
        .map(|index| tool_call(&format!("call_{index}"), &format!("t{index}"), json!({})))
        .collect();
    let (stream_fn, _) = scripted_stream_fn(vec![
        assistant_message(calls, StopReason::ToolUse),
        assistant_message(
            vec![AssistantContent::Text(TextContent::new("done"))],
            StopReason::Stop,
        ),
    ]);

    let started = Instant::now();
    let stream = agent_loop(
        vec![user_message("run")],
        context,
        base_config(model()),
        None,
        Some(stream_fn),
    );
    let (_, _) = collect(stream).await;
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_millis(300),
        "three 150 ms tools took {elapsed:?}; that is not parallel"
    );
    for tool in &tools {
        assert_eq!(tool.started.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_sequential_tool_forces_the_whole_batch_to_run_one_at_a_time() {
    let sequential = Arc::new(TestTool {
        execution_mode: Some(ToolExecutionMode::Sequential),
        delay: Duration::from_millis(30),
        ..TestTool::new("seq")
    });
    // Both tools share one counter so overlapping executions are observable.
    let concurrent = Arc::clone(&sequential.concurrent);
    let max_concurrent = Arc::clone(&sequential.max_concurrent);
    let parallel = Arc::new(TestTool {
        concurrent: Arc::clone(&concurrent),
        max_concurrent: Arc::clone(&max_concurrent),
        ..TestTool {
            delay: Duration::from_millis(30),
            ..TestTool::new("par")
        }
    });
    let context = AgentContext {
        system_prompt: String::new(),
        messages: vec![],
        tools: Some(vec![
            sequential.clone() as Arc<dyn AgentTool>,
            parallel.clone() as Arc<dyn AgentTool>,
        ]),
    };
    let (stream_fn, _) = scripted_stream_fn(vec![
        assistant_message(
            vec![
                tool_call("call_seq", "seq", json!({})),
                tool_call("call_par", "par", json!({})),
            ],
            StopReason::ToolUse,
        ),
        assistant_message(
            vec![AssistantContent::Text(TextContent::new("done"))],
            StopReason::Stop,
        ),
    ]);

    let stream = agent_loop(
        vec![user_message("run")],
        context,
        base_config(model()),
        None,
        Some(stream_fn),
    );
    collect(stream).await;

    assert_eq!(
        max_concurrent.load(Ordering::SeqCst),
        1,
        "one sequential tool serializes the batch"
    );
}

#[tokio::test]
async fn terminates_the_run_only_when_every_result_sets_terminate() {
    let terminating = Arc::new(TestTool {
        result: AgentToolResult {
            terminate: Some(true),
            details: Some(json!({})),
            ..Default::default()
        },
        ..TestTool::new("stop")
    });
    let plain = Arc::new(TestTool::new("go"));

    // Every result terminates -> the loop ends after the batch.
    let context = AgentContext {
        system_prompt: String::new(),
        messages: vec![],
        tools: Some(vec![terminating.clone() as Arc<dyn AgentTool>]),
    };
    let (stream_fn, calls) = scripted_stream_fn(vec![assistant_message(
        vec![tool_call("call_1", "stop", json!({}))],
        StopReason::ToolUse,
    )]);
    let stream = agent_loop(
        vec![user_message("run")],
        context,
        base_config(model()),
        None,
        Some(stream_fn),
    );
    collect(stream).await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "no further turn after a terminating batch"
    );

    // A mixed batch continues.
    let context = AgentContext {
        system_prompt: String::new(),
        messages: vec![],
        tools: Some(vec![
            terminating as Arc<dyn AgentTool>,
            plain as Arc<dyn AgentTool>,
        ]),
    };
    let (stream_fn, calls) = scripted_stream_fn(vec![
        assistant_message(
            vec![
                tool_call("call_1", "stop", json!({})),
                tool_call("call_2", "go", json!({})),
            ],
            StopReason::ToolUse,
        ),
        assistant_message(
            vec![AssistantContent::Text(TextContent::new("done"))],
            StopReason::Stop,
        ),
    ]);
    let stream = agent_loop(
        vec![user_message("run")],
        context,
        base_config(model()),
        None,
        Some(stream_fn),
    );
    collect(stream).await;
    assert_eq!(calls.load(Ordering::SeqCst), 2, "a mixed batch keeps going");
}

#[tokio::test]
async fn stops_after_the_turn_when_should_stop_after_turn_returns_true() {
    let context = AgentContext {
        system_prompt: String::new(),
        messages: vec![],
        tools: Some(vec![]),
    };
    let (stream_fn, calls) = scripted_stream_fn(vec![
        assistant_message(
            vec![AssistantContent::Text(TextContent::new("first"))],
            StopReason::Stop,
        ),
        assistant_message(
            vec![AssistantContent::Text(TextContent::new("second"))],
            StopReason::Stop,
        ),
    ]);
    let mut config = base_config(model());
    config.should_stop_after_turn = Some(Arc::new(|_context| Box::pin(async { true })));
    config.get_follow_up_messages = Some(Arc::new(|| {
        Box::pin(async { vec![AgentMessage::User(UserMessage::default())] })
    }));

    let stream = agent_loop(
        vec![user_message("run")],
        context,
        config,
        None,
        Some(stream_fn),
    );
    let (events, _) = collect(stream).await;

    assert_eq!(calls.load(Ordering::SeqCst), 1);
    // shouldStopAfterTurn exits before the follow-up queue is polled.
    assert_eq!(event_names(&events).last(), Some(&"agent_end"));
}

#[tokio::test]
async fn injects_steering_messages_before_the_next_turn() {
    let delivered = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&delivered);
    let context = AgentContext {
        system_prompt: String::new(),
        messages: vec![],
        tools: Some(vec![]),
    };
    let (stream_fn, calls) = scripted_stream_fn(vec![
        assistant_message(
            vec![AssistantContent::Text(TextContent::new("first"))],
            StopReason::Stop,
        ),
        assistant_message(
            vec![AssistantContent::Text(TextContent::new("second"))],
            StopReason::Stop,
        ),
    ]);
    let mut config = base_config(model());
    config.get_steering_messages = Some(Arc::new(move || {
        let counter = Arc::clone(&counter);
        Box::pin(async move {
            // Steer exactly once, after the first turn.
            if counter.fetch_add(1, Ordering::SeqCst) == 1 {
                vec![user_message("steered")]
            } else {
                Vec::new()
            }
        })
    }));

    let stream = agent_loop(
        vec![user_message("run")],
        context,
        config,
        None,
        Some(stream_fn),
    );
    let (_, messages) = collect(stream).await;

    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "a steering message triggers another turn"
    );
    assert!(
        messages
            .iter()
            .any(|message| matches!(message, AgentMessage::User(user)
            if matches!(&user.content, UserContent::Text(text) if text == "steered"))),
        "the steering message is part of the transcript"
    );
}

#[tokio::test]
async fn follow_up_messages_restart_the_loop_once_it_would_end() {
    let served = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&served);
    let context = AgentContext {
        system_prompt: String::new(),
        messages: vec![],
        tools: Some(vec![]),
    };
    let (stream_fn, calls) = scripted_stream_fn(vec![
        assistant_message(
            vec![AssistantContent::Text(TextContent::new("first"))],
            StopReason::Stop,
        ),
        assistant_message(
            vec![AssistantContent::Text(TextContent::new("second"))],
            StopReason::Stop,
        ),
    ]);
    let mut config = base_config(model());
    config.get_follow_up_messages = Some(Arc::new(move || {
        let counter = Arc::clone(&counter);
        Box::pin(async move {
            if counter.fetch_add(1, Ordering::SeqCst) == 0 {
                vec![user_message("follow up")]
            } else {
                Vec::new()
            }
        })
    }));

    let stream = agent_loop(
        vec![user_message("run")],
        context,
        config,
        None,
        Some(stream_fn),
    );
    let (_, messages) = collect(stream).await;

    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        messages.len(),
        4,
        "prompt, first answer, follow-up message, second answer"
    );
}

#[tokio::test]
async fn an_errored_assistant_turn_ends_the_run() {
    let context = AgentContext {
        system_prompt: String::new(),
        messages: vec![],
        tools: Some(vec![]),
    };
    let (stream_fn, calls) = scripted_stream_fn(vec![
        AssistantMessage {
            stop_reason: StopReason::Error,
            error_message: Some("boom".to_string()),
            ..assistant_message(vec![], StopReason::Error)
        },
        assistant_message(
            vec![AssistantContent::Text(TextContent::new("never"))],
            StopReason::Stop,
        ),
    ]);

    let stream = agent_loop(
        vec![user_message("run")],
        context,
        base_config(model()),
        None,
        Some(stream_fn),
    );
    let (events, _) = collect(stream).await;

    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let names = event_names(&events);
    assert_eq!(names[names.len() - 2..], ["turn_end", "agent_end"]);
}

#[tokio::test]
async fn prepare_next_turn_replaces_model_and_thinking_level() {
    let seen_models = Arc::new(Mutex::new(Vec::<String>::new()));
    let recorder = Arc::clone(&seen_models);
    let stream_fn: StreamFn = Arc::new(move |model: Model, _context, _options| {
        recorder.lock().expect("poisoned").push(model.id.clone());
        let is_first = recorder.lock().expect("poisoned").len() == 1;
        Box::pin(async move {
            let stream = create_assistant_message_event_stream();
            let message = if is_first {
                assistant_message(
                    vec![AssistantContent::Text(TextContent::new("first"))],
                    StopReason::Stop,
                )
            } else {
                assistant_message(
                    vec![AssistantContent::Text(TextContent::new("second"))],
                    StopReason::Stop,
                )
            };
            stream.push(AssistantMessageEvent::Done {
                reason: DoneReason::Stop,
                message,
            });
            stream
        })
    });

    let served = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&served);
    let mut config = base_config(model());
    config.prepare_next_turn = Some(Arc::new(|_context| {
        Box::pin(async {
            Some(AgentLoopTurnUpdate {
                model: Some(Model {
                    id: "replacement".to_string(),
                    ..model()
                }),
                thinking_level: Some(notagent_agent::types::ThinkingLevel::Off),
                context: None,
            })
        })
    }));
    config.get_follow_up_messages = Some(Arc::new(move || {
        let counter = Arc::clone(&counter);
        Box::pin(async move {
            if counter.fetch_add(1, Ordering::SeqCst) == 0 {
                vec![user_message("again")]
            } else {
                Vec::new()
            }
        })
    }));

    let context = AgentContext {
        system_prompt: String::new(),
        messages: vec![],
        tools: Some(vec![]),
    };
    let stream = agent_loop(
        vec![user_message("run")],
        context,
        config,
        None,
        Some(stream_fn),
    );
    collect(stream).await;

    let models = seen_models.lock().expect("poisoned").clone();
    assert_eq!(
        models,
        ["mock", "replacement"],
        "the replacement model is used for the next turn"
    );
}

#[tokio::test]
async fn before_tool_call_can_block_a_call() {
    let tool = Arc::new(TestTool::new("echo"));
    let context = AgentContext {
        system_prompt: String::new(),
        messages: vec![],
        tools: Some(vec![tool.clone() as Arc<dyn AgentTool>]),
    };
    let (stream_fn, _) = scripted_stream_fn(vec![
        assistant_message(
            vec![tool_call("call_1", "echo", json!({}))],
            StopReason::ToolUse,
        ),
        assistant_message(
            vec![AssistantContent::Text(TextContent::new("done"))],
            StopReason::Stop,
        ),
    ]);
    let mut config = base_config(model());
    config.before_tool_call = Some(Arc::new(|_context, _signal| {
        Box::pin(async {
            Some(BeforeToolCallResult {
                block: Some(true),
                reason: Some("not allowed".to_string()),
                terminate: None,
            })
        })
    }));

    let stream = agent_loop(
        vec![user_message("run")],
        context,
        config,
        None,
        Some(stream_fn),
    );
    let (_, messages) = collect(stream).await;

    assert_eq!(tool.started.load(Ordering::SeqCst), 0);
    let AgentMessage::ToolResult(result) = &messages[2] else {
        panic!("tool result")
    };
    assert!(result.is_error);
    let TextOrImageContent::Text(text) = &result.content[0] else {
        panic!("text")
    };
    assert_eq!(text.text, "not allowed");
}

#[tokio::test]
async fn after_tool_call_overrides_fields_of_the_result() {
    let tool = Arc::new(TestTool::new("echo"));
    let context = AgentContext {
        system_prompt: String::new(),
        messages: vec![],
        tools: Some(vec![tool as Arc<dyn AgentTool>]),
    };
    let (stream_fn, _) = scripted_stream_fn(vec![
        assistant_message(
            vec![tool_call("call_1", "echo", json!({}))],
            StopReason::ToolUse,
        ),
        assistant_message(
            vec![AssistantContent::Text(TextContent::new("done"))],
            StopReason::Stop,
        ),
    ]);
    let mut config = base_config(model());
    config.after_tool_call = Some(Arc::new(|_context, _signal| {
        Box::pin(async {
            Some(AfterToolCallResult {
                content: Some(vec![TextOrImageContent::Text(TextContent::new("replaced"))]),
                is_error: Some(true),
                ..Default::default()
            })
        })
    }));

    let stream = agent_loop(
        vec![user_message("run")],
        context,
        config,
        None,
        Some(stream_fn),
    );
    let (_, messages) = collect(stream).await;

    let AgentMessage::ToolResult(result) = &messages[2] else {
        panic!("tool result")
    };
    assert!(result.is_error, "isError is overridden");
    let TextOrImageContent::Text(text) = &result.content[0] else {
        panic!("text")
    };
    assert_eq!(text.text, "replaced");
}

#[tokio::test]
async fn a_failing_tool_becomes_an_error_result() {
    let tool = Arc::new(TestTool {
        fail: true,
        ..TestTool::new("boom")
    });
    let context = AgentContext {
        system_prompt: String::new(),
        messages: vec![],
        tools: Some(vec![tool as Arc<dyn AgentTool>]),
    };
    let (stream_fn, _) = scripted_stream_fn(vec![
        assistant_message(
            vec![tool_call("call_1", "boom", json!({}))],
            StopReason::ToolUse,
        ),
        assistant_message(
            vec![AssistantContent::Text(TextContent::new("done"))],
            StopReason::Stop,
        ),
    ]);

    let stream = agent_loop(
        vec![user_message("run")],
        context,
        base_config(model()),
        None,
        Some(stream_fn),
    );
    let (_, messages) = collect(stream).await;

    let AgentMessage::ToolResult(result) = &messages[2] else {
        panic!("tool result")
    };
    assert!(result.is_error);
    let TextOrImageContent::Text(text) = &result.content[0] else {
        panic!("text")
    };
    assert_eq!(text.text, "tool failed");
}

#[tokio::test]
async fn continue_rejects_an_empty_or_assistant_terminated_context() {
    let config = base_config(model());
    let empty = AgentContext {
        system_prompt: String::new(),
        messages: vec![],
        tools: None,
    };
    let Err(error) = agent_loop_continue(empty, config.clone(), None, None) else {
        panic!("must fail")
    };
    assert_eq!(error.to_string(), "Cannot continue: no messages in context");

    let ends_with_assistant = AgentContext {
        system_prompt: String::new(),
        messages: vec![AgentMessage::Assistant(assistant_message(
            vec![],
            StopReason::Stop,
        ))],
        tools: None,
    };
    let Err(error) = agent_loop_continue(ends_with_assistant, config, None, None) else {
        panic!("must fail")
    };
    assert_eq!(
        error.to_string(),
        "Cannot continue from message role: assistant"
    );
}

#[tokio::test]
async fn continue_resumes_without_emitting_prompt_events() {
    let context = AgentContext {
        system_prompt: String::new(),
        messages: vec![user_message("earlier")],
        tools: Some(vec![]),
    };
    let (stream_fn, _) = scripted_stream_fn(vec![assistant_message(
        vec![AssistantContent::Text(TextContent::new("resumed"))],
        StopReason::Stop,
    )]);

    let stream = agent_loop_continue(context, base_config(model()), None, Some(stream_fn))
        .expect("continues");
    let (events, messages) = collect(stream).await;

    assert_eq!(
        event_names(&events),
        [
            "agent_start",
            "turn_start",
            "message_start",
            "message_end",
            "turn_end",
            "agent_end"
        ]
    );
    assert_eq!(
        messages.len(),
        1,
        "only the new assistant message is returned"
    );
}
