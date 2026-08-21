//! Context compaction for long sessions. Pure functions: the session manager
//! does the I/O and rebuilds its context afterwards.
//!
//! A compaction keeps the user's own messages verbatim and replaces everything
//! else with one note the agent writes to itself. That split follows from what
//! a session is actually made of: measured over a 377k-token session, tool
//! results were 57% of it and the user's messages were 0.7%. Keeping the cheap
//! part is nearly free, and it is the part that cannot be reconstructed — a
//! paraphrase of a request is not a request.
//!
//! The note is written fresh each time from what the agent can still see, which
//! after an earlier compaction is the retained messages plus that earlier note.
//! Nothing folds an old summary into a new one behind the model's back, so a
//! mistake in one generation is corrected by the messages rather than inherited.

use std::sync::Arc;

use notagent_agent::stream_fn::get_default_stream_fn;
use notagent_agent::types::{AgentMessage, StreamFn, ThinkingLevel};
use notagent_ai::types::{
    AssistantContent, AssistantMessage, Context, Message, Model, SimpleStreamOptions, StopReason,
    StreamOptions, TextContent, TextOrImageContent, Usage, UsageCost, UserContent, UserMessage,
};
use notagent_ai::utils::retry::{RetryCallbacks, RetryPolicy, retry_assistant_call};
use notagent_ai::utils::text::content_text;
use notagent_ai::utils::uuid::uuidv7;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::core::compaction::retention::{
    RETAINED_HEAD_TOKENS, RETAINED_USER_TOKENS, RetainedSelection, apply_retention,
    elision_message, is_user_input, select_retained,
};
use crate::core::compaction::utils::{
    Operation, OperationOutline, OperationRecord, SUMMARIZATION_SYSTEM_PROMPT,
    extract_operations_from_message, format_operation_outline, serialize_conversation_within,
};
use crate::core::messages::convert_to_llm;
use crate::core::session_manager::{
    LeafSelector, SessionEntry, build_context_entries, build_session_context,
    session_entry_to_context_messages,
};

// ============================================================================
// Operation tracking
// ============================================================================

/// What a compaction entry stores about the work its history covered.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionDetails {
    #[serde(default)]
    pub operations: Vec<OperationRecord>,
}

/// Builds the outline from the messages plus the previous compaction's record.
///
/// The carry-forward is what makes the outline cumulative. It has to be
/// explicit: after a compaction the assistant turns that performed the earlier
/// work are gone from the context, so extracting from the messages alone would
/// silently shorten the record every time.
fn extract_operations(
    messages: &[AgentMessage],
    previous: Option<&SessionEntry>,
) -> OperationOutline {
    let mut outline = OperationOutline::new();

    if let Some(SessionEntry::Compaction(previous)) = previous
        // `fromHook` is retained for session-file compatibility; an entry an
        // extension wrote carries details this crate cannot interpret.
        && previous.from_hook != Some(true)
        && let Some(details) = previous.details.as_ref()
    {
        if let Some(records) = details.get("operations").and_then(|value| value.as_array()) {
            for record in records {
                if let Ok(record) = serde_json::from_value::<OperationRecord>(record.clone()) {
                    outline.record(record.target, record.operation);
                }
            }
        }
        // Entries written before the outline existed carry two flat path lists.
        for (key, operation) in [
            ("readFiles", Operation::Read),
            ("modifiedFiles", Operation::Patched),
        ] {
            if let Some(paths) = details.get(key).and_then(|value| value.as_array()) {
                for path in paths.iter().filter_map(|path| path.as_str()) {
                    outline.record(path, operation);
                }
            }
        }
    }

    for message in messages {
        extract_operations_from_message(message, &mut outline);
    }

    outline
}

/// What `compact` produces. The session manager stamps uuid and parentUuid when
/// it saves.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CompactionResult {
    pub summary: String,
    pub first_kept_entry_id: String,
    /// The messages carried through verbatim.
    pub retained: RetainedSelection,
    pub tokens_before: u64,
    pub estimated_tokens_after: Option<u64>,
    /// How much history the request budget forced out of the summarization
    /// call. Zero in the ordinary case.
    pub dropped_tokens: u64,
    /// Usage of the LLM call that produced the summary.
    pub usage: Option<Usage>,
    pub details: Option<serde_json::Value>,
}

