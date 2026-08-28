mod suite;

use notagent::core::agent_session::PromptOptions;
use notagent::core::goal::{
    GOAL_REMINDER_CONTINUATION, GOAL_REMINDER_KIND, GOAL_REMINDER_TYPE, GOAL_REMINDER_WRAP_UP,
    MAX_CONTINUATIONS_PER_TURN, ThreadGoal, ThreadGoalStatus,
};
use notagent::core::todos::reminder::TODO_REMINDER_TYPE;
use notagent::core::todos::{Todo, TodoStatus};
use notagent_agent::types::AgentMessage;
use notagent_ai::providers::faux::{FauxResponseStep, faux_assistant_message, faux_text};
use notagent_ai::types::StopReason;
use serde_json::Value;

use suite::{Harness, HarnessOptions, create_harness};

fn reply(text: &str) -> FauxResponseStep {
    faux_assistant_message(vec![faux_text(text)], StopReason::Stop).into()
}

/// The driver's reminders in the transcript, as their kind.
fn reminders(harness: &Harness) -> Vec<String> {
    harness
        .session
        .messages()
        .into_iter()
        .filter_map(|message| {
            let AgentMessage::Custom(custom) = message else {
                return None;
            };
            if custom.custom_type != GOAL_REMINDER_TYPE {
                return None;
            }
            custom
                .details
                .as_ref()
                .and_then(|details| details.get(GOAL_REMINDER_KIND))
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect()
}

fn set_goal(harness: &Harness, goal: ThreadGoal) {
    harness
        .session
        .goal_state()
        .lock()
        .expect("goal state")
        .goal = Some(goal);
}

fn goal_status(harness: &Harness) -> Option<ThreadGoalStatus> {
    harness.session.goal().map(|goal| goal.status)
}

#[tokio::test]
async fn a_turn_without_a_goal_is_not_continued() {
    let harness = create_harness(HarnessOptions::default());
    harness.set_responses(vec![reply("done")]);

    harness
        .session
        .prompt("go", Default::default())
        .await
        .expect("prompt");

    assert!(reminders(&harness).is_empty());
}

#[tokio::test]
async fn an_active_goal_hands_the_agent_a_continuation() {
    let harness = create_harness(HarnessOptions::default());
    harness.set_responses(vec![reply("a first step")]);
    set_goal(&harness, ThreadGoal::new("ship the release", Some(1), None));

    harness
        .session
        .prompt("go", Default::default())
        .await
        .expect("prompt");

    // A tight token budget stops the loop after the first continuation, which
    // is what makes the count here readable.
    assert_eq!(
        reminders(&harness).first().map(String::as_str),
        Some(GOAL_REMINDER_CONTINUATION)
    );
    // The turn was counted against the goal even without a turn budget.
    assert!(harness.session.goal().expect("goal").turns_used >= 1);
}

#[tokio::test]
async fn an_abort_does_not_drive_an_active_goal_or_open_todos() {
    let harness = create_harness(HarnessOptions {
        tokens_per_second: Some(5.0),
        ..HarnessOptions::default()
    });
    harness.set_responses(vec![
        reply("a long answer that remains in flight until it is cancelled"),
        reply("must not run"),
    ]);
    set_goal(&harness, ThreadGoal::new("ship", None, None));
    harness
        .session
        .todo_store()
        .lock()
        .expect("todo store")
        .replace(vec![Todo {
            content: "finish the change".to_string(),
            active_form: "finishing the change".to_string(),
            status: TodoStatus::InProgress,
        }])
        .expect("valid todo");

    let session = std::sync::Arc::clone(&harness.session);
    let running = tokio::spawn(async move {
        session
            .prompt("go", PromptOptions::default())
            .await
            .expect("prompt");
    });
    harness.wait_until_streaming().await;

    harness.session.abort().await;
    running.await.expect("prompt task");

    let internal_continuations = harness
        .session
        .messages()
        .into_iter()
        .filter(|message| {
            matches!(
                message,
                AgentMessage::Custom(custom)
                    if custom.custom_type == GOAL_REMINDER_TYPE
                        || custom.custom_type == TODO_REMINDER_TYPE
            )
        })
        .count();
    assert_eq!(internal_continuations, 0);
    assert_eq!(harness.pending_response_count(), 1);
    assert_eq!(harness.session.goal().expect("goal").turns_used, 0);
}

/// The stop that holds when the user gave no budget at all.
/// A model that keeps answering is continued until the cap and then control
/// returns to the user — without this, an unbudgeted goal would never end.
#[tokio::test]
async fn an_unbudgeted_goal_stops_at_the_per_turn_cap() {
    let harness = create_harness(HarnessOptions::default());
    // The faux provider replays its last response, so this agent never stops
    // on its own — exactly the case the cap exists for.
    harness.set_responses(vec![reply("still working")]);
    set_goal(&harness, ThreadGoal::new("never finishes", None, None));

    harness
        .session
        .prompt("go", Default::default())
        .await
        .expect("prompt");

    assert_eq!(reminders(&harness).len(), MAX_CONTINUATIONS_PER_TURN);
    assert!(
        reminders(&harness)
            .iter()
            .all(|kind| kind == GOAL_REMINDER_CONTINUATION)
    );
    // Still active: the cap hands control back without ending the goal, so the
    // user can decide whether to keep going.
    assert_eq!(goal_status(&harness), Some(ThreadGoalStatus::Active));
}

#[tokio::test]
async fn a_paused_or_blocked_goal_is_never_continued() {
    for stop in [ThreadGoalStatus::Paused, ThreadGoalStatus::Blocked] {
        let harness = create_harness(HarnessOptions::default());
        harness.set_responses(vec![reply("stopped")]);
        let mut goal = ThreadGoal::new("ship", None, None);
        goal.status = stop;
        set_goal(&harness, goal);

        harness
            .session
            .prompt("go", Default::default())
            .await
            .expect("prompt");

        assert!(reminders(&harness).is_empty(), "{stop:?}");
    }
}

#[tokio::test]
async fn a_turn_budget_stops_the_goal_and_asks_for_a_wrap_up() {
    let harness = create_harness(HarnessOptions::default());
    harness.set_responses(vec![reply("the only turn")]);
    set_goal(&harness, ThreadGoal::new("ship", None, Some(1)));

    harness
        .session
        .prompt("go", Default::default())
        .await
        .expect("prompt");

    // The turn used up the budget, so the goal wraps up rather than continuing.
    assert_eq!(goal_status(&harness), Some(ThreadGoalStatus::BudgetLimited));
    assert_eq!(reminders(&harness), vec![GOAL_REMINDER_WRAP_UP]);
}

#[tokio::test]
async fn the_wrap_up_is_asked_for_only_once() {
    let harness = create_harness(HarnessOptions::default());
    harness.set_responses(vec![reply("first"), reply("second")]);
    set_goal(&harness, ThreadGoal::new("ship", None, Some(1)));

    harness
        .session
        .prompt("go", Default::default())
        .await
        .expect("prompt");
    let after_first = reminders(&harness);

    // A second turn inside the same user turn would be a second wrap-up.
    harness
        .session
        .prompt("carry on", Default::default())
        .await
        .expect("prompt");

    assert_eq!(after_first, vec![GOAL_REMINDER_WRAP_UP]);
    // The real user message reset the region, so one more is allowed — what
    // must never happen is two inside one user turn.
    assert_eq!(
        reminders(&harness),
        vec![GOAL_REMINDER_WRAP_UP, GOAL_REMINDER_WRAP_UP]
    );
}

#[tokio::test]
async fn the_continuation_carries_the_objective_and_the_way_out() {
    let harness = create_harness(HarnessOptions::default());
    harness.set_responses(vec![reply("a first step")]);
    set_goal(&harness, ThreadGoal::new("ship the release", None, None));

    harness
        .session
        .prompt("go", Default::default())
        .await
        .expect("prompt");

    let text = harness
        .session
        .messages()
        .into_iter()
        .find_map(|message| match message {
            AgentMessage::Custom(custom) if custom.custom_type == GOAL_REMINDER_TYPE => {
                Some(format!("{:?}", custom.content))
            }
            _ => None,
        })
        .expect("a continuation was handed over");

    assert!(text.contains("ship the release"), "{text}");
    assert!(text.contains("update_goal"), "{text}");
    // The sentence that makes the tool call the only way out.
    assert!(text.contains("only the tool call does"), "{text}");
}
