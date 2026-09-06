use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Mutex, MutexGuard, OnceLock};

use notagent::modes::interactive::components::assistant_message::AssistantMessageComponent;
use notagent::modes::interactive::components::markdown_transform::{
    MarkdownMessageType, MarkdownTransformContext, MarkdownTransformer,
};
use notagent::modes::interactive::components::user_message::UserMessageComponent;
use notagent::modes::interactive::theme::theme::{
    BlockStyle, ThemeColor, init_theme, set_block_style, theme,
};
use notagent::utils::ansi::strip_ansi;
use notagent_ai::types::{
    AssistantContent, AssistantMessage, StopReason, TextContent, ThinkingContent, ToolCall, Usage,
    UsageCost,
};
use notagent_tui::tui::Component;

const OSC133_ZONE_START: &str = "\x1b]133;A\x07";
const OSC133_ZONE_END: &str = "\x1b]133;B\x07";
const OSC133_ZONE_FINAL: &str = "\x1b]133;C\x07";

fn theme_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let guard = LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    // tests pin the standard layout, the badge tests set Badge themselves.
    set_block_style(BlockStyle::Standard);
    guard
}

fn text(text: &str) -> AssistantContent {
    AssistantContent::Text(TextContent {
        text: text.to_string(),
        ..TextContent::default()
    })
}

fn thinking(thinking: &str) -> AssistantContent {
    AssistantContent::Thinking(ThinkingContent {
        thinking: thinking.to_string(),
        ..ThinkingContent::default()
    })
}

fn tool_call(id: &str, name: &str) -> AssistantContent {
    AssistantContent::ToolCall(ToolCall {
        id: id.to_string(),
        name: name.to_string(),
        arguments: serde_json::json!({ "path": "file.txt" })
            .as_object()
            .expect("object")
            .clone(),
        ..ToolCall::default()
    })
}

fn create_assistant_message(
    content: Vec<AssistantContent>,
    stop_reason: StopReason,
) -> AssistantMessage {
    AssistantMessage {
        content,
        api: "openai-responses".to_string(),
        provider: "openai".to_string(),
        model: "gpt-4o-mini".to_string(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage {
            input: 0,
            output: 0,
            cache_read: 0,
            cache_write: 0,
            cache_write1h: None,
            reasoning: None,
            total_tokens: Some(0),
            cost: UsageCost {
                input: 0.0,
                output: 0.0,
                cache_read: 0.0,
                cache_write: 0.0,
                total: 0.0,
            },
        },
        stop_reason,
        deferred: None,
        error_message: None,
        timestamp: 0,
        raw_stop_reason: None,
        end_turn: None,
    }
}

#[test]
fn adds_osc_133_zone_markers_to_assistant_messages_without_tool_calls() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);

    let mut component = AssistantMessageComponent::new(
        Some(create_assistant_message(
            vec![text("hello")],
            StopReason::Stop,
        )),
        false,
        None,
        None,
        None,
        Vec::new(),
    );
    let lines = component.render(40);

    assert!(!lines.is_empty());
    assert!(lines[0].contains(OSC133_ZONE_START));
    assert!(
        lines[lines.len() - 1].starts_with(&format!("{OSC133_ZONE_END}{OSC133_ZONE_FINAL}")),
        "{:?}",
        lines[lines.len() - 1]
    );
}

#[test]
fn does_not_add_osc_133_zone_markers_when_the_message_contains_tool_calls() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);

    let mut component = AssistantMessageComponent::new(
        Some(create_assistant_message(
            vec![text("calling tool"), tool_call("tool-1", "read")],
            StopReason::Stop,
        )),
        false,
        None,
        None,
        None,
        Vec::new(),
    );
    let rendered = component.render(60).join("\n");

    assert!(!rendered.contains(OSC133_ZONE_START));
    assert!(!rendered.contains(OSC133_ZONE_END));
    assert!(!rendered.contains(OSC133_ZONE_FINAL));
}

#[test]
fn renders_length_stops_with_neutral_truncation_wording() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);

    let mut component = AssistantMessageComponent::new(
        Some(create_assistant_message(
            vec![thinking("private reasoning")],
            StopReason::Length,
        )),
        true,
        None,
        None,
        None,
        Vec::new(),
    );
    let rendered = component.render(80).join("\n");

    assert!(rendered.contains("Thinking..."));
    assert!(rendered.contains("Response was truncated before completion."));
}