// ============================================================================
// Settings
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactionSettings {
    pub enabled: bool,
    /// Headroom kept free of context, and the budget the summary is written in.
    pub reserve_tokens: u64,
    /// Budget for the user messages carried through.
    pub retained_user_tokens: u64,
}

pub const DEFAULT_COMPACTION_SETTINGS: CompactionSettings = CompactionSettings {
    enabled: true,
    reserve_tokens: 16384,
    retained_user_tokens: RETAINED_USER_TOKENS,
};

impl From<crate::core::settings_manager::ResolvedCompactionSettings> for CompactionSettings {
    fn from(settings: crate::core::settings_manager::ResolvedCompactionSettings) -> Self {
        Self {
            enabled: settings.enabled,
            reserve_tokens: settings.reserve_tokens,
            retained_user_tokens: settings.retained_user_tokens,
        }
    }
}

// ============================================================================
// Token calculation
// ============================================================================

/// Total context tokens from a usage record, preferring the provider's own
/// figure and falling back to the sum of the parts.
pub fn calculate_context_tokens(usage: &Usage) -> u64 {
    match usage.total_tokens {
        Some(total) if total > 0 => total,
        _ => usage.input + usage.output + usage.cache_read + usage.cache_write,
    }
}

/// The usage of an assistant message, when it has one worth trusting. Aborted
/// and errored turns, and all-zero records, do not.
fn assistant_usage(message: &AgentMessage) -> Option<Usage> {
    let AgentMessage::Assistant(message) = message else {
        return None;
    };
    if message.stop_reason == StopReason::Aborted || message.stop_reason == StopReason::Error {
        return None;
    }
    (calculate_context_tokens(&message.usage) > 0).then_some(message.usage)
}

