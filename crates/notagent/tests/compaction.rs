use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use notagent::core::compaction::{
    CompactionPreparation, CompactionSettings, DEFAULT_COMPACTION_SETTINGS, OperationOutline,
    RetainedSelection, SummarizationRequest, calculate_context_tokens, compact,
    estimate_context_tokens, generate_summary, generate_summary_with_usage,
    get_last_assistant_usage, prepare_compaction, serialize_conversation,
    serialize_conversation_within, should_compact,
};
use notagent::core::session_manager::{
    FileEntry, SessionEntry, migrate_session_entries, parse_session_entries,
};
use notagent_agent::types::{AgentMessage, StreamFn, ThinkingLevel};
use notagent_ai::types::{
    AssistantContent, AssistantMessage, CacheRetention, Message, Modality, Model, ModelCost,
    SimpleStreamOptions, StopReason, TextContent, TextOrImageContent, ToolResultMessage, Usage,
    UsageCost, UserContent, UserMessage,
};
use notagent_ai::utils::event_stream::create_assistant_message_event_stream;
use serde_json::json;

// ------------------------------------------------------------------ fixtures

fn model_with(reasoning: bool, max_tokens: u64) -> Model {
    Model {
        id: if reasoning {
            "reasoning-model".to_owned()
        } else {
            "non-reasoning-model".to_owned()
        },
        name: "mock".to_owned(),
        api: "anthropic-messages".to_owned(),
        provider: "anthropic".to_owned(),
        base_url: "https://example.invalid".to_owned(),
        reasoning,
        thinking_level_map: None,
        input: vec![Modality::Text],
        cost: ModelCost::default(),
        context_window: 200000,
        max_tokens,
        sampling_params: None,
        headers: None,
        compat: None,
    }
}

fn mock_usage(input: u64, output: u64, cache_read: u64, cache_write: u64) -> Usage {
    Usage {
        input,
        output,
        cache_read,
        cache_write,
        cache_write1h: None,
        reasoning: None,
        total_tokens: Some(input + output + cache_read + cache_write),
        cost: UsageCost {
            input: 0.0,
            output: 0.0,
            cache_read: 0.0,
            cache_write: 0.0,
            total: 0.0,
        },
    }
}

fn user_message(text: &str) -> AgentMessage {
    AgentMessage::User(UserMessage {
        content: UserContent::Text(text.to_owned()),
        timestamp: 0,
    })
}

