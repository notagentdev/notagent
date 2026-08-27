//! A side question must cost the main conversation nothing — not context, and
//! not its cached prefix.
//!
//! Each case here guards one way the cache is lost. They are separate
//! assertions on purpose: a single "the child looks right" test would go green
//! again after a refactor that quietly filtered the tools.

mod suite;

use std::sync::{Arc, Mutex};

use notagent::core::agent_session::PromptOptions;
use notagent::core::side_question::{
    SIDE_QUESTION_REMINDER_TYPE, SideQuestionFork, TOOL_CALL_DISABLED_MESSAGE, complete_messages,
    create_side_question_agent,
};
use notagent::modes::interactive::components::footer::FooterSession;
use notagent_agent::types::{AgentEvent, AgentMessage};
use notagent_ai::providers::faux::{FauxResponseStep, faux_assistant_message, faux_text};
use notagent_ai::types::StopReason;
use serde_json::json;

use suite::{Harness, HarnessOptions, create_harness};

fn reply(text: &str) -> FauxResponseStep {
    faux_assistant_message(vec![faux_text(text)], StopReason::Stop).into()
}

/// A parent with one exchange behind it, which is the prefix the child inherits.
async fn parent_with_history() -> Harness {
    let harness = create_harness(HarnessOptions::default());
    harness.set_responses(vec![reply("the first answer")]);
    harness
        .session
        .prompt("the first question", PromptOptions::default())
        .await
        .expect("prompt");
    harness
}

fn fork_of(harness: &Harness) -> Arc<notagent_agent::agent::Agent> {
    let parent = harness.session.agent();
    create_side_question_agent(&SideQuestionFork {
        parent: Arc::clone(&parent),
        session_id: harness.session.session_id(),
        history: complete_messages(&parent),
    })
}

// ---------------------------------------------------------------------------
// The cache invariants
// ---------------------------------------------------------------------------

/// The Anthropic breakpoint sits on the last tool of the list, so a child with
/// a shorter list moves it and invalidates everything behind it. Filtering is
/// the obvious way to stop the child calling tools, and it is the wrong one.
#[tokio::test]
async fn the_child_gets_the_parents_tools_unfiltered_and_in_order() {
    let harness = parent_with_history().await;
    let parent_tools: Vec<String> = harness
        .session
        .agent()
        .state()
        .tools
        .iter()
        .map(|tool| tool.name().to_string())
        .collect();
    assert!(parent_tools.len() > 1, "the parent has tools to compare");

    let child_tools: Vec<String> = fork_of(&harness)
        .state()
        .tools
        .iter()
        .map(|tool| tool.name().to_string())
        .collect();

    assert_eq!(
        child_tools, parent_tools,
        "same tools, same order — the cache breakpoint rides on the last one"
    );
}

/// The system prompt is the head of the cached prefix; appending so much as a
/// paragraph rewrites all of it.
#[tokio::test]
async fn the_child_gets_the_parents_system_prompt_unchanged() {
    let harness = parent_with_history().await;
    assert_eq!(
        fork_of(&harness).state().system_prompt,
        harness.session.agent().state().system_prompt
    );
}

/// The id becomes the OpenAI prompt cache key, the Anthropic session-affinity
/// header and the local server's session header. Delegation mints a fresh one
/// because a subagent shares no prefix; this child shares all of it.
#[tokio::test]
async fn the_child_inherits_the_parents_cache_identity() {
    let harness = parent_with_history().await;
    assert_eq!(
        fork_of(&harness).options().session_id,
        Some(harness.session.session_id()),
        "a fresh id would tell a prefix cache this conversation is a stranger"
    );
}

