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
fn raw_json_renders_like_a_pretty_printed_json_code_block() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);
    let raw = r#"{"reviewerId":"A","assessments":[{"outcome":"fail","evidence":"VPN broken"}]}"#;
    let pretty = "```json\n{\n  \"reviewerId\": \"A\",\n  \"assessments\": [\n    {\n      \"outcome\": \"fail\",\n      \"evidence\": \"VPN broken\"\n    }\n  ]\n}\n```";
    for streaming in [true, false] {
        let render = |source: &str| {
            let message = create_assistant_message(vec![text(source)], StopReason::Stop);
            let mut component =
                AssistantMessageComponent::new(None, false, None, None, None, Vec::new());
            component.update_content(message, Some(streaming));
            component.render(60)
        };
        assert_eq!(
            render(raw),
            render(pretty),
            "raw JSON must get the same indentation and syntax colors as fenced JSON (streaming={streaming})"
        );
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
    assert!(
        lines
            .iter()
            .any(|line| line.contains(&theme().fg(ThemeColor::Text, "⠿"))),
        "the answer marker must use the text color"
    );
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

#[test]
fn plain_stream_paragraphs_enter_history_once_while_the_tail_stays_mutable() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);
    let mut component =
        AssistantMessageComponent::new(None, false, None, None, Some(1), Vec::new());
    component.set_regular_streaming(true);
    component.update_content(
        create_assistant_message(vec![text("First paragraph.\n\nTail")], StopReason::Stop),
        Some(true),
    );
    let first = strip_ansi(&component.take_stream_history(60).join("\n"));
    assert!(first.contains("First paragraph."), "{first}");
    assert!(component.take_stream_history(60).is_empty());
    let preview = strip_ansi(&component.render(60).join("\n"));
    assert!(
        preview.contains("Tail") && !preview.contains("First paragraph."),
        "{preview}"
    );

    component.update_content(
        create_assistant_message(
            vec![text("First paragraph.\n\nSecond paragraph.\n\nTail")],
            StopReason::Stop,
        ),
        Some(true),
    );
    let second = strip_ansi(&component.take_stream_history(60).join("\n"));
    assert!(second.contains("Second paragraph."), "{second}");
    assert!(!second.contains("First paragraph."), "{second}");
    assert!(component.has_stream_history());
}

#[test]
fn a_stream_delta_ending_inside_a_multibyte_character_does_not_break_the_next_scan() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);
    let mut component =
        AssistantMessageComponent::new(None, false, None, None, Some(1), Vec::new());
    component.set_regular_streaming(true);
    component.update_content(
        create_assistant_message(vec![text("")], StopReason::Stop),
        Some(true),
    );
    assert!(component.append_stream_delta(0, "Größ", false));
    assert!(component.take_stream_history(60).is_empty());
    assert!(component.append_stream_delta(0, "e bleibt.\n\nTail", false));
    let history = strip_ansi(&component.take_stream_history(60).join("\n"));
    assert!(
        history.contains("Größe bleibt."),
        "the finished paragraph must enter history: {history}"
    );
}

#[test]
fn an_answer_held_back_by_markdown_keeps_its_first_words_while_it_streams_and_tables_stay_mutable()
{
    let _guard = theme_lock();
    init_theme(Some("dark"), false);
    let mut component =
        AssistantMessageComponent::new(None, false, None, None, Some(1), Vec::new());
    component.set_regular_streaming(true);
    let mut source = String::from("Opening with `code` holds commits back.\n\n");
    for word in 0..2_000 {
        source.push_str(&format!("word{word} "));
        component.update_content(
            create_assistant_message(vec![text(&source)], StopReason::Stop),
            Some(true),
        );
        assert!(component.take_stream_history(60).is_empty());
    }
    let preview = strip_ansi(&component.render(60).join("\n"));
    assert!(
        preview.contains("Opening with")
            && preview.contains("word0 ")
            && preview.contains("word1999"),
        "a streaming answer must not lose its beginning as it grows: {}",
        &preview[..preview.len().min(300)]
    );

    component.update_content(
        create_assistant_message(
            vec![text("| A | B |\n|---|---|\n| one | two |\n\nTail")],
            StopReason::Stop,
        ),
        Some(true),
    );
    assert!(
        component.take_stream_history(60).is_empty(),
        "a table can reshape earlier rows"
    );
}

