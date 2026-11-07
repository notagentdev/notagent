//! Context folding: the model replaces a stretch of finished work with a
//! summary it writes itself, and can take the summary back.
//! Compaction rewrites the whole history at once, when the window is nearly
//! full, through a separate summarization call. A fold is the small version
//! the model chooses: a finished search, a test run that has been read, a
//! dead end. The rest of the context stays as it was, so a fold costs no
//! extra model call and moves the prompt cache only from the fold onwards.
//! Folds go in order, oldest unfolded step first: a fold that starts further
//! back would re-send everything after it uncached, and repeated across a
//! session those rewrites cost more than the fold saves. Started at the
//! oldest unfolded step, everything before it stays cached and what is
//! re-sent is the summary and the step in flight.
//! A fold is stored as a `custom` session entry naming the first and the last
//! entry it covers. The entries themselves stay in the session file untouched,
//! which is what makes a fold reversible and searchable, and what lets a build
//! that knows nothing about folds show the full history instead of failing:
//! every build already ignores custom entries when it builds the context.
//! Folds and compaction cannot collide. Only entries after the newest
//! compaction can be folded, and a fold whose range a later compaction
//! replaced has nothing left to apply to; the compaction summary was written
//! from the folded context, so what the fold said survives in it.

use std::collections::{HashMap, HashSet};

use notagent_agent::types::AgentMessage;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::core::compaction::compaction::estimate_tokens;
use crate::core::session_manager::{SessionEntry, session_entry_to_context_messages};

/// `customType` of the entry that records a fold.
pub const FOLD_ENTRY_TYPE: &str = "context_fold";
/// `customType` of the entry that takes a fold back.
pub const UNFOLD_ENTRY_TYPE: &str = "context_unfold";
/// `customType` of the message that stands in for a folded range.
pub const FOLD_MESSAGE_TYPE: &str = "context_fold";
/// `customType` of the reminder to fold. It is kept in the session so a
/// rebuilt context still shows what the model was told, until a fold answers
/// it.
pub const FOLD_REMINDER_TYPE: &str = "context_fold_reminder";

/// The data of a fold entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FoldRecord {
    pub first_entry_id: String,
    pub last_entry_id: String,
    pub summary: String,
    #[serde(default)]
    pub steps: usize,
    #[serde(default)]
    pub folded_tokens: u64,
}

/// The data of an unfold entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnfoldRecord {
    pub fold_id: String,
}

/// A fold in force, located in a context entry list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveFold {
    /// The id of the fold entry, which is how the model names it.
    pub id: String,
    pub record: FoldRecord,
    pub start: usize,
    pub end: usize,
}

/// One unit the model can fold: an assistant message and the tool results it
/// produced. Folding whole steps is what keeps every tool call next to its
/// result, which providers require.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    /// The id of the assistant entry.
    pub id: String,
    pub start: usize,
    pub end: usize,
    pub tokens: u64,
    pub preview: String,
}

/// What the context is built from once folds are applied.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ContextPiece<'a> {
    Entry(&'a SessionEntry),
    Fold(&'a ActiveFold),
}

fn custom_data<'a>(entry: &'a SessionEntry, custom_type: &str) -> Option<&'a Value> {
    match entry {
        SessionEntry::Custom(custom) if custom.custom_type == custom_type => custom.data.as_ref(),
        _ => None,
    }
}

fn message_role(entry: &SessionEntry) -> Option<&str> {
    match entry {
        SessionEntry::Message(message) => message.message.get("role").and_then(Value::as_str),
        _ => None,
    }
}

/// Where foldable history begins: just after the newest compaction, whose
/// summary and retained messages are not this mechanism's to touch.
fn foldable_start(entries: &[&SessionEntry]) -> usize {
    entries
        .iter()
        .rposition(|entry| matches!(entry, SessionEntry::Compaction(_)))
        .map_or(0, |index| index + 1)
}

