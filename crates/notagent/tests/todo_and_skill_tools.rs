//! Port of the tool cases of `packages/coding-agent/test/todos.test.ts` and of
//! `packages/coding-agent/test/skill-tool.test.ts`.
//!
//! The store, the model-facing rendering and the transcript diff are unit-tested
//! in `core::todos`; the reminder cases of `todos.test.ts` belong to
//! `todos/reminder.ts` (plan task 11) and the `renderResult` case to task 13.
//!
//! The skill cases run against constructed sources rather than `loadModes`,
//! which lands with plan task 9; the tool's own behaviour — mode before skill,
//! case-insensitive resolution, envelope, resource listing, token estimate — is
//! what is checked here.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use notagent::core::todos::{Todo, TodoStatus, TodoStore};
use notagent::core::tools::skill::{
    SkillToolMode, SkillToolModeSkill, SkillToolSkill, SkillToolSources,
    create_skill_tool_definition,
};
use notagent::core::tools::todo_write::{TodoWriteToolSources, create_todo_write_tool_definition};
use notagent::core::tools::tool_definition::ToolDefinition;
use notagent_agent::types::AgentToolResult;
use notagent_ai::types::TextOrImageContent;
use serde_json::{Value, json};

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let directory = tempfile::Builder::new()
            .prefix("notagent-test-skill-")
            .tempdir()
            .expect("temp dir");
        let path = directory.path().to_path_buf();
        let _ = directory.keep();
        Self { path }
    }

    fn write(&self, name: &str, content: &str) -> String {
        let path = self.path.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("creates");
        }
        std::fs::write(&path, content).expect("writes");
        path.to_string_lossy().into_owned()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

async fn run(
    tool: &dyn ToolDefinition,
    input: Value,
) -> Result<AgentToolResult, notagent_agent::types::ToolExecutionError> {
    tool.execute("call-1", input, None, None, None).await
}