#[test]
fn authoritative_stream_rewrite_requests_a_bounded_reflow() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);
    let mut component =
        AssistantMessageComponent::new(None, false, None, None, Some(1), Vec::new());
    component.set_regular_streaming(true);
    component.update_content(
        create_assistant_message(vec![text("Earlier.\n\nTail")], StopReason::Stop),
        Some(true),
    );
    assert!(!component.take_stream_history(60).is_empty());
    component.update_content(
        create_assistant_message(vec![text("Rewritten.\n\nTail")], StopReason::Stop),
        Some(true),
    );
    assert!(component.take_stream_reflow());
    assert!(!component.take_stream_reflow());
}

#[test]
fn regular_stream_holds_structural_markdown_until_final_render() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);
    let mut component =
        AssistantMessageComponent::new(None, false, None, None, Some(1), Vec::new());
    component.set_regular_streaming(true);
    for source in [
        "```rust\nfn main() {}\n\n",
        "$$\nx + y\n\n",
        "```mermaid\ngraph TD\n\n",
        "1. first item\n\n",
    ] {
        component.update_content(
            create_assistant_message(vec![text(source)], StopReason::Stop),
            Some(true),
        );
        assert!(
            component.take_stream_history(60).is_empty(),
            "mutable Markdown must remain in the active tail: {source:?}"
        );
    }
}

#[test]
fn resizing_a_running_stream_replays_its_stable_prefix_once() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);
    let mut component =
        AssistantMessageComponent::new(None, false, None, None, Some(1), Vec::new());
    component.set_regular_streaming(true);
    component.update_content(
        create_assistant_message(
            vec![text("Stable paragraph.\n\nMutable tail")],
            StopReason::Stop,
        ),
        Some(true),
    );
    assert!(!component.take_stream_history(60).is_empty());
    Component::prepare_reflow(&mut component, 20);
    let prefix = strip_ansi(&component.take_stream_history(20).join("\n"));
    let tail = strip_ansi(&component.render(20).join("\n"));
    assert_eq!(prefix.matches("Stable paragraph.").count(), 1);
    assert!(
        !tail.contains("Stable paragraph."),
        "prefix repeated in tail: {tail}"
    );
    assert!(tail.contains("Mutable tail"));
}

#[test]
fn thinking_and_text_blocks_advance_history_without_repeating_the_active_tail() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);
    let mut component =
        AssistantMessageComponent::new(None, false, None, None, Some(1), Vec::new());
    component.set_regular_streaming(true);
    component.update_content(
        create_assistant_message(vec![thinking("Reasoning")], StopReason::Stop),
        Some(true),
    );
    assert!(component.take_stream_history(60).is_empty());

    component.update_content(
        create_assistant_message(
            vec![thinking("Reasoning"), text("First answer.\n\nTail")],
            StopReason::Stop,
        ),
        Some(true),
    );
    let history = strip_ansi(&component.take_stream_history(60).join("\n"));
    assert_eq!(history.matches("Reasoning").count(), 1, "{history}");
    assert_eq!(history.matches("First answer.").count(), 1, "{history}");
    assert!(component.take_stream_history(60).is_empty());
    let tail = strip_ansi(&component.render(60).join("\n"));
    assert!(tail.contains("Tail"), "{tail}");
    assert!(
        !tail.contains("Reasoning") && !tail.contains("First answer."),
        "{tail}"
    );

    component.update_content(
        create_assistant_message(
            vec![
                thinking("Reasoning"),
                text("First answer.\n\nTail"),
                tool_call("call-1", "read"),
            ],
            StopReason::Stop,
        ),
        Some(true),
    );
    let settled = strip_ansi(&component.take_stream_history(60).join("\n"));
    assert_eq!(settled.matches("Tail").count(), 1, "{settled}");
    assert!(component.render(60).is_empty());
}

