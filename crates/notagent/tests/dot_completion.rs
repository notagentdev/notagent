use notagent::core::{
    tasks::{
        lifecycle::TaskLifecycleRecord,
        types::{
            ShellTaskInfo, SubagentTaskInfo, TERMINAL_TASK_STATUSES, TaskInfo, TaskInfoBase,
            TaskStatus,
        },
    },
    tools::ALL_TOOL_NAMES,
};
use notagent::modes::interactive::{
    components::{
        bash_execution::BashExecutionComponent,
        branch_summary_message::BranchSummaryMessageComponent,
        compaction_summary_message::CompactionSummaryMessageComponent,
        custom_message::CustomMessageComponent,
        explore_block::ExploreBlockComponent,
        skill_invocation_message::SkillInvocationMessageComponent,
        task_lifecycle::TaskLifecycleComponent,
        tool_execution::{ToolExecutionComponent, ToolExecutionOptions, ToolExecutionResult},
    },
    theme::theme::{BlockStyle, block_style, init_theme, set_block_style},
};
use notagent_ai::types::{TextContent, TextOrImageContent, UserContent};
use notagent_tui::{
    activity::RUNNING_DOT,
    test_terminal::VirtualTerminal,
    tui::{ComponentRef, RenderLoop, component_ref},
    tui_alt_screen::{TuiAltScreen, TuiAltScreenOptions},
    tui_main_screen::TuiMainScreen,
};
use serde_json::json;
use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};

fn result(reason: &str) -> ToolExecutionResult {
    ToolExecutionResult {
        content: vec![TextOrImageContent::Text(TextContent::new(reason))],
        details: None,
        is_error: reason != "success",
    }
}
fn record(kind: usize, status: TaskStatus) -> TaskLifecycleRecord {
    let base = TaskInfoBase {
        task_id: format!("task-{kind}"),
        description: "work".into(),
        status,
        detached: Some(kind != 2),
        started_at: 0,
        ended_at: (status != TaskStatus::Running).then_some(1000),
        stop_reason: None,
        notification_suppressed: None,
        timeout_ms: None,
    };
    let task = if kind == 0 {
        TaskInfo::Shell(ShellTaskInfo {
            base,
            command: "command".into(),
            pid: 1,
            exit_code: (status != TaskStatus::Running)
                .then_some(i32::from(status != TaskStatus::Completed)),
        })
    } else {
        TaskInfo::Subagent(SubagentTaskInfo {
            base,
            tokens: 1,
            session_id: "session".into(),
            agent: "worker".into(),
            alias: "Agent".into(),
        })
    };
    if status == TaskStatus::Running {
        TaskLifecycleRecord::started(task)
    } else {
        TaskLifecycleRecord::ended(task)
    }
}