/// The last trustworthy assistant usage in a list of entries.
pub fn get_last_assistant_usage(entries: &[SessionEntry]) -> Option<Usage> {
    for entry in entries.iter().rev() {
        if matches!(entry, SessionEntry::Message(_))
            && let Some(message) = session_entry_to_context_messages(entry).first()
            && let Some(usage) = assistant_usage(message)
        {
            return Some(usage);
        }
    }
    None
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ContextUsageEstimate {
    pub tokens: u64,
    pub usage_tokens: u64,
    pub trailing_tokens: u64,
    pub last_usage_index: Option<usize>,
}

fn last_assistant_usage_info(messages: &[AgentMessage]) -> Option<(Usage, usize)> {
    for (index, message) in messages.iter().enumerate().rev() {
        if let Some(usage) = assistant_usage(message) {
            return Some((usage, index));
        }
    }
    None
}

/// Estimates the context size, preferring the provider's own count for
/// everything up to the last assistant turn and estimating only what came
/// after it.
pub fn estimate_context_tokens(messages: &[AgentMessage]) -> ContextUsageEstimate {
    let Some((usage, index)) = last_assistant_usage_info(messages) else {
        let estimated: u64 = messages.iter().map(estimate_tokens).sum();
        return ContextUsageEstimate {
            tokens: estimated,
            usage_tokens: 0,
            trailing_tokens: estimated,
            last_usage_index: None,
        };
    };

    let usage_tokens = calculate_context_tokens(&usage);
    let trailing_tokens: u64 = messages[index + 1..].iter().map(estimate_tokens).sum();

    ContextUsageEstimate {
        tokens: usage_tokens + trailing_tokens,
        usage_tokens,
        trailing_tokens,
        last_usage_index: Some(index),
    }
}

/// Whether the context has grown into the space reserved for the next request.
pub fn should_compact(
    context_tokens: u64,
    context_window: u64,
    settings: &CompactionSettings,
) -> bool {
    if !settings.enabled {
        return false;
    }
    context_tokens > context_window.saturating_sub(settings.reserve_tokens)
}

// ============================================================================
// Cut point detection
// ============================================================================

const ESTIMATED_IMAGE_CHARS: usize = 4800;

fn estimate_text_and_image_content_chars(content: &UserContent) -> usize {
    match content {
        UserContent::Text(text) => text.encode_utf16().count(),
        UserContent::Blocks(blocks) => blocks
            .iter()
            .map(|block| match block {
                TextOrImageContent::Text(text) => text.text.encode_utf16().count(),
                TextOrImageContent::Image(_) => ESTIMATED_IMAGE_CHARS,
            })
            .sum(),
    }
}

/// Estimates a message's token count with the chars/4 heuristic. Deliberately
/// generous: an overestimate compacts a little early, an underestimate
/// overflows the window.
pub fn estimate_tokens(message: &AgentMessage) -> u64 {
    let chars = match message {
        AgentMessage::User(message) => estimate_text_and_image_content_chars(&message.content),
        AgentMessage::Assistant(message) => message
            .content
            .iter()
            .map(|block| match block {
                AssistantContent::Text(text) => text.text.encode_utf16().count(),
                AssistantContent::Thinking(thinking) => thinking.thinking.encode_utf16().count(),
                AssistantContent::ToolCall(call) => {
                    call.name.encode_utf16().count()
                        + serde_json::to_string(&call.arguments)
                            .unwrap_or_default()
                            .encode_utf16()
                            .count()
                }
            })
            .sum(),
        AgentMessage::Custom(message) => estimate_text_and_image_content_chars(&message.content),
        AgentMessage::ToolResult(message) => {
            estimate_text_and_image_content_chars(&UserContent::Blocks(message.content.clone()))
        }
        AgentMessage::BashExecution(message) => {
            message.command.encode_utf16().count() + message.output.encode_utf16().count()
        }
        AgentMessage::BranchSummary(message) => message.summary.encode_utf16().count(),
        AgentMessage::CompactionSummary(message) => message.summary.encode_utf16().count(),
    };
    chars.div_ceil(4) as u64
}

// ============================================================================
// Summarization
// ============================================================================

/// The instruction that turns the conversation into a handoff note.
///
/// It asks for a note rather than a form on purpose. A fixed set of headings
/// gets filled in even when a section has nothing behind it, and what fills it
/// is invention — that was the single largest source of wrong statements in
/// summaries written by the mechanism this replaces.
const HANDOFF_PROMPT: &str = r#"You are about to run out of context. Write a first-person handoff note to yourself so you can continue this task after the earlier conversation is cleared.

--- This message is a direct task, not part of the above conversation ---

Write the note as your own continuing train of thought: first person, present tense, the way you would reason through the next move. Do not write a third-party report about someone else's work, and do not impose fixed section headings — let the shape follow the task. Write the note in the same language the conversation has been using; do not switch to English because these instructions are in English.

Make the note self-sufficient. The next turn will see the user's own messages and this note, and nothing else: every assistant message, tool call and tool result above will be gone. In your own words, preserve what you genuinely need:

- What the current request is asking for: your reading of its intent, and any ambiguity you have already resolved. Do not re-transcribe the request itself, since the user's messages are kept verbatim. If several requests are in play, say which one governs the next move.
- The instructions and constraints in force — user preferences, project rules, tooling limits — condensed to what still matters. Keep decisions you have already settled (what you chose and why) separate from questions still open, so the next turn neither reopens a closed choice nor treats an undecided point as decided.
- What has actually been done, at high fidelity: the exact commands run, the exact paths touched, whether each succeeded or failed, and the results themselves — the concrete values, the key lines, the error text, the signature a lookup revealed. Re-running to recover them may be slow or impossible. Keep only the final working version of any code and drop intermediate attempts.
- What you still do not know: files referenced but not read, schemas assumed but unseen, questions the user has not answered. Name these gaps so the next turn checks them instead of assuming.
- The forward plan. You hold more context on this task now than you ever will again, so invest here. Give the exact next command or tool call, then the remaining sequence, the decisions already made for those steps, the edge cases you can foresee and how you mean to handle them.

Be honest about uncertainty. If an earlier step claimed something was done but never verified it — tests "passing", a fix "working", a file "created" — say so plainly and treat it as unverified.

Keep the note proportional to the task. A long multi-step task warrants detail; a nearly finished one needs a sentence or two. Include the identifiers and references needed to continue, and omit anything that does not change the next move.

Respond with text only. Do not call any tools."#;

/// What a summarization request needs from its caller.
#[derive(Clone, Default)]
pub struct SummarizationRequest {
    pub api_key: Option<String>,
    pub headers: Option<Vec<(String, String)>>,
    pub env: Option<Vec<(String, String)>>,
    pub signal: Option<CancellationToken>,
    pub thinking_level: Option<ThinkingLevel>,
    pub stream_fn: Option<StreamFn>,
    pub retry: Option<RetryPolicy>,
    pub callbacks: Option<RetryCallbacks>,
}

/// The request options a branch summary uses: the same auth and signal wiring,
/// with its own token budget applied by the caller.
pub(crate) fn branch_summary_options(
    model: &Model,
    request: &SummarizationRequest,
) -> SimpleStreamOptions {
    create_summarization_options(model, 0, request)
}

fn create_summarization_options(
    model: &Model,
    max_tokens: u64,
    request: &SummarizationRequest,
) -> SimpleStreamOptions {
    let mut options = SimpleStreamOptions {
        base: StreamOptions {
            max_tokens: Some(max_tokens),
            session_id: None,
            ..StreamOptions::default()
        },
        ..SimpleStreamOptions::default()
    };
    options.base.base.api_key = request.api_key.clone();
    options.base.base.headers = request.headers.as_ref().map(|headers| {
        headers
            .iter()
            .map(|(key, value)| (key.clone(), Some(value.clone())))
            .collect()
    });
    options.base.base.env = request
        .env
        .as_ref()
        .map(|env| env.iter().cloned().collect());
    options.base.base.signal = request.signal.clone();
    if model.reasoning
        && let Some(level) = request.thinking_level
        && level != ThinkingLevel::Off
    {
        options.reasoning = level.to_reasoning();
    }
    options
}

/// The single choke point every summarization call goes through.
///
/// Wrapping the one call in `retry_assistant_call` means a dropped stream
/// retries under the configured policy instead of failing the whole
/// compaction; deterministic errors and aborts return immediately.
pub async fn complete_summarization(
    model: &Model,
    context: Context,
    options: SimpleStreamOptions,
    stream_fn: Option<StreamFn>,
    retry: Option<RetryPolicy>,
    callbacks: Option<&RetryCallbacks>,
) -> AssistantMessage {
    // Summaries are standalone requests: isolate routing and write no cache
    // entry that could never be reused.
    let mut request_options = options;
    request_options.base.cache_retention = Some(notagent_ai::types::CacheRetention::None);
    request_options.base.session_id = Some(uuidv7());
    let signal = request_options.base.base.signal.clone();

    let stream_fn = match stream_fn {
        Some(stream_fn) => Some(stream_fn),
        None => get_default_stream_fn().ok(),
    };
    let Some(stream_fn) = stream_fn else {
        return error_message(model, "No default stream function configured");
    };

    retry_assistant_call(
        || {
            let stream_fn = Arc::clone(&stream_fn);
            let model = model.clone();
            let context = context.clone();
            let options = request_options.clone();
            async move {
                stream_fn(model, context, Some(options))
                    .await
                    .result()
                    .await
            }
        },
        retry,
        signal,
        callbacks,
    )
    .await
}

/// The shape a failed request takes: TypeScript's `completeSimple` reaches the
/// global provider registry, and this port's stream function is handed in. When
/// neither is configured the call cannot be made, and the caller reads that as
/// an errored response — the same path a provider failure takes.
fn error_message(model: &Model, message: &str) -> AssistantMessage {
    AssistantMessage {
        content: Vec::new(),
        api: model.api.clone(),
        provider: model.provider.clone(),
        model: model.id.clone(),
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
            total_tokens: None,
            cost: UsageCost {
                input: 0.0,
                output: 0.0,
                cache_read: 0.0,
                cache_write: 0.0,
                total: 0.0,
            },
        },
        stop_reason: StopReason::Error,
        deferred: None,
        error_message: Some(message.to_string()),
        raw_stop_reason: None,
        end_turn: None,
        timestamp: chrono::Utc::now().timestamp_millis(),
    }
}