#[test]
fn streamed_rows_are_the_start_of_the_finished_message() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);
    let without_zones = |line: &str| {
        [OSC133_ZONE_START, OSC133_ZONE_END, OSC133_ZONE_FINAL]
            .iter()
            .fold(line.to_string(), |line, zone| line.replace(zone, ""))
    };
    let mut component =
        AssistantMessageComponent::new(None, false, None, None, Some(1), Vec::new());
    component.set_regular_streaming(true);
    let mut streamed = Vec::new();
    let mut source = String::new();
    for delta in [
        "First paragraph that is long enough to wrap across more than one row here.",
        "\n\nSecond paragraph.",
        "\n\nThird ",
        "paragraph with an ending.",
    ] {
        source.push_str(delta);
        component.update_content(
            create_assistant_message(vec![thinking("Reasoning"), text(&source)], StopReason::Stop),
            Some(true),
        );
        streamed.extend(component.take_stream_history(40));
    }
    assert!(!streamed.is_empty(), "complete paragraphs must stream");

    component.update_content(
        create_assistant_message(vec![thinking("Reasoning"), text(&source)], StopReason::Stop),
        Some(false),
    );
    let finished = component.render(40);
    assert!(
        finished.len() > streamed.len(),
        "the unfinished paragraph must remain to be appended"
    );
    for (index, (streamed_row, finished_row)) in streamed.iter().zip(&finished).enumerate() {
        assert_eq!(
            without_zones(streamed_row),
            without_zones(finished_row),
            "streamed row {index} differs from the finished message, so the end of the stream \
             would need a scrollback rebuild"
        );
    }
}

#[test]
fn a_long_streamed_answer_matches_its_finished_render_row_for_row() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);
    let without_zones = |line: &str| {
        [OSC133_ZONE_START, OSC133_ZONE_END, OSC133_ZONE_FINAL]
            .iter()
            .fold(line.to_string(), |line, zone| line.replace(zone, ""))
    };
    for width in [37, 80] {
        let mut component =
            AssistantMessageComponent::new(None, false, None, None, Some(1), Vec::new());
        component.set_regular_streaming(true);
        let mut source = String::new();
        let mut streamed = Vec::new();
        for index in 0..30 {
            let words = "word ".repeat(3 + (index * 7) % 23);
            source.push_str(&format!("Paragraph {index} {words}ends.\n\n"));
            component.update_content(
                create_assistant_message(
                    vec![thinking("Reasoning"), text(&source)],
                    StopReason::Stop,
                ),
                Some(true),
            );
            streamed.extend(component.take_stream_history(width));
        }
        source.push_str("Unfinished tail");
        component.update_content(
            create_assistant_message(vec![thinking("Reasoning"), text(&source)], StopReason::Stop),
            Some(false),
        );
        let finished = component.render(width);
        assert!(finished.len() > streamed.len());
        for (index, (streamed_row, finished_row)) in streamed.iter().zip(&finished).enumerate() {
            assert_eq!(
                without_zones(streamed_row),
                without_zones(finished_row),
                "width {width}: streamed row {index} differs from the finished message"
            );
        }
    }
}

#[test]
fn a_streamed_thought_before_a_tool_has_no_transient_blank_row() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);
    set_block_style(BlockStyle::Badge);
    let mut component =
        AssistantMessageComponent::new(None, false, None, None, Some(1), Vec::new());
    component.set_regular_streaming(true);
    let message = create_assistant_message(
        vec![thinking("Reasoning"), tool_call("call-1", "read")],
        StopReason::Stop,
    );
    component.update_content(message.clone(), Some(true));
    let history = component.take_stream_history(60);
    assert_eq!(
        history.len(),
        2,
        "a tool call must not add an empty row after the thought: {history:?}"
    );
    assert!(
        !strip_ansi(&history[1]).is_empty(),
        "the final emitted row must be the thought heading"
    );
    component.update_content(message, Some(false));
    assert_eq!(
        component.render(60).len(),
        history.len(),
        "final consolidation must preserve the thought height"
    );
}

#[test]
fn streamed_text_before_a_tool_has_no_transient_blank_row() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);
    set_block_style(BlockStyle::Badge);
    let mut component =
        AssistantMessageComponent::new(None, false, None, None, Some(1), Vec::new());
    component.set_regular_streaming(true);
    let message = create_assistant_message(
        vec![text("Answer"), tool_call("call-1", "read")],
        StopReason::Stop,
    );
    component.update_content(message.clone(), Some(true));
    let history = component.take_stream_history(60);
    component.update_content(message, Some(false));
    assert_eq!(
        history.len(),
        component.render(60).len(),
        "a tool call must not add an empty row after completed text: {history:?}"
    );
}