/// The folds in force on a context entry list: recorded after the newest
/// compaction, not taken back, and with both ends of their range still in the
/// list. A fold that contains an earlier one replaces it; one that only partly
/// overlaps another cannot be recorded and is ignored if a hand-edited file
/// contains it.
pub fn active_folds(entries: &[&SessionEntry]) -> Vec<ActiveFold> {
    let first = foldable_start(entries);
    let positions: HashMap<&str, usize> = entries
        .iter()
        .enumerate()
        .skip(first)
        .map(|(index, entry)| (entry.id(), index))
        .collect();
    let unfolded: HashSet<String> = entries[first..]
        .iter()
        .filter_map(|entry| custom_data(entry, UNFOLD_ENTRY_TYPE))
        .filter_map(|data| serde_json::from_value::<UnfoldRecord>(data.clone()).ok())
        .map(|record| record.fold_id)
        .collect();

    let mut accepted: Vec<ActiveFold> = Vec::new();
    for (index, entry) in entries.iter().enumerate().skip(first) {
        let Some(data) = custom_data(entry, FOLD_ENTRY_TYPE) else {
            continue;
        };
        if unfolded.contains(entry.id()) {
            continue;
        }
        let Ok(record) = serde_json::from_value::<FoldRecord>(data.clone()) else {
            continue;
        };
        let (Some(&start), Some(&end)) = (
            positions.get(record.first_entry_id.as_str()),
            positions.get(record.last_entry_id.as_str()),
        ) else {
            continue;
        };
        // A fold covers what came before it, never itself or what follows.
        if start > end || end >= index {
            continue;
        }
        let partial = accepted.iter().any(|fold| {
            fold.start <= end && start <= fold.end && !(start <= fold.start && fold.end <= end)
        });
        if partial {
            continue;
        }
        accepted.retain(|fold| !(start <= fold.start && fold.end <= end));
        accepted.push(ActiveFold {
            id: entry.id().to_owned(),
            record,
            start,
            end,
        });
    }
    accepted.sort_by_key(|fold| fold.start);
    accepted
}

fn entry_tokens(entry: &SessionEntry) -> u64 {
    session_entry_to_context_messages(entry)
        .iter()
        .map(estimate_tokens)
        .sum()
}

fn step_preview(entry: &SessionEntry) -> String {
    let SessionEntry::Message(message) = entry else {
        return String::new();
    };
    let mut text = String::new();
    let mut tools: Vec<&str> = Vec::new();
    for block in message
        .message
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        match block.get("type").and_then(Value::as_str) {
            Some("text") if text.is_empty() => {
                text = block
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ");
            }
            Some("toolCall") => {
                if let Some(name) = block.get("name").and_then(Value::as_str) {
                    tools.push(name);
                }
            }
            _ => {}
        }
    }
    let text: String = text.chars().take(70).collect();
    match (tools.is_empty(), text.is_empty()) {
        (true, _) => text,
        (false, true) => tools.join(", "),
        (false, false) => format!("{} | {text}", tools.join(", ")),
    }
}

/// The steps after the newest compaction, in order.
pub fn steps(entries: &[&SessionEntry]) -> Vec<Step> {
    let mut steps: Vec<Step> = Vec::new();
    let mut open: Option<Step> = None;
    for (index, entry) in entries.iter().enumerate().skip(foldable_start(entries)) {
        match message_role(entry) {
            Some("assistant") => {
                steps.extend(open.take());
                open = Some(Step {
                    id: entry.id().to_owned(),
                    start: index,
                    end: index,
                    tokens: entry_tokens(entry),
                    preview: step_preview(entry),
                });
            }
            Some("toolResult") => {
                if let Some(step) = open.as_mut() {
                    step.end = index;
                    step.tokens += entry_tokens(entry);
                }
            }
            // Anything else the model reads closes the step: a user message,
            // a shell command the user ran, a notice.
            Some(_) => steps.extend(open.take()),
            None => {}
        }
    }
    steps.extend(open);
    steps
}

/// The steps that may be folded: not inside a fold, and not the newest one,
/// which is the step making the call and whose results are still to come.
pub fn foldable_steps(entries: &[&SessionEntry]) -> Vec<Step> {
    let folds = active_folds(entries);
    let mut all = steps(entries);
    all.pop();
    all.into_iter()
        .filter(|step| {
            !folds
                .iter()
                .any(|fold| fold.start <= step.start && step.end <= fold.end)
        })
        .collect()
}

/// Where the next fold has to begin: the oldest foldable step, or the fold
/// right before it, which a fold may extend. `None` when nothing is foldable.
fn next_start<'a>(
    folds: &'a [ActiveFold],
    foldable: &'a [Step],
) -> Option<(&'a Step, Option<&'a ActiveFold>)> {
    let first = foldable.first()?;
    let preceding = folds
        .iter()
        .filter(|fold| fold.end < first.start)
        .max_by_key(|fold| fold.end);
    Some((first, preceding))
}

