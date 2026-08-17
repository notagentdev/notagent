//! The two goal tools as a session uses them (port addition, v0.1.21).
//!
//! The state machine, the completion guard and the reminder texts are covered
//! by the ported suite in `core/goal`. What runs here is what the tools do with
//! them: the refusals, the completion guard reaching the real task list, and the
//! two tools standing down where there is no session to hold a goal.

use std::sync::{Arc, Mutex};

use notagent::core::goal::{GoalState, ThreadGoal, ThreadGoalStatus};
use notagent::core::todos::{Todo, TodoStatus};
use notagent::core::tools::goal::{
    GoalToolSources, create_goal_tool_definition, create_update_goal_tool_definition,
};
use notagent::core::tools::tool_definition::ToolDefinition;
use notagent_agent::types::AgentToolResult;
use notagent_ai::types::TextOrImageContent;
use serde_json::{Value, json};

fn text_output(result: &AgentToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|block| match block {
            TextOrImageContent::Text(text) => Some(text.text.clone()),
            TextOrImageContent::Image(_) => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A session's goal state and task list, as the tools reach them.
struct Fixture {
    state: Arc<Mutex<GoalState>>,
    todos: Arc<Mutex<Vec<Todo>>>,
}

impl Fixture {
    fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(GoalState::default())),
            todos: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn sources(&self) -> Option<GoalToolSources> {
        let state = Arc::clone(&self.state);
        let todos = Arc::clone(&self.todos);
        Some(GoalToolSources {
            state: Arc::new(move || Some(Arc::clone(&state))),
            todos: Arc::new(move || todos.lock().expect("todos").clone()),
        })
    }

    fn open_todo(&self, content: &str) {
        self.todos.lock().expect("todos").push(Todo {
            content: content.to_owned(),
            active_form: content.to_owned(),
            status: TodoStatus::Pending,
        });
    }

    fn finish_all_todos(&self) {
        for todo in self.todos.lock().expect("todos").iter_mut() {
            todo.status = TodoStatus::Completed;
        }
    }

    fn goal(&self) -> Option<ThreadGoal> {
        self.state.lock().expect("goal state").goal.clone()
    }

    fn set_goal(&self, goal: ThreadGoal) {
        self.state.lock().expect("goal state").goal = Some(goal);
    }
}

async fn create(fixture: &Fixture, input: Value) -> Result<AgentToolResult, String> {
    create_goal_tool_definition(fixture.sources())
        .execute("call", input, None, None, None)
        .await
        .map_err(|error| error.message)
}

async fn update(fixture: &Fixture, input: Value) -> Result<AgentToolResult, String> {
    create_update_goal_tool_definition(fixture.sources())
        .execute("call", input, None, None, None)
        .await
        .map_err(|error| error.message)
}

// ---- create_goal -----------------------------------------------------------

#[tokio::test]
async fn creating_a_goal_starts_it_active_and_reports_its_budgets() {
    let fixture = Fixture::new();
    let result = create(
        &fixture,
        json!({ "objective": "ship the release", "turn_budget": 4, "token_budget": 500 }),
    )
    .await
    .expect("create");

    assert_eq!(
        text_output(&result),
        "Goal set (budget 500 tokens, 4 turns): ship the release"
    );
    let goal = fixture.goal().expect("goal");
    assert_eq!(goal.status, ThreadGoalStatus::Active);
    assert_eq!(goal.turn_budget, Some(4));
}

#[tokio::test]
async fn a_second_goal_is_refused_rather_than_replacing_the_first() {
    let fixture = Fixture::new();
    create(&fixture, json!({ "objective": "ship the release" }))
        .await
        .expect("create");

    let error = create(&fixture, json!({ "objective": "something else" }))
        .await
        .expect_err("a session holds one goal");

    assert!(error.contains("already has a goal"), "{error}");
    assert_eq!(
        fixture.goal().expect("goal").objective,
        "ship the release",
        "the running goal is left alone"
    );
}

#[tokio::test]
async fn an_empty_objective_and_a_zero_budget_are_refused() {
    let fixture = Fixture::new();
    assert!(
        create(&fixture, json!({ "objective": "  " }))
            .await
            .is_err()
    );
    assert!(
        create(&fixture, json!({ "objective": "ship", "turn_budget": 0 }))
            .await
            .is_err()
    );
    assert_eq!(fixture.goal(), None);
}

// ---- update_goal -----------------------------------------------------------

#[tokio::test]
async fn completing_a_goal_with_a_clear_list_reports_the_usage() {
    let fixture = Fixture::new();
    fixture.set_goal(ThreadGoal::new("ship", Some(500), None));

    let result = update(&fixture, json!({ "status": "complete" }))
        .await
        .expect("complete");

    assert_eq!(
        text_output(&result),
        "Goal complete. Final usage: 0 of 500 tokens."
    );
    assert_eq!(
        fixture.goal().expect("goal").status,
        ThreadGoalStatus::Complete
    );
}

#[tokio::test]
async fn blocking_needs_a_reason_and_records_it() {
    let fixture = Fixture::new();
    fixture.set_goal(ThreadGoal::new("deploy", None, None));

    let without = update(&fixture, json!({ "status": "blocked" }))
        .await
        .expect_err("a blocked goal needs a reason");
    assert!(without.contains("needs a reason"), "{without}");

    let result = update(
        &fixture,
        json!({ "status": "blocked", "reason": "needs a production credential" }),
    )
    .await
    .expect("block");

    assert!(
        text_output(&result).contains("/goal resume"),
        "{}",
        text_output(&result)
    );
    let goal = fixture.goal().expect("goal");
    assert_eq!(goal.status, ThreadGoalStatus::Blocked);
    assert_eq!(
        goal.blocked_reason.as_deref(),
        Some("needs a production credential")
    );
}

#[tokio::test]
async fn there_is_nothing_to_update_without_a_goal() {
    let fixture = Fixture::new();
    let error = update(&fixture, json!({ "status": "complete" }))
        .await
        .expect_err("no goal");
    assert!(error.contains("no goal to update"), "{error}");
}

// ---- the completion guard --------------------------------------------------

#[tokio::test]
async fn a_completion_claim_over_an_open_list_is_refused_then_honoured() {
    let fixture = Fixture::new();
    fixture.set_goal(ThreadGoal::new("ship", None, None));
    fixture.open_todo("write the tests");

    let refused = update(&fixture, json!({ "status": "complete" }))
        .await
        .expect_err("the list is still open");
    assert!(refused.contains("- write the tests"), "{refused}");
    assert_eq!(
        fixture.goal().expect("goal").status,
        ThreadGoalStatus::Active,
        "a refused claim leaves the goal running"
    );

    update(&fixture, json!({ "status": "complete" }))
        .await
        .expect("the second claim is honoured");
    assert_eq!(
        fixture.goal().expect("goal").status,
        ThreadGoalStatus::Complete
    );
}

#[tokio::test]
async fn anything_in_between_makes_the_next_claim_a_first_claim_again() {
    let fixture = Fixture::new();
    fixture.set_goal(ThreadGoal::new("ship", None, None));
    fixture.open_todo("write the tests");

    update(&fixture, json!({ "status": "complete" }))
        .await
        .expect_err("first claim refused");
    // A block resets the run of consecutive completion claims.
    update(
        &fixture,
        json!({ "status": "blocked", "reason": "waiting on review" }),
    )
    .await
    .expect("block");
    fixture.state.lock().expect("goal state").goal = Some(ThreadGoal::new("ship", None, None));

    let refused = update(&fixture, json!({ "status": "complete" }))
        .await
        .expect_err("this is a first claim again");
    assert!(refused.contains("- write the tests"), "{refused}");
}

#[tokio::test]
async fn a_strict_goal_never_lets_a_claim_override_the_refusal() {
    let fixture = Fixture::new();
    fixture.set_goal(ThreadGoal::new("ship", None, None).strict(true));
    fixture.open_todo("write the tests");

    for _ in 0..3 {
        update(&fixture, json!({ "status": "complete" }))
            .await
            .expect_err("strict refuses every claim while the list is open");
    }
    assert_eq!(
        fixture.goal().expect("goal").status,
        ThreadGoalStatus::Active
    );
}

#[tokio::test]
async fn a_strict_goal_asks_for_one_quality_pass_once_the_list_is_clear() {
    let fixture = Fixture::new();
    fixture.set_goal(ThreadGoal::new("ship", None, None).strict(true));
    fixture.open_todo("write the tests");
    fixture.finish_all_todos();

    let self_check = update(&fixture, json!({ "status": "complete" }))
        .await
        .expect_err("one quality pass first");
    assert!(self_check.contains("quality pass"), "{self_check}");
    assert_eq!(
        fixture.goal().expect("goal").status,
        ThreadGoalStatus::Active
    );

    update(&fixture, json!({ "status": "complete" }))
        .await
        .expect("the pass is done");
    assert_eq!(
        fixture.goal().expect("goal").status,
        ThreadGoalStatus::Complete
    );
}

// ---- no session ------------------------------------------------------------

#[tokio::test]
async fn both_tools_stand_down_where_there_is_no_session_goal_state() {
    // A subagent's tools are built with no sources, which is what the default
    // stands for here.
    let create_error = create_goal_tool_definition(None)
        .execute("call", json!({ "objective": "ship" }), None, None, None)
        .await
        .expect_err("no session")
        .message;
    let update_error = create_update_goal_tool_definition(None)
        .execute("call", json!({ "status": "complete" }), None, None, None)
        .await
        .expect_err("no session")
        .message;

    assert!(
        create_error.contains("not available here"),
        "{create_error}"
    );
    assert!(
        update_error.contains("not available here"),
        "{update_error}"
    );
}