#[test]
fn a_long_running_thought_streams_with_the_rows_it_will_finish_with() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);
    let long = format!("First thought. {}", "reasoning ".repeat(2_000));
    let mut component =
        AssistantMessageComponent::new(None, false, None, None, Some(1), Vec::new());
    component.set_regular_streaming(true);
    component.update_content(
        create_assistant_message(vec![thinking(&long)], StopReason::Stop),
        Some(true),
    );
    assert!(component.take_stream_history(60).is_empty());
    let streaming = strip_ansi(&component.render(60).join("\n"));
    let mut finished = AssistantMessageComponent::new(None, false, None, None, Some(1), Vec::new());
    finished.update_content(
        create_assistant_message(vec![thinking(&long)], StopReason::Stop),
        Some(false),
    );
    let finished = strip_ansi(&finished.render(60).join("\n"));
    assert_eq!(
        streaming.contains("First thought."),
        finished.contains("First thought."),
        "a streaming thought must show the same beginning it finishes with"
    );
}

#[test]
fn text_deltas_advance_the_stream_without_reloading_its_emitted_prefix() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);
    let mut component =
        AssistantMessageComponent::new(None, false, None, None, Some(1), Vec::new());
    component.set_regular_streaming(true);
    component.update_content(
        create_assistant_message(vec![text("First.\n\nTail")], StopReason::Stop),
        Some(true),
    );
    assert!(component.append_stream_delta(0, "\n\nSecond.\n\nNew tail", false));
    let history = strip_ansi(&component.take_stream_history(60).join("\n"));
    assert_eq!(history.matches("First.").count(), 1, "{history}");
    assert_eq!(history.matches("Second.").count(), 1, "{history}");
    let preview = strip_ansi(&component.render(60).join("\n"));
    assert!(preview.contains("New tail"), "{preview}");
    assert!(!preview.contains("First.") && !preview.contains("Second."));
    assert!(component.take_stream_history(60).is_empty());
}

#[test]
fn a_paragraph_boundary_split_across_deltas_enters_history_once() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);
    let mut component =
        AssistantMessageComponent::new(None, false, None, None, Some(1), Vec::new());
    component.set_regular_streaming(true);
    component.update_content(
        create_assistant_message(vec![text("A long paragraph")], StopReason::Stop),
        Some(true),
    );
    assert!(component.take_stream_history(60).is_empty());
    assert!(component.append_stream_delta(0, "\n", false));
    assert!(component.take_stream_history(60).is_empty());
    assert!(component.append_stream_delta(0, "\nTail", false));
    let history = strip_ansi(&component.take_stream_history(60).join("\n"));
    assert_eq!(history.matches("A long paragraph").count(), 1, "{history}");
    assert!(component.take_stream_history(60).is_empty());
    assert!(strip_ansi(&component.render(60).join("\n")).contains("Tail"));
}

#[test]
fn a_structural_block_holds_later_deltas_until_it_settles() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);
    let mut component =
        AssistantMessageComponent::new(None, false, None, None, Some(1), Vec::new());
    component.set_regular_streaming(true);
    component.update_content(
        create_assistant_message(
            vec![text("| A | B |\n|---|---|\n| one | two |\n\n")],
            StopReason::Stop,
        ),
        Some(true),
    );
    assert!(component.take_stream_history(60).is_empty());
    assert!(component.append_stream_delta(0, "Plain text.\n\n", false));
    assert!(
        component.take_stream_history(60).is_empty(),
        "later prose cannot pass a mutable table"
    );
}