pub(crate) fn summarization_context_for(prompt_text: String) -> Context {
    Context {
        system_prompt: Some(SUMMARIZATION_SYSTEM_PROMPT.to_string()),
        messages: vec![Message::User(UserMessage {
            content: UserContent::Blocks(vec![TextOrImageContent::Text(TextContent::new(
                prompt_text,
            ))]),
            timestamp: chrono::Utc::now().timestamp_millis(),
        })],
        tools: None,
    }
}

/// The note produced by one summarization call, and what it cost.
#[derive(Debug, Clone, Default)]
pub struct GeneratedSummary {
    pub text: String,
    pub usage: Usage,
    /// History the request budget forced out, in estimated tokens.
    pub dropped_tokens: u64,
}

/// Writes the handoff note for a conversation, and reports what it cost.
///
/// `request_budget` caps the serialized conversation; zero means no cap. The
/// size is worked out here rather than discovered from a provider rejection,
/// so the call is made once with a request that fits.
pub async fn generate_summary_with_usage(
    current_messages: &[AgentMessage],
    model: &Model,
    reserve_tokens: u64,
    request_budget: u64,
    custom_instructions: Option<&str>,
    request: &SummarizationRequest,
) -> Result<GeneratedSummary, String> {
    let budget = (0.8 * reserve_tokens as f64).floor() as u64;
    let max_tokens = if model.max_tokens > 0 {
        budget.min(model.max_tokens)
    } else {
        budget
    };

    let mut base_prompt = HANDOFF_PROMPT.to_string();
    if let Some(instructions) = custom_instructions {
        base_prompt =
            format!("{base_prompt}\n\nOptional instruction from the user:\n{instructions}");
    }

    // Serialized rather than replayed, so the model summarizes instead of
    // continuing. Custom message types are folded in first.
    let llm_messages = convert_to_llm(current_messages);
    let conversation = serialize_conversation_within(&llm_messages, request_budget);

    let mut prompt_text = String::new();
    if conversation.dropped_messages > 0 {
        prompt_text.push_str(&format!(
            "[The oldest {} messages of this conversation did not fit and are not shown.]\n\n",
            conversation.dropped_messages
        ));
    }
    prompt_text.push_str(&format!(
        "<conversation>\n{}\n</conversation>\n\n",
        conversation.text
    ));
    prompt_text.push_str(&base_prompt);

    let response = complete_summarization(
        model,
        summarization_context_for(prompt_text),
        create_summarization_options(model, max_tokens, request),
        request.stream_fn.clone(),
        request.retry,
        request.callbacks.as_ref(),
    )
    .await;

    if response.stop_reason == StopReason::Error {
        return Err(format!(
            "Summarization failed: {}",
            response
                .error_message
                .clone()
                .unwrap_or_else(|| "Unknown error".to_string())
        ));
    }

    Ok(GeneratedSummary {
        text: content_text(&response.content),
        usage: response.usage,
        dropped_tokens: conversation.dropped_tokens,
    })
}