#[test]
fn every_dot_block_finishes_visibly_after_a_hidden_frame_in_both_renderers() {
    struct RestoreStyle(BlockStyle);
    impl Drop for RestoreStyle {
        fn drop(&mut self) {
            set_block_style(self.0);
        }
    }
    let _restore = RestoreStyle(block_style());
    init_theme(Some("dark"), false);
    set_block_style(BlockStyle::Dot);
    for fullscreen in [false, true] {
        let terminal = VirtualTerminal::new(120, 1200);
        let mut screen: Box<dyn RenderLoop> = if fullscreen {
            let mut screen =
                TuiAltScreen::new(Box::new(terminal.clone()), TuiAltScreenOptions::default());
            screen.start();
            Box::new(screen)
        } else {
            let mut screen = TuiMainScreen::new(Box::new(terminal.clone()));
            screen.start();
            Box::new(screen)
        };
        let core = screen.core().clone();
        let mut components: Vec<ComponentRef> = Vec::new();
        let mut finish: Vec<Box<dyn FnOnce()>> = Vec::new();
        for name in ALL_TOOL_NAMES
            .iter()
            .map(|name| name.as_str())
            .chain(["mcp__test__query"])
        {
            for reason in ["success", "failure", "cancelled", "rejected"] {
                let mut tool = ToolExecutionComponent::new(
                    name,
                    format!("{name}-{reason}"),
                    json!({"command":"ls", "path":"file", "content":"body"}),
                    ToolExecutionOptions::default(),
                    None,
                    Rc::new(|| {}),
                    "/tmp",
                );
                tool.mark_execution_started();
                tool.update_result(result("success"), true);
                let tool = Rc::new(RefCell::new(tool));
                components.push(tool.clone());
                finish.push(Box::new(move || {
                    tool.borrow_mut().update_result(result(reason), false)
                }));
            }
        }
        for expanded in [false, true] {
            for reason in ["success", "failure", "cancelled"] {
                let mut group = ExploreBlockComponent::new();
                group.push_call("read", "settled".into(), &json!({"path":"first"}));
                group.complete_call("settled", false);
                group.push_call("ls", "active".into(), &json!({"path":"second"}));
                group.mark_execution_started("active");
                group.set_expanded(expanded);
                let group = Rc::new(RefCell::new(group));
                components.push(group.clone());
                finish.push(Box::new(move || {
                    let mut group = group.borrow_mut();
                    if reason == "cancelled" {
                        group.fail_running_calls();
                    } else {
                        group.complete_call("active", reason != "success");
                    }
                    group.close();
                }));
            }
        }
        for excluded in [false, true] {
            for (code, cancelled) in [(0, false), (1, false), (0, true)] {
                let mut shell = BashExecutionComponent::new("command", excluded);
                shell.append_output("partial output");
                let shell = Rc::new(RefCell::new(shell));
                components.push(shell.clone());
                finish.push(Box::new(move || {
                    shell
                        .borrow_mut()
                        .set_complete(Some(code), cancelled, None, None)
                }));
            }
        }
        for kind in 0..3 {
            components.push(component_ref(TaskLifecycleComponent::new(record(
                kind,
                TaskStatus::Running,
            ))));
        }
        components.push(component_ref(CustomMessageComponent::new(
            notagent_agent::CustomMessage {
                custom_type: "note".into(),
                content: UserContent::Text("body".into()),
                display: true,
                details: None,
                timestamp: 0,
            },
            None,
            None,
        )));
        components.push(component_ref(SkillInvocationMessageComponent::new(
            notagent::core::agent_session::ParsedSkillBlock {
                name: "sample".into(),
                location: "/skills/sample".into(),
                content: "body".into(),
                user_message: None,
            },
            None,
        )));
        components.push(component_ref(BranchSummaryMessageComponent::new(
            notagent_agent::BranchSummaryMessage {
                summary: "body".into(),
                from_id: "entry".into(),
                timestamp: 0,
            },
            None,
        )));
        components.push(component_ref(CompactionSummaryMessageComponent::new(
            notagent_agent::CompactionSummaryMessage {
                summary: "body".into(),
                tokens_before: 10,
                tokens_after: Some(1),
                timestamp: 0,
                recovery: None,
            },
            None,
        )));
        let mut total = 0;
        let mut running = 0;
        for component in &components {
            for line in component.borrow_mut().render(120) {
                total += line.matches('●').count();
                running += line.matches(RUNNING_DOT).count();
            }
            core.add_child(component.clone());
        }
        assert_eq!(
            running,
            finish.len(),
            "each live block needs one running marker"
        );
        let static_count = total - running;
        core.set_activity_animation(true);
        let deadline = Instant::now() + Duration::from_secs(3);
        // Observe the actual hidden frame instead of assuming when the clock started.
        loop {
            core.request_immediate_render();
            screen.render_pending_frame();
            let visible = terminal.get_viewport().join("\n").matches('●').count();
            if visible == static_count {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "no hidden frame in fullscreen={fullscreen}: {visible} dots, expected {static_count}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(core.activity_deadline().is_some());
        for settle in finish {
            settle();
        }
        // Background outcomes append records; their launch records must stay visible.
        let mut appended = 0;
        for kind in 0..3 {
            for status in TERMINAL_TASK_STATUSES {
                let component = component_ref(TaskLifecycleComponent::new(record(kind, status)));
                components.push(component.clone());
                core.add_child(component);
                appended += 1;
            }
        }
        for component in &components {
            assert!(
                component
                    .borrow_mut()
                    .render(120)
                    .iter()
                    .all(|line| !line.contains(RUNNING_DOT)),
                "a final or static block must never retain animation metadata"
            );
        }
        for _ in 0..3 {
            core.request_immediate_render();
            screen.render_pending_frame();
            assert_eq!(
                terminal.get_viewport().join("\n").matches('●').count(),
                total + appended,
                "every final marker must be visible in fullscreen={fullscreen}"
            );
            assert!(
                core.activity_deadline().is_none(),
                "no blink deadline may survive completion"
            );
        }
        assert!(!terminal.get_writes().contains("notagent:a"));
        core.stop();
    }
}
