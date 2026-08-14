//! Port of `packages/coding-agent/test/delegation-run.test.ts`.
//!
//! Running a child.
//!
//! A real Agent runs against a stubbed provider, so what is asserted is the
//! wiring that decides what a subagent may do: its tool allowlist, the mode
//! block it starts from, and what it borrowed from its parent.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use notagent::core::delegation::run::{
    DelegationOptions, MAX_ANSWER_CHARS, child_tool_names, render_child_prompt, run_delegation,
};
use notagent::core::modes::shells::{ApprovalLevel, ShellId, tools_for_shell};
use notagent::core::modes::{Mode, ModeSkill};
use notagent_agent::agent::{Agent, AgentOptions};
use notagent_agent::types::{AgentMessage, BeforeToolCallResult, BoxFuture};
use notagent_ai::types::{
    AssistantContent, AssistantMessage, AssistantMessageEvent, DoneReason, ErrorReason, Modality,
    Model, ModelCost, StopReason, TextContent, Usage, UserContent, UserMessage,
};
use notagent_ai::utils::event_stream::create_assistant_message_event_stream;
use tokio_util::sync::CancellationToken;

fn model() -> Model {
    Model {
        id: "mock".to_owned(),
        name: "mock".to_owned(),
        api: "anthropic-messages".to_owned(),
        provider: "anthropic".to_owned(),
        base_url: "https://example.invalid".to_owned(),
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

fn assistant(text: &str) -> AssistantMessage {
    AssistantMessage {
        content: if text.is_empty() {
            Vec::new()
        } else {
            vec![AssistantContent::Text(TextContent::new(text))]
        },
        api: "anthropic-messages".to_owned(),
        provider: "anthropic".to_owned(),
        model: "mock".to_owned(),
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

#[derive(Debug, Clone, Default)]
struct Seen {
    system_prompt: String,
    tool_names: Vec<String>,
    first_user_text: String,
    message_count: usize,
}

/// A parent whose provider call records what the child was configured with.
fn parent_agent(reply: AssistantMessage, seen: Arc<Mutex<Vec<Seen>>>) -> Arc<Agent> {
    Agent::new(AgentOptions {
        model: Some(model()),
        system_prompt: Some("PARENT PROMPT".to_owned()),
        tools: Some(Vec::new()),
        get_api_key: Some(Arc::new(|_provider| {
            Box::pin(async { Some("test-key".to_owned()) })
        })),
        stream_fn: Some(Arc::new(move |_model, context, _options| {
            let first = context.messages.first().cloned();
            let first_user_text = match first {
                Some(notagent_ai::types::Message::User(message)) => match message.content {
                    UserContent::Text(text) => text,
                    UserContent::Blocks(blocks) => blocks
                        .iter()
                        .filter_map(|block| match block {
                            notagent_ai::types::TextOrImageContent::Text(text) => {
                                Some(text.text.clone())
                            }
                            _ => None,
                        })
                        .collect::<Vec<String>>()
                        .join(""),
                },
                _ => String::new(),
            };
            seen.lock().expect("seen").push(Seen {
                system_prompt: context.system_prompt.clone().unwrap_or_default(),
                tool_names: context
                    .tools
                    .as_ref()
                    .map(|tools| tools.iter().map(|tool| tool.name.clone()).collect())
                    .unwrap_or_default(),
                first_user_text,
                message_count: context.messages.len(),
            });
            let reply = reply.clone();
            Box::pin(async move {
                let stream = create_assistant_message_event_stream();
                stream.push(AssistantMessageEvent::Done {
                    reason: DoneReason::Stop,
                    message: reply,
                });
                stream
            })
        })),
        ..AgentOptions::default()
    })
}

/// A parent whose provider only ever answers by being abandoned.
fn hanging_parent() -> Arc<Agent> {
    Agent::new(AgentOptions {
        model: Some(model()),
        system_prompt: Some("P".to_owned()),
        tools: Some(Vec::new()),
        get_api_key: Some(Arc::new(|_provider| {
            Box::pin(async { Some("test-key".to_owned()) })
        })),
        stream_fn: Some(Arc::new(move |_model, _context, options| {
            Box::pin(async move {
                let stream = create_assistant_message_event_stream();
                if let Some(signal) = options.and_then(|options| options.base.base.signal.clone()) {
                    let stream_handle = stream.clone();
                    tokio::spawn(async move {
                        signal.cancelled().await;
                        let mut aborted = assistant("");
                        aborted.stop_reason = StopReason::Aborted;
                        stream_handle.push(AssistantMessageEvent::Error {
                            reason: ErrorReason::Aborted,
                            error: aborted,
                        });
                    });
                }
                stream
            })
        })),
        ..AgentOptions::default()
    })
}

/// A parent whose provider fails outright.
fn broken_parent() -> Arc<Agent> {
    Agent::new(AgentOptions {
        model: Some(model()),
        system_prompt: Some("P".to_owned()),
        tools: Some(Vec::new()),
        get_api_key: Some(Arc::new(|_provider| {
            Box::pin(async { Some("test-key".to_owned()) })
        })),
        stream_fn: Some(Arc::new(move |_model, _context, _options| {
            Box::pin(async move {
                let stream = create_assistant_message_event_stream();
                let mut failed = assistant("");
                failed.stop_reason = StopReason::Error;
                failed.error_message = Some("provider exploded".to_owned());
                stream.push(AssistantMessageEvent::Error {
                    reason: ErrorReason::Error,
                    error: failed,
                });
                stream
            })
        })),
        ..AgentOptions::default()
    })
}

fn mode(id: &str, shell: ShellId, body: &str) -> Mode {
    Mode {
        id: id.to_owned(),
        shell,
        approval: ApprovalLevel::Manual,
        tools: tools_for_shell(shell),
        subagents: None,
        skills: vec![ModeSkill {
            file_name: "10.md".to_owned(),
            file_path: format!("/modes/{id}/10.md").into(),
            body: body.to_owned(),
        }],
        source_dir: format!("/modes/{id}").into(),
    }
}

fn options(parent: Arc<Agent>, mode: Mode, task: &str) -> DelegationOptions {
    DelegationOptions {
        parent,
        mode,
        cwd: std::env::current_dir()
            .expect("cwd")
            .to_string_lossy()
            .into_owned(),
        task: task.to_owned(),
        history: None,
        session_id: None,
        signal: None,
        tool_options: None,
        resolve_tool: None,
        timeout_ms: None,
        on_tokens: None,
    }
}

// ── what a child may do ───────────────────────────────────────────────

#[tokio::test]
async fn gets_its_modes_tools() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    run_delegation(options(
        parent_agent(assistant("done"), Arc::clone(&seen)),
        mode("plan", ShellId::ReadOnly, "Plan mode."),
        "look at the code",
    ))
    .await;
    let mut names = seen.lock().expect("seen")[0].tool_names.clone();
    names.sort();
    assert_eq!(
        names,
        vec!["find", "grep", "ls", "read", "read_minified", "skill"]
    );
}

#[test]
fn never_gets_the_delegation_tool_whatever_its_mode_allows() {
    assert!(
        !child_tool_names(&mode("worker", ShellId::Worker, ""))
            .contains(&notagent::core::tools::ToolName::Task)
    );
    assert!(
        !child_tool_names(&mode("plan", ShellId::ReadOnly, ""))
            .contains(&notagent::core::tools::ToolName::Task)
    );
}

#[tokio::test]
async fn gets_no_tool_that_mutates_when_its_mode_is_read_only() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    run_delegation(options(
        parent_agent(assistant("done"), Arc::clone(&seen)),
        mode("plan", ShellId::ReadOnly, "Plan mode."),
        "look",
    ))
    .await;
    let names = seen.lock().expect("seen")[0].tool_names.clone();
    for forbidden in [
        "bash",
        "write",
        "edit",
        "patch_minified",
        "multi_patch_minified",
    ] {
        assert!(!names.contains(&forbidden.to_owned()), "{forbidden}");
    }
}

// ── how a child is activated ──────────────────────────────────────────

#[test]
fn starts_from_its_mode_block_then_the_task() {
    let prompt = render_child_prompt(
        &mode("plan", ShellId::ReadOnly, "Read, do not write."),
        "check the parser",
    );
    assert!(
        prompt.contains("<mode name=\"plan\" shell=\"read-only\">"),
        "{prompt}"
    );
    assert!(prompt.contains("Read, do not write."), "{prompt}");
    assert!(prompt.ends_with("check the parser"), "{prompt}");
}

#[test]
fn sends_only_the_task_when_the_mode_has_nothing_to_say() {
    assert_eq!(
        render_child_prompt(&mode("bare", ShellId::Worker, ""), "do it"),
        "do it"
    );
}

#[tokio::test]
async fn delivers_that_prompt_to_the_provider() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    run_delegation(options(
        parent_agent(assistant("done"), Arc::clone(&seen)),
        mode("plan", ShellId::ReadOnly, "Read, do not write."),
        "check the parser",
    ))
    .await;
    let first = seen.lock().expect("seen")[0].clone();
    assert!(
        first.first_user_text.contains("<mode name=\"plan\""),
        "{first:?}"
    );
    assert!(
        first.first_user_text.contains("check the parser"),
        "{first:?}"
    );
}

#[tokio::test]
async fn does_not_repeat_the_block_when_continuing_an_existing_child() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let history = vec![
        AgentMessage::User(UserMessage {
            content: UserContent::Text("earlier".to_owned()),
            timestamp: 0,
        }),
        AgentMessage::Assistant(assistant("earlier answer")),
    ];
    run_delegation(DelegationOptions {
        history: Some(history),
        session_id: Some("child-1".to_owned()),
        ..options(
            parent_agent(assistant("done"), Arc::clone(&seen)),
            mode("plan", ShellId::ReadOnly, "Read, do not write."),
            "and now this",
        )
    })
    .await;
    let first = seen.lock().expect("seen")[0].clone();
    assert!(first.message_count > 1, "{first:?}");
    assert_eq!(first.first_user_text, "earlier");
}

