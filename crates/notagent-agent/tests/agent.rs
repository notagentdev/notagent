//! Port of the core cases of `packages/agent/test/agent.test.ts` (810 LOC).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use notagent_agent::agent::{Agent, AgentOptions};
use notagent_agent::types::*;
use notagent_ai::types::*;
use notagent_ai::utils::event_stream::create_assistant_message_event_stream;

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

fn assistant_message(text: &str) -> AssistantMessage {
    AssistantMessage {
        content: vec![AssistantContent::Text(TextContent::new(text))],
        api: "openai-responses".to_string(),
        provider: "openai".to_string(),
        model: "mock".to_string(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: StopReason::Stop,
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

/// Answers every turn with the given texts, one per call.
fn scripted_stream_fn(texts: Vec<&'static str>) -> (StreamFn, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let stream_fn: StreamFn = Arc::new(move |_model, _context, _options| {
        let index = counter.fetch_add(1, Ordering::SeqCst);
        let texts = texts.clone();
        Box::pin(async move {
            let stream = create_assistant_message_event_stream();
            let message = assistant_message(texts.get(index).copied().unwrap_or("done"));
            stream.push(AssistantMessageEvent::Done {
                reason: DoneReason::Stop,
                message,
            });
            stream
        })
    });
    (stream_fn, calls)
}

fn options(stream_fn: StreamFn) -> AgentOptions {
    AgentOptions {
        model: Some(model()),
        stream_fn: Some(stream_fn),
        ..Default::default()
    }
}

#[tokio::test]
async fn a_prompt_appends_the_transcript_and_notifies_listeners() {
    let (stream_fn, _) = scripted_stream_fn(vec!["Hi there!"]);
    let agent = Agent::new(options(stream_fn));

    let events = Arc::new(Mutex::new(Vec::<&'static str>::new()));
    let recorder = Arc::clone(&events);
    let _unsubscribe = agent.subscribe(Arc::new(move |event, _signal| {
        recorder.lock().expect("poisoned").push(event.type_name());
        Box::pin(async {})
    }));

    agent
        .prompt(vec![user_message("Hello")])
        .await
        .expect("prompt");

    let state = agent.state();
    assert_eq!(state.messages.len(), 2);
    assert!(matches!(state.messages[0], AgentMessage::User(_)));
    assert!(matches!(state.messages[1], AgentMessage::Assistant(_)));
    assert!(!state.is_streaming, "the run has settled");
    assert!(state.streaming_message.is_none());

    let names = events.lock().expect("poisoned").clone();
    assert_eq!(names.first(), Some(&"agent_start"));
    assert_eq!(names.last(), Some(&"agent_end"));
}

#[tokio::test]
async fn a_second_prompt_during_a_run_is_rejected() {
    let (stream_fn, _) = scripted_stream_fn(vec!["one"]);
    let agent = Agent::new(options(stream_fn));

    // The run is driven to completion first; a nested prompt is what TS rejects.
    let blocker = Arc::clone(&agent);
    let error = Arc::new(Mutex::new(None));
    let sink = Arc::clone(&error);
    let _unsubscribe = agent.subscribe(Arc::new(move |event, _signal| {
        let agent = Arc::clone(&blocker);
        let sink = Arc::clone(&sink);
        Box::pin(async move {
            if event.type_name() == "turn_start" {
                let result = agent.prompt(vec![user_message("nested")]).await;
                *sink.lock().expect("poisoned") =
                    Some(result.expect_err("must reject").to_string());
            }
        })
    }));

    agent
        .prompt(vec![user_message("Hello")])
        .await
        .expect("prompt");
    let message = error
        .lock()
        .expect("poisoned")
        .clone()
        .expect("nested prompt was attempted");
    assert!(
        message.starts_with("Agent is already processing a prompt"),
        "{message}"
    );
}

#[tokio::test]
async fn steering_messages_are_drained_one_at_a_time_by_default() {
    let (stream_fn, calls) = scripted_stream_fn(vec!["first", "second", "third"]);
    let agent = Agent::new(options(stream_fn));
    assert_eq!(agent.steering_mode(), QueueMode::OneAtATime);

    agent.steer(user_message("steer one"));
    agent.steer(user_message("steer two"));
    assert!(agent.has_queued_messages());

    agent
        .prompt(vec![user_message("start")])
        .await
        .expect("prompt");

    // runLoop polls the steering queue once before the first turn, so turn one carries
    // the prompt plus "steer one" and turn two carries "steer two": two provider calls.
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(!agent.has_queued_messages());
}

#[tokio::test]
async fn queue_mode_all_drains_everything_at_once() {
    let (stream_fn, calls) = scripted_stream_fn(vec!["first", "second"]);
    let agent = Agent::new(AgentOptions {
        steering_mode: Some(QueueMode::All),
        ..options(stream_fn)
    });

    agent.steer(user_message("one"));
    agent.steer(user_message("two"));
    agent
        .prompt(vec![user_message("start")])
        .await
        .expect("prompt");

    // "all" drains both in the initial poll, so a single turn carries everything.
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "both messages are injected into one turn"
    );
    let state = agent.state();
    let user_texts: Vec<String> = state
        .messages
        .iter()
        .filter_map(|message| match message {
            AgentMessage::User(user) => match &user.content {
                UserContent::Text(text) => Some(text.clone()),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(user_texts, ["start", "one", "two"]);
}

#[tokio::test]
async fn follow_up_messages_run_after_the_agent_would_stop() {
    let (stream_fn, calls) = scripted_stream_fn(vec!["first", "second"]);
    let agent = Agent::new(options(stream_fn));

    agent.follow_up(user_message("later"));
    agent
        .prompt(vec![user_message("start")])
        .await
        .expect("prompt");

    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(!agent.has_queued_messages());
}

#[tokio::test]
async fn clearing_the_queues_drops_pending_messages() {
    let (stream_fn, calls) = scripted_stream_fn(vec!["only"]);
    let agent = Agent::new(options(stream_fn));

    agent.steer(user_message("dropped"));
    agent.follow_up(user_message("dropped too"));
    assert!(agent.has_queued_messages());
    agent.clear_all_queues();
    assert!(!agent.has_queued_messages());

    agent
        .prompt(vec![user_message("start")])
        .await
        .expect("prompt");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn continue_requires_messages_and_rejects_an_assistant_tail() {
    let (stream_fn, _) = scripted_stream_fn(vec!["answer"]);
    let agent = Agent::new(options(stream_fn));

    let error = agent.continue_run().await.expect_err("no messages");
    assert_eq!(error.to_string(), "No messages to continue from");

    agent.set_messages(vec![AgentMessage::Assistant(assistant_message("prior"))]);
    let error = agent.continue_run().await.expect_err("assistant tail");
    assert_eq!(
        error.to_string(),
        "Cannot continue from message role: assistant"
    );
}

#[tokio::test]
async fn continue_after_an_assistant_message_consumes_the_steering_queue() {
    let (stream_fn, calls) = scripted_stream_fn(vec!["answer"]);
    let agent = Agent::new(options(stream_fn));
    agent.set_messages(vec![AgentMessage::Assistant(assistant_message("prior"))]);
    agent.steer(user_message("queued"));

    agent
        .continue_run()
        .await
        .expect("continues from the queue");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    // The queued message became the prompt of this run rather than being polled again.
    let state = agent.state();
    assert!(
        state
            .messages
            .iter()
            .any(|message| matches!(message, AgentMessage::User(user)
        if matches!(&user.content, UserContent::Text(text) if text == "queued")))
    );
}

#[tokio::test]
async fn continue_resumes_from_a_user_tail() {
    let (stream_fn, calls) = scripted_stream_fn(vec!["resumed"]);
    let agent = Agent::new(options(stream_fn));
    agent.set_messages(vec![user_message("earlier")]);

    agent.continue_run().await.expect("continues");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(agent.state().messages.len(), 2);
}

#[tokio::test]
async fn reset_clears_the_transcript_and_the_queues() {
    let (stream_fn, _) = scripted_stream_fn(vec!["answer"]);
    let agent = Agent::new(options(stream_fn));
    agent
        .prompt(vec![user_message("hi")])
        .await
        .expect("prompt");
    agent.steer(user_message("queued"));

    agent.reset().expect("reset");
    assert!(agent.state().messages.is_empty());
    assert!(!agent.has_queued_messages());
    assert!(agent.state().error_message.is_none());
}

#[tokio::test]
async fn pending_tool_calls_are_tracked_while_a_tool_runs() {
    struct SlowTool {
        parameters: serde_json::Value,
    }
    impl AgentTool for SlowTool {
        fn name(&self) -> &str {
            "slow"
        }
        fn description(&self) -> &str {
            "slow"
        }
        fn parameters(&self) -> &serde_json::Value {
            &self.parameters
        }
        fn label(&self) -> &str {
            "Slow"
        }
        fn execute<'a>(
            &'a self,
            _tool_call_id: &'a str,
            _params: serde_json::Value,
            _signal: Option<tokio_util::sync::CancellationToken>,
            _on_update: Option<AgentToolUpdateCallback>,
        ) -> BoxFuture<'a, Result<AgentToolResult, ToolExecutionError>> {
            Box::pin(async { Ok(AgentToolResult::text("done")) })
        }
    }

    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let stream_fn: StreamFn = Arc::new(move |_model, _context, _options| {
        let index = counter.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            let stream = create_assistant_message_event_stream();
            let message = if index == 0 {
                AssistantMessage {
                    content: vec![AssistantContent::ToolCall(ToolCall {
                        id: "call_1".to_string(),
                        name: "slow".to_string(),
                        ..Default::default()
                    })],
                    stop_reason: StopReason::ToolUse,
                    ..assistant_message("")
                }
            } else {
                assistant_message("done")
            };
            let reason = if index == 0 {
                DoneReason::ToolUse
            } else {
                DoneReason::Stop
            };
            stream.push(AssistantMessageEvent::Done { reason, message });
            stream
        })
    });

    let agent = Agent::new(AgentOptions {
        tools: Some(vec![Arc::new(SlowTool {
            parameters: serde_json::json!({"type": "object"}),
        }) as Arc<dyn AgentTool>]),
        ..options(stream_fn)
    });

    let seen_pending = Arc::new(Mutex::new(Vec::<usize>::new()));
    let recorder = Arc::clone(&seen_pending);
    let observed = Arc::clone(&agent);
    let _unsubscribe = agent.subscribe(Arc::new(move |event, _signal| {
        if event.type_name() == "tool_execution_start" || event.type_name() == "tool_execution_end"
        {
            recorder
                .lock()
                .expect("poisoned")
                .push(observed.state().pending_tool_calls.len());
        }
        Box::pin(async {})
    }));

    agent
        .prompt(vec![user_message("run")])
        .await
        .expect("prompt");

    assert_eq!(
        seen_pending.lock().expect("poisoned").as_slice(),
        [1, 0],
        "the id is tracked while running"
    );
    assert!(
        agent.state().pending_tool_calls.is_empty(),
        "cleared once the run finishes"
    );
}

#[tokio::test]
async fn an_error_turn_is_recorded_in_the_state() {
    let stream_fn: StreamFn = Arc::new(move |_model, _context, _options| {
        Box::pin(async move {
            let stream = create_assistant_message_event_stream();
            let message = AssistantMessage {
                stop_reason: StopReason::Error,
                error_message: Some("provider down".to_string()),
                ..assistant_message("")
            };
            stream.push(AssistantMessageEvent::Error {
                reason: ErrorReason::Error,
                error: message,
            });
            stream
        })
    });
    let agent = Agent::new(options(stream_fn));

    agent
        .prompt(vec![user_message("hi")])
        .await
        .expect("prompt");
    assert_eq!(
        agent.state().error_message.as_deref(),
        Some("provider down")
    );
}

#[tokio::test]
async fn wait_for_idle_returns_immediately_without_an_active_run() {
    let (stream_fn, _) = scripted_stream_fn(vec!["answer"]);
    let agent = Agent::new(options(stream_fn));
    // Must not hang.
    tokio::time::timeout(std::time::Duration::from_secs(1), agent.wait_for_idle())
        .await
        .expect("wait_for_idle returns without an active run");
}

/// `waitForIdle()` hands out the promise of the run that was active at call
/// time, and that promise stays resolved. Neither a run finishing while the
/// waiter registers nor a successor run taking the slot may make the wait hang.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wait_for_idle_never_misses_the_end_of_its_own_run() {
    for _ in 0..300 {
        let (stream_fn, _) = scripted_stream_fn(vec!["one", "two"]);
        let agent = Agent::new(options(stream_fn));

        let runner = {
            let agent = Arc::clone(&agent);
            tokio::spawn(async move {
                agent
                    .prompt(vec![user_message("first")])
                    .await
                    .expect("first prompt");
                // A successor run occupies the slot right after the first one
                // released it — the window in which the waiter may look.
                agent
                    .prompt(vec![user_message("second")])
                    .await
                    .expect("second prompt");
            })
        };

        let waiters: Vec<_> = (0..4)
            .map(|_| {
                let agent = Arc::clone(&agent);
                tokio::spawn(async move { agent.wait_for_idle().await })
            })
            .collect();

        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            runner.await.expect("runner");
            for waiter in waiters {
                waiter.await.expect("waiter");
            }
        })
        .await
        .expect("wait_for_idle resolves");
    }
}