fn assistant_message(text: &str, usage: Usage) -> AssistantMessage {
    AssistantMessage {
        content: vec![AssistantContent::Text(TextContent::new(text))],
        api: "anthropic-messages".to_owned(),
        provider: "anthropic".to_owned(),
        model: "claude-sonnet-4-5".to_owned(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage,
        stop_reason: StopReason::Stop,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: 0,
    }
}

fn assistant(text: &str) -> AgentMessage {
    AgentMessage::Assistant(assistant_message(text, mock_usage(100, 50, 0, 0)))
}

/// parented on the last.
#[derive(Default)]
struct EntryBuilder {
    counter: usize,
    last_id: Option<String>,
}

impl EntryBuilder {
    fn next_id(&mut self) -> String {
        let id = format!("test-id-{}", self.counter);
        self.counter += 1;
        self.last_id = Some(id.clone());
        id
    }

    fn message(&mut self, message: AgentMessage) -> SessionEntry {
        let parent = self.last_id.clone();
        let id = self.next_id();
        self.entry(json!({
            "type": "message",
            "id": id,
            "parentId": parent,
            "timestamp": "2026-08-14T00:00:00.000Z",
            "message": serde_json::to_value(&message).expect("message"),
        }))
    }

    fn compaction(&mut self, summary: &str, first_kept_entry_id: &str) -> SessionEntry {
        let parent = self.last_id.clone();
        let id = self.next_id();
        self.entry(json!({
            "type": "compaction",
            "id": id,
            "parentId": parent,
            "timestamp": "2026-08-14T00:00:00.000Z",
            "summary": summary,
            "firstKeptEntryId": first_kept_entry_id,
            "tokensBefore": 10000,
        }))
    }

    fn compaction_retaining(
        &mut self,
        summary: &str,
        first_kept_entry_id: &str,
        retained: &[&str],
    ) -> SessionEntry {
        let parent = self.last_id.clone();
        let id = self.next_id();
        self.entry(json!({
            "type": "compaction",
            "id": id,
            "parentId": parent,
            "timestamp": "2026-08-14T00:00:00.000Z",
            "summary": summary,
            "firstKeptEntryId": first_kept_entry_id,
            "retained": {
                "head": [],
                "tail": retained.iter().map(|id| json!({ "id": id })).collect::<Vec<_>>(),
                "omittedTokens": 0,
            },
            "tokensBefore": 10000,
        }))
    }

    fn entry(&self, value: serde_json::Value) -> SessionEntry {
        serde_json::from_value(value).expect("session entry")
    }
}

fn extract_text(messages: &[AgentMessage]) -> String {
    messages
        .iter()
        .map(|message| match message {
            AgentMessage::User(message) => match &message.content {
                UserContent::Text(text) => text.clone(),
                UserContent::Blocks(blocks) => blocks
                    .iter()
                    .filter_map(|block| match block {
                        TextOrImageContent::Text(text) => Some(text.text.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join(" "),
            },
            AgentMessage::Assistant(message) => message
                .content
                .iter()
                .filter_map(|block| match block {
                    AssistantContent::Text(text) => Some(text.text.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(" "),
            AgentMessage::BranchSummary(message) => message.summary.clone(),
            AgentMessage::CompactionSummary(message) => message.summary.clone(),
            AgentMessage::Custom(message) => match &message.content {
                UserContent::Text(text) => text.clone(),
                UserContent::Blocks(blocks) => blocks
                    .iter()
                    .filter_map(|block| match block {
                        TextOrImageContent::Text(text) => Some(text.text.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join(" "),
            },
            AgentMessage::BashExecution(message) => {
                format!("{}\n{}", message.command, message.output)
            }
            AgentMessage::ToolResult(_) => String::new(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn large_session_entries() -> Vec<SessionEntry> {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/large-session.jsonl");
    let content = std::fs::read_to_string(path).expect("fixture");
    let mut entries = parse_session_entries(&content);
    // Adds id/parentId for the v1 fixture.
    migrate_session_entries(&mut entries);
    entries
        .into_iter()
        .filter_map(|entry| match entry {
            FileEntry::Entry(entry) => Some(entry),
            FileEntry::Session(_) => None,
        })
        .collect()
}

// ------------------------------------------------------- token calculation

#[test]
fn calculates_total_context_tokens_from_usage() {
    assert_eq!(
        calculate_context_tokens(&mock_usage(1000, 500, 200, 100)),
        1800
    );
    assert_eq!(calculate_context_tokens(&mock_usage(0, 0, 0, 0)), 0);
}

// -------------------------------------------------- getLastAssistantUsage

#[test]
fn finds_the_last_usable_assistant_usage() {
    let mut builder = EntryBuilder::default();
    let entries = vec![
        builder.message(user_message("Hello")),
        builder.message(AgentMessage::Assistant(assistant_message(
            "Hi",
            mock_usage(100, 50, 0, 0),
        ))),
        builder.message(user_message("How are you?")),
        builder.message(AgentMessage::Assistant(assistant_message(
            "Good",
            mock_usage(200, 100, 0, 0),
        ))),
    ];

    assert_eq!(
        get_last_assistant_usage(&entries).expect("usage").input,
        200
    );
}

#[test]
fn skips_aborted_assistant_messages() {
    let mut builder = EntryBuilder::default();
    let mut aborted = assistant_message("Aborted", mock_usage(300, 150, 0, 0));
    aborted.stop_reason = StopReason::Aborted;
    let entries = vec![
        builder.message(user_message("Hello")),
        builder.message(AgentMessage::Assistant(assistant_message(
            "Hi",
            mock_usage(100, 50, 0, 0),
        ))),
        builder.message(user_message("How are you?")),
        builder.message(AgentMessage::Assistant(aborted)),
    ];

    assert_eq!(
        get_last_assistant_usage(&entries).expect("usage").input,
        100
    );
}

#[test]
fn skips_all_zero_assistant_usage() {
    let mut builder = EntryBuilder::default();
    let entries = vec![
        builder.message(user_message("Hello")),
        builder.message(AgentMessage::Assistant(assistant_message(
            "Hi",
            mock_usage(100, 50, 0, 0),
        ))),
        builder.message(user_message("continue")),
        builder.message(AgentMessage::Assistant(assistant_message(
            "Partial",
            mock_usage(0, 0, 0, 0),
        ))),
    ];

    assert_eq!(
        get_last_assistant_usage(&entries).expect("usage").input,
        100
    );
}

#[test]
fn reports_no_usage_when_there_is_no_assistant_message() {
    let mut builder = EntryBuilder::default();
    let entries = vec![builder.message(user_message("Hello"))];
    assert_eq!(get_last_assistant_usage(&entries), None);
}

// ------------------------------------------------------ estimateContextTokens

#[test]
fn anchors_the_estimate_on_the_last_non_zero_assistant_usage() {
    let messages = vec![
        user_message("Hello"),
        AgentMessage::Assistant(assistant_message("Hi", mock_usage(100, 50, 0, 0))),
        user_message("continue"),
        AgentMessage::Assistant(assistant_message(
            "Partial thinking",
            mock_usage(0, 0, 0, 0),
        )),
    ];

    let estimate = estimate_context_tokens(&messages);

    assert_eq!(estimate.usage_tokens, 150);
    assert_eq!(estimate.last_usage_index, Some(1));
    assert!(estimate.trailing_tokens > 0);
    assert_eq!(estimate.tokens, 150 + estimate.trailing_tokens);
}

// ------------------------------------------------------------- shouldCompact

#[test]
fn triggers_once_the_context_eats_into_the_reserve() {
    let settings = CompactionSettings {
        enabled: true,
        reserve_tokens: 10000,
        retained_user_tokens: 20000,
    };
    assert!(should_compact(95000, 100000, &settings));
    assert!(!should_compact(89000, 100000, &settings));
}

#[test]
fn never_triggers_when_disabled() {
    let settings = CompactionSettings {
        enabled: false,
        reserve_tokens: 10000,
        retained_user_tokens: 20000,
    };
    assert!(!should_compact(95000, 100000, &settings));
}

// ------------------------------------------------------------ what is kept

/// The claim the whole rework rests on: a request survives a compaction word
/// for word.
#[test]
fn keeps_every_user_message_verbatim() {
    let mut builder = EntryBuilder::default();
    let mut entries = Vec::new();
    for index in 0..10u64 {
        entries.push(builder.message(user_message(&format!("User {index}"))));
        entries.push(builder.message(AgentMessage::Assistant(assistant_message(
            &format!("Assistant {index}"),
            mock_usage(0, 100, (index + 1) * 1000, 0),
        ))));
    }

    let preparation = prepare_compaction(&entries, &DEFAULT_COMPACTION_SETTINGS).expect("prepared");

    let kept = extract_text(&preparation.retained_messages);
    for index in 0..10u64 {
        assert!(kept.contains(&format!("User {index}")), "{kept}");
    }
    assert!(!kept.contains("Assistant 0"), "{kept}");
    assert_eq!(preparation.retained.omitted_tokens, 0);
    assert!(preparation.retained.head.is_empty());
    assert_eq!(preparation.retained.tail.len(), 10);
}

/// Under budget pressure both ends survive and the gap between them is
/// declared, so the model does not read the two halves as adjacent.
#[test]
fn keeps_both_ends_and_marks_the_gap() {
    let mut builder = EntryBuilder::default();
    let filler = "x".repeat(400); // 100 tokens each
    let mut entries = Vec::new();
    for index in 0..6u64 {
        entries.push(builder.message(user_message(&format!("{index} {filler}"))));
        entries.push(builder.message(assistant("work")));
    }

    let settings = CompactionSettings {
        retained_user_tokens: 300,
        ..DEFAULT_COMPACTION_SETTINGS
    };
    let preparation = prepare_compaction(&entries, &settings).expect("prepared");

    assert!(!preparation.retained.head.is_empty(), "no head kept");
    assert!(!preparation.retained.tail.is_empty(), "no tail kept");
    assert!(preparation.retained.omitted_tokens > 0);

    let kept = extract_text(&preparation.retained_messages);
    assert!(kept.contains("0 xxx"), "the opening was lost: {kept}");
    assert!(kept.contains("5 xxx"), "the latest was lost: {kept}");
    assert_eq!(
        kept.matches("dropped roughly").count(),
        1,
        "expected exactly one elision note: {kept}"
    );
}

/// The note is only there when something really was dropped.
#[test]
fn carries_no_elision_note_when_nothing_was_dropped() {
    let mut builder = EntryBuilder::default();
    let entries = vec![
        builder.message(user_message("short")),
        builder.message(assistant("work")),
    ];
    let preparation = prepare_compaction(&entries, &DEFAULT_COMPACTION_SETTINGS).expect("prepared");
    let kept = extract_text(&preparation.retained_messages);
    assert!(!kept.contains("dropped roughly"), "{kept}");
}

#[test]
fn refuses_a_session_that_is_only_user_messages() {
    let mut builder = EntryBuilder::default();
    let entries = vec![
        builder.message(user_message("one")),
        builder.message(user_message("two")),
    ];
    assert!(prepare_compaction(&entries, &DEFAULT_COMPACTION_SETTINGS).is_none());
}

#[test]
fn refuses_to_compact_twice_in_a_row() {
    let mut builder = EntryBuilder::default();
    let first = builder.message(user_message("one"));
    let first_id = first.id().to_string();
    let entries = vec![
        first,
        builder.message(assistant("work")),
        builder.compaction("summary", &first_id),
    ];
    assert!(prepare_compaction(&entries, &DEFAULT_COMPACTION_SETTINGS).is_none());
}

/// The oldest retained entry is what an older build would keep from, and it
/// keeps a superset — a degraded session rather than a broken one.
#[test]
fn points_the_compatibility_field_at_the_oldest_retained_entry() {
    let mut builder = EntryBuilder::default();
    let first = builder.message(user_message("first"));
    let first_id = first.id().to_string();
    let entries = vec![
        first,
        builder.message(assistant("work")),
        builder.message(user_message("second")),
    ];
    let preparation = prepare_compaction(&entries, &DEFAULT_COMPACTION_SETTINGS).expect("prepared");
    assert_eq!(preparation.first_kept_entry_id, first_id);
}

// ------------------------------------------- prepareCompaction, second round

/// A second compaction sees the retained messages and the first note, and
/// summarizes them together — no separate update path, so nothing compounds
/// behind the model's back.
#[test]
fn a_second_compaction_summarizes_the_retained_messages_and_the_first_note() {
    let mut builder = EntryBuilder::default();
    let u1 = builder.message(user_message("user msg 1"));
    let a1 = builder.message(assistant("assistant msg 1"));
    let u2 = builder.message(user_message("user msg 2"));
    let u1_id = u1.id().to_string();
    let u2_id = u2.id().to_string();
    let compaction = builder.compaction_retaining("First summary", &u1_id, &[&u1_id, &u2_id]);
    let u3 = builder.message(user_message("user msg 3"));
    let a3 = builder.message(assistant("assistant msg 3"));

    let entries = vec![u1, a1, u2, compaction, u3, a3];
    let preparation = prepare_compaction(&entries, &DEFAULT_COMPACTION_SETTINGS).expect("prepared");

    let summarized = extract_text(&preparation.messages_to_summarize);
    assert!(summarized.contains("user msg 1"), "{summarized}");
    assert!(summarized.contains("First summary"), "{summarized}");
    assert!(!summarized.contains("assistant msg 1"), "{summarized}");

    // All three user messages survive the second round as well.
    let kept = extract_text(&preparation.retained_messages);
    for text in ["user msg 1", "user msg 2", "user msg 3"] {
        assert!(kept.contains(text), "{kept}");
    }
}

// -------------------------------------------------------- large session file

#[test]
fn parses_the_large_session_fixture() {
    let entries = large_session_entries();
    assert!(entries.len() > 100);
    assert!(
        entries
            .iter()
            .filter(|entry| entry.entry_type() == "message")
            .count()
            > 100
    );
}

/// The measurement the rework was decided on, pinned: the user's messages are a
/// rounding error next to the transcript, so keeping all of them costs almost
/// nothing.
#[test]
fn keeps_the_whole_of_a_large_sessions_user_input() {
    let entries = large_session_entries();
    let preparation = prepare_compaction(&entries, &DEFAULT_COMPACTION_SETTINGS).expect("prepared");

    assert_eq!(preparation.retained.omitted_tokens, 0);
    let kept: u64 = preparation
        .retained_messages
        .iter()
        .map(notagent::core::compaction::estimate_tokens)
        .sum();
    assert!(
        kept * 4 < preparation.tokens_before,
        "retained {kept} of {} tokens",
        preparation.tokens_before
    );
}

/// A session compacted by an earlier version still opens, and its compactions
/// keep the meaning they were written with: a summary followed by the suffix
/// the field named.
#[test]
fn a_session_compacted_before_the_change_still_builds_a_context() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/before-compaction.jsonl");
    let content = std::fs::read_to_string(path).expect("fixture");
    let mut entries = parse_session_entries(&content);
    migrate_session_entries(&mut entries);
    let entries: Vec<SessionEntry> = entries
        .into_iter()
        .filter_map(|entry| match entry {
            FileEntry::Entry(entry) => Some(entry),
            FileEntry::Session(_) => None,
        })
        .collect();

    let compactions = entries
        .iter()
        .filter(|entry| entry.entry_type() == "compaction")
        .count();
    assert!(compactions > 0, "fixture has no compaction to check");

    let context = notagent::core::session_manager::build_session_context(
        &entries,
        notagent::core::session_manager::LeafSelector::Undefined,
    );
    assert!(!context.messages.is_empty());
    assert!(
        matches!(
            context.messages.first(),
            Some(AgentMessage::CompactionSummary(_))
        ),
        "the old shape puts the summary first"
    );
}

// ------------------------------------------------------- operation outline

#[test]
fn the_outline_keeps_the_last_operation_per_target() {
    let mut outline = OperationOutline::new();
    outline.record("/a.rs", notagent::core::compaction::Operation::Read);
    outline.record("/b.rs", notagent::core::compaction::Operation::Read);
    outline.record("/a.rs", notagent::core::compaction::Operation::Patched);

    let rendered = notagent::core::compaction::format_operation_outline(&outline);
    assert!(rendered.contains("patched /a.rs"), "{rendered}");
    assert!(rendered.contains("read /b.rs"), "{rendered}");
    assert!(!rendered.contains("read /a.rs"), "{rendered}");
    assert_eq!(outline.records().len(), 2);
}

#[test]
fn the_outline_is_empty_when_nothing_happened() {
    assert!(
        notagent::core::compaction::format_operation_outline(&OperationOutline::new()).is_empty()
    );
}

// ------------------------------------------------------ serializeConversation

fn tool_result(text: &str) -> Message {
    Message::ToolResult(ToolResultMessage {
        tool_call_id: "tc1".to_owned(),
        tool_name: "read".to_owned(),
        content: vec![TextOrImageContent::Text(TextContent::new(text))],
        details: None,
        usage: None,
        added_tool_names: None,
        is_error: false,
        timestamp: 0,
    })
}

#[test]
fn truncates_long_tool_results() {
    let result = serialize_conversation(&[tool_result(&"x".repeat(5000))]);

    assert!(result.contains("[Tool result]:"));
    assert!(result.contains("[... 3000 more characters truncated]"));
    assert!(!result.contains(&"x".repeat(3000)));
    assert!(result.contains(&"x".repeat(2000)));
}

#[test]
fn leaves_short_tool_results_alone() {
    let short = "x".repeat(1500);
    let result = serialize_conversation(&[tool_result(&short)]);

    assert_eq!(result, format!("[Tool result]: {short}"));
    assert!(!result.contains("truncated"));
}

#[test]
fn never_truncates_user_or_assistant_messages() {
    let long = "y".repeat(5000);
    let result = serialize_conversation(&[
        Message::User(UserMessage {
            content: UserContent::Blocks(vec![TextOrImageContent::Text(TextContent::new(&long))]),
            timestamp: 0,
        }),
        Message::Assistant(assistant_message(&long, Usage::default())),
    ]);

    assert!(!result.contains("truncated"));
    assert!(result.contains(&long));
}

// ------------------------------------------------ summarization request shape

#[derive(Clone, Default)]
struct Recorded {
    options: Vec<SimpleStreamOptions>,
}

/// A stream function that answers with a fixed summary and records what it was
/// asked for.
fn recording_stream_fn(recorded: Arc<Mutex<Recorded>>) -> StreamFn {
    Arc::new(move |_model, _context, options| {
        if let Some(options) = options.clone() {
            recorded.lock().expect("recorded").options.push(options);
        }
        Box::pin(async move {
            let mut response = assistant_message("## Goal\nTest summary", mock_usage(10, 10, 0, 0));
            response.model = "claude-sonnet-4-5".to_owned();
            let stream = create_assistant_message_event_stream();
            stream.push(notagent_ai::types::AssistantMessageEvent::Done {
                reason: notagent_ai::types::DoneReason::Stop,
                message: response.clone(),
            });
            stream.end(Some(response));
            stream
        })
    })
}

fn request_with(
    stream_fn: StreamFn,
    thinking_level: Option<ThinkingLevel>,
) -> SummarizationRequest {
    SummarizationRequest {
        api_key: Some("test-key".to_owned()),
        thinking_level,
        stream_fn: Some(stream_fn),
        ..SummarizationRequest::default()
    }
}

fn summarize_messages() -> Vec<AgentMessage> {
    vec![user_message("Summarize this.")]
}

#[tokio::test]
async fn passes_the_thinking_level_through_for_reasoning_models() {
    let recorded = Arc::new(Mutex::new(Recorded::default()));
    let request = request_with(
        recording_stream_fn(Arc::clone(&recorded)),
        Some(ThinkingLevel::Medium),
    );

    let summary = generate_summary_with_usage(
        &summarize_messages(),
        &model_with(true, 8192),
        2000,
        0,
        None,
        &request,
    )
    .await
    .expect("summary");

    assert_eq!(summary.text, "## Goal\nTest summary");
    assert_eq!(summary.usage, mock_usage(10, 10, 0, 0));
    assert_eq!(summary.dropped_tokens, 0);

    let recorded = recorded.lock().expect("recorded");
    assert_eq!(recorded.options.len(), 1);
    assert_eq!(
        recorded.options[0].reasoning,
        Some(notagent_ai::types::ThinkingLevel::Medium)
    );
    assert_eq!(
        recorded.options[0].base.base.api_key.as_deref(),
        Some("test-key")
    );
}

#[tokio::test]
async fn returns_the_summary_text_from_generate_summary() {
    let recorded = Arc::new(Mutex::new(Recorded::default()));
    let request = request_with(recording_stream_fn(recorded), None);

    let text = generate_summary(
        &summarize_messages(),
        &model_with(false, 8192),
        2000,
        0,
        None,
        &request,
    )
    .await
    .expect("summary");

    assert_eq!(text, "## Goal\nTest summary");
}

#[tokio::test]
async fn uses_a_fresh_routing_session_and_writes_no_cache_entry() {
    let recorded = Arc::new(Mutex::new(Recorded::default()));
    let request = request_with(recording_stream_fn(Arc::clone(&recorded)), None);

    for _ in 0..2 {
        generate_summary(
            &summarize_messages(),
            &model_with(false, 8192),
            2000,
            0,
            None,
            &request,
        )
        .await
        .expect("summary");
    }

    let recorded = recorded.lock().expect("recorded");
    assert_eq!(recorded.options.len(), 2);
    assert!(
        recorded
            .options
            .iter()
            .all(|options| options.base.cache_retention == Some(CacheRetention::None))
    );
    assert_ne!(
        recorded.options[0].base.session_id,
        recorded.options[1].base.session_id
    );
}

#[tokio::test]
async fn asks_for_no_reasoning_when_thinking_is_off() {
    let recorded = Arc::new(Mutex::new(Recorded::default()));
    let request = request_with(
        recording_stream_fn(Arc::clone(&recorded)),
        Some(ThinkingLevel::Off),
    );

    generate_summary(
        &summarize_messages(),
        &model_with(true, 8192),
        2000,
        0,
        None,
        &request,
    )
    .await
    .expect("summary");

    let recorded = recorded.lock().expect("recorded");
    assert_eq!(recorded.options.len(), 1);
    assert_eq!(recorded.options[0].reasoning, None);
}

#[tokio::test]
async fn asks_for_no_reasoning_from_a_model_that_cannot_reason() {
    let recorded = Arc::new(Mutex::new(Recorded::default()));
    let request = request_with(
        recording_stream_fn(Arc::clone(&recorded)),
        Some(ThinkingLevel::Medium),
    );

    generate_summary(
        &summarize_messages(),
        &model_with(false, 8192),
        2000,
        0,
        None,
        &request,
    )
    .await
    .expect("summary");

    let recorded = recorded.lock().expect("recorded");
    assert_eq!(recorded.options.len(), 1);
    assert_eq!(recorded.options[0].reasoning, None);
}

#[tokio::test]
async fn clamps_the_summary_budget_to_the_models_output_cap() {
    let recorded = Arc::new(Mutex::new(Recorded::default()));
    let request = request_with(recording_stream_fn(Arc::clone(&recorded)), None);

    let preparation = CompactionPreparation {
        first_kept_entry_id: "entry-keep".to_owned(),
        messages_to_summarize: summarize_messages(),
        retained: RetainedSelection::default(),
        retained_messages: Vec::new(),
        tokens_before: 600000,
        outline: OperationOutline::new(),
        settings: CompactionSettings {
            enabled: true,
            reserve_tokens: 500000,
            retained_user_tokens: 20000,
        },
    };

    let result = compact(&preparation, &model_with(false, 128000), None, &request)
        .await
        .expect("compaction");

    assert_eq!(result.usage.expect("usage"), mock_usage(10, 10, 0, 0));
    let recorded = recorded.lock().expect("recorded");
    assert_eq!(
        recorded
            .options
            .iter()
            .map(|options| options.base.max_tokens)
            .collect::<Vec<_>>(),
        vec![Some(128000)]
    );
}

/// One compaction, one model call. The mechanism this replaced made a second
/// call for the prefix of a split turn; the counter is what would catch that
/// coming back.
#[tokio::test]
async fn writes_the_note_in_a_single_call() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let stream_fn: StreamFn = Arc::new(move |_model, _context, _options| {
        counter.fetch_add(1, Ordering::Relaxed);
        Box::pin(async move {
            let response = assistant_message("NOTE", mock_usage(1, 1, 0, 0));
            let stream = create_assistant_message_event_stream();
            stream.end(Some(response));
            stream
        })
    });

    let mut outline = OperationOutline::new();
    outline.record(
        "/src/main.rs",
        notagent::core::compaction::Operation::Patched,
    );

    let preparation = CompactionPreparation {
        first_kept_entry_id: "entry-keep".to_owned(),
        messages_to_summarize: summarize_messages(),
        retained: RetainedSelection::default(),
        retained_messages: summarize_messages(),
        tokens_before: 1000,
        outline,
        settings: DEFAULT_COMPACTION_SETTINGS,
    };

    let result = compact(
        &preparation,
        &model_with(false, 8192),
        None,
        &request_with(stream_fn, None),
    )
    .await
    .expect("compaction");

    assert_eq!(calls.load(Ordering::Relaxed), 1);
    // The note, then the outline the tool calls were read off — not asked for,
    // so not inventable.
    assert_eq!(
        result.summary,
        "NOTE\n\n<operations>\npatched /src/main.rs\n</operations>"
    );
    assert!(result.estimated_tokens_after.is_some());
    assert_eq!(result.dropped_tokens, 0);
}

/// A history too large for the window is cut once, before the call, rather
/// than discovered by a rejection.
#[tokio::test]
async fn cuts_an_oversized_history_once_before_sending() {
    let long = "x".repeat(4000); // 1000 tokens per message
    let messages: Vec<Message> = (0..10)
        .map(|index| {
            Message::User(UserMessage {
                content: UserContent::Text(format!("{index} {long}")),
                timestamp: 0,
            })
        })
        .collect();

    let full = serialize_conversation_within(&messages, 0);
    assert_eq!(full.dropped_messages, 0);
    assert!(full.text.contains("0 xxx"));

    let cut = serialize_conversation_within(&messages, 5_000);
    assert!(cut.dropped_messages >= 5, "{}", cut.dropped_messages);
    assert!(cut.dropped_tokens > 0);
    // The oldest went; the newest stayed.
    assert!(!cut.text.contains("[User]: 0 "), "{}", &cut.text[..80]);
    assert!(cut.text.contains("9 xxx"));
}