/// Generates a summary and discards everything but the text.
pub async fn generate_summary(
    current_messages: &[AgentMessage],
    model: &Model,
    reserve_tokens: u64,
    request_budget: u64,
    custom_instructions: Option<&str>,
    request: &SummarizationRequest,
) -> Result<String, String> {
    generate_summary_with_usage(
        current_messages,
        model,
        reserve_tokens,
        request_budget,
        custom_instructions,
        request,
    )
    .await
    .map(|summary| summary.text)
}

// ============================================================================
// Preparation
// ============================================================================

/// Everything decided before the first token is spent.
#[derive(Debug, Clone, PartialEq)]
pub struct CompactionPreparation {
    /// The oldest retained entry, or the oldest context entry when the session
    /// has no user messages at all.
    ///
    /// A build that predates the retained selection reads only this field and
    /// keeps everything from it onward, which is a superset of what the
    /// selection keeps — a degraded session rather than a broken one.
    pub first_kept_entry_id: String,
    /// The whole context as the agent currently sees it. This is what gets
    /// summarized; after an earlier compaction it already contains that
    /// compaction's note, so nothing has to be folded in behind the model.
    pub messages_to_summarize: Vec<AgentMessage>,
    /// The user messages carried through verbatim.
    pub retained: RetainedSelection,
    /// Those same messages, with any truncation already applied, in context
    /// order. Kept so the size after the compaction can be worked out before
    /// the entry that produces it exists.
    pub retained_messages: Vec<AgentMessage>,
    pub tokens_before: u64,
    pub outline: OperationOutline,
    pub settings: CompactionSettings,
}