#[test]
fn coalesces_adjacent_thinking_blocks_into_one_hidden_thinking_label() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);

    let mut component = AssistantMessageComponent::new(
        Some(create_assistant_message(
            vec![
                thinking("first thought"),
                thinking(""),
                thinking("second thought"),
                text("answer"),
            ],
            StopReason::Stop,
        )),
        true,
        None,
        None,
        None,
        Vec::new(),
    );
    let rendered = strip_ansi(&component.render(80).join("\n"));

    assert_eq!(rendered.matches("Thinking...").count(), 1, "{rendered}");
    assert!(rendered.contains("answer"));
}

#[test]
fn uses_configured_output_padding_for_text_and_thinking() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);

    let mut component = AssistantMessageComponent::new(
        Some(create_assistant_message(
            vec![text("hello"), thinking("reasoning")],
            StopReason::Stop,
        )),
        false,
        None,
        Some("Thinking...".to_string()),
        Some(1),
        Vec::new(),
    );
    let lines: Vec<String> = component.render(80).iter().map(|l| strip_ansi(l)).collect();

    assert!(lines.iter().any(|line| line.contains(" hello")));
    assert!(lines.iter().any(|line| line.contains(" reasoning")));

    component.set_output_pad(0);
    let updated: Vec<String> = component.render(80).iter().map(|l| strip_ansi(l)).collect();
    assert!(updated.iter().any(|line| line.starts_with("⠿ hello")));
    assert!(updated.iter().any(|line| line.starts_with("  reasoning")));
}

#[test]
fn chains_markdown_transformers_in_registration_order() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);
    let calls: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));

    let formula_calls = Rc::clone(&calls);
    let formula: MarkdownTransformer = Rc::new(move |markdown: &str, context| {
        formula_calls.borrow_mut().push("formula".to_string());
        assert_eq!(
            context,
            &MarkdownTransformContext {
                message_type: MarkdownMessageType::Assistant,
                is_streaming: false,
                available_width: 77,
            }
        );
        markdown.replace("$x^2$", "x²")
    });
    let suffix_calls = Rc::clone(&calls);
    let suffix: MarkdownTransformer = Rc::new(move |markdown: &str, _| {
        suffix_calls.borrow_mut().push("suffix".to_string());
        format!("{markdown} Done.")
    });

    let mut component = AssistantMessageComponent::new(
        Some(create_assistant_message(
            vec![text("The result is $x^2$.")],
            StopReason::Stop,
        )),
        false,
        None,
        Some("Thinking...".to_string()),
        Some(1),
        vec![formula, suffix],
    );

    assert!(strip_ansi(&component.render(80).join("\n")).contains("The result is x². Done."));
    assert_eq!(*calls.borrow(), vec!["formula", "suffix"]);
}

#[test]
fn identifies_partial_assistant_markdown_as_streaming() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);
    let streaming_states: Rc<RefCell<Vec<bool>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&streaming_states);
    let transformer: MarkdownTransformer = Rc::new(move |markdown: &str, context| {
        sink.borrow_mut().push(context.is_streaming);
        if context.is_streaming {
            markdown.to_string()
        } else {
            format!("{markdown} transformed")
        }
    });

    let message = create_assistant_message(vec![text("partial")], StopReason::Stop);
    let mut component = AssistantMessageComponent::new(
        None,
        false,
        None,
        Some("Thinking...".to_string()),
        Some(1),
        vec![transformer],
    );

    component.update_content(message.clone(), Some(true));
    assert!(!strip_ansi(&component.render(80).join("\n")).contains("transformed"));

    component.update_content(message, Some(false));
    assert!(strip_ansi(&component.render(80).join("\n")).contains("partial transformed"));
    assert_eq!(*streaming_states.borrow(), vec![true, false]);
}

#[test]
fn reapplies_markdown_transformers_when_available_width_changes() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);
    let widths: Rc<RefCell<Vec<usize>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&widths);
    let transformer: MarkdownTransformer = Rc::new(move |markdown: &str, context| {
        sink.borrow_mut().push(context.available_width);
        format!("{markdown} ({})", context.available_width)
    });

    let mut component = AssistantMessageComponent::new(
        Some(create_assistant_message(
            vec![text("answer")],
            StopReason::Stop,
        )),
        false,
        None,
        Some("Thinking...".to_string()),
        Some(1),
        vec![transformer],
    );

    assert!(strip_ansi(&component.render(80).join("\n")).contains("answer (77)"));
    component.render(80);
    assert!(strip_ansi(&component.render(60).join("\n")).contains("answer (57)"));
    assert_eq!(*widths.borrow(), vec![77, 57]);
}