/// Everything before the instruction stays byte-identical, so the provider
/// reads the prefix rather than rewriting it.
#[tokio::test]
async fn the_instruction_is_appended_and_nothing_before_it_moves() {
    let harness = parent_with_history().await;
    let parent_messages = harness.session.agent().state().messages;
    let child_messages = fork_of(&harness).state().messages;

    assert_eq!(
        child_messages.len(),
        parent_messages.len() + 1,
        "the parent's history plus exactly one entry"
    );
    for (index, parent_message) in parent_messages.iter().enumerate() {
        assert_eq!(
            serde_json::to_value(&child_messages[index]).expect("child message"),
            serde_json::to_value(parent_message).expect("parent message"),
            "message {index} differs, so the prefix is not shared"
        );
    }
    let last = child_messages.last().expect("the instruction");
    assert!(
        matches!(last, AgentMessage::Custom(custom)
            if custom.custom_type == SIDE_QUESTION_REMINDER_TYPE && !custom.display),
        "the instruction is last and undisplayed: {last:?}"
    );
}

// ---------------------------------------------------------------------------
// What the child may and may not do
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_tool_call_from_the_child_is_refused_rather_than_run() {
    let harness = parent_with_history().await;
    let file = harness.temp.path().join("secret.txt");
    std::fs::write(&file, "untouched\n").expect("write");

    harness.session.start_side_question().expect("start");
    harness.set_responses(vec![
        faux_assistant_message(
            vec![notagent_ai::providers::faux::faux_tool_call(
                "read",
                json!({ "path": file.to_string_lossy() }),
                None,
            )],
            StopReason::ToolUse,
        )
        .into(),
        reply("I cannot use tools here."),
    ]);

    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    harness
        .session
        .ask_side_question(
            "what is in that file?",
            Arc::new(move |event: AgentEvent| {
                if let AgentEvent::ToolExecutionEnd { result, .. } = event {
                    let text = serde_json::to_string(&result.content).unwrap_or_default();
                    sink.lock().expect("poisoned").push(text);
                }
            }),
        )
        .await
        .expect("the child answers");

    let refusals = seen.lock().expect("poisoned").clone();
    assert!(
        refusals
            .iter()
            .any(|text| text.contains(TOOL_CALL_DISABLED_MESSAGE)),
        "the refusal is what the model reads back: {refusals:?}"
    );
}

/// The point of the whole feature: the exchange leaves no trace.
#[tokio::test]
async fn the_main_conversation_is_untouched_by_a_side_question() {
    let harness = parent_with_history().await;
    let before = serde_json::to_value(harness.session.agent().state().messages).expect("before");
    let entries_before = harness.session.entries().len();

    harness.session.start_side_question().expect("start");
    harness.set_responses(vec![reply("a side answer")]);
    harness
        .session
        .ask_side_question("a side question", Arc::new(|_event| {}))
        .await
        .expect("the child answers");

    let after = serde_json::to_value(harness.session.agent().state().messages).expect("after");
    assert_eq!(before, after, "not one message reached the main agent");
    assert_eq!(
        harness.session.entries().len(),
        entries_before,
        "and nothing was written to the session"
    );
}

#[tokio::test]
async fn a_follow_up_reaches_the_same_child() {
    let harness = parent_with_history().await;
    harness.session.start_side_question().expect("start");
    harness.set_responses(vec![
        reply("first side answer"),
        reply("second side answer"),
    ]);

    harness
        .session
        .ask_side_question("one", Arc::new(|_event| {}))
        .await
        .expect("first");
    harness
        .session
        .ask_side_question("two", Arc::new(|_event| {}))
        .await
        .expect("second");

    // Both questions and both answers are in the one child, on top of the
    // inherited history and the instruction.
    assert_eq!(
        harness.faux.pending_response_count(),
        0,
        "both turns really ran"
    );
}

#[tokio::test]
async fn asking_without_starting_is_refused() {
    let harness = parent_with_history().await;
    let error = harness
        .session
        .ask_side_question("anything", Arc::new(|_event| {}))
        .await
        .expect_err("there is no child");
    assert!(error.contains("No side question"), "{error}");
}

#[tokio::test]
async fn starting_again_replaces_the_open_child() {
    let harness = parent_with_history().await;
    harness.session.start_side_question().expect("first");
    harness.session.start_side_question().expect("second");
    assert!(!harness.session.side_question_is_running());

    harness.session.cancel_side_question();
    let error = harness
        .session
        .ask_side_question("anything", Arc::new(|_event| {}))
        .await
        .expect_err("cancelling drops the child");
    assert!(error.contains("No side question"), "{error}");
}