fn text_output(result: &AgentToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|block| match block {
            TextOrImageContent::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn item(content: &str, status: TodoStatus) -> Todo {
    Todo {
        content: content.to_owned(),
        active_form: format!("Doing {content}"),
        status,
    }
}

fn todo_json(todo: &Todo) -> Value {
    json!({
        "content": todo.content,
        "activeForm": todo.active_form,
        "status": todo.status.as_str(),
    })
}

// ---------------------------------------------------------------------------
// todo_write
// ---------------------------------------------------------------------------

fn todo_harness() -> (
    Arc<Mutex<TodoStore>>,
    notagent::core::tools::todo_write::TodoWriteToolDefinition,
) {
    let store = Arc::new(Mutex::new(TodoStore::new()));
    let source = Arc::clone(&store);
    let tool = create_todo_write_tool_definition(Some(TodoWriteToolSources {
        store: Arc::new(move || Some(Arc::clone(&source))),
    }));
    (store, tool)
}

#[tokio::test]
async fn carries_both_states_in_details() {
    let (_store, tool) = todo_harness();
    run(
        &tool,
        json!({ "todos": [todo_json(&item("Task A", TodoStatus::Pending))] }),
    )
    .await
    .expect("writes");
    let second = run(
        &tool,
        json!({ "todos": [todo_json(&item("Task A", TodoStatus::Completed))] }),
    )
    .await
    .expect("writes");

    let details = second.details.expect("details");
    assert_eq!(
        details["before"],
        json!([todo_json(&item("Task A", TodoStatus::Pending))])
    );
    assert_eq!(
        details["after"],
        json!([todo_json(&item("Task A", TodoStatus::Completed))])
    );
}

#[tokio::test]
async fn still_reports_an_all_completed_list_although_the_store_dropped_it() {
    let (store, tool) = todo_harness();
    run(
        &tool,
        json!({ "todos": [todo_json(&item("Task A", TodoStatus::InProgress))] }),
    )
    .await
    .expect("writes");
    let second = run(
        &tool,
        json!({ "todos": [todo_json(&item("Task A", TodoStatus::Completed))] }),
    )
    .await
    .expect("writes");

    // What the panel and the transcript show …
    assert_eq!(
        second.details.expect("details")["after"],
        json!([todo_json(&item("Task A", TodoStatus::Completed))])
    );
    // … is not what the session keeps.
    assert!(store.lock().expect("store").all().is_empty());
}

#[tokio::test]
async fn tells_the_model_what_changed() {
    let (_store, tool) = todo_harness();
    let result = run(
        &tool,
        json!({ "todos": [todo_json(&item("Task A", TodoStatus::Pending))] }),
    )
    .await
    .expect("writes");
    let text = text_output(&result);
    assert!(text.contains("changes=\"1\""), "{text}");
    assert!(text.contains("change=\"added\""), "{text}");
    assert!(text.contains("Task A"), "{text}");
}

#[tokio::test]
async fn refuses_when_no_list_is_available() {
    let tool = create_todo_write_tool_definition(None);
    let error = run(
        &tool,
        json!({ "todos": [todo_json(&item("Task A", TodoStatus::Pending))] }),
    )
    .await
    .expect_err("no store");
    assert!(error.message.contains("not available"), "{}", error.message);
}

#[tokio::test]
async fn rejects_an_invalid_item_and_leaves_the_list_untouched() {
    let (store, tool) = todo_harness();
    run(
        &tool,
        json!({ "todos": [todo_json(&item("Task A", TodoStatus::Pending))] }),
    )
    .await
    .expect("writes");
    let error = run(
        &tool,
        json!({ "todos": [{ "content": "  ", "activeForm": "Doing", "status": "pending" }] }),
    )
    .await
    .expect_err("invalid");
    assert!(
        error.message.contains("content cannot be empty"),
        "{}",
        error.message
    );
    assert_eq!(
        store.lock().expect("store").all(),
        &[item("Task A", TodoStatus::Pending)]
    );
}

// ---------------------------------------------------------------------------
// skill
// ---------------------------------------------------------------------------

fn sources(modes: Vec<SkillToolMode>, skills: Vec<SkillToolSkill>) -> SkillToolSources {
    SkillToolSources {
        modes: Arc::new(move || modes.clone()),
        skills: Arc::new(move || skills.clone()),
    }
}

fn mode(directory: &TempDir, id: &str, body: &str) -> SkillToolMode {
    let file_path = directory.write(&format!("{id}/10-main.md"), body);
    SkillToolMode {
        id: id.to_owned(),
        source_dir: directory.path.join(id).to_string_lossy().into_owned(),
        skills: vec![SkillToolModeSkill {
            file_path,
            body: body.to_owned(),
        }],
    }
}

#[tokio::test]
async fn loads_a_mode_body_by_name() {
    let directory = TempDir::new();
    let tool = create_skill_tool_definition(Some(sources(
        vec![mode(&directory, "plan", "plan mode guidance")],
        Vec::new(),
    )));

    let text = text_output(&run(&tool, json!({ "name": "plan" })).await.expect("loads"));
    assert!(text.contains("<skill name=\"plan\""), "{text}");
    assert!(text.contains("plan mode guidance"), "{text}");
    assert!(text.ends_with("</skill>"), "{text}");
}

#[tokio::test]
async fn carries_the_source_path_in_the_envelope() {
    let directory = TempDir::new();
    let mode = mode(&directory, "manual", "manual guidance");
    let source_dir = mode.source_dir.clone();
    let tool = create_skill_tool_definition(Some(sources(vec![mode], Vec::new())));

    let text = text_output(
        &run(&tool, json!({ "name": "manual" }))
            .await
            .expect("loads"),
    );
    assert!(text.contains(&format!("path=\"{source_dir}\"")), "{text}");
}

#[tokio::test]
async fn resolves_names_case_insensitively() {
    let directory = TempDir::new();
    let tool = create_skill_tool_definition(Some(sources(
        vec![mode(&directory, "plan", "plan mode guidance")],
        Vec::new(),
    )));

    let result = run(&tool, json!({ "name": "PLAN" })).await.expect("loads");
    assert_eq!(result.details.expect("details")["name"], json!("plan"));
}

#[tokio::test]
async fn reports_token_volume_so_lazy_loads_stay_measurable() {
    let directory = TempDir::new();
    let tool = create_skill_tool_definition(Some(sources(
        vec![mode(&directory, "plan", "plan mode guidance")],
        Vec::new(),
    )));

    let result = run(&tool, json!({ "name": "plan" })).await.expect("loads");
    assert!(
        result.details.expect("details")["tokens"]
            .as_u64()
            .expect("tokens")
            > 0
    );
}

#[tokio::test]
async fn names_the_available_options_when_a_skill_is_unknown() {
    let directory = TempDir::new();
    let tool = create_skill_tool_definition(Some(sources(
        vec![mode(&directory, "plan", "plan mode guidance")],
        vec![SkillToolSkill {
            name: "research".to_owned(),
            file_path: directory.write("research.md", "research"),
        }],
    )));

    let error = run(&tool, json!({ "name": "nope" }))
        .await
        .expect_err("unknown");
    assert!(
        error.message.contains("Unknown skill \"nope\""),
        "{}",
        error.message
    );
    assert!(error.message.contains("plan"), "{}", error.message);
    assert!(error.message.contains("research"), "{}", error.message);
}

#[tokio::test]
async fn reports_that_nothing_is_loaded_when_there_are_no_sources() {
    let tool = create_skill_tool_definition(None);
    let error = run(&tool, json!({ "name": "nope" }))
        .await
        .expect_err("unknown");
    assert!(error.message.contains("(none loaded)"), "{}", error.message);
}

#[tokio::test]
async fn lists_sibling_resources_without_inlining_them() {
    let directory = TempDir::new();
    let mode = mode(&directory, "research", "BODY");
    directory.write("research/reference.csv", "secret,payload");
    let tool = create_skill_tool_definition(Some(sources(vec![mode], Vec::new())));

    let result = run(&tool, json!({ "name": "research" }))
        .await
        .expect("loads");
    let text = text_output(&result);
    assert!(text.contains("<resources>"), "{text}");
    assert!(text.contains("reference.csv"), "{text}");
    assert!(!text.contains("secret,payload"), "{text}");
    assert_eq!(
        result.details.expect("details")["resources"],
        json!(["reference.csv"])
    );
}

#[tokio::test]
async fn omits_the_resources_block_when_there_are_no_siblings() {
    let directory = TempDir::new();
    let tool = create_skill_tool_definition(Some(sources(
        vec![mode(&directory, "solo", "BODY")],
        Vec::new(),
    )));

    let text = text_output(&run(&tool, json!({ "name": "solo" })).await.expect("loads"));
    assert!(!text.contains("<resources>"), "{text}");
}

#[tokio::test]
async fn returns_identical_bodies_for_the_runtime_and_model_paths() {
    let directory = TempDir::new();
    let tool = create_skill_tool_definition(Some(sources(
        vec![mode(&directory, "plan", "plan mode guidance")],
        Vec::new(),
    )));

    let first = text_output(&run(&tool, json!({ "name": "plan" })).await.expect("loads"));
    let second = text_output(&run(&tool, json!({ "name": "plan" })).await.expect("loads"));
    assert_eq!(first, second);
}

#[tokio::test]
async fn resolves_a_mode_before_a_skill_of_the_same_name() {
    let directory = TempDir::new();
    let mode = mode(&directory, "plan", "the mode body");
    let skill_path = directory.write("plan.md", "the skill body");
    let tool = create_skill_tool_definition(Some(sources(
        vec![mode],
        vec![SkillToolSkill {
            name: "plan".to_owned(),
            file_path: skill_path,
        }],
    )));

    let text = text_output(&run(&tool, json!({ "name": "plan" })).await.expect("loads"));
    assert!(text.contains("the mode body"), "{text}");
    assert!(!text.contains("the skill body"), "{text}");
}

#[tokio::test]
async fn reads_a_skill_body_from_its_file() {
    let directory = TempDir::new();
    let file_path = directory.write("skills/writing.md", "---\nname: writing\n---\nthe body\n");
    let tool = create_skill_tool_definition(Some(sources(
        Vec::new(),
        vec![SkillToolSkill {
            name: "writing".to_owned(),
            file_path: file_path.clone(),
        }],
    )));

    let text = text_output(
        &run(&tool, json!({ "name": "writing" }))
            .await
            .expect("loads"),
    );
    assert!(text.contains(&format!("path=\"{file_path}\"")), "{text}");
    assert!(text.contains("the body"), "{text}");
}