/// The id of the step the next fold has to start at, and the id of the fold
/// it may extend instead.
pub fn next_fold_start(entries: &[&SessionEntry]) -> Option<(String, Option<String>)> {
    let folds = active_folds(entries);
    let foldable = foldable_steps(entries);
    next_start(&folds, &foldable)
        .map(|(step, fold)| (step.id.clone(), fold.map(|fold| fold.id.clone())))
}

/// A fold the model asked for, checked and ready to be recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedFold {
    pub record: FoldRecord,
}

/// Checks a fold request: `from` and `to` each name a foldable step or an
/// active fold, the range between them must cover whole steps and whole
/// folds only, and it starts where the next fold has to begin.
pub fn plan_fold(
    entries: &[&SessionEntry],
    from: &str,
    to: &str,
    summary: &str,
) -> Result<PlannedFold, String> {
    let summary = summary.trim();
    if summary.is_empty() {
        return Err(
            "summary must not be empty: it is all that remains of the folded steps".to_owned(),
        );
    }
    let folds = active_folds(entries);
    let foldable = foldable_steps(entries);
    let bounds = |id: &str| -> Option<(usize, usize)> {
        foldable
            .iter()
            .find(|step| step.id == id)
            .map(|step| (step.start, step.end))
            .or_else(|| {
                folds
                    .iter()
                    .find(|fold| fold.id == id)
                    .map(|fold| (fold.start, fold.end))
            })
    };
    let all_steps = steps(entries);
    let unknown = |id: &str| {
        let inside = all_steps
            .iter()
            .find(|step| step.id == id)
            .and_then(|step| {
                folds
                    .iter()
                    .find(|fold| fold.start <= step.start && step.end <= fold.end)
            });
        match inside {
            Some(fold) => format!(
                "{id} is already folded into {}; name the fold instead, or unfold it first",
                fold.id
            ),
            None => format!(
                "{id} is not a foldable step or an active fold; list them with action \"status\". The newest step, the one making this call, cannot be folded."
            ),
        }
    };
    let (start, _) = bounds(from).ok_or_else(|| unknown(from))?;
    let (_, end) = bounds(to).ok_or_else(|| unknown(to))?;
    if start > end {
        return Err(format!(
            "{from} comes after {to}; give the older step as from"
        ));
    }
    let Some((first, preceding)) = next_start(&folds, &foldable) else {
        return Err("no finished steps to fold yet".to_owned());
    };
    if start != first.start && preceding.is_none_or(|fold| fold.start != start) {
        let extend = preceding
            .map(|fold| format!(", or at fold {} to extend it", fold.id))
            .unwrap_or_default();
        return Err(format!(
            "folds go in order: start at {}, the oldest unfolded step{extend}. A fold further back would re-send everything after it without the prompt cache.",
            first.id
        ));
    }
    if let Some(fold) = folds.iter().find(|fold| {
        fold.start <= end && start <= fold.end && !(start <= fold.start && fold.end <= end)
    }) {
        return Err(format!(
            "the range cuts through fold {}; include all of it or none of it",
            fold.id
        ));
    }
    let covered: Vec<&Step> = all_steps
        .iter()
        .filter(|step| start <= step.start && step.end <= end)
        .collect();
    let inner_folds = folds
        .iter()
        .filter(|fold| start <= fold.start && fold.end <= end)
        .count();
    if covered.is_empty() && inner_folds == 0 {
        return Err("the range contains nothing to fold".to_owned());
    }
    Ok(PlannedFold {
        record: FoldRecord {
            first_entry_id: entries[start].id().to_owned(),
            last_entry_id: entries[end].id().to_owned(),
            summary: summary.to_owned(),
            steps: covered.len(),
            folded_tokens: entries[start..=end]
                .iter()
                .map(|entry| entry_tokens(entry))
                .sum(),
        },
    })
}