#[test]
fn a_structured_answer_taller_than_the_screen_settles_without_rebuilding_scrollback() {
    use notagent_tui::test_terminal::VirtualTerminal;
    use notagent_tui::transcript_container::TranscriptContainer;
    use notagent_tui::tui::{ComponentRef, component_ref};
    use notagent_tui::tui_main_screen::TuiMainScreen;

    let _guard = theme_lock();
    init_theme(Some("dark"), false);
    let mut source = String::from("## Findings\n\nThe `render` path drops rows.\n\n");
    for item in 0..12 {
        source.push_str(&format!(
            "- item {item} explains one [finding] in a sentence\n"
        ));
    }
    source.push_str("\n```rust\nfn main() {}\n```\n\nClosing words after the list.");
    let words: Vec<&str> = source.split_inclusive(' ').collect();

    let terminal = VirtualTerminal::new(60, 8);
    let mut screen = TuiMainScreen::new(Box::new(terminal.clone()));
    let transcript = Rc::new(RefCell::new(TranscriptContainer::new()));
    transcript.borrow_mut().set_regular(true);
    let answer = Rc::new(RefCell::new(AssistantMessageComponent::new(
        None,
        false,
        None,
        None,
        Some(1),
        Vec::new(),
    )));
    answer.borrow_mut().set_regular_streaming(true);
    let entry = Rc::clone(&answer) as ComponentRef;
    transcript.borrow_mut().add_child(Rc::clone(&entry));
    transcript.borrow_mut().mark_mutable(&entry);
    screen
        .core()
        .add_child(Rc::clone(&transcript) as ComponentRef);
    screen
        .core()
        .add_child(component_ref(notagent_tui::components::text::Text::new(
            "prompt", 0, 0,
        )));

    let mut streamed = String::new();
    for word in &words {
        streamed.push_str(word);
        answer.borrow_mut().update_content(
            create_assistant_message(vec![text(&streamed)], StopReason::Stop),
            Some(true),
        );
        transcript.borrow_mut().mark_changed(&entry);
        screen.render_now(false);
    }
    let history = strip_ansi(&terminal.get_scroll_buffer().join("\n"));
    assert!(
        history.contains("Findings") && history.contains("item 0 "),
        "rows above the screen must be in scrollback while the answer streams: {history}"
    );

    let redraws = screen.full_redraws();
    answer.borrow_mut().update_content(
        create_assistant_message(vec![text(&source)], StopReason::Stop),
        Some(false),
    );
    transcript.borrow_mut().mark_changed(&entry);
    transcript.borrow_mut().mark_stable(&entry);
    screen.render_now(false);
    let history = strip_ansi(&terminal.get_scroll_buffer().join("\n"));
    for row in [
        "Findings",
        "item 0 ",
        "item 11 ",
        "fn main",
        "Closing words",
    ] {
        assert_eq!(history.matches(row).count(), 1, "{row}: {history}");
    }
    assert_eq!(
        screen.full_redraws(),
        redraws,
        "rows that left the screen while streaming must match the finished answer: {history}"
    );
}