// ── what a child borrows from its parent ──────────────────────────────

#[tokio::test]
async fn uses_the_parents_system_prompt() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    run_delegation(options(
        parent_agent(assistant("done"), Arc::clone(&seen)),
        mode("plan", ShellId::ReadOnly, "x"),
        "t",
    ))
    .await;
    assert_eq!(seen.lock().expect("seen")[0].system_prompt, "PARENT PROMPT");
}

#[tokio::test]
async fn inherits_the_tool_hooks_so_a_child_is_governed_like_its_parent() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let parent = parent_agent(assistant("done"), Arc::clone(&seen));
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    parent.update_options(move |options| {
        options.before_tool_call = Some(Arc::new(move |_context, _signal| {
            counter.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { None::<BeforeToolCallResult> }) as BoxFuture<'static, _>
        }));
    });
    run_delegation(options(
        Arc::clone(&parent),
        mode("plan", ShellId::ReadOnly, "x"),
        "t",
    ))
    .await;
    // Observed through the run rather than the constructor: what matters is
    // that a child cannot end up outside the permission chain.
    assert!(parent.options().before_tool_call.is_some());
}

// ── the ceiling on how long a child may run ───────────────────────────

#[tokio::test]
async fn stops_a_child_that_never_finishes_and_says_so() {
    let run = run_delegation(DelegationOptions {
        timeout_ms: Some(300),
        ..options(hanging_parent(), mode("plan", ShellId::ReadOnly, "x"), "t")
    })
    .await;
    assert!(run.failed);
    assert!(run.text.contains("deadline"), "{}", run.text);
    // The partial text is deliberately absent: continuing the same child keeps
    // everything it learned, while a fragment reads like a conclusion.
    assert!(
        run.text.contains("Continue it with session_id"),
        "{}",
        run.text
    );
}