/// The context entry list with active folds in place of the ranges they
/// cover. The user's own messages inside a fold are kept, ahead of the
/// summary: a paraphrase of a request is not a request, the same reason
/// compaction keeps them. A reminder to fold that a later fold answered is
/// left out: it sits behind the start of that fold, where the prompt cache
/// was rewritten anyway, and kept it would pile up one per fold.
pub fn fold_context<'a>(
    entries: &[&'a SessionEntry],
    folds: &'a [ActiveFold],
) -> Vec<ContextPiece<'a>> {
    let answered_before = if folds.is_empty() {
        0
    } else {
        entries
            .iter()
            .rposition(|entry| custom_data(entry, FOLD_ENTRY_TYPE).is_some())
            .unwrap_or(0)
    };
    let mut pieces = Vec::with_capacity(entries.len());
    let mut index = 0;
    while index < entries.len() {
        if index < answered_before
            && matches!(entries[index], SessionEntry::CustomMessage(message) if message.custom_type == FOLD_REMINDER_TYPE)
        {
            index += 1;
            continue;
        }
        match folds.iter().find(|fold| fold.start == index) {
            Some(fold) => {
                for entry in &entries[fold.start..=fold.end] {
                    if message_role(entry) == Some("user") {
                        pieces.push(ContextPiece::Entry(entry));
                    }
                }
                pieces.push(ContextPiece::Fold(fold));
                index = fold.end + 1;
            }
            None => {
                pieces.push(ContextPiece::Entry(entries[index]));
                index += 1;
            }
        }
    }
    pieces
}

/// The message that stands in for a folded range.
pub fn fold_message(fold: &ActiveFold, timestamp: i64) -> AgentMessage {
    let header = format!(
        "[Folded context {}: {} step(s), about {} tokens of earlier work, replaced by the summary below. context_fold with action \"unfold\" and fold_id \"{}\" restores it; action \"search\" looks inside it.]",
        fold.id, fold.record.steps, fold.record.folded_tokens, fold.id
    );
    AgentMessage::Custom(notagent_agent::harness::messages::create_custom_message(
        FOLD_MESSAGE_TYPE,
        notagent_ai::types::UserContent::Text(format!("{header}\n\n{}", fold.record.summary)),
        false,
        Some(serde_json::json!({ "foldId": fold.id })),
        timestamp,
    ))
}

/// Text of a message, for search.
fn searchable_text(message: &AgentMessage) -> String {
    serde_json::to_value(message)
        .ok()
        .map(|value| {
            let mut out = String::new();
            collect_strings(&value, &mut out);
            out
        })
        .unwrap_or_default()
}

fn collect_strings(value: &Value, out: &mut String) {
    match value {
        Value::String(text) => {
            out.push_str(text);
            out.push('\n');
        }
        Value::Array(items) => items.iter().for_each(|item| collect_strings(item, out)),
        Value::Object(map) => map
            .iter()
            .filter(|(key, _)| !matches!(key.as_str(), "id" | "timestamp" | "toolCallId"))
            .for_each(|(_, item)| collect_strings(item, out)),
        _ => {}
    }
}

/// One place a search matched inside a fold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoldHit {
    pub fold_id: String,
    pub entry_id: String,
    pub snippet: String,
}

/// Case-insensitive search through the original entries of the active folds.
pub fn search_folds(entries: &[&SessionEntry], query: &str, limit: usize) -> Vec<FoldHit> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }
    let mut hits = Vec::new();
    for fold in active_folds(entries) {
        for entry in &entries[fold.start..=fold.end] {
            for message in session_entry_to_context_messages(entry) {
                let text = searchable_text(&message);
                let lower = text.to_lowercase();
                let mut from = 0;
                while let Some(found) = lower[from..].find(&needle) {
                    let at = from + found;
                    hits.push(FoldHit {
                        fold_id: fold.id.clone(),
                        entry_id: entry.id().to_owned(),
                        snippet: snippet(&text, at, needle.len()),
                    });
                    if hits.len() >= limit {
                        return hits;
                    }
                    from = at + needle.len();
                }
            }
        }
    }
    hits
}

/// About 80 characters either side of a match, on one line. `to_lowercase`
/// can change byte lengths, so the cut points are clamped to char boundaries
/// of the original text.
fn snippet(text: &str, at: usize, length: usize) -> String {
    let floor = |mut index: usize| {
        index = index.min(text.len());
        while !text.is_char_boundary(index) {
            index -= 1;
        }
        index
    };
    let start = floor(at.saturating_sub(80));
    let end = floor(at + length + 80);
    text[start..end]
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
