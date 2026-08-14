//! Port of `packages/coding-agent/src/core/compaction/compaction.ts`.
//!
//! Context compaction for long sessions. Pure functions: the session manager
//! does the I/O and rebuilds its context afterwards.
//!
//! Two rules shape the cut. The first is that a tool result is never a cut
//! point — it has to follow its tool call, and a context that opens with an
//! orphaned result is one no provider accepts. The second is the split-turn
//! path: when the cut lands in the middle of a turn, the prefix of that turn is
//! summarized separately, because the retained suffix would otherwise start
//! mid-thought with no statement of what was asked.

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

use crate::core::compaction::utils::{
    FileOperations, SUMMARIZATION_SYSTEM_PROMPT, compute_file_lists, create_file_ops,
    extract_file_ops_from_message, format_file_operations, serialize_conversation,
};
use crate::core::messages::convert_to_llm;
use crate::core::session_manager::{
    LeafSelector, SessionEntry, build_session_context, session_entry_to_context_messages,
};

// ============================================================================
// File operation tracking
// ============================================================================

/// What a compaction entry stores about the files its window touched.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionDetails {
    pub read_files: Vec<String>,
    pub modified_files: Vec<String>,
}

/// Collects file operations from the messages plus the previous compaction's
/// record, so the list is cumulative across compactions rather than a window.
fn extract_file_operations(
    messages: &[AgentMessage],
    entries: &[SessionEntry],
    previous_compaction_index: Option<usize>,
) -> FileOperations {
    let mut file_ops = create_file_ops();

    if let Some(index) = previous_compaction_index
        && let Some(SessionEntry::Compaction(previous)) = entries.get(index)
        // `fromHook` is retained for session-file compatibility; an entry an
        // extension wrote carries details this port cannot interpret.
        && previous.from_hook != Some(true)
        && let Some(details) = previous.details.as_ref()
    {
        if let Some(read_files) = details.get("readFiles").and_then(|value| value.as_array()) {
            for file in read_files.iter().filter_map(|file| file.as_str()) {
                file_ops.read.insert(file.to_string());
            }
        }
        if let Some(modified) = details
            .get("modifiedFiles")
            .and_then(|value| value.as_array())
        {
            for file in modified.iter().filter_map(|file| file.as_str()) {
                file_ops.edited.insert(file.to_string());
            }
        }
    }

    for message in messages {
        extract_file_ops_from_message(message, &mut file_ops);
    }

    file_ops
}

// ============================================================================
// Message extraction
// ============================================================================

/// The message an entry contributes, or nothing when it contributes none.
/// Compaction entries are skipped: their summary is the boundary, not content.
fn message_from_entry_for_compaction(entry: &SessionEntry) -> Option<AgentMessage> {
    if matches!(entry, SessionEntry::Compaction(_)) {
        return None;
    }
    session_entry_to_context_messages(entry).into_iter().next()
}

/// What `compact` produces. The session manager stamps uuid and parentUuid when
/// it saves.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CompactionResult {
    pub summary: String,
    pub first_kept_entry_id: String,
    pub tokens_before: u64,
    pub estimated_tokens_after: Option<u64>,
    /// Usage of the LLM call(s) that produced the summary.
    pub usage: Option<Usage>,
    pub details: Option<serde_json::Value>,
}

fn combine_usage(first: &Usage, second: &Usage) -> Usage {
    Usage {
        input: first.input + second.input,
        output: first.output + second.output,
        cache_read: first.cache_read + second.cache_read,
        cache_write: first.cache_write + second.cache_write,
        cache_write1h: match (first.cache_write1h, second.cache_write1h) {
            (None, None) => None,
            (left, right) => Some(left.unwrap_or(0) + right.unwrap_or(0)),
        },
        reasoning: match (first.reasoning, second.reasoning) {
            (None, None) => None,
            (left, right) => Some(left.unwrap_or(0) + right.unwrap_or(0)),
        },
        total_tokens: Some(first.total_tokens.unwrap_or(0) + second.total_tokens.unwrap_or(0)),
        cost: UsageCost {
            input: first.cost.input + second.cost.input,
            output: first.cost.output + second.cost.output,
            cache_read: first.cost.cache_read + second.cost.cache_read,
            cache_write: first.cost.cache_write + second.cost.cache_write,
            total: first.cost.total + second.cost.total,
        },
    }
}

