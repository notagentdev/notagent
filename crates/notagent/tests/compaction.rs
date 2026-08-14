//! Ported from `packages/coding-agent/test/compaction.test.ts`,
//! `compaction-serialization.test.ts` and `compaction-summary-reasoning.test.ts`.
//!
//! Not ported: `compaction-extensions*.test.ts` and `trigger-compact-extension.test.ts`
//! (extension system), and the two `describe.skipIf(!ANTHROPIC_OAUTH_TOKEN)` cases,
//! which need a real provider key. The `buildSessionContext` cases of
//! `compaction.test.ts` live with the session manager, where they were ported
//! with plan task 6.
//!
//! Deviation (class 1): where the TypeScript mocks `completeSimple`, the cases
//! below hand in a recording stream function — the same seam the session uses in
//! production, so the assertions run through the real request path.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use notagent::core::compaction::{
    CompactionPreparation, CompactionSettings, DEFAULT_COMPACTION_SETTINGS, SummarizationRequest,
    calculate_context_tokens, compact, create_file_ops, estimate_context_tokens, find_cut_point,
    generate_summary, generate_summary_with_usage, get_last_assistant_usage,
    serialize_conversation, should_compact,
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
        ..UserMessage::default()
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

/// Builds entries the way the TypeScript helpers do: sequential ids, each one
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

    fn custom_message(&mut self, content: &str) -> SessionEntry {
        let parent = self.last_id.clone();
        let id = self.next_id();
        self.entry(json!({
            "type": "custom_message",
            "id": id,
            "parentId": parent,
            "timestamp": "2026-08-14T00:00:00.000Z",
            "customType": "test",
            "content": content,
            "display": true,
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
        keep_recent_tokens: 20000,
    };
    assert!(should_compact(95000, 100000, &settings));
    assert!(!should_compact(89000, 100000, &settings));
}

#[test]
fn never_triggers_when_disabled() {
    let settings = CompactionSettings {
        enabled: false,
        reserve_tokens: 10000,
        keep_recent_tokens: 20000,
    };
    assert!(!should_compact(95000, 100000, &settings));
}

// -------------------------------------------------------------- findCutPoint

#[test]
fn cuts_at_a_message_entry() {
    let mut builder = EntryBuilder::default();
    let mut entries = Vec::new();
    for index in 0..10u64 {
        entries.push(builder.message(user_message(&format!("User {index}"))));
        entries.push(builder.message(AgentMessage::Assistant(assistant_message(
            &format!("Assistant {index}"),
            mock_usage(0, 100, (index + 1) * 1000, 0),
        ))));
    }

    let result = find_cut_point(&entries, 0, entries.len(), 2500);

    let entry = &entries[result.first_kept_entry_index];
    assert_eq!(entry.entry_type(), "message");
}

#[test]
fn returns_the_start_index_when_there_is_no_valid_cut_point() {
    let mut builder = EntryBuilder::default();
    let entries = vec![builder.message(assistant("a"))];
    let result = find_cut_point(&entries, 0, entries.len(), 1000);
    assert_eq!(result.first_kept_entry_index, 0);
}

#[test]
fn keeps_everything_that_fits_the_budget() {
    let mut builder = EntryBuilder::default();
    let entries = vec![
        builder.message(user_message("1")),
        builder.message(AgentMessage::Assistant(assistant_message(
            "a",
            mock_usage(0, 50, 500, 0),
        ))),
        builder.message(user_message("2")),
        builder.message(AgentMessage::Assistant(assistant_message(
            "b",
            mock_usage(0, 50, 1000, 0),
        ))),
    ];

    let result = find_cut_point(&entries, 0, entries.len(), 50000);
    assert_eq!(result.first_kept_entry_index, 0);
}

#[test]
fn reports_a_split_turn_when_it_cuts_at_an_assistant_message() {
    let mut builder = EntryBuilder::default();
    let entries = vec![
        builder.message(user_message("Turn 1")),
        builder.message(AgentMessage::Assistant(assistant_message(
            "A1",
            mock_usage(0, 100, 1000, 0),
        ))),
        builder.message(user_message("Turn 2")),
        builder.message(AgentMessage::Assistant(assistant_message(
            "A2-1",
            mock_usage(0, 100, 5000, 0),
        ))),
        builder.message(AgentMessage::Assistant(assistant_message(
            "A2-2",
            mock_usage(0, 100, 8000, 0),
        ))),
        builder.message(AgentMessage::Assistant(assistant_message(
            "A2-3",
            mock_usage(0, 100, 10000, 0),
        ))),
    ];

    let result = find_cut_point(&entries, 0, entries.len(), 3000);

    let cut = &entries[result.first_kept_entry_index];
    let role = cut
        .as_message()
        .and_then(|entry| entry.message.get("role"))
        .and_then(|role| role.as_str())
        .unwrap_or_default();
    if role == "assistant" {
        assert!(result.is_split_turn);
        assert_eq!(result.turn_start_index, Some(2));
    }
}

#[test]
fn budgets_context_visible_custom_message_entries() {
    let mut builder = EntryBuilder::default();
    let entries = vec![
        builder.message(user_message("hi")),
        builder.message(assistant("hello")),
        builder.custom_message(&"x".repeat(4000)),
        builder.message(assistant("ok")),
    ];

    let tiny_budget = find_cut_point(&entries, 0, entries.len(), 1);
    assert_eq!(tiny_budget.first_kept_entry_index, 3);
    assert!(tiny_budget.is_split_turn);
    assert_eq!(tiny_budget.turn_start_index, Some(2));

    let custom_fits = find_cut_point(&entries, 0, entries.len(), 2);
    assert_eq!(custom_fits.first_kept_entry_index, 2);
    assert!(!custom_fits.is_split_turn);
    assert_eq!(custom_fits.turn_start_index, None);
}

// ------------------------------------------- prepareCompaction, second round

#[test]
fn does_not_compact_again_while_the_kept_messages_still_fit() {
    let mut builder = EntryBuilder::default();
    let u1 = builder.message(user_message("user msg 1 (summarized by compaction1)"));
    let a1 = builder.message(assistant("assistant msg 1"));
    let u2 = builder.message(user_message("user msg 2 - kept by compaction1"));
    let a2 = builder.message(assistant("assistant msg 2"));
    let u3 = builder.message(user_message("user msg 3 - kept by compaction1"));
    let a3 = builder.message(AgentMessage::Assistant(assistant_message(
        "assistant msg 3",
        mock_usage(5000, 1000, 0, 0),
    )));
    let u2_id = u2.id().to_string();
    let compaction = builder.compaction("First summary", &u2_id);
    let u4 = builder.message(user_message("user msg 4 (new after compaction1)"));
    let a4 = builder.message(AgentMessage::Assistant(assistant_message(
        "assistant msg 4",
        mock_usage(8000, 2000, 0, 0),
    )));

    let entries = vec![u1, a1, u2, a2, u3, a3, compaction, u4, a4];
    assert!(
        notagent::core::compaction::prepare_compaction(&entries, &DEFAULT_COMPACTION_SETTINGS)
            .is_none()
    );
}

#[test]
fn re_summarizes_previously_kept_messages_once_the_window_moves_past_them() {
    let mut builder = EntryBuilder::default();
    let u1 = builder.message(user_message(
        &"user msg 1 (summarized by compaction1)".repeat(4),
    ));
    let a1 = builder.message(assistant(&"assistant msg 1".repeat(4)));
    let u2 = builder.message(user_message(
        &"user msg 2 - kept by compaction1 ".repeat(12),
    ));
    let a2 = builder.message(assistant(&"assistant msg 2 ".repeat(12)));
    let u3 = builder.message(user_message(
        &"user msg 3 - kept by compaction1 ".repeat(12),
    ));
    let a3 = builder.message(AgentMessage::Assistant(assistant_message(
        &"assistant msg 3 ".repeat(12),
        mock_usage(5000, 1000, 0, 0),
    )));
    let u2_id = u2.id().to_string();
    let compaction = builder.compaction("First summary", &u2_id);
    let u4 = builder.message(user_message(
        &"user msg 4 (new after compaction1) ".repeat(12),
    ));
    let a4 = builder.message(AgentMessage::Assistant(assistant_message(
        &"assistant msg 4 ".repeat(12),
        mock_usage(8000, 2000, 0, 0),
    )));

    let settings = CompactionSettings {
        keep_recent_tokens: 100,
        ..DEFAULT_COMPACTION_SETTINGS
    };
    let entries = vec![u1, a1, u2, a2, u3, a3, compaction, u4, a4];
    let preparation =
        notagent::core::compaction::prepare_compaction(&entries, &settings).expect("preparation");

    let summarized = extract_text(&preparation.messages_to_summarize);
    assert!(summarized.contains("user msg 2 - kept by compaction1"));
    assert!(summarized.contains("user msg 3 - kept by compaction1"));
    assert!(!summarized.contains("First summary"));
    assert_eq!(
        preparation.previous_summary.as_deref(),
        Some("First summary")
    );
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

#[test]
fn finds_a_cut_point_in_the_large_session() {
    let entries = large_session_entries();
    let result = find_cut_point(
        &entries,
        0,
        entries.len(),
        DEFAULT_COMPACTION_SETTINGS.keep_recent_tokens,
    );

    let entry = &entries[result.first_kept_entry_index];
    assert_eq!(entry.entry_type(), "message");
    let role = entry
        .as_message()
        .and_then(|entry| entry.message.get("role"))
        .and_then(|role| role.as_str())
        .unwrap_or_default();
    assert!(role == "user" || role == "assistant");
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
            ..UserMessage::default()
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

    let (text, usage) = generate_summary_with_usage(
        &summarize_messages(),
        &model_with(true, 8192),
        2000,
        None,
        None,
        &request,
    )
    .await
    .expect("summary");

    assert_eq!(text, "## Goal\nTest summary");
    assert_eq!(usage, mock_usage(10, 10, 0, 0));

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
        None,
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
            None,
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
        None,
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
        None,
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
        turn_prefix_messages: summarize_messages(),
        is_split_turn: true,
        tokens_before: 600000,
        previous_summary: None,
        file_ops: create_file_ops(),
        settings: CompactionSettings {
            enabled: true,
            reserve_tokens: 500000,
            keep_recent_tokens: 20000,
        },
    };

    let result = compact(&preparation, &model_with(false, 128000), None, &request)
        .await
        .expect("compaction");

    // Both halves of the split turn were summarized, and their usage combined.
    assert_eq!(result.usage.expect("usage"), mock_usage(20, 20, 0, 0));
    let recorded = recorded.lock().expect("recorded");
    assert_eq!(
        recorded
            .options
            .iter()
            .map(|options| options.base.max_tokens)
            .collect::<Vec<_>>(),
        vec![Some(128000), Some(128000)]
    );
}

/// The counter proves the two calls really are the history summary and the
/// turn-prefix summary, not one call retried.
#[tokio::test]
async fn a_split_turn_produces_both_summaries_in_one_result() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let stream_fn: StreamFn = Arc::new(move |_model, _context, _options| {
        let index = counter.fetch_add(1, Ordering::Relaxed);
        Box::pin(async move {
            let response = assistant_message(
                if index == 0 { "HISTORY" } else { "PREFIX" },
                mock_usage(1, 1, 0, 0),
            );
            let stream = create_assistant_message_event_stream();
            stream.end(Some(response));
            stream
        })
    });

    let preparation = CompactionPreparation {
        first_kept_entry_id: "entry-keep".to_owned(),
        messages_to_summarize: summarize_messages(),
        turn_prefix_messages: summarize_messages(),
        is_split_turn: true,
        tokens_before: 1000,
        previous_summary: None,
        file_ops: create_file_ops(),
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

    assert_eq!(calls.load(Ordering::Relaxed), 2);
    assert_eq!(
        result.summary,
        "HISTORY\n\n---\n\n**Turn Context (split turn):**\n\nPREFIX"
    );
}