#[tokio::test]
async fn unsubscribing_stops_notifications() {
    let (stream_fn, _) = scripted_stream_fn(vec!["answer"]);
    let agent = Agent::new(options(stream_fn));
    let seen = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&seen);

    let unsubscribe = agent.subscribe(Arc::new(move |_event, _signal| {
        counter.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {})
    }));
    unsubscribe();

    agent
        .prompt(vec![user_message("hi")])
        .await
        .expect("prompt");
    assert_eq!(seen.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn listeners_are_awaited_in_subscription_order() {
    let (stream_fn, _) = scripted_stream_fn(vec!["answer"]);
    let agent = Agent::new(options(stream_fn));
    let order = Arc::new(Mutex::new(Vec::<&'static str>::new()));

    let first = Arc::clone(&order);
    let _unsubscribe = agent.subscribe(Arc::new(move |_event, _signal| {
        let sink = Arc::clone(&first);
        Box::pin(async move {
            tokio::task::yield_now().await;
            sink.lock().expect("poisoned").push("first");
        })
    }));
    let second = Arc::clone(&order);
    let _unsubscribe = agent.subscribe(Arc::new(move |_event, _signal| {
        let sink = Arc::clone(&second);
        Box::pin(async move {
            sink.lock().expect("poisoned").push("second");
        })
    }));

    agent
        .prompt(vec![user_message("hi")])
        .await
        .expect("prompt");

    // Sequential awaits: "first" always completes before "second" starts.
    let order = order.lock().expect("poisoned").clone();
    for pair in order.chunks(2) {
        assert_eq!(
            pair,
            ["first", "second"],
            "listeners run in subscription order"
        );
    }
}

// ---------------------------------------------------------------------------
// Provider wiring (interface request C-8)
// ---------------------------------------------------------------------------

/// TS keeps `streamFunction`, `getApiKey`, `onPayload`, `onResponse`, `beforeToolCall`,
/// `afterToolCall`, `thinkingBudgets`, `transport`, `maxRetryDelayMs` and `toolExecution`
/// as public fields (`agent.ts:180-201`), and `delegation/run.ts:170-203` copies them onto
/// a child agent. The Rust port exposes the same through `options`/`update_options`.
#[tokio::test]
async fn the_provider_wiring_is_readable_and_writable() {
    let (stream_fn, _) = scripted_stream_fn(vec!["ok"]);
    let agent = Agent::new(AgentOptions {
        transport: Some(Transport::Sse),
        max_retry_delay_ms: Some(1234),
        ..options(stream_fn)
    });

    let current = agent.options();
    assert_eq!(current.transport, Some(Transport::Sse));
    assert_eq!(current.max_retry_delay_ms, Some(1234));
    assert!(current.stream_fn.is_some());

    // The runtime assignments TS does (`session.agent.transport = …`).
    agent.update_options(|options| {
        options.transport = Some(Transport::Websocket);
        options.tool_execution = Some(ToolExecutionMode::Sequential);
    });

    let current = agent.options();
    assert_eq!(current.transport, Some(Transport::Websocket));
    assert_eq!(current.tool_execution, Some(ToolExecutionMode::Sequential));
}

/// `onPayload`/`onResponse` reach the provider through the loop config, as TS forwards
/// them into `streamSimple` (`agent.ts:452-453`).
#[tokio::test]
async fn on_payload_and_on_response_reach_the_stream_options() {
    let seen = Arc::new(Mutex::new(Vec::<&'static str>::new()));
    let sink = Arc::clone(&seen);
    let stream_fn: StreamFn = Arc::new(move |_model, _context, options| {
        if let Some(options) = options.as_ref() {
            if options.base.base.on_payload.is_some() {
                sink.lock().expect("poisoned").push("onPayload");
            }
            if options.base.base.on_response.is_some() {
                sink.lock().expect("poisoned").push("onResponse");
            }
        }
        Box::pin(async move {
            let stream = create_assistant_message_event_stream();
            let message = assistant_message("ok");
            stream.push(AssistantMessageEvent::Done {
                reason: DoneReason::Stop,
                message: message.clone(),
            });
            stream.end(Some(message));
            stream
        })
    });

    let agent = Agent::new(AgentOptions {
        on_payload: Some(Arc::new(|payload, _model| {
            Box::pin(async move { Some(payload) })
        })),
        on_response: Some(Arc::new(|_response, _model| Box::pin(async {}))),
        ..options(stream_fn)
    });

    agent
        .prompt(vec![user_message("hi")])
        .await
        .expect("prompt");

    assert_eq!(
        seen.lock().expect("poisoned").clone(),
        vec!["onPayload", "onResponse"]
    );
}