// ============================================================================
// Settings
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactionSettings {
    pub enabled: bool,
    pub reserve_tokens: u64,
    pub keep_recent_tokens: u64,
}

pub const DEFAULT_COMPACTION_SETTINGS: CompactionSettings = CompactionSettings {
    enabled: true,
    reserve_tokens: 16384,
    keep_recent_tokens: 20000,
};

impl From<crate::core::settings_manager::ResolvedCompactionSettings> for CompactionSettings {
    fn from(settings: crate::core::settings_manager::ResolvedCompactionSettings) -> Self {
        Self {
            enabled: settings.enabled,
            reserve_tokens: settings.reserve_tokens,
            keep_recent_tokens: settings.keep_recent_tokens,
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

fn is_cut_point_message(message: &AgentMessage) -> bool {
    !matches!(message, AgentMessage::ToolResult(_))
}

fn is_turn_start_message(message: &AgentMessage) -> bool {
    matches!(
        message,
        AgentMessage::User(_)
            | AgentMessage::BashExecution(_)
            | AgentMessage::Custom(_)
            | AgentMessage::BranchSummary(_)
            | AgentMessage::CompactionSummary(_)
    )
}

fn is_turn_start_entry(entry: &SessionEntry) -> bool {
    if matches!(entry, SessionEntry::Compaction(_)) {
        return false;
    }
    session_entry_to_context_messages(entry)
        .iter()
        .any(is_turn_start_message)
}

/// Indices that may be cut at: context-visible messages that are not tool
/// results. Cutting at an assistant message with tool calls is fine — its
/// results follow it and are kept.
fn find_valid_cut_points(
    entries: &[SessionEntry],
    start_index: usize,
    end_index: usize,
) -> Vec<usize> {
    let mut cut_points = Vec::new();
    for index in start_index..end_index {
        let Some(entry) = entries.get(index) else {
            continue;
        };
        if matches!(entry, SessionEntry::Compaction(_)) {
            continue;
        }
        if session_entry_to_context_messages(entry)
            .iter()
            .any(is_cut_point_message)
        {
            cut_points.push(index);
        }
    }
    cut_points
}

/// The context-visible user-role entry that starts the turn containing
/// `entry_index`, or `None`.
pub fn find_turn_start_index(
    entries: &[SessionEntry],
    entry_index: usize,
    start_index: usize,
) -> Option<usize> {
    let mut index = entry_index;
    loop {
        if index < start_index {
            return None;
        }
        if entries.get(index).is_some_and(is_turn_start_entry) {
            return Some(index);
        }
        if index == 0 {
            return None;
        }
        index -= 1;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CutPointResult {
    /// Index of the first entry to keep.
    pub first_kept_entry_index: usize,
    /// The user message that starts the turn being split, if one is.
    pub turn_start_index: Option<usize>,
    pub is_split_turn: bool,
}

/// Finds the cut point that keeps roughly `keep_recent_tokens`.
///
/// Walks backwards from the newest entry accumulating estimated sizes, and cuts
/// at the first valid point at or after where the budget was reached.
pub fn find_cut_point(
    entries: &[SessionEntry],
    start_index: usize,
    end_index: usize,
    keep_recent_tokens: u64,
) -> CutPointResult {
    let cut_points = find_valid_cut_points(entries, start_index, end_index);

    if cut_points.is_empty() {
        return CutPointResult {
            first_kept_entry_index: start_index,
            turn_start_index: None,
            is_split_turn: false,
        };
    }

    let mut accumulated: u64 = 0;
    // Default: keep everything from the first message (not the header).
    let mut cut_index = cut_points[0];

    let mut index = end_index;
    while index > start_index {
        index -= 1;
        let Some(entry) = entries.get(index) else {
            continue;
        };
        let message_tokens: u64 = session_entry_to_context_messages(entry)
            .iter()
            .map(estimate_tokens)
            .sum();
        if message_tokens == 0 {
            continue;
        }
        accumulated += message_tokens;

        if accumulated >= keep_recent_tokens {
            if let Some(&point) = cut_points.iter().find(|&&point| point >= index) {
                cut_index = point;
            }
            break;
        }
    }

    // Pull adjacent metadata entries — model changes, labels — into the kept
    // range: they do not affect context, and leaving them behind the boundary
    // would drop information the UI still shows.
    while cut_index > start_index {
        let Some(previous) = entries.get(cut_index - 1) else {
            break;
        };
        if matches!(previous, SessionEntry::Compaction(_))
            || !session_entry_to_context_messages(previous).is_empty()
        {
            break;
        }
        cut_index -= 1;
    }

    let starts_turn = entries.get(cut_index).is_some_and(is_turn_start_entry);
    let turn_start_index = if starts_turn {
        None
    } else {
        find_turn_start_index(entries, cut_index, start_index)
    };

    CutPointResult {
        first_kept_entry_index: cut_index,
        turn_start_index,
        is_split_turn: !starts_turn && turn_start_index.is_some(),
    }
}

// ============================================================================
// Summarization
// ============================================================================

const SUMMARIZATION_PROMPT: &str = r#"The messages above are a conversation to summarize. Create a structured context checkpoint summary that another LLM will use to continue the work.

Use this EXACT format:

## Goal
[What is the user trying to accomplish? Can be multiple items if the session covers different tasks.]

## Constraints & Preferences
- [Any constraints, preferences, or requirements mentioned by user]
- [Or "(none)" if none were mentioned]

## Progress
### Done
- [x] [Completed tasks/changes]

### In Progress
- [ ] [Current work]

### Blocked
- [Issues preventing progress, if any]

## Key Decisions
- **[Decision]**: [Brief rationale]

## Next Steps
1. [Ordered list of what should happen next]

## Critical Context
- [Any data, examples, or references needed to continue]
- [Or "(none)" if not applicable]

Keep each section concise. Preserve exact file paths, function names, and error messages."#;

const UPDATE_SUMMARIZATION_PROMPT: &str = r#"The messages above are NEW conversation messages to incorporate into the existing summary provided in <previous-summary> tags.

Update the existing structured summary with new information. RULES:
- PRESERVE all existing information from the previous summary
- ADD new progress, decisions, and context from the new messages
- UPDATE the Progress section: move items from "In Progress" to "Done" when completed
- UPDATE "Next Steps" based on what was accomplished
- PRESERVE exact file paths, function names, and error messages
- If something is no longer relevant, you may remove it

Use this EXACT format:

## Goal
[Preserve existing goals, add new ones if the task expanded]

## Constraints & Preferences
- [Preserve existing, add new ones discovered]

## Progress
### Done
- [x] [Include previously done items AND newly completed items]

### In Progress
- [ ] [Current work - update based on progress]

### Blocked
- [Current blockers - remove if resolved]

## Key Decisions
- **[Decision]**: [Brief rationale] (preserve all previous, add new)

## Next Steps
1. [Update based on current state]

## Critical Context
- [Preserve important context, add new if needed]

Keep each section concise. Preserve exact file paths, function names, and error messages."#;

const TURN_PREFIX_SUMMARIZATION_PROMPT: &str = r#"This is the PREFIX of a turn that was too large to keep. The SUFFIX (recent work) is retained.

Summarize the prefix to provide context for the retained suffix:

## Original Request
[What did the user ask for in this turn?]

## Early Progress
- [Key decisions and work done in the prefix]

## Context for Suffix
- [Information needed to understand the retained recent work]

Be concise. Focus on what's needed to understand the kept suffix."#;

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

/// Generates or updates a conversation summary, and reports what it cost.
pub async fn generate_summary_with_usage(
    current_messages: &[AgentMessage],
    model: &Model,
    reserve_tokens: u64,
    custom_instructions: Option<&str>,
    previous_summary: Option<&str>,
    request: &SummarizationRequest,
) -> Result<(String, Usage), String> {
    let budget = (0.8 * reserve_tokens as f64).floor() as u64;
    let max_tokens = if model.max_tokens > 0 {
        budget.min(model.max_tokens)
    } else {
        budget
    };

    let mut base_prompt = if previous_summary.is_some() {
        UPDATE_SUMMARIZATION_PROMPT.to_string()
    } else {
        SUMMARIZATION_PROMPT.to_string()
    };
    if let Some(instructions) = custom_instructions {
        base_prompt = format!("{base_prompt}\n\nAdditional focus: {instructions}");
    }

    // Serialized rather than replayed, so the model summarizes instead of
    // continuing. Custom message types are folded in first.
    let llm_messages = convert_to_llm(current_messages);
    let conversation_text = serialize_conversation(&llm_messages);

    let mut prompt_text = format!("<conversation>\n{conversation_text}\n</conversation>\n\n");
    if let Some(previous) = previous_summary {
        prompt_text.push_str(&format!(
            "<previous-summary>\n{previous}\n</previous-summary>\n\n"
        ));
    }
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

    Ok((content_text(&response.content), response.usage))
}

/// Generates a summary and discards the usage.
pub async fn generate_summary(
    current_messages: &[AgentMessage],
    model: &Model,
    reserve_tokens: u64,
    custom_instructions: Option<&str>,
    previous_summary: Option<&str>,
    request: &SummarizationRequest,
) -> Result<String, String> {
    generate_summary_with_usage(
        current_messages,
        model,
        reserve_tokens,
        custom_instructions,
        previous_summary,
        request,
    )
    .await
    .map(|(text, _)| text)
}

// ============================================================================
// Preparation
// ============================================================================

/// Everything decided before the first token is spent.
#[derive(Debug, Clone, PartialEq)]
pub struct CompactionPreparation {
    pub first_kept_entry_id: String,
    /// Messages that will be summarized and dropped.
    pub messages_to_summarize: Vec<AgentMessage>,
    /// Messages that become the turn-prefix summary, when a turn is split.
    pub turn_prefix_messages: Vec<AgentMessage>,
    pub is_split_turn: bool,
    pub tokens_before: u64,
    /// The previous compaction's summary, for an iterative update.
    pub previous_summary: Option<String>,
    pub file_ops: FileOperations,
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

    let previous_compaction_index = path_entries
        .iter()
        .rposition(|entry| matches!(entry, SessionEntry::Compaction(_)));

    let mut previous_summary: Option<String> = None;
    let mut boundary_start = 0usize;
    if let Some(index) = previous_compaction_index
        && let Some(SessionEntry::Compaction(previous)) = path_entries.get(index)
    {
        previous_summary = Some(previous.summary.clone());
        boundary_start = path_entries
            .iter()
            .position(|entry| entry.id() == previous.first_kept_entry_id)
            .unwrap_or(index + 1);
    }
    let boundary_end = path_entries.len();

    let owned: Vec<SessionEntry> = path_entries.to_vec();
    let tokens_before =
        estimate_context_tokens(&build_session_context(&owned, LeafSelector::Undefined).messages)
            .tokens;

    let cut_point = find_cut_point(
        path_entries,
        boundary_start,
        boundary_end,
        settings.keep_recent_tokens,
    );

    let first_kept_entry = path_entries.get(cut_point.first_kept_entry_index)?;
    let first_kept_entry_id = first_kept_entry.id().to_string();
    if first_kept_entry_id.is_empty() {
        // Session needs migration.
        return None;
    }

    let history_end = if cut_point.is_split_turn {
        cut_point.turn_start_index.unwrap_or(0)
    } else {
        cut_point.first_kept_entry_index
    };

    let mut messages_to_summarize: Vec<AgentMessage> = Vec::new();
    for index in boundary_start..history_end {
        if let Some(entry) = path_entries.get(index)
            && let Some(message) = message_from_entry_for_compaction(entry)
        {
            messages_to_summarize.push(message);
        }
    }

    let mut turn_prefix_messages: Vec<AgentMessage> = Vec::new();
    if cut_point.is_split_turn
        && let Some(turn_start) = cut_point.turn_start_index
    {
        for index in turn_start..cut_point.first_kept_entry_index {
            if let Some(entry) = path_entries.get(index)
                && let Some(message) = message_from_entry_for_compaction(entry)
            {
                turn_prefix_messages.push(message);
            }
        }
    }

    if messages_to_summarize.is_empty() && turn_prefix_messages.is_empty() {
        return None;
    }

    let mut file_ops = extract_file_operations(
        &messages_to_summarize,
        path_entries,
        previous_compaction_index,
    );

    if cut_point.is_split_turn {
        for message in &turn_prefix_messages {
            extract_file_ops_from_message(message, &mut file_ops);
        }
    }

    Some(CompactionPreparation {
        first_kept_entry_id,
        messages_to_summarize,
        turn_prefix_messages,
        is_split_turn: cut_point.is_split_turn,
        tokens_before,
        previous_summary,
        file_ops,
        settings: *settings,
    })
}

// ============================================================================
// Compaction
// ============================================================================

/// Generates the summaries a prepared compaction calls for.
pub async fn compact(
    preparation: &CompactionPreparation,
    model: &Model,
    custom_instructions: Option<&str>,
    request: &SummarizationRequest,
) -> Result<CompactionResult, String> {
    let summary;
    let summary_usage;

    if preparation.is_split_turn && !preparation.turn_prefix_messages.is_empty() {
        let mut history_text = "No prior history.".to_string();
        let mut history_usage: Option<Usage> = None;
        if !preparation.messages_to_summarize.is_empty() {
            let (text, usage) = generate_summary_with_usage(
                &preparation.messages_to_summarize,
                model,
                preparation.settings.reserve_tokens,
                custom_instructions,
                preparation.previous_summary.as_deref(),
                request,
            )
            .await?;
            history_text = text;
            history_usage = Some(usage);
        }
        let (turn_prefix_text, turn_prefix_usage) = generate_turn_prefix_summary(
            &preparation.turn_prefix_messages,
            model,
            preparation.settings.reserve_tokens,
            request,
        )
        .await?;
        summary = format!(
            "{history_text}\n\n---\n\n**Turn Context (split turn):**\n\n{turn_prefix_text}"
        );
        summary_usage = match history_usage {
            Some(history) => combine_usage(&history, &turn_prefix_usage),
            None => turn_prefix_usage,
        };
    } else {
        let (text, usage) = generate_summary_with_usage(
            &preparation.messages_to_summarize,
            model,
            preparation.settings.reserve_tokens,
            custom_instructions,
            preparation.previous_summary.as_deref(),
            request,
        )
        .await?;
        summary = text;
        summary_usage = usage;
    }

    let (read_files, modified_files) = compute_file_lists(&preparation.file_ops);
    let summary = format!(
        "{summary}{}",
        format_file_operations(&read_files, &modified_files)
    );

    if preparation.first_kept_entry_id.is_empty() {
        return Err("First kept entry has no UUID - session may need migration".to_string());
    }

    Ok(CompactionResult {
        summary,
        first_kept_entry_id: preparation.first_kept_entry_id.clone(),
        tokens_before: preparation.tokens_before,
        estimated_tokens_after: None,
        usage: Some(summary_usage),
        details: serde_json::to_value(CompactionDetails {
            read_files,
            modified_files,
        })
        .ok(),
    })
}

/// The smaller summary that stands in for the dropped prefix of a split turn.
async fn generate_turn_prefix_summary(
    messages: &[AgentMessage],
    model: &Model,
    reserve_tokens: u64,
    request: &SummarizationRequest,
) -> Result<(String, Usage), String> {
    // A smaller budget than the history summary: it only has to explain one
    // turn's opening.
    let budget = (0.5 * reserve_tokens as f64).floor() as u64;
    let max_tokens = if model.max_tokens > 0 {
        budget.min(model.max_tokens)
    } else {
        budget
    };
    let llm_messages = convert_to_llm(messages);
    let conversation_text = serialize_conversation(&llm_messages);
    let prompt_text = format!(
        "<conversation>\n{conversation_text}\n</conversation>\n\n{TURN_PREFIX_SUMMARIZATION_PROMPT}"
    );

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
            "Turn prefix summarization failed: {}",
            response
                .error_message
                .clone()
                .unwrap_or_else(|| "Unknown error".to_string())
        ));
    }

    Ok((content_text(&response.content), response.usage))
}