#[test]
fn transforms_text_and_thinking_markdown_without_mutating_the_original_message() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);
    let transformer: MarkdownTransformer = Rc::new(|markdown: &str, context| {
        let label = match context.message_type {
            MarkdownMessageType::User => "user",
            MarkdownMessageType::Assistant => "assistant",
            MarkdownMessageType::AssistantThinking => "assistant-thinking",
        };
        format!("{label}:{markdown}")
    });

    let message = create_assistant_message(
        vec![text("answer"), thinking("reasoning")],
        StopReason::Stop,
    );
    let mut component = AssistantMessageComponent::new(
        Some(message.clone()),
        false,
        None,
        Some("Thinking...".to_string()),
        Some(1),
        vec![transformer],
    );

    let rendered = strip_ansi(&component.render(80).join("\n"));
    assert!(rendered.contains("assistant:answer"), "{rendered}");
    assert!(
        rendered.contains("assistant-thinking:reasoning"),
        "{rendered}"
    );
    assert_eq!(message.content, vec![text("answer"), thinking("reasoning")]);
}

#[test]
fn uses_configured_output_padding_for_user_messages() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);

    let mut padded = UserMessageComponent::new("hello", None, Some(1), Vec::new());
    let padded_lines: Vec<String> = padded.render(40).iter().map(|l| strip_ansi(l)).collect();
    assert!(padded_lines.iter().any(|line| line.starts_with("❯ hello")));

    let mut unpadded = UserMessageComponent::new("hello", None, Some(0), Vec::new());
    let unpadded_lines: Vec<String> = unpadded.render(40).iter().map(|l| strip_ansi(l)).collect();
    assert!(
        unpadded_lines
            .iter()
            .any(|line| line.starts_with("❯ hello"))
    );
}

#[test]
fn answer_markers_keep_wrapped_text_aligned_without_moving_the_thought_star() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);
    set_block_style(BlockStyle::Badge);
    let mut component = AssistantMessageComponent::new(
        Some(create_assistant_message(
            vec![
                thinking("reasoning"),
                text("alpha beta gamma delta epsilon"),
            ],
            StopReason::Stop,
        )),
        false,
        None,
        None,
        Some(1),
        Vec::new(),
    );
    let lines = component.render(16);
    let plain: Vec<_> = lines.iter().map(|line| strip_ansi(line)).collect();
    assert!(
        plain.iter().any(|line| line.starts_with("* Thought")),
        "the star stays at the margin: {plain:?}"
    );
    assert!(
        plain.iter().any(|line| line.starts_with("⠿ alpha")),
        "the answer starts with its marker: {plain:?}"
    );
    assert!(
        plain.iter().any(|line| line.starts_with("  gamma")),
        "wrapped text aligns after the marker: {plain:?}"
    );
    assert_eq!(
        plain.join("\n").matches('⠿').count(),
        1,
        "one marker per answer"
    );
    component.update_content(
        create_assistant_message(
            vec![text("alpha beta gamma delta epsilon")],
            StopReason::Stop,
        ),
        Some(false),
    );
    for width in 1..=20 {
        for line in component.render(width) {
            assert!(
                notagent_tui::utils::visible_width(&line) <= width,
                "line exceeds width {width}: {line:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Compact thinking rendering in the badge block style
// ---------------------------------------------------------------------------

#[test]
fn badge_style_collapses_thinking_behind_a_muted_heading() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);
    set_block_style(BlockStyle::Badge);

    let mut component = AssistantMessageComponent::new(
        Some(create_assistant_message(
            vec![thinking("secret reasoning"), text("answer")],
            StopReason::Stop,
        )),
        false,
        None,
        None,
        Some(1),
        Vec::new(),
    );
    let lines = component.render(80);
    let muted_heading = format!(
        "{} {}",
        theme().fg(ThemeColor::Muted, "*"),
        theme().fg(ThemeColor::Muted, "Thought")
    );
    assert!(
        lines.iter().any(|line| line.starts_with(&muted_heading)),
        "the heading must start in column zero and use the muted foreground: {lines:#?}"
    );
    let rendered = strip_ansi(&lines.join("\n"));
    assert!(
        rendered.lines().any(|line| line.starts_with("* Thought")),
        "{rendered}"
    );
    assert!(
        rendered
            .lines()
            .any(|line| line.starts_with("* Thought (") && line.contains("to expand)")),
        "the expand hint must share the Thought heading: {rendered}"
    );
    assert!(
        !rendered.contains("secret reasoning"),
        "collapsed thinking stays hidden: {rendered}"
    );
    assert!(rendered.contains("answer"), "{rendered}");
}

#[test]
fn badge_style_expand_toggle_reveals_the_thinking_text() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);
    set_block_style(BlockStyle::Badge);

    let mut component = AssistantMessageComponent::new(
        Some(create_assistant_message(
            vec![thinking("secret reasoning"), text("answer")],
            StopReason::Stop,
        )),
        false,
        None,
        None,
        Some(1),
        Vec::new(),
    );
    component.set_expanded(true);
    let rendered = strip_ansi(&component.render(80).join("\n"));
    assert!(
        rendered
            .lines()
            .any(|line| line.starts_with("  secret reasoning")),
        "thinking text aligns in column two: {rendered}"
    );
    assert!(
        rendered
            .lines()
            .any(|line| line.starts_with("* Thought (") && line.contains("to collapse)")),
        "the collapse hint must share the Thought heading: {rendered}"
    );
}