#[test]
fn an_answer_after_a_thought_settles_without_rebuilding_scrollback() {
    use notagent_tui::test_terminal::VirtualTerminal;
    use notagent_tui::transcript_container::TranscriptContainer;
    use notagent_tui::tui::{ComponentRef, component_ref};
    use notagent_tui::tui_main_screen::TuiMainScreen;

    let _guard = theme_lock();
    init_theme(Some("dark"), false);
    let mut source = String::from("## Findings\n\nThe `render` path drops rows.\n\n");
    for item in 0..12 {
        source.push_str(&format!(
            "- item {item} explains one [finding] in a sentence\n"
        ));
    }
    source.push_str("\n```rust\nfn main() {}\n```\n\nClosing words after the list.");
    let words: Vec<&str> = source.split_inclusive(' ').collect();

    let terminal = VirtualTerminal::new(60, 8);
    let mut screen = TuiMainScreen::new(Box::new(terminal.clone()));
    let transcript = Rc::new(RefCell::new(TranscriptContainer::new()));
    transcript.borrow_mut().set_regular(true);
    let answer = Rc::new(RefCell::new(AssistantMessageComponent::new(
        None,
        false,
        None,
        None,
        Some(1),
        Vec::new(),
    )));
    answer.borrow_mut().set_regular_streaming(true);
    let entry = Rc::clone(&answer) as ComponentRef;
    transcript.borrow_mut().add_child(Rc::clone(&entry));
    transcript.borrow_mut().mark_mutable(&entry);
    screen
        .core()
        .add_child(Rc::clone(&transcript) as ComponentRef);
    screen
        .core()
        .add_child(component_ref(notagent_tui::components::text::Text::new(
            "prompt", 0, 0,
        )));

    let mut streamed = String::new();
    for word in &words {
        streamed.push_str(word);
        answer.borrow_mut().update_content(
            create_assistant_message(
                vec![thinking("Reasoning about rows"), text(&streamed)],
                StopReason::Stop,
            ),
            Some(true),
        );
        transcript.borrow_mut().mark_changed(&entry);
        screen.render_now(false);
    }
    let history = strip_ansi(&terminal.get_scroll_buffer().join("\n"));
    assert!(
        history.contains("Findings") && history.contains("item 0 "),
        "rows above the screen must be in scrollback while the answer streams: {history}"
    );

    let redraws = screen.full_redraws();
    answer.borrow_mut().update_content(
        create_assistant_message(
            vec![thinking("Reasoning about rows"), text(&source)],
            StopReason::Stop,
        ),
        Some(false),
    );
    transcript.borrow_mut().mark_changed(&entry);
    transcript.borrow_mut().mark_stable(&entry);
    screen.render_now(false);
    let history = strip_ansi(&terminal.get_scroll_buffer().join("\n"));
    for row in [
        "Findings",
        "item 0 ",
        "item 11 ",
        "fn main",
        "Closing words",
    ] {
        assert_eq!(history.matches(row).count(), 1, "{row}: {history}");
    }
    assert_eq!(
        screen.full_redraws(),
        redraws,
        "rows that left the screen while streaming must match the finished answer: {history}"
    );
}

#[test]
fn a_running_thought_taller_than_the_screen_stays_out_of_scrollback_until_it_ends() {
    use notagent_tui::test_terminal::VirtualTerminal;
    use notagent_tui::transcript_container::TranscriptContainer;
    use notagent_tui::tui::{ComponentRef, component_ref};
    use notagent_tui::tui_main_screen::TuiMainScreen;

    let _guard = theme_lock();
    init_theme(Some("dark"), false);
    set_block_style(BlockStyle::Badge);
    let terminal = VirtualTerminal::new(60, 8);
    let mut screen = TuiMainScreen::new(Box::new(terminal.clone()));
    let transcript = Rc::new(RefCell::new(TranscriptContainer::new()));
    transcript.borrow_mut().set_regular(true);
    let answer = Rc::new(RefCell::new(AssistantMessageComponent::new(
        None,
        false,
        None,
        None,
        Some(1),
        Vec::new(),
    )));
    answer.borrow_mut().set_regular_streaming(true);
    answer.borrow_mut().set_expanded(true);
    let entry = Rc::clone(&answer) as ComponentRef;
    transcript.borrow_mut().add_child(Rc::clone(&entry));
    transcript.borrow_mut().mark_mutable(&entry);
    screen
        .core()
        .add_child(Rc::clone(&transcript) as ComponentRef);
    screen
        .core()
        .add_child(component_ref(notagent_tui::components::text::Text::new(
            "prompt", 0, 0,
        )));

    let mut thought = String::new();
    for line in 0..20 {
        thought.push_str(&format!("step {line} of the reasoning\n\n"));
        answer.borrow_mut().update_content(
            create_assistant_message(vec![thinking(&thought)], StopReason::Stop),
            Some(true),
        );
        transcript.borrow_mut().mark_changed(&entry);
        screen.render_now(false);
    }
    let redraws = screen.full_redraws();
    answer.borrow_mut().update_content(
        create_assistant_message(vec![thinking(&thought), text("Answer.")], StopReason::Stop),
        Some(false),
    );
    transcript.borrow_mut().mark_changed(&entry);
    transcript.borrow_mut().mark_stable(&entry);
    screen.render_now(false);
    set_block_style(BlockStyle::Standard);
    let history = strip_ansi(&terminal.get_scroll_buffer().join("\n"));
    assert_eq!(
        history.matches("step 0 of").count(),
        1,
        "the thought must reach scrollback once: {history}"
    );
    assert_eq!(
        screen.full_redraws(),
        redraws,
        "a heading written while its timer ran would differ from the finished one: {history}"
    );
}