/// Works out what a compaction would do, or `None` when there is nothing to do.
pub fn prepare_compaction(
    path_entries: &[SessionEntry],
    settings: &CompactionSettings,
) -> Option<CompactionPreparation> {
    if matches!(path_entries.last(), Some(SessionEntry::Compaction(_))) {
        return None;
    }

    let context_entries = build_context_entries(path_entries, LeafSelector::Undefined);
    let messages_to_summarize =
        build_session_context(path_entries, LeafSelector::Undefined).messages;
    if messages_to_summarize.is_empty() {
        return None;
    }

    // A session that is nothing but user messages has nothing to summarize:
    // compacting it would replace the messages with themselves plus a note
    // about them.
    if !messages_to_summarize
        .iter()
        .any(|message| !is_user_input(message))
    {
        return None;
    }

    let candidates: Vec<(String, AgentMessage)> = context_entries
        .iter()
        .filter_map(|entry| {
            let mut messages = session_entry_to_context_messages(entry);
            let message = messages.pop()?;
            is_user_input(&message).then(|| (entry.id().to_string(), message))
        })
        .collect();

    let retained = select_retained(
        &candidates,
        settings.retained_user_tokens,
        RETAINED_HEAD_TOKENS,
    );

    let mut retained_messages: Vec<AgentMessage> = Vec::new();
    for record in retained.entries() {
        if let Some((_, message)) = candidates.iter().find(|(id, _)| *id == record.id) {
            retained_messages.push(apply_retention(message, record));
        }
        if retained.elision_after() == Some(record.id.as_str()) {
            retained_messages.push(elision_message(retained.omitted_tokens, 0));
        }
    }

    let first_kept_entry_id = retained
        .oldest_id()
        .map(str::to_string)
        .or_else(|| context_entries.first().map(|entry| entry.id().to_string()))
        .unwrap_or_default();
    if first_kept_entry_id.is_empty() {
        // Session needs migration.
        return None;
    }

    let tokens_before = estimate_context_tokens(&messages_to_summarize).tokens;
    let previous = path_entries
        .iter()
        .rev()
        .find(|entry| matches!(entry, SessionEntry::Compaction(_)));
    let outline = extract_operations(&messages_to_summarize, previous);

    Some(CompactionPreparation {
        first_kept_entry_id,
        messages_to_summarize,
        retained,
        retained_messages,
        tokens_before,
        outline,
        settings: *settings,
    })
}

// ============================================================================
// Compaction
// ============================================================================

/// The share of the model's window a summarization request may occupy.
///
/// The rest is headroom for the note itself and for whatever the estimate got
/// wrong; the estimate rounds up per message and charges a flat rate per image,
/// so it errs high, and this only has to cover the cases where it does not.
fn request_budget(model: &Model, settings: &CompactionSettings) -> u64 {
    model.context_window.saturating_sub(settings.reserve_tokens)
}

/// Writes the note a prepared compaction calls for.
pub async fn compact(
    preparation: &CompactionPreparation,
    model: &Model,
    custom_instructions: Option<&str>,
    request: &SummarizationRequest,
) -> Result<CompactionResult, String> {
    if preparation.first_kept_entry_id.is_empty() {
        return Err("First kept entry has no UUID - session may need migration".to_string());
    }

    let budget = request_budget(model, &preparation.settings);
    let summary = match generate_summary_with_usage(
        &preparation.messages_to_summarize,
        model,
        preparation.settings.reserve_tokens,
        budget,
        custom_instructions,
        request,
    )
    .await
    {
        Ok(summary) => summary,
        // The estimate decided the size, and an estimate can be wrong. One
        // retry at half the budget covers that without turning the rejection
        // into the mechanism: a compaction that cannot be written at all costs
        // the session, and a second call costs one round trip.
        Err(error) if budget > 0 => generate_summary_with_usage(
            &preparation.messages_to_summarize,
            model,
            preparation.settings.reserve_tokens,
            budget / 2,
            custom_instructions,
            request,
        )
        .await
        .map_err(|retry_error| {
            format!("{error}\nRetrying with a smaller request also failed: {retry_error}")
        })?,
        Err(error) => return Err(error),
    };

    // The outline is appended rather than asked for, because it is derived from
    // the recorded tool calls and so cannot name a path that was never touched.
    let text = format!(
        "{}{}",
        summary.text,
        format_operation_outline(&preparation.outline)
    );

    // What the context will measure once this is applied: the retained
    // messages, the elision note among them, and the summary itself. Worked out
    // here because the entry that would let it be measured does not exist yet.
    let tokens_after = preparation
        .retained_messages
        .iter()
        .map(estimate_tokens)
        .sum::<u64>()
        + estimate_tokens(&AgentMessage::CompactionSummary(
            notagent_agent::create_compaction_summary_message(
                text.clone(),
                preparation.tokens_before,
                None,
                0,
            ),
        ));

    Ok(CompactionResult {
        summary: text,
        first_kept_entry_id: preparation.first_kept_entry_id.clone(),
        retained: preparation.retained.clone(),
        tokens_before: preparation.tokens_before,
        estimated_tokens_after: Some(tokens_after),
        dropped_tokens: summary.dropped_tokens,
        usage: Some(summary.usage),
        details: serde_json::to_value(CompactionDetails {
            operations: preparation.outline.records().to_vec(),
        })
        .ok(),
    })
}
