use notagent::modes::interactive::components::{
    bash_execution::BashExecutionComponent,
    explore_block::ExploreBlockComponent,
    tool_execution::{ToolExecutionComponent, ToolExecutionOptions, ToolExecutionResult},
};
use notagent::modes::interactive::theme::theme::{
    BlockStyle, ThemeColor, block_style, init_theme, set_block_style, theme,
};
fn strip_ansi(text: &str) -> String {
    notagent::utils::ansi::strip_ansi(&notagent_tui::activity::static_markers(text))
}
use notagent_ai::types::{TextContent, TextOrImageContent};
use notagent_tui::activity::RUNNING_DOT;
use notagent_tui::tui::Component;
use notagent_tui::utils::visible_width;
use serde_json::json;
use std::rc::Rc;

static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
struct StyleGuard(BlockStyle);
impl Drop for StyleGuard {
    fn drop(&mut self) {
        set_block_style(self.0);
    }
}
fn setup() -> (std::sync::MutexGuard<'static, ()>, StyleGuard) {
    let lock = LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let guard = StyleGuard(block_style());
    init_theme(Some("dark"), false);
    set_block_style(BlockStyle::Dot);
    (lock, guard)
}
fn result(error: bool) -> ToolExecutionResult {
    ToolExecutionResult {
        content: vec![TextOrImageContent::Text(TextContent {
            text: "result stays intact".to_owned(),
            ..Default::default()
        })],
        details: None,
        is_error: error,
    }
}
fn render(component: &mut dyn Component) -> String {
    component
        .render(80)
        .iter()
        .map(|l| l.as_ref())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn tool_dots_distinguish_queue_partial_output_and_final_outcomes() {
    let (_lock, _style) = setup();
    for error in [false, true] {
        let mut tool = ToolExecutionComponent::new(
            "bash",
            "call",
            json!({"command":"echo value"}),
            ToolExecutionOptions::default(),
            None,
            Rc::new(|| {}),
            "/tmp",
        );
        let queued = render(&mut tool);
        assert!(!queued.contains(RUNNING_DOT), "queued tools cannot blink");
        assert!(strip_ansi(&queued).contains("● Bash"), "{queued:?}");
        tool.mark_execution_started();
        assert!(render(&mut tool).contains(RUNNING_DOT));
        tool.update_result(result(false), true);
        assert!(
            render(&mut tool).contains(RUNNING_DOT),
            "partial output is not completion"
        );
        tool.update_result(result(error), false);
        let done = render(&mut tool);
        assert!(!done.contains(RUNNING_DOT));
        assert!(done.contains(&theme().fg(
            if error {
                ThemeColor::Error
            } else {
                ThemeColor::Success
            },
            "●"
        )));
        assert!(strip_ansi(&done).contains("result stays intact"));
    }
}

#[test]
fn compact_and_expanded_exploration_track_real_execution_and_replay() {
    let (_lock, _style) = setup();
    let mut group = ExploreBlockComponent::new();
    group.push_call("read", "read-1".into(), &json!({"path":"src/main.rs"}));
    assert!(!render(&mut group).contains(RUNNING_DOT));
    group.mark_execution_started("read-1");
    assert!(render(&mut group).contains(RUNNING_DOT));
    group.push_call("read", "read-1".into(), &json!({"path":"src/updated.rs"}));
    assert!(
        render(&mut group).contains(RUNNING_DOT),
        "argument updates cannot undo execution start"
    );
    group.set_expanded(true);
    assert_eq!(
        render(&mut group).matches(RUNNING_DOT).count(),
        1,
        "no duplicate group spinner"
    );
    group.close();
    assert!(
        render(&mut group).contains(RUNNING_DOT),
        "closing must not stop an unfinished call"
    );
    group.complete_call("read-1", false);
    group.set_expanded(false);
    let completed = render(&mut group);
    assert!(
        strip_ansi(&completed)
            .lines()
            .any(|line| line.starts_with("● Explored")),
        "completed compact groups must retain a visible status dot: {completed:?}"
    );
    assert!(
        !completed.contains(RUNNING_DOT),
        "completed groups must stop blinking"
    );
    group.complete_call("read-1", true);
    let failed = render(&mut group);
    assert!(
        failed.contains(&theme().fg(ThemeColor::Error, "●")),
        "failed groups must retain a visible error dot: {failed:?}"
    );
    assert!(!failed.contains(RUNNING_DOT));
    let mut replay = ExploreBlockComponent::new();
    replay.push_call("read", "old".into(), &json!({"path":"old.rs"}));
    replay.mark_execution_started("old");
    replay.mark_replayed();
    assert!(!render(&mut replay).contains(RUNNING_DOT));
}

#[test]
fn dot_headers_and_bodies_stay_inside_narrow_terminal_widths() {
    let (_lock, _style) = setup();
    for name in [
        "bash",
        "edit",
        "write",
        "patch_minified",
        "mcp__sample__query",
    ] {
        let mut tool = ToolExecutionComponent::new(
            name,
            "call",
            json!({"command":"a long command with wide 路径 characters", "path":"/tmp/very-long-file.rs", "content":"first\nsecond"}),
            ToolExecutionOptions::default(),
            None,
            Rc::new(|| {}),
            "/tmp",
        );
        tool.mark_execution_started();
        tool.update_result(result(false), true);
        for width in [0, 1, 2, 20, 40, 80] {
            for line in tool.render(width) {
                assert!(
                    visible_width(&line) <= width,
                    "{name} exceeds {width}: {line:?}"
                );
            }
        }
    }
}

#[test]
fn a_style_switch_preserves_shell_output_and_does_not_duplicate_loaders() {
    let (_lock, _style) = setup();
    let mut shell = BashExecutionComponent::new("echo output", true);
    shell.append_output("output before change");
    let dot = render(&mut shell);
    assert!(dot.contains(RUNNING_DOT));
    assert!(strip_ansi(&dot).contains("excluded from context"));
    assert!(!strip_ansi(&dot).contains("Running..."));
    set_block_style(BlockStyle::Badge);
    let badge = render(&mut shell);
    assert!(!badge.contains(RUNNING_DOT));
    assert!(strip_ansi(&badge).contains("output before change"));
    set_block_style(BlockStyle::Dot);
    shell.set_complete(Some(1), false, None, None);
    let failed = render(&mut shell);
    assert!(!failed.contains(RUNNING_DOT));
    assert!(failed.contains(&theme().fg(ThemeColor::Error, "●")));
    for width in [0, 1, 2, 20, 40, 80] {
        for line in shell.render(width) {
            assert!(visible_width(&line) <= width, "{width}: {line:?}");
        }
    }
}

#[test]
fn every_tool_uses_the_left_margin_and_the_same_dot_body_column() {
    let (_lock, _style) = setup();
    let names = notagent::core::tools::ALL_TOOL_NAMES
        .iter()
        .map(|name| name.as_str())
        .chain(["mcp__sample__query"]);
    for name in names {
        let mut tool = ToolExecutionComponent::new(
            name,
            "alignment",
            json!({"command":"ls", "path":"/tmp/sample.rs", "content":"first\nsecond"}),
            ToolExecutionOptions::default(),
            None,
            Rc::new(|| {}),
            "/tmp",
        );
        for stage in 0..3 {
            if stage == 1 {
                tool.mark_execution_started();
                tool.update_result(result(false), true);
            }
            if stage == 2 {
                tool.update_result(result(false), false);
                tool.set_expanded(true);
            }
            for width in [0, 1, 2, 20, 40, 80] {
                let lines = tool.render(width);
                for line in &lines {
                    assert!(
                        visible_width(line) <= width,
                        "{name}, stage {stage}, width {width}: {line:?}"
                    );
                }
                if width < 20 {
                    continue;
                }
                let plain: Vec<_> = lines
                    .iter()
                    .map(|line| strip_ansi(line))
                    .filter(|line| !line.trim().is_empty())
                    .collect();
                if plain.is_empty() {
                    continue;
                }
                assert!(
                    plain[0].starts_with("● "),
                    "{name} must place its dot at column zero: {plain:?}"
                );
                assert!(
                    plain.iter().skip(1).all(|line| line.starts_with("  ")),
                    "{name} must reserve the two-column marker gutter for every body row: {plain:?}"
                );
            }
        }
    }
}

#[test]
fn shell_and_exploration_share_the_dot_margin_and_body_column() {
    let (_lock, _style) = setup();
    let mut shell = BashExecutionComponent::new("ls", false);
    shell.append_output("ENTRY_ONE\nENTRY_TWO");
    let lines: Vec<_> = shell
        .render(80)
        .iter()
        .map(|line| strip_ansi(line))
        .collect();
    assert!(
        lines.iter().any(|line| line.starts_with("● Bash")),
        "{lines:?}"
    );
    for entry in ["ENTRY_ONE", "ENTRY_TWO"] {
        let line = lines.iter().find(|line| line.contains(entry)).unwrap();
        assert_eq!(
            line.find(entry),
            Some(2),
            "shell output must align with the title: {line:?}"
        );
    }
    let mut group = ExploreBlockComponent::new();
    group.push_call("read", "read".into(), &json!({"path":"sample.rs"}));
    group.mark_execution_started("read");
    let compact = strip_ansi(&render(&mut group));
    assert!(
        compact.lines().any(|line| line.starts_with("● Exploring")),
        "{compact}"
    );
    group.set_expanded(true);
    let expanded = strip_ansi(&render(&mut group));
    assert!(
        expanded.lines().any(|line| line.starts_with("● Read")),
        "{expanded}"
    );
}

#[test]
fn expanded_message_boxes_keep_content_under_the_dot_title() {
    use notagent::modes::interactive::components::{
        branch_summary_message::BranchSummaryMessageComponent,
        compaction_summary_message::CompactionSummaryMessageComponent,
        custom_message::CustomMessageComponent,
        skill_invocation_message::SkillInvocationMessageComponent,
    };
    let (_lock, _style) = setup();
    let mut branch = BranchSummaryMessageComponent::new(
        notagent_agent::BranchSummaryMessage {
            summary: "BODY".into(),
            from_id: "entry".into(),
            timestamp: 0,
        },
        None,
    );
    branch.set_expanded(true);
    let mut compaction = CompactionSummaryMessageComponent::new(
        notagent_agent::CompactionSummaryMessage {
            summary: "BODY".into(),
            tokens_before: 100,
            tokens_after: Some(10),
            timestamp: 0,
            recovery: None,
        },
        None,
    );
    compaction.set_expanded(true);
    let custom = CustomMessageComponent::new(
        notagent_agent::CustomMessage {
            custom_type: "note".into(),
            content: notagent_ai::types::UserContent::Text("BODY".into()),
            display: true,
            details: None,
            timestamp: 0,
        },
        None,
        Some(1),
    );
    let mut skill = SkillInvocationMessageComponent::new(
        notagent::core::agent_session::ParsedSkillBlock {
            name: "sample".into(),
            location: "/skills/sample".into(),
            content: "BODY".into(),
            user_message: None,
        },
        None,
    );
    skill.set_expanded(true);
    for mut component in [
        Box::new(branch) as Box<dyn Component>,
        Box::new(compaction),
        Box::new(custom),
        Box::new(skill),
    ] {
        let lines: Vec<_> = component
            .render(80)
            .iter()
            .map(|line| strip_ansi(line))
            .filter(|line| !line.trim().is_empty())
            .collect();
        assert!(
            lines[0].starts_with("● "),
            "message marker must share the tool margin: {lines:?}"
        );
        let body = lines.iter().find(|line| line.contains("BODY")).unwrap();
        assert_eq!(
            body.find("BODY"),
            Some(2),
            "message content must align with its title: {lines:?}"
        );
    }
}

#[test]
fn tool_images_share_the_dot_text_column() {
    use notagent_tui::terminal_image::{ImageProtocol, get_capabilities, set_capabilities};
    let (_lock, _style) = setup();
    let previous = get_capabilities();
    struct Restore(notagent_tui::terminal_image::TerminalCapabilities);
    impl Drop for Restore {
        fn drop(&mut self) {
            set_capabilities(self.0);
        }
    }
    let _restore = Restore(previous);
    for protocol in [ImageProtocol::ITerm2, ImageProtocol::Kitty] {
        set_capabilities(notagent_tui::terminal_image::TerminalCapabilities {
            images: Some(protocol),
            ..previous
        });
        let mut tool = ToolExecutionComponent::new(
            "read",
            "image",
            json!({"path":"sample.png"}),
            ToolExecutionOptions::default(),
            None,
            Rc::new(|| {}),
            "/tmp",
        );
        tool.update_result(ToolExecutionResult {
            content: vec![TextOrImageContent::Image(notagent_ai::types::ImageContent { data: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII=".into(), mime_type: "image/png".into() })],
            details: None, is_error: false,
        }, false);
        let lines = tool.render(80);
        let image = lines
            .iter()
            .find(|line| notagent_tui::terminal_image::is_image_line(line))
            .expect("image protocol output");
        assert!(
            image.starts_with("  "),
            "image output must start in the text column: {image:?}"
        );
        assert!(
            !image.starts_with("   "),
            "image output must not inherit the badge gutter: {image:?}"
        );
    }
}

#[test]
fn dots_and_badges_share_the_adjusted_status_palette() {
    use notagent::modes::interactive::theme::theme::{ThemeBg, badge};
    let (_lock, _style) = setup();
    for (name, success, error) in [
        ("dark", (70, 167, 91), (230, 96, 115)),
        ("light", (40, 110, 51), (154, 39, 57)),
    ] {
        init_theme(Some(name), false);
        for (color, background, expected) in [
            (ThemeColor::Success, ThemeBg::ToolSuccessBg, success),
            (ThemeColor::Error, ThemeBg::ToolErrorBg, error),
        ] {
            assert_eq!(theme().get_fg_rgb(color), Some(expected));
            set_block_style(BlockStyle::Badge);
            let painted = badge(&theme(), background, "tool");
            assert!(
                painted.contains(&format!(
                    "\x1b[48;2;{};{};{}m",
                    expected.0, expected.1, expected.2
                )),
                "badge fill must match the dot color: {painted:?}"
            );
            if name == "dark" {
                assert!(
                    painted.contains("\x1b[38;2;16;16;16m"),
                    "bright badge fills need dark readable text"
                );
            }
        }
    }
}