#[test]
fn badge_style_hiding_thinking_drops_the_block_whole() {
    // The badge style has no static label to fall back to — the badge is the
    // block — so hiding drops it entirely and the answer stands alone.
    let _guard = theme_lock();
    init_theme(Some("dark"), false);
    set_block_style(BlockStyle::Badge);

    let mut component = AssistantMessageComponent::new(
        Some(create_assistant_message(
            vec![thinking("secret reasoning"), text("answer")],
            StopReason::Stop,
        )),
        true,
        None,
        Some("Thinking...".to_string()),
        Some(1),
        Vec::new(),
    );
    let rendered = strip_ansi(&component.render(80).join("\n"));
    assert!(!rendered.contains("* Thought"), "{rendered}");
    assert!(!rendered.contains("Thinking..."), "{rendered}");
    assert!(!rendered.contains("secret reasoning"), "{rendered}");
    assert!(rendered.contains("answer"), "{rendered}");

    // Showing them again brings the heading back.
    component.set_hide_thinking_block(false);
    let shown = strip_ansi(&component.render(80).join("\n"));
    assert!(shown.contains("* Thought"), "{shown}");
}

#[test]
fn badge_style_thinking_timer_runs_while_streaming_and_freezes_on_text() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);
    set_block_style(BlockStyle::Badge);

    let mut component =
        AssistantMessageComponent::new(None, false, None, None, Some(1), Vec::new());
    component.update_content(
        create_assistant_message(vec![thinking("reasoning")], StopReason::Stop),
        Some(true),
    );
    assert!(component.has_running_thinking());
    let rendered = strip_ansi(&component.render(80).join("\n"));
    assert!(rendered.contains("* Thinking"), "{rendered}");
    // No info line while the heading still counts.
    assert!(!rendered.contains("to expand)"), "{rendered}");

    // Visible text after the thinking freezes the timer.
    component.update_content(
        create_assistant_message(
            vec![thinking("reasoning"), text("answer")],
            StopReason::Stop,
        ),
        Some(true),
    );
    assert!(!component.has_running_thinking());
    let rendered = strip_ansi(&component.render(80).join("\n"));
    assert!(rendered.contains("* Thought"), "{rendered}");
    assert!(rendered.contains("to expand)"), "{rendered}");
}

#[test]
fn a_collapsed_thought_uses_one_line_even_in_a_narrow_terminal() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);
    set_block_style(BlockStyle::Badge);
    let mut component = AssistantMessageComponent::new(
        Some(create_assistant_message(
            vec![thinking("reasoning")],
            StopReason::Stop,
        )),
        false,
        None,
        None,
        Some(1),
        Vec::new(),
    );
    for width in 1..=80 {
        let lines = component.render(width);
        let visible: Vec<_> = lines
            .iter()
            .map(|line| strip_ansi(line))
            .filter(|line| !line.trim().is_empty())
            .collect();
        assert_eq!(
            visible.len(),
            1,
            "a collapsed thought must occupy only one visible line at width {width}: {visible:?}"
        );
        assert!(
            lines
                .iter()
                .all(|line| notagent_tui::utils::visible_width(line) <= width),
            "thought heading must fit width {width}: {lines:?}"
        );
    }
}

#[test]
fn badge_style_replayed_thought_carries_no_runtime() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);
    set_block_style(BlockStyle::Badge);

    let mut component = AssistantMessageComponent::new(
        Some(create_assistant_message(
            vec![thinking("reasoning")],
            StopReason::Stop,
        )),
        false,
        None,
        None,
        Some(1),
        Vec::new(),
    );
    let rendered = strip_ansi(&component.render(80).join("\n"));
    assert!(rendered.contains("* Thought"), "{rendered}");
    assert!(!rendered.contains("ms)"), "{rendered}");
    assert!(
        !rendered.contains("s)") || rendered.contains("to expand)"),
        "{rendered}"
    );
}
