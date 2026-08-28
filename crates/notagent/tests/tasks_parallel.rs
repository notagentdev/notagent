use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use notagent::core::tasks::manager::{RegisterTaskOptions, TaskManager, TaskManagerOptions};
use notagent::core::tasks::store::TaskStore;
use notagent::core::tasks::subagent_task::{SubagentRunResult, SubagentTask, SubagentTaskOptions};
use notagent::core::tasks::types::{BackgroundTask, TaskStatus};
use notagent::core::tools::bash::{
    BashTaskManager, BashToolOptions, BashToolSources, create_bash_tool_definition,
};
use notagent::core::tools::tool_definition::ToolDefinition;
use serde_json::json;
use tokio::sync::oneshot;

/// Each command sleeps this long; eight of them serialised would take eight
/// times as long, which is the failure these cases are looking for.
const SLEEP_SECONDS: f64 = 0.5;
const TASK_COUNT: usize = 8;

fn manager() -> (tempfile::TempDir, TaskManager) {
    let directory = tempfile::Builder::new()
        .prefix("tasks-parallel-")
        .tempdir()
        .expect("temp dir");
    let store = Arc::new(TaskStore::new(directory.path()));
    (
        directory,
        TaskManager::new(store, TaskManagerOptions::default()),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn eight_backgrounded_shell_commands_run_at_once() {
    let (_directory, manager) = manager();
    let source_manager = manager.clone();
    let definition = create_bash_tool_definition(
        &std::env::current_dir().unwrap().to_string_lossy(),
        Some(BashToolOptions {
            expose_session_environment: Some(false),
            sources: Some(BashToolSources {
                manager: Arc::new(move || {
                    Some(Arc::new(source_manager.clone()) as Arc<dyn BashTaskManager>)
                }),
                background_allowed: Arc::new(|| true),
                auto_background_on_timeout: None,
            }),
            ..BashToolOptions::default()
        }),
    );

    let started = Instant::now();
    let mut task_ids = Vec::new();
    for index in 0..TASK_COUNT {
        let result = definition
            .execute(
                "call-1",
                json!({
                    "command": format!("sleep {SLEEP_SECONDS}; echo done-{index}"),
                    "run_in_background": true,
                    "description": format!("sleeper {index}"),
                }),
                None,
                None,
                None,
            )
            .await
            .expect("registered");
        let text = match &result.content[0] {
            notagent_ai::types::TextOrImageContent::Text(text) => text.text.clone(),
            _ => panic!("text result"),
        };
        let task_id = text
            .lines()
            .find_map(|line| line.strip_prefix("task_id: "))
            .expect("task id")
            .to_owned();
        task_ids.push(task_id);
    }
    // Registration itself must not wait for any of them.
    assert!(
        started.elapsed() < Duration::from_secs_f64(SLEEP_SECONDS),
        "starting {TASK_COUNT} background commands took {:?}",
        started.elapsed()
    );

    for task_id in &task_ids {
        let info = manager
            .wait(task_id, 30_000, None)
            .await
            .expect("task info");
        assert_eq!(info.status(), TaskStatus::Completed, "{task_id}");
    }
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs_f64(SLEEP_SECONDS * 3.0),
        "{TASK_COUNT} half-second commands took {elapsed:?}; they were not running in parallel"
    );

    for (index, task_id) in task_ids.iter().enumerate() {
        let output = manager.read_output(task_id, None).await;
        assert!(output.contains(&format!("done-{index}")), "{output}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn eight_delegated_subagents_run_at_once() {
    let (_directory, manager) = manager();
    let peak = Arc::new(Mutex::new(0_usize));
    let live = Arc::new(AtomicUsize::new(0));

    let started = Instant::now();
    let mut task_ids = Vec::new();
    for index in 0..TASK_COUNT {
        let (sender, receiver) = oneshot::channel::<SubagentRunResult>();
        let live = Arc::clone(&live);
        let peak = Arc::clone(&peak);
        // Stands in for `runDelegation`: an independent tokio task whose answer
        // the subagent task observes.
        tokio::spawn(async move {
            let running = live.fetch_add(1, Ordering::SeqCst) + 1;
            {
                let mut peak = peak.lock().expect("peak");
                *peak = (*peak).max(running);
            }
            tokio::time::sleep(Duration::from_secs_f64(SLEEP_SECONDS)).await;
            live.fetch_sub(1, Ordering::SeqCst);
            let _ = sender.send(SubagentRunResult {
                text: format!("answer {index}"),
                failed: false,
            });
        });
        let task: Arc<dyn BackgroundTask> = Arc::new(SubagentTask::new(SubagentTaskOptions {
            description: format!("investigation {index}"),
            tokens: None,
            session_id: format!("session-{index}"),
            agent: "read-only".to_owned(),
            alias: format!("Star {index}"),
            run: receiver,
            cancel: Arc::new(|| {}),
        }));
        task_ids.push(
            manager
                .register(task, RegisterTaskOptions::default())
                .expect("registered"),
        );
    }

    for task_id in &task_ids {
        let info = manager
            .wait(task_id, 30_000, None)
            .await
            .expect("task info");
        assert_eq!(info.status(), TaskStatus::Completed, "{task_id}");
    }
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs_f64(SLEEP_SECONDS * 3.0),
        "{TASK_COUNT} subagents took {elapsed:?}; they were not running in parallel"
    );
    assert_eq!(
        *peak.lock().expect("peak"),
        TASK_COUNT,
        "all {TASK_COUNT} runs must be in flight at the same time"
    );
    for (index, task_id) in task_ids.iter().enumerate() {
        assert_eq!(
            manager.read_output(task_id, None).await,
            format!("answer {index}")
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_foreground_command_does_not_block_the_background_ones() {
    let (_directory, manager) = manager();
    let source_manager = manager.clone();
    let definition = create_bash_tool_definition(
        &std::env::current_dir().unwrap().to_string_lossy(),
        Some(BashToolOptions {
            expose_session_environment: Some(false),
            sources: Some(BashToolSources {
                manager: Arc::new(move || {
                    Some(Arc::new(source_manager.clone()) as Arc<dyn BashTaskManager>)
                }),
                background_allowed: Arc::new(|| true),
                auto_background_on_timeout: None,
            }),
            ..BashToolOptions::default()
        }),
    );

    let started = Instant::now();
    definition
        .execute(
            "call-1",
            json!({
                "command": format!("sleep {SLEEP_SECONDS}"),
                "run_in_background": true,
                "description": "the background one",
            }),
            None,
            None,
            None,
        )
        .await
        .expect("registered");
    // The foreground call waits for its own command and for nothing else.
    definition
        .execute(
            "call-2",
            json!({ "command": format!("sleep {SLEEP_SECONDS}") }),
            None,
            None,
            None,
        )
        .await
        .expect("ran");
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs_f64(SLEEP_SECONDS * 2.0),
        "the foreground command waited for the background one ({elapsed:?})"
    );
}

/// The strongest form of the claim: eight subagents started by one `task` call,
/// each waiting half a second on its provider, all answering inside one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_task_call_runs_its_eight_subagents_at_once() {
    use notagent::core::modes::shells::{ApprovalLevel, ShellId, tools_for_shell};
    use notagent::core::modes::{Mode, ModeSkill};
    use notagent::core::tools::task::{TaskToolSources, create_task_tool_definition};
    use notagent::core::tools::tool_definition::ToolDefinition;
    use notagent_agent::agent::{Agent, AgentOptions};
    use notagent_ai::types::{
        AssistantContent, AssistantMessage, AssistantMessageEvent, DoneReason, Modality, Model,
        ModelCost, StopReason, TextContent, Usage,
    };
    use notagent_ai::utils::event_stream::create_assistant_message_event_stream;

    let model = Model {
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
    };
    let live = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(Mutex::new(0_usize));
    let live_sink = Arc::clone(&live);
    let peak_sink = Arc::clone(&peak);
    let parent = Agent::new(AgentOptions {
        model: Some(model.clone()),
        system_prompt: Some("P".to_owned()),
        tools: Some(Vec::new()),
        get_api_key: Some(Arc::new(|_provider| {
            Box::pin(async { Some("test-key".to_owned()) })
        })),
        stream_fn: Some(Arc::new(move |_model, _context, _options| {
            let live = Arc::clone(&live_sink);
            let peak = Arc::clone(&peak_sink);
            Box::pin(async move {
                let running = live.fetch_add(1, Ordering::SeqCst) + 1;
                {
                    let mut peak = peak.lock().expect("peak");
                    *peak = (*peak).max(running);
                }
                tokio::time::sleep(Duration::from_secs_f64(SLEEP_SECONDS)).await;
                live.fetch_sub(1, Ordering::SeqCst);
                let stream = create_assistant_message_event_stream();
                stream.push(AssistantMessageEvent::Done {
                    reason: DoneReason::Stop,
                    message: AssistantMessage {
                        content: vec![AssistantContent::Text(TextContent::new(format!(
                            "an answer long enough to hand back {}",
                            "detail ".repeat(40)
                        )))],
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
                    },
                });
                stream
            })
        })),
        ..AgentOptions::default()
    });

    let (_directory, manager) = manager();
    let source_manager = manager.clone();
    let tool = create_task_tool_definition(Some(TaskToolSources {
        modes: Arc::new(move || {
            vec![Mode {
                id: "plan".to_owned(),
                shell: ShellId::ReadOnly,
                approval: ApprovalLevel::Manual,
                tools: tools_for_shell(ShellId::ReadOnly),
                subagents: None,
                skills: vec![ModeSkill {
                    file_name: "10.md".to_owned(),
                    file_path: "/modes/plan/10.md".into(),
                    body: "Plan mode.".to_owned(),
                }],
                source_dir: "/modes/plan".into(),
            }]
        }),
        skills: None,
        aliases: None,
        parent_shell: Arc::new(|| Some(ShellId::ReadOnly)),
        parent_mode: None,
        parent: Arc::new(move || Some(Arc::clone(&parent))),
        manager: Some(Arc::new(move || Some(source_manager.clone()))),
        background_allowed: Some(Arc::new(|| true)),
        resolve_tool: None,
        tool_options: None,
        transcripts: Default::default(),
        cwd: None,
        subagent_model: None,
    }));

    let tasks: Vec<String> = (0..TASK_COUNT)
        .map(|index| format!("investigate branch {index}"))
        .collect();
    let started = Instant::now();
    let result = tool
        .execute(
            "call-1",
            json!({ "tasks": tasks, "agent": "read-only" }),
            None,
            None,
            None,
        )
        .await
        .expect("delegated");
    let elapsed = started.elapsed();

    assert_eq!(
        *peak.lock().expect("peak"),
        TASK_COUNT,
        "all {TASK_COUNT} children must talk to the provider at the same time"
    );
    assert!(
        elapsed < Duration::from_secs_f64(SLEEP_SECONDS * 3.0),
        "{TASK_COUNT} subagents took {elapsed:?}; they were not running in parallel"
    );
    assert_eq!(
        result
            .details
            .as_ref()
            .and_then(|details| details.get("results"))
            .and_then(|results| results.as_array())
            .map(Vec::len),
        Some(TASK_COUNT)
    );
}