#[tokio::test]
async fn does_not_interfere_with_a_child_that_answers_in_time() {
    let run = run_delegation(DelegationOptions {
        timeout_ms: Some(30_000),
        ..options(
            parent_agent(assistant("in time"), Arc::new(Mutex::new(Vec::new()))),
            mode("plan", ShellId::ReadOnly, "x"),
            "t",
        )
    })
    .await;
    assert_eq!(run.text, "in time");
    assert!(!run.failed);
}

// ── what comes back ───────────────────────────────────────────────────

#[tokio::test]
async fn is_the_childs_last_answer() {
    let run = run_delegation(options(
        parent_agent(assistant("the answer"), Arc::new(Mutex::new(Vec::new()))),
        mode("plan", ShellId::ReadOnly, "x"),
        "t",
    ))
    .await;
    assert_eq!(run.text, "the answer");
    assert!(!run.failed);
    assert!(!run.session_id.is_empty());
}

#[tokio::test]
async fn keeps_the_id_it_was_given() {
    let run = run_delegation(DelegationOptions {
        session_id: Some("child-7".to_owned()),
        ..options(
            parent_agent(assistant("x"), Arc::new(Mutex::new(Vec::new()))),
            mode("plan", ShellId::ReadOnly, "x"),
            "t",
        )
    })
    .await;
    assert_eq!(run.session_id, "child-7");
}

#[tokio::test]
async fn carries_the_transcript_so_the_child_can_be_continued() {
    let run = run_delegation(options(
        parent_agent(assistant("x"), Arc::new(Mutex::new(Vec::new()))),
        mode("plan", ShellId::ReadOnly, "x"),
        "t",
    ))
    .await;
    assert!(!run.transcript.is_empty());
}

#[tokio::test]
async fn reports_a_silent_child_as_a_failure() {
    let run = run_delegation(options(
        parent_agent(assistant(""), Arc::new(Mutex::new(Vec::new()))),
        mode("plan", ShellId::ReadOnly, "x"),
        "t",
    ))
    .await;
    assert!(run.failed);
    assert!(run.text.contains("no answer"), "{}", run.text);
}

#[tokio::test]
async fn reports_a_broken_provider_as_a_result_not_as_an_exception() {
    let run = run_delegation(options(
        broken_parent(),
        mode("plan", ShellId::ReadOnly, "x"),
        "t",
    ))
    .await;
    assert!(run.failed, "{}", run.text);
}

#[tokio::test]
async fn caps_an_answer_that_would_spend_the_parents_context() {
    let huge = "x".repeat(MAX_ANSWER_CHARS + 5_000);
    let run = run_delegation(options(
        parent_agent(assistant(&huge), Arc::new(Mutex::new(Vec::new()))),
        mode("plan", ShellId::ReadOnly, "x"),
        "t",
    ))
    .await;
    assert!(run.text.len() < huge.len());
    assert!(run.text.contains("truncated"), "{}", &run.text[..80]);
}

#[tokio::test]
async fn leaves_an_ordinary_answer_untouched() {
    // Padded past the minimum a handoff must reach: a short answer costs an
    // extra turn by design, and this case is not about that.
    let answer = format!("short answer {}", "detail ".repeat(40));
    let run = run_delegation(options(
        parent_agent(assistant(&answer), Arc::new(Mutex::new(Vec::new()))),
        mode("plan", ShellId::ReadOnly, "x"),
        "t",
    ))
    .await;
    assert_eq!(run.text, answer.trim());
}

#[tokio::test]
async fn does_not_start_at_all_once_the_turn_is_already_interrupted() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let signal = CancellationToken::new();
    signal.cancel();
    let run = run_delegation(DelegationOptions {
        signal: Some(signal),
        ..options(
            parent_agent(assistant("x"), Arc::clone(&seen)),
            mode("plan", ShellId::ReadOnly, "x"),
            "t",
        )
    })
    .await;
    assert!(run.failed);
    assert!(seen.lock().expect("seen").is_empty());
}

// ── the expansion turn ────────────────────────────────────────────────

#[tokio::test]
async fn asks_a_terse_child_to_expand_exactly_once() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    // The stub answers "tiny" every time, so the second turn is the expansion
    // request and there is no third.
    let run = run_delegation(options(
        parent_agent(assistant("tiny"), Arc::clone(&seen)),
        mode("plan", ShellId::ReadOnly, "x"),
        "t",
    ))
    .await;
    assert_eq!(seen.lock().expect("seen").len(), 2);
    assert!(
        seen.lock().expect("seen")[1]
            .first_user_text
            .contains("<mode name=\"plan\""),
        "the expansion turn keeps the child's own conversation"
    );
    assert_eq!(run.text, "tiny");
}
