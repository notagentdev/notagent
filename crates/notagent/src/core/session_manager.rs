//! Port of `packages/coding-agent/src/core/session-manager.ts`.
//!
//! Sessions are append-only trees of JSONL entries. Entries carry `id`/`parentId`,
//! the `leaf` pointer marks the current position, and appending creates a child of
//! the leaf. Branching only moves the pointer; nothing is ever rewritten.
//!
//! Deviation (class 1): TS reads session files without validating them, so a legacy
//! or hand-edited entry survives unchanged. The Rust types keep that property by
//! defaulting every field and by falling back to [`SessionEntry::Unknown`], which
//! round-trips the raw JSON object. One difference remains: a v1 entry has no
//! `id`/`parentId` at all, and re-serializing it before the migration ran writes
//! both keys. Every rewrite path migrates first, so the written file is the same
//! either way.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use notagent_agent::harness::messages::{
    create_branch_summary_message, create_compaction_summary_message, create_custom_message,
};
use notagent_agent::types::AgentMessage;
use notagent_ai::types::{Usage, UserContent};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::config::{get_agent_dir, get_sessions_dir};
use crate::utils::paths::{normalize_path_default, resolve_path_default};

pub const CURRENT_SESSION_VERSION: u32 = 3;

// =============================================================================
// Entry types
// =============================================================================

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionHeader {
    /// v1 sessions have no version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub timestamp: String,
    #[serde(default)]
    pub cwd: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct NewSessionOptions {
    pub id: Option<String>,
    pub parent_session: Option<String>,
}

macro_rules! entry_struct {
    ($name:ident { $($(#[$meta:meta])* pub $field:ident : $ty:ty),* $(,)? }) => {
        #[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
        #[serde(rename_all = "camelCase")]
        pub struct $name {
            #[serde(default)]
            pub id: String,
            #[serde(default)]
            pub parent_id: Option<String>,
            #[serde(default)]
            pub timestamp: String,
            $($(#[$meta])* pub $field: $ty,)*
            #[serde(flatten)]
            pub extra: Map<String, Value>,
        }
    };
}

entry_struct!(SessionMessageEntry {
    /// The raw message; see the module note on unvalidated reads.
    #[serde(default)]
    pub message: Value,
});

entry_struct!(ThinkingLevelChangeEntry {
    #[serde(default)]
    pub thinking_level: String,
});

entry_struct!(ModelChangeEntry {
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub model_id: String,
});

entry_struct!(CompactionEntry {
    #[serde(default)]
    pub summary: String,
    /// The oldest entry the compaction kept.
    ///
    /// Read on its own by builds that predate `retained`, which then keep
    /// everything from it onward — a superset of the retained selection, so an
    /// older build degrades rather than breaks.
    #[serde(default)]
    pub first_kept_entry_id: String,
    /// The user messages carried through, and how much of each survived.
    ///
    /// Persisted rather than recomputed on load: the selection depends on a
    /// token estimate, and re-deriving it would let a changed estimator quietly
    /// rewrite the history of a session that was compacted months ago.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retained: Option<crate::core::compaction::retention::RetainedSelection>,
    #[serde(default)]
    pub tokens_before: i64,
    /// What the context measured once this compaction had been applied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_after: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_hook: Option<bool>,
});

entry_struct!(BranchSummaryEntry {
    #[serde(default)]
    pub from_id: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_hook: Option<bool>,
});

entry_struct!(CustomEntry {
    #[serde(default)]
    pub custom_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
});

entry_struct!(LabelEntry {
    #[serde(default)]
    pub target_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
});

entry_struct!(SessionInfoEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
});

entry_struct!(CustomMessageEntry {
    #[serde(default)]
    pub custom_type: String,
    #[serde(default)]
    pub content: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
    #[serde(default)]
    pub display: bool,
});

/// Session entry — carries `id`/`parentId` for the tree structure.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionEntry {
    Message(SessionMessageEntry),
    ThinkingLevelChange(ThinkingLevelChangeEntry),
    ModelChange(ModelChangeEntry),
    Compaction(CompactionEntry),
    BranchSummary(BranchSummaryEntry),
    Custom(CustomEntry),
    CustomMessage(CustomMessageEntry),
    Label(LabelEntry),
    SessionInfo(SessionInfoEntry),
    /// An entry this build does not know; kept verbatim.
    Unknown(Map<String, Value>),
}

macro_rules! dispatch_entry {
    ($self:expr, $entry:ident => $body:expr, $unknown:expr) => {
        match $self {
            SessionEntry::Message($entry) => $body,
            SessionEntry::ThinkingLevelChange($entry) => $body,
            SessionEntry::ModelChange($entry) => $body,
            SessionEntry::Compaction($entry) => $body,
            SessionEntry::BranchSummary($entry) => $body,
            SessionEntry::Custom($entry) => $body,
            SessionEntry::CustomMessage($entry) => $body,
            SessionEntry::Label($entry) => $body,
            SessionEntry::SessionInfo($entry) => $body,
            SessionEntry::Unknown(_) => $unknown,
        }
    };
}

impl SessionEntry {
    pub fn entry_type(&self) -> &str {
        match self {
            SessionEntry::Message(_) => "message",
            SessionEntry::ThinkingLevelChange(_) => "thinking_level_change",
            SessionEntry::ModelChange(_) => "model_change",
            SessionEntry::Compaction(_) => "compaction",
            SessionEntry::BranchSummary(_) => "branch_summary",
            SessionEntry::Custom(_) => "custom",
            SessionEntry::CustomMessage(_) => "custom_message",
            SessionEntry::Label(_) => "label",
            SessionEntry::SessionInfo(_) => "session_info",
            SessionEntry::Unknown(map) => {
                map.get("type").and_then(Value::as_str).unwrap_or_default()
            }
        }
    }

    pub fn id(&self) -> &str {
        dispatch_entry!(
            self,
            entry => &entry.id,
            match self {
                SessionEntry::Unknown(map) => map.get("id").and_then(Value::as_str).unwrap_or_default(),
                _ => unreachable!(),
            }
        )
    }

    pub fn parent_id(&self) -> Option<&str> {
        dispatch_entry!(
            self,
            entry => entry.parent_id.as_deref(),
            match self {
                SessionEntry::Unknown(map) => map.get("parentId").and_then(Value::as_str),
                _ => unreachable!(),
            }
        )
    }

    pub fn timestamp(&self) -> &str {
        dispatch_entry!(
            self,
            entry => &entry.timestamp,
            match self {
                SessionEntry::Unknown(map) => {
                    map.get("timestamp").and_then(Value::as_str).unwrap_or_default()
                }
                _ => unreachable!(),
            }
        )
    }

    fn set_id(&mut self, id: String) {
        dispatch_entry!(
            self,
            entry => entry.id = id,
            match self {
                SessionEntry::Unknown(map) => {
                    map.insert("id".to_owned(), Value::from(id));
                }
                _ => unreachable!(),
            }
        )
    }

    fn set_parent_id(&mut self, parent_id: Option<String>) {
        dispatch_entry!(
            self,
            entry => entry.parent_id = parent_id,
            match self {
                SessionEntry::Unknown(map) => {
                    map.insert("parentId".to_owned(), parent_id.map_or(Value::Null, Value::from));
                }
                _ => unreachable!(),
            }
        )
    }

    pub fn as_message(&self) -> Option<&SessionMessageEntry> {
        match self {
            SessionEntry::Message(entry) => Some(entry),
            _ => None,
        }
    }

    pub fn as_compaction(&self) -> Option<&CompactionEntry> {
        match self {
            SessionEntry::Compaction(entry) => Some(entry),
            _ => None,
        }
    }

    /// True when this entry holds an assistant message.
    fn is_assistant_message(&self) -> bool {
        self.as_message()
            .and_then(|entry| entry.message.get("role"))
            .and_then(Value::as_str)
            .is_some_and(|role| role == "assistant")
    }
}

impl Serialize for SessionEntry {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let value = match self {
            SessionEntry::Unknown(map) => Value::Object(map.clone()),
            entry => {
                let mut map = match dispatch_entry!(
                    entry,
                    entry => serde_json::to_value(entry),
                    unreachable!()
                ) {
                    Ok(Value::Object(map)) => map,
                    Ok(other) => return other.serialize(serializer),
                    Err(error) => return Err(serde::ser::Error::custom(error)),
                };
                // `type` comes first, as in the TS object literals.
                let mut ordered = Map::new();
                ordered.insert("type".to_owned(), Value::from(entry.entry_type()));
                ordered.append(&mut map);
                Value::Object(ordered)
            }
        };
        value.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for SessionEntry {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        Ok(SessionEntry::from_value(value))
    }
}

impl SessionEntry {
    /// Build an entry from raw JSON, keeping anything unrecognized verbatim.
    pub fn from_value(value: Value) -> Self {
        let Value::Object(map) = value else {
            return SessionEntry::Unknown(Map::new());
        };
        let entry_type = map
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let typed = Value::Object(map.clone());
        macro_rules! parse {
            ($variant:ident) => {
                match serde_json::from_value(typed) {
                    Ok(entry) => SessionEntry::$variant(entry),
                    Err(_) => SessionEntry::Unknown(map),
                }
            };
        }
        match entry_type.as_str() {
            "message" => parse!(Message),
            "thinking_level_change" => parse!(ThinkingLevelChange),
            "model_change" => parse!(ModelChange),
            "compaction" => parse!(Compaction),
            "branch_summary" => parse!(BranchSummary),
            "custom" => parse!(Custom),
            "custom_message" => parse!(CustomMessage),
            "label" => parse!(Label),
            "session_info" => parse!(SessionInfo),
            _ => SessionEntry::Unknown(map),
        }
    }
}

/// Raw file entry (includes the header).
#[derive(Debug, Clone, PartialEq)]
pub enum FileEntry {
    Session(SessionHeader),
    Entry(SessionEntry),
}

impl FileEntry {
    pub fn from_value(value: Value) -> Self {
        let is_header = value.get("type").and_then(Value::as_str) == Some("session");
        if is_header {
            match serde_json::from_value::<SessionHeader>(value.clone()) {
                Ok(header) => return FileEntry::Session(header),
                Err(_) => return FileEntry::Entry(SessionEntry::from_value(value)),
            }
        }
        FileEntry::Entry(SessionEntry::from_value(value))
    }

    pub fn as_header(&self) -> Option<&SessionHeader> {
        match self {
            FileEntry::Session(header) => Some(header),
            FileEntry::Entry(_) => None,
        }
    }

    pub fn as_entry(&self) -> Option<&SessionEntry> {
        match self {
            FileEntry::Entry(entry) => Some(entry),
            FileEntry::Session(_) => None,
        }
    }

    pub fn to_json(&self) -> String {
        match self {
            FileEntry::Session(header) => {
                let mut ordered = Map::new();
                ordered.insert("type".to_owned(), Value::from("session"));
                if let Ok(Value::Object(mut map)) = serde_json::to_value(header) {
                    ordered.append(&mut map);
                }
                Value::Object(ordered).to_string()
            }
            FileEntry::Entry(entry) => serde_json::to_value(entry)
                .unwrap_or(Value::Null)
                .to_string(),
        }
    }
}

/// Tree node for [`SessionManager::get_tree`].
#[derive(Debug, Clone, PartialEq)]
pub struct SessionTreeNode {
    pub entry: SessionEntry,
    pub children: Vec<SessionTreeNode>,
    pub label: Option<String>,
    pub label_timestamp: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionModel {
    pub provider: String,
    pub model_id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionContext {
    pub messages: Vec<AgentMessage>,
    pub thinking_level: String,
    pub model: Option<SessionModel>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionInfo {
    pub path: String,
    pub id: String,
    /// Working directory where the session was started; empty for old sessions.
    pub cwd: String,
    pub name: Option<String>,
    pub parent_session_path: Option<String>,
    /// Milliseconds since the epoch.
    pub created: i64,
    pub modified: i64,
    pub message_count: usize,
    pub first_message: String,
    pub all_messages_text: String,
}

// =============================================================================
// Ids, timestamps, migrations
// =============================================================================

fn create_session_id() -> String {
    notagent_ai::uuidv7()
}

pub fn assert_valid_session_id(id: &str) -> Result<(), String> {
    let valid = {
        let bytes = id.as_bytes();
        let alphanumeric = |byte: u8| byte.is_ascii_alphanumeric();
        let inner = |byte: u8| alphanumeric(byte) || matches!(byte, b'.' | b'_' | b'-');
        match bytes {
            [] => false,
            [single] => alphanumeric(*single),
            [first, middle @ .., last] => {
                alphanumeric(*first)
                    && alphanumeric(*last)
                    && middle.iter().all(|byte| inner(*byte))
            }
        }
    };
    if valid {
        return Ok(());
    }
    Err("Session id must be non-empty, contain only alphanumeric characters, '-', '_', and '.', and start and end with an alphanumeric character".to_owned())
}

/// Generate a unique short id (8 hex chars, collision-checked).
fn generate_id(taken: &dyn Fn(&str) -> bool) -> String {
    for _ in 0..100 {
        let id = uuid::Uuid::new_v4().to_string()[..8].to_owned();
        if !taken(&id) {
            return id;
        }
    }
    // Fall back to a full UUID if we somehow keep colliding.
    uuid::Uuid::new_v4().to_string()
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// `new Date(value).getTime()`; `None` stands for `NaN`.
fn parse_timestamp_ms(value: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|time| time.timestamp_millis())
}

/// Migrate v1 → v2: add the id/parentId tree structure.
fn migrate_v1_to_v2(entries: &mut [FileEntry]) {
    let mut ids: HashSet<String> = HashSet::new();
    let mut previous_id: Option<String> = None;
    // The compaction migration resolves `firstKeptEntryIndex` against the final ids.
    let mut assigned: Vec<Option<String>> = vec![None; entries.len()];

    for (index, entry) in entries.iter_mut().enumerate() {
        match entry {
            FileEntry::Session(header) => {
                header.version = Some(2);
            }
            FileEntry::Entry(entry) => {
                let id = generate_id(&|id| ids.contains(id));
                ids.insert(id.clone());
                entry.set_id(id.clone());
                entry.set_parent_id(previous_id.clone());
                previous_id = Some(id.clone());
                assigned[index] = Some(id);
            }
        }
    }

    for (index, entry) in entries.iter_mut().enumerate() {
        let FileEntry::Entry(SessionEntry::Compaction(entry)) = entry else {
            continue;
        };
        let _ = index;
        let Some(target_index) = entry
            .extra
            .get("firstKeptEntryIndex")
            .and_then(Value::as_i64)
            .and_then(|index| usize::try_from(index).ok())
        else {
            continue;
        };
        entry.extra.shift_remove("firstKeptEntryIndex");
        if let Some(Some(target_id)) = assigned.get(target_index).cloned() {
            entry.first_kept_entry_id = target_id;
        }
    }
}

/// Migrate v2 → v3: rename the `hookMessage` role to `custom`.
fn migrate_v2_to_v3(entries: &mut [FileEntry]) {
    for entry in entries {
        match entry {
            FileEntry::Session(header) => header.version = Some(3),
            FileEntry::Entry(SessionEntry::Message(entry)) => {
                if entry.message.get("role").and_then(Value::as_str) == Some("hookMessage")
                    && let Some(message) = entry.message.as_object_mut()
                {
                    message.insert("role".to_owned(), Value::from("custom"));
                }
            }
            FileEntry::Entry(_) => {}
        }
    }
}

/// Run all migrations needed to reach the current version. Returns true if one ran.
fn migrate_to_current_version(entries: &mut [FileEntry]) -> bool {
    let version = entries
        .iter()
        .find_map(FileEntry::as_header)
        .and_then(|header| header.version)
        .unwrap_or(1);
    if version >= CURRENT_SESSION_VERSION {
        return false;
    }
    if version < 2 {
        migrate_v1_to_v2(entries);
    }
    if version < 3 {
        migrate_v2_to_v3(entries);
    }
    true
}

/// Exported for testing.
pub fn migrate_session_entries(entries: &mut [FileEntry]) {
    migrate_to_current_version(entries);
}

pub fn parse_session_entries(content: &str) -> Vec<FileEntry> {
    content
        .trim()
        .split('\n')
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .map(FileEntry::from_value)
        .collect()
}

pub fn get_latest_compaction_entry(entries: &[SessionEntry]) -> Option<&CompactionEntry> {
    entries.iter().rev().find_map(SessionEntry::as_compaction)
}

type EntryIndex<'a> = HashMap<&'a str, &'a SessionEntry>;

fn build_entry_index(entries: &[SessionEntry]) -> EntryIndex<'_> {
    entries.iter().map(|entry| (entry.id(), entry)).collect()
}

/// `leaf_id`: `Some(None)` is TS's explicit `null` (empty path), `None` is `undefined`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeafSelector<'a> {
    Undefined,
    Null,
    Id(&'a str),
}

impl<'a> From<Option<&'a str>> for LeafSelector<'a> {
    fn from(value: Option<&'a str>) -> Self {
        match value {
            Some(id) => LeafSelector::Id(id),
            None => LeafSelector::Null,
        }
    }
}

fn build_session_path<'a>(
    entries: &'a [SessionEntry],
    leaf: LeafSelector<'_>,
    index: &EntryIndex<'a>,
) -> Vec<&'a SessionEntry> {
    let leaf_entry = match leaf {
        LeafSelector::Null => return Vec::new(),
        LeafSelector::Id(id) => index.get(id).copied().or_else(|| entries.last()),
        LeafSelector::Undefined => entries.last(),
    };
    let Some(leaf_entry) = leaf_entry else {
        return Vec::new();
    };

    let mut path: Vec<&SessionEntry> = Vec::new();
    let mut current = Some(leaf_entry);
    while let Some(entry) = current {
        path.push(entry);
        current = entry.parent_id().and_then(|id| index.get(id).copied());
    }
    path.reverse();
    path
}

fn get_session_context_settings(path: &[&SessionEntry]) -> (String, Option<SessionModel>) {
    let mut thinking_level = "off".to_owned();
    let mut model: Option<SessionModel> = None;
    for entry in path {
        match entry {
            SessionEntry::ThinkingLevelChange(entry) => {
                thinking_level = entry.thinking_level.clone();
            }
            SessionEntry::ModelChange(entry) => {
                model = Some(SessionModel {
                    provider: entry.provider.clone(),
                    model_id: entry.model_id.clone(),
                });
            }
            SessionEntry::Message(entry)
                if entry.message.get("role").and_then(Value::as_str) == Some("assistant") =>
            {
                model = Some(SessionModel {
                    provider: entry
                        .message
                        .get("provider")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    model_id: entry
                        .message
                        .get("model")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                });
            }
            _ => {}
        }
    }
    (thinking_level, model)
}

/// Project one selected session entry into LLM/runtime messages.
///
/// Plain custom entries are display/state entries and do not participate in context.
pub fn session_entry_to_context_messages(entry: &SessionEntry) -> Vec<AgentMessage> {
    match entry {
        SessionEntry::Message(entry) => {
            let mut message = entry.message.clone();
            // Session files are parsed without validation; old versions, forks or
            // hand-edited files can contain messages with null/missing content.
            if let Some(object) = message.as_object_mut() {
                let role = object
                    .get("role")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if matches!(role, "user" | "assistant" | "toolResult")
                    && object.get("content").is_none_or(Value::is_null)
                {
                    object.insert("content".to_owned(), json!([]));
                }
            }
            // A message that cannot be read as an `AgentMessage` carries no usable
            // content for the agent, so it is left out of the context.
            serde_json::from_value(message).ok().into_iter().collect()
        }
        SessionEntry::CustomMessage(entry) => {
            let content: UserContent = serde_json::from_value(entry.content.clone())
                .unwrap_or_else(|_| UserContent::Blocks(Vec::new()));
            vec![AgentMessage::Custom(create_custom_message(
                entry.custom_type.clone(),
                content,
                entry.display,
                entry.details.clone(),
                parse_timestamp_ms(&entry.timestamp).unwrap_or(0),
            ))]
        }
        SessionEntry::BranchSummary(entry) if !entry.summary.is_empty() => {
            vec![AgentMessage::BranchSummary(create_branch_summary_message(
                entry.summary.clone(),
                entry.from_id.clone(),
                parse_timestamp_ms(&entry.timestamp).unwrap_or(0),
            ))]
        }
        SessionEntry::Compaction(entry) => {
            vec![AgentMessage::CompactionSummary(
                create_compaction_summary_message(
                    entry.summary.clone(),
                    entry.tokens_before.max(0) as u64,
                    entry.tokens_after.map(|tokens| tokens.max(0) as u64),
                    parse_timestamp_ms(&entry.timestamp).unwrap_or(0),
                ),
            )]
        }
        _ => Vec::new(),
    }
}

/// Build the active, compaction-aware session entry list.
pub fn build_context_entries<'a>(
    entries: &'a [SessionEntry],
    leaf: LeafSelector<'_>,
) -> Vec<&'a SessionEntry> {
    let index = build_entry_index(entries);
    build_context_entries_indexed(entries, leaf, &index)
}

fn build_context_entries_indexed<'a>(
    entries: &'a [SessionEntry],
    leaf: LeafSelector<'_>,
    index: &EntryIndex<'a>,
) -> Vec<&'a SessionEntry> {
    let path = build_session_path(entries, leaf, index);
    let Some(compaction_id) = path
        .iter()
        .rev()
        .find(|entry| matches!(entry, SessionEntry::Compaction(_)))
        .map(|entry| entry.id().to_owned())
    else {
        return path;
    };
    let Some(compaction_index) = path.iter().position(|entry| entry.id() == compaction_id) else {
        return path;
    };
    let compaction = path[compaction_index].as_compaction();

    // Entries the compaction carried through, in order, followed by the
    // compaction itself and then everything that has happened since.
    //
    // The summary going last is the point of the ordering: it is the newest
    // thing said about the older history, so it reads as a conclusion rather
    // than as a preamble the retained messages then appear to contradict. Only
    // the *context* is ordered this way — the transcript is rendered from the
    // entry tree and stays chronological.
    let Some(retained) = compaction.and_then(|entry| entry.retained.as_ref()) else {
        // Written before the retained selection existed. Keep the suffix from
        // `first_kept_entry_id` with the summary ahead of it, which is what
        // that field meant and what the messages behind it were chosen for.
        let first_kept_entry_id = compaction
            .map(|entry| entry.first_kept_entry_id.clone())
            .unwrap_or_default();
        let mut context_entries: Vec<&SessionEntry> = vec![path[compaction_index]];
        let mut found_first_kept = false;
        for entry in path.iter().take(compaction_index) {
            if entry.id() == first_kept_entry_id {
                found_first_kept = true;
            }
            if found_first_kept {
                context_entries.push(entry);
            }
        }
        context_entries.extend_from_slice(&path[compaction_index + 1..]);
        return context_entries;
    };

    let mut context_entries: Vec<&SessionEntry> = path
        .iter()
        .take(compaction_index)
        .filter(|entry| retained.find(entry.id()).is_some())
        .copied()
        .collect();
    context_entries.push(path[compaction_index]);
    context_entries.extend_from_slice(&path[compaction_index + 1..]);
    context_entries
}

/// Build the session context from entries using tree traversal.
pub fn build_session_context(entries: &[SessionEntry], leaf: LeafSelector<'_>) -> SessionContext {
    let index = build_entry_index(entries);
    let path = build_session_path(entries, leaf, &index);
    let (thinking_level, model) = get_session_context_settings(&path);
    let context_entries = build_context_entries_indexed(entries, leaf, &index);

    // A retained message may have been truncated to fit the compaction's
    // budget. The entry keeps the whole message — the session file is the
    // record — so the cut is applied here, on the way into the context.
    let retained = context_entries
        .iter()
        .rev()
        .find_map(|entry| entry.as_compaction())
        .and_then(|entry| entry.retained.as_ref());

    let mut messages: Vec<AgentMessage> = Vec::new();
    for entry in context_entries {
        let entry_messages = session_entry_to_context_messages(entry);
        match retained.and_then(|retained| retained.find(entry.id())) {
            Some(record) => messages.extend(entry_messages.iter().map(|message| {
                crate::core::compaction::retention::apply_retention(message, record)
            })),
            None => messages.extend(entry_messages),
        }
        if let Some(retained) = retained
            && retained.elision_after() == Some(entry.id())
        {
            let timestamp = parse_timestamp_ms(entry.timestamp()).unwrap_or(0);
            messages.push(crate::core::compaction::retention::elision_message(
                retained.omitted_tokens,
                timestamp,
            ));
        }
    }

    SessionContext {
        messages,
        thinking_level,
        model,
    }
}

// =============================================================================
// Session directories and files
// =============================================================================

fn resolve_or_raw(path: &str) -> String {
    let cwd = std::env::current_dir()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    resolve_path_default(path, &cwd).unwrap_or_else(|_| path.to_owned())
}

fn normalize_or_raw(path: &str) -> String {
    normalize_path_default(path).unwrap_or_else(|_| path.to_owned())
}

/// Compute the default session directory for a cwd.
fn get_default_session_dir_path(cwd: &str, agent_dir: Option<&str>) -> String {
    let resolved_cwd = resolve_or_raw(cwd);
    let resolved_agent_dir = resolve_or_raw(
        &agent_dir
            .map(str::to_owned)
            .unwrap_or_else(|| get_agent_dir().to_string_lossy().into_owned()),
    );
    let stripped = resolved_cwd
        .strip_prefix(['/', '\\'])
        .unwrap_or(&resolved_cwd);
    let safe_path = format!("--{}--", stripped.replace(['/', '\\', ':'], "-"));
    Path::new(&resolved_agent_dir)
        .join("sessions")
        .join(safe_path)
        .to_string_lossy()
        .into_owned()
}

pub fn get_default_session_dir(cwd: &str, agent_dir: Option<&str>) -> String {
    let session_dir = get_default_session_dir_path(cwd, agent_dir);
    if !Path::new(&session_dir).exists() {
        let _ = std::fs::create_dir_all(&session_dir);
    }
    session_dir
}

const SESSION_READ_BUFFER_SIZE: usize = 1024 * 1024;
const SESSION_HEADER_READ_BUFFER_SIZE: usize = 4096;
/// Bound synchronous header discovery while allowing large cwd and metadata fields.
const MAX_SESSION_HEADER_SCAN_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SessionFileError {
    #[error("Session header exceeds {MAX_SESSION_HEADER_SCAN_BYTES}-byte scan limit: {0}")]
    HeaderScanLimit(String),
    #[error("{0}")]
    Io(String),
}

fn parse_session_entry_line(line: &str) -> Option<FileEntry> {
    if line.trim().is_empty() {
        return None;
    }
    serde_json::from_str::<Value>(line)
        .ok()
        .map(FileEntry::from_value)
}

/// Exported for testing.
pub fn load_entries_from_file(file_path: &str) -> Vec<FileEntry> {
    let resolved = normalize_or_raw(file_path);
    if !Path::new(&resolved).exists() {
        return Vec::new();
    }
    let Ok(mut file) = std::fs::File::open(&resolved) else {
        return Vec::new();
    };

    let mut entries: Vec<FileEntry> = Vec::new();
    let mut buffer = vec![0u8; SESSION_READ_BUFFER_SIZE];
    let mut pending: Vec<u8> = Vec::new();
    while let Ok(bytes_read) = file.read(&mut buffer) {
        if bytes_read == 0 {
            break;
        }
        pending.extend_from_slice(&buffer[..bytes_read]);
        let mut line_start = 0;
        while let Some(offset) = pending[line_start..].iter().position(|byte| *byte == b'\n') {
            let line = String::from_utf8_lossy(&pending[line_start..line_start + offset]);
            if let Some(entry) = parse_session_entry_line(&line) {
                entries.push(entry);
            }
            line_start += offset + 1;
        }
        pending.drain(..line_start);
    }
    let line = String::from_utf8_lossy(&pending);
    if let Some(entry) = parse_session_entry_line(&line) {
        entries.push(entry);
    }

    // Validate the session header.
    match entries.first() {
        None => entries,
        Some(FileEntry::Session(header)) if !header.id.is_empty() => entries,
        Some(_) => Vec::new(),
    }
}

/// Inspect a physical line while searching for the first parsed session entry.
///
/// `None` keeps scanning, `Some(None)` is a parsed non-header entry, `Some(Some(_))`
/// is the header.
#[allow(clippy::option_option)]
fn parse_session_header_candidate(line: &str) -> Option<Option<SessionHeader>> {
    if line.trim().is_empty() {
        return None;
    }
    let entry = parse_session_entry_line(line)?;
    match entry {
        FileEntry::Session(header) if !header.id.is_empty() => Some(Some(header)),
        _ => Some(None),
    }
}

fn read_session_header(file_path: &str) -> Result<Option<SessionHeader>, SessionFileError> {
    let mut file =
        std::fs::File::open(file_path).map_err(|error| SessionFileError::Io(error.to_string()))?;
    let mut buffer = vec![0u8; SESSION_HEADER_READ_BUFFER_SIZE];
    let mut line_chunks: Vec<u8> = Vec::new();
    let mut scanned_bytes = 0usize;

    while scanned_bytes < MAX_SESSION_HEADER_SCAN_BYTES {
        let read_length = buffer
            .len()
            .min(MAX_SESSION_HEADER_SCAN_BYTES - scanned_bytes);
        let bytes_read = file
            .read(&mut buffer[..read_length])
            .map_err(|error| SessionFileError::Io(error.to_string()))?;
        if bytes_read == 0 {
            let line = String::from_utf8_lossy(&line_chunks);
            return Ok(parse_session_header_candidate(&line).unwrap_or(None));
        }
        scanned_bytes += bytes_read;

        let chunk = &buffer[..bytes_read];
        let mut line_start = 0;
        while let Some(offset) = chunk[line_start..].iter().position(|byte| *byte == b'\n') {
            line_chunks.extend_from_slice(&chunk[line_start..line_start + offset]);
            let line = String::from_utf8_lossy(&line_chunks);
            if let Some(header) = parse_session_header_candidate(&line) {
                return Ok(header);
            }
            line_chunks.clear();
            line_start += offset + 1;
        }
        line_chunks.extend_from_slice(&chunk[line_start..]);
    }

    // Probe for EOF so a final header without a newline is allowed when it ends
    // exactly at the scan limit. Any additional byte exceeds the bounded scan.
    let mut probe = [0u8; 1];
    if file
        .read(&mut probe)
        .map_err(|error| SessionFileError::Io(error.to_string()))?
        == 0
    {
        let line = String::from_utf8_lossy(&line_chunks);
        return Ok(parse_session_header_candidate(&line).unwrap_or(None));
    }
    Err(SessionFileError::HeaderScanLimit(file_path.to_owned()))
}

fn read_session_header_for_discovery(file_path: &str) -> Option<SessionHeader> {
    // Discovery is best-effort: unreadable or oversized files are not sessions,
    // and one corrupt file must not hide the others.
    read_session_header(file_path).ok().flatten()
}

fn get_session_header_cwd(header: &SessionHeader) -> Option<&str> {
    header.cwd.as_str()
}

fn session_cwd_matches(cwd: Option<&str>, resolved_cwd: &str) -> bool {
    cwd.is_some_and(|cwd| !cwd.is_empty() && resolve_or_raw(cwd) == resolved_cwd)
}

/// Exported for testing.
pub fn find_most_recent_session(session_dir: &str, cwd: Option<&str>) -> Option<String> {
    let resolved_session_dir = normalize_or_raw(session_dir);
    let resolved_cwd = cwd.map(resolve_or_raw);
    let mut files: Vec<(String, std::time::SystemTime)> = std::fs::read_dir(&resolved_session_dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "jsonl")
        })
        .filter_map(|path| {
            let path = path.to_string_lossy().into_owned();
            let header = read_session_header_for_discovery(&path)?;
            let matches = resolved_cwd.as_ref().is_none_or(|resolved_cwd| {
                session_cwd_matches(get_session_header_cwd(&header), resolved_cwd)
            });
            if !matches {
                return None;
            }
            let modified = std::fs::metadata(&path).ok()?.modified().ok()?;
            Some((path, modified))
        })
        .collect();
    files.sort_by_key(|(_, modified)| std::cmp::Reverse(*modified));
    files.into_iter().next().map(|(path, _)| path)
}

// =============================================================================
// Session info and listing
// =============================================================================

/// `(loaded, total)`
pub type SessionListProgress<'a> = &'a (dyn Fn(usize, usize) + Send + Sync);

const MAX_CONCURRENT_SESSION_INFO_LOADS: usize = 10;

fn is_message_with_content(message: &Value) -> bool {
    message.get("role").is_some_and(Value::is_string) && message.get("content").is_some()
}

fn extract_text_content(message: &Value) -> String {
    match message.get("content") {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    }
}

fn get_message_activity_time(entry: &SessionMessageEntry) -> Option<i64> {
    let message = &entry.message;
    if !is_message_with_content(message) {
        return None;
    }
    let role = message
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if role != "user" && role != "assistant" {
        return None;
    }
    if let Some(timestamp) = message.get("timestamp").and_then(Value::as_i64) {
        return Some(timestamp);
    }
    parse_timestamp_ms(&entry.timestamp)
}

async fn build_session_info(file_path: &str) -> Option<SessionInfo> {
    use tokio::io::AsyncBufReadExt;

    let metadata = tokio::fs::metadata(file_path).await.ok()?;
    let file = tokio::fs::File::open(file_path).await.ok()?;
    let mut lines = tokio::io::BufReader::new(file).lines();

    let mut header: Option<SessionHeader> = None;
    let mut message_count = 0usize;
    let mut first_message = String::new();
    let mut all_messages: Vec<String> = Vec::new();
    let mut name: Option<String> = None;
    let mut last_activity_time: Option<i64> = None;

    while let Ok(Some(line)) = lines.next_line().await {
        let Some(entry) = parse_session_entry_line(&line) else {
            continue;
        };
        if header.is_none() {
            match entry {
                FileEntry::Session(parsed) => {
                    header = Some(parsed);
                    continue;
                }
                FileEntry::Entry(_) => return None,
            }
        }

        let FileEntry::Entry(entry) = entry else {
            continue;
        };
        // The latest session_info wins, including an explicit clear.
        if let SessionEntry::SessionInfo(entry) = &entry {
            name = entry
                .name
                .as_ref()
                .map(|name| name.trim().to_owned())
                .filter(|name| !name.is_empty());
        }
        let SessionEntry::Message(entry) = &entry else {
            continue;
        };
        message_count += 1;

        if let Some(activity_time) = get_message_activity_time(entry) {
            last_activity_time = Some(last_activity_time.unwrap_or(0).max(activity_time));
        }

        let message = &entry.message;
        if !is_message_with_content(message) {
            continue;
        }
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if role != "user" && role != "assistant" {
            continue;
        }
        let text_content = extract_text_content(message);
        if text_content.is_empty() {
            continue;
        }
        if first_message.is_empty() && role == "user" {
            first_message = text_content.clone();
        }
        all_messages.push(text_content);
    }

    let header = header?;
    let cwd = header.cwd.as_str().unwrap_or_default().to_owned();
    let header_time = parse_timestamp_ms(&header.timestamp);
    let modified = match last_activity_time {
        Some(time) if time > 0 => time,
        _ => header_time.unwrap_or_else(|| system_time_ms(metadata.modified().ok())),
    };

    Some(SessionInfo {
        path: file_path.to_owned(),
        id: header.id,
        cwd,
        name,
        parent_session_path: header.parent_session,
        created: header_time.unwrap_or(0),
        modified,
        message_count,
        first_message: if first_message.is_empty() {
            "(no messages)".to_owned()
        } else {
            first_message
        },
        all_messages_text: all_messages.join(" "),
    })
}

fn system_time_ms(time: Option<std::time::SystemTime>) -> i64 {
    time.and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

async fn build_session_infos_with_concurrency(
    files: &[String],
    on_loaded: &(dyn Fn() + Send + Sync),
) -> Vec<Option<SessionInfo>> {
    use futures::StreamExt;

    futures::stream::iter(files.iter().map(|file| async move {
        let info = build_session_info(file).await;
        on_loaded();
        info
    }))
    .buffered(MAX_CONCURRENT_SESSION_INFO_LOADS)
    .collect()
    .await
}

async fn list_sessions_from_dir(
    dir: &str,
    on_progress: Option<SessionListProgress<'_>>,
    progress_offset: usize,
    progress_total: Option<usize>,
) -> Vec<SessionInfo> {
    if !Path::new(dir).exists() {
        return Vec::new();
    }
    let Ok(mut dir_entries) = tokio::fs::read_dir(dir).await else {
        return Vec::new();
    };
    let mut files: Vec<String> = Vec::new();
    while let Ok(Some(entry)) = dir_entries.next_entry().await {
        let path = entry.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "jsonl")
        {
            files.push(path.to_string_lossy().into_owned());
        }
    }
    let total = progress_total.unwrap_or(files.len());
    let loaded = std::sync::atomic::AtomicUsize::new(0);
    let results = build_session_infos_with_concurrency(&files, &|| {
        let loaded = loaded.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        if let Some(on_progress) = on_progress {
            on_progress(progress_offset + loaded, total);
        }
    })
    .await;
    results.into_iter().flatten().collect()
}

fn sort_sessions_by_modified(sessions: &mut [SessionInfo]) {
    // JS `Array.sort` is stable, so equal timestamps keep their order.
    sessions.sort_by_key(|session| std::cmp::Reverse(session.modified));
}

// =============================================================================
// SessionManager
// =============================================================================

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SessionManagerError {
    #[error("Entry {0} not found")]
    EntryNotFound(String),
    #[error("Session file is not a valid notagent session: {0}")]
    InvalidSessionFile(String),
    #[error("{0}")]
    InvalidSessionId(String),
    #[error("Cannot fork: source session file is empty or invalid: {0}")]
    ForkSourceEmpty(String),
    #[error("Cannot fork: source session has no header: {0}")]
    ForkSourceWithoutHeader(String),
    #[error("{0}")]
    Io(String),
}

fn io_error(error: std::io::Error) -> SessionManagerError {
    SessionManagerError::Io(error.to_string())
}

/// Manages conversation sessions as append-only trees stored in JSONL files.
#[derive(Debug)]
pub struct SessionManager {
    session_id: String,
    session_file: Option<String>,
    session_dir: String,
    cwd: String,
    persist: bool,
    flushed: bool,
    file_entries: Vec<FileEntry>,
    ids: HashSet<String>,
    labels_by_id: BTreeMap<String, String>,
    label_timestamps_by_id: BTreeMap<String, String>,
    leaf_id: Option<String>,
}

use std::collections::BTreeMap;

impl SessionManager {
    fn construct(
        cwd: &str,
        session_dir: &str,
        session_file: Option<&str>,
        persist: bool,
        new_session_options: Option<NewSessionOptions>,
        preloaded_file_entries: Option<Vec<FileEntry>>,
    ) -> Result<Self, SessionManagerError> {
        let mut manager = SessionManager {
            session_id: String::new(),
            session_file: None,
            session_dir: normalize_or_raw(session_dir),
            cwd: resolve_or_raw(cwd),
            persist,
            flushed: false,
            file_entries: Vec::new(),
            ids: HashSet::new(),
            labels_by_id: BTreeMap::new(),
            label_timestamps_by_id: BTreeMap::new(),
            leaf_id: None,
        };
        if persist && !manager.session_dir.is_empty() && !Path::new(&manager.session_dir).exists() {
            std::fs::create_dir_all(&manager.session_dir).map_err(io_error)?;
        }
        match session_file {
            Some(session_file) => {
                manager.set_session_file_inner(session_file, preloaded_file_entries)?
            }
            None => {
                manager.new_session(new_session_options)?;
            }
        }
        Ok(manager)
    }

    /// Switch to a different session file (used for resume and branching).
    pub fn set_session_file(&mut self, session_file: &str) -> Result<(), SessionManagerError> {
        self.set_session_file_inner(session_file, None)
    }

    fn set_session_file_inner(
        &mut self,
        session_file: &str,
        preloaded_file_entries: Option<Vec<FileEntry>>,
    ) -> Result<(), SessionManagerError> {
        let resolved = resolve_or_raw(session_file);
        self.session_file = Some(resolved.clone());
        if !Path::new(&resolved).exists() {
            // Preserve the explicit path from the --session flag.
            self.new_session(None)?;
            self.session_file = Some(resolved);
            return Ok(());
        }

        self.file_entries =
            preloaded_file_entries.unwrap_or_else(|| load_entries_from_file(&resolved));

        // An empty file is initialized with a valid header; a non-empty file that
        // does not parse as a notagent session is left untouched.
        if self.file_entries.is_empty() {
            let size = std::fs::metadata(&resolved).map_err(io_error)?.len();
            if size > 0 {
                return Err(SessionManagerError::InvalidSessionFile(resolved));
            }
            self.new_session(None)?;
            self.session_file = Some(resolved);
            self.rewrite_file()?;
            self.flushed = true;
            return Ok(());
        }

        self.session_id = self
            .file_entries
            .iter()
            .find_map(FileEntry::as_header)
            .map(|header| header.id.clone())
            .unwrap_or_else(create_session_id);

        if migrate_to_current_version(&mut self.file_entries) {
            self.rewrite_file()?;
        }
        self.build_index();
        self.flushed = true;
        Ok(())
    }

    pub fn new_session(
        &mut self,
        options: Option<NewSessionOptions>,
    ) -> Result<Option<String>, SessionManagerError> {
        let options = options.unwrap_or_default();
        if let Some(id) = &options.id {
            assert_valid_session_id(id).map_err(SessionManagerError::InvalidSessionId)?;
        }
        self.session_id = options.id.unwrap_or_else(create_session_id);
        let timestamp = now_iso();
        let header = SessionHeader {
            version: Some(CURRENT_SESSION_VERSION),
            id: self.session_id.clone(),
            timestamp: timestamp.clone(),
            cwd: Value::from(self.cwd.clone()),
            parent_session: options.parent_session,
            extra: Map::new(),
        };
        self.file_entries = vec![FileEntry::Session(header)];
        self.ids.clear();
        self.labels_by_id.clear();
        self.label_timestamps_by_id.clear();
        self.leaf_id = None;
        self.flushed = false;

        if self.persist {
            let file_timestamp = timestamp.replace([':', '.'], "-");
            self.session_file = Some(
                Path::new(&self.session_dir)
                    .join(format!("{file_timestamp}_{}.jsonl", self.session_id))
                    .to_string_lossy()
                    .into_owned(),
            );
        }
        Ok(self.session_file.clone())
    }

    fn build_index(&mut self) {
        self.ids.clear();
        self.labels_by_id.clear();
        self.label_timestamps_by_id.clear();
        self.leaf_id = None;
        for entry in &self.file_entries {
            let FileEntry::Entry(entry) = entry else {
                continue;
            };
            self.ids.insert(entry.id().to_owned());
            self.leaf_id = Some(entry.id().to_owned());
            if let SessionEntry::Label(label) = entry {
                match label.label.as_ref().filter(|label| !label.is_empty()) {
                    Some(value) => {
                        self.labels_by_id
                            .insert(label.target_id.clone(), value.clone());
                        self.label_timestamps_by_id
                            .insert(label.target_id.clone(), label.timestamp.clone());
                    }
                    None => {
                        self.labels_by_id.remove(&label.target_id);
                        self.label_timestamps_by_id.remove(&label.target_id);
                    }
                }
            }
        }
    }

    fn rewrite_file(&self) -> Result<(), SessionManagerError> {
        let (true, Some(session_file)) = (self.persist, &self.session_file) else {
            return Ok(());
        };
        let mut file = std::fs::File::create(session_file).map_err(io_error)?;
        for entry in &self.file_entries {
            writeln!(file, "{}", entry.to_json()).map_err(io_error)?;
        }
        Ok(())
    }

    pub fn is_persisted(&self) -> bool {
        self.persist
    }

    pub fn get_cwd(&self) -> &str {
        &self.cwd
    }

    pub fn get_session_dir(&self) -> &str {
        &self.session_dir
    }

    pub fn uses_default_session_dir(&self) -> bool {
        self.session_dir == get_default_session_dir_path(&self.cwd, None)
    }

    pub fn get_session_id(&self) -> &str {
        &self.session_id
    }

    pub fn get_session_file(&self) -> Option<&str> {
        self.session_file.as_deref()
    }

    fn persist_entry(&mut self, entry: &SessionEntry) -> Result<(), SessionManagerError> {
        let (true, Some(session_file)) = (self.persist, self.session_file.clone()) else {
            return Ok(());
        };

        let has_assistant = self
            .file_entries
            .iter()
            .filter_map(FileEntry::as_entry)
            .any(SessionEntry::is_assistant_message);
        if !has_assistant {
            if self.flushed {
                let mut file = std::fs::OpenOptions::new()
                    .append(true)
                    .create(true)
                    .open(&session_file)
                    .map_err(io_error)?;
                writeln!(file, "{}", FileEntry::Entry(entry.clone()).to_json())
                    .map_err(io_error)?;
            } else {
                // Stay unflushed so the first assistant message writes everything.
                self.flushed = false;
            }
            return Ok(());
        }

        if !self.flushed {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&session_file)
                .map_err(io_error)?;
            for entry in &self.file_entries {
                writeln!(file, "{}", entry.to_json()).map_err(io_error)?;
            }
            self.flushed = true;
        } else {
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .create(true)
                .open(&session_file)
                .map_err(io_error)?;
            writeln!(file, "{}", FileEntry::Entry(entry.clone()).to_json()).map_err(io_error)?;
        }
        Ok(())
    }

    fn append_entry(&mut self, entry: SessionEntry) -> Result<String, SessionManagerError> {
        let id = entry.id().to_owned();
        self.file_entries.push(FileEntry::Entry(entry.clone()));
        self.ids.insert(id.clone());
        self.leaf_id = Some(id.clone());
        self.persist_entry(&entry)?;
        Ok(id)
    }

    fn next_id(&self) -> String {
        generate_id(&|id| self.ids.contains(id))
    }

    /// Append a message as a child of the current leaf, then advance the leaf.
    ///
    /// Compaction and branch summaries must go through [`Self::append_compaction`]
    /// and [`Self::branch_with_summary`] so they stay top-level entries; the TS
    /// signature enforces that at compile time.
    pub fn append_message(
        &mut self,
        message: &AgentMessage,
    ) -> Result<String, SessionManagerError> {
        let entry = SessionMessageEntry {
            id: self.next_id(),
            parent_id: self.leaf_id.clone(),
            timestamp: now_iso(),
            message: serde_json::to_value(message).unwrap_or(Value::Null),
            extra: Map::new(),
        };
        self.append_entry(SessionEntry::Message(entry))
    }

    /// Append a raw message value; used by importers that carry unvalidated JSON.
    pub fn append_message_value(&mut self, message: Value) -> Result<String, SessionManagerError> {
        let entry = SessionMessageEntry {
            id: self.next_id(),
            parent_id: self.leaf_id.clone(),
            timestamp: now_iso(),
            message,
            extra: Map::new(),
        };
        self.append_entry(SessionEntry::Message(entry))
    }

    pub fn append_thinking_level_change(
        &mut self,
        thinking_level: &str,
    ) -> Result<String, SessionManagerError> {
        let entry = ThinkingLevelChangeEntry {
            id: self.next_id(),
            parent_id: self.leaf_id.clone(),
            timestamp: now_iso(),
            thinking_level: thinking_level.to_owned(),
            extra: Map::new(),
        };
        self.append_entry(SessionEntry::ThinkingLevelChange(entry))
    }

    pub fn append_model_change(
        &mut self,
        provider: &str,
        model_id: &str,
    ) -> Result<String, SessionManagerError> {
        let entry = ModelChangeEntry {
            id: self.next_id(),
            parent_id: self.leaf_id.clone(),
            timestamp: now_iso(),
            provider: provider.to_owned(),
            model_id: model_id.to_owned(),
            extra: Map::new(),
        };
        self.append_entry(SessionEntry::ModelChange(entry))
    }

    /// `retained` is `None` only for a compaction this crate did not produce —
    /// a hook's, or an imported session's — which then falls back to the
    /// suffix meaning of `first_kept_entry_id`.
    #[allow(clippy::too_many_arguments)]
    pub fn append_compaction(
        &mut self,
        summary: &str,
        first_kept_entry_id: &str,
        retained: Option<crate::core::compaction::retention::RetainedSelection>,
        tokens_before: i64,
        tokens_after: Option<i64>,
        details: Option<Value>,
        from_hook: Option<bool>,
        usage: Option<Usage>,
    ) -> Result<String, SessionManagerError> {
        let entry = CompactionEntry {
            id: self.next_id(),
            parent_id: self.leaf_id.clone(),
            timestamp: now_iso(),
            summary: summary.to_owned(),
            first_kept_entry_id: first_kept_entry_id.to_owned(),
            retained,
            tokens_before,
            tokens_after,
            details,
            usage,
            from_hook,
            extra: Map::new(),
        };
        self.append_entry(SessionEntry::Compaction(entry))
    }

    pub fn append_custom_entry(
        &mut self,
        custom_type: &str,
        data: Option<Value>,
    ) -> Result<String, SessionManagerError> {
        let entry = CustomEntry {
            id: self.next_id(),
            parent_id: self.leaf_id.clone(),
            timestamp: now_iso(),
            custom_type: custom_type.to_owned(),
            data,
            extra: Map::new(),
        };
        self.append_entry(SessionEntry::Custom(entry))
    }

    pub fn append_session_info(&mut self, name: &str) -> Result<String, SessionManagerError> {
        let sanitized: String = {
            let collapsed = name
                .split(['\r', '\n'])
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            collapsed.trim().to_owned()
        };
        let entry = SessionInfoEntry {
            id: self.next_id(),
            parent_id: self.leaf_id.clone(),
            timestamp: now_iso(),
            name: Some(sanitized),
            extra: Map::new(),
        };
        self.append_entry(SessionEntry::SessionInfo(entry))
    }

    /// The session name from the latest `session_info` entry, if any.
    pub fn get_session_name(&self) -> Option<String> {
        // Walk in reverse; an empty name explicitly clears the title.
        self.file_entries
            .iter()
            .rev()
            .filter_map(FileEntry::as_entry)
            .find_map(|entry| match entry {
                SessionEntry::SessionInfo(entry) => Some(
                    entry
                        .name
                        .as_ref()
                        .map(|name| name.trim().to_owned())
                        .filter(|name| !name.is_empty()),
                ),
                _ => None,
            })
            .flatten()
    }

    pub fn append_custom_message_entry(
        &mut self,
        custom_type: &str,
        content: Value,
        display: bool,
        details: Option<Value>,
    ) -> Result<String, SessionManagerError> {
        let entry = CustomMessageEntry {
            id: self.next_id(),
            parent_id: self.leaf_id.clone(),
            timestamp: now_iso(),
            custom_type: custom_type.to_owned(),
            content,
            details,
            display,
            extra: Map::new(),
        };
        self.append_entry(SessionEntry::CustomMessage(entry))
    }

    // =========================================================================
    // Tree traversal
    // =========================================================================

    pub fn get_leaf_id(&self) -> Option<&str> {
        self.leaf_id.as_deref()
    }

    fn entries_ref(&self) -> Vec<&SessionEntry> {
        self.file_entries
            .iter()
            .filter_map(FileEntry::as_entry)
            .collect()
    }

    fn find_entry(&self, id: &str) -> Option<&SessionEntry> {
        self.file_entries
            .iter()
            .filter_map(FileEntry::as_entry)
            .find(|entry| entry.id() == id)
    }

    pub fn get_leaf_entry(&self) -> Option<&SessionEntry> {
        self.leaf_id.as_deref().and_then(|id| self.find_entry(id))
    }

    pub fn get_entry(&self, id: &str) -> Option<&SessionEntry> {
        self.find_entry(id)
    }

    /// All direct children of an entry.
    pub fn get_children(&self, parent_id: &str) -> Vec<&SessionEntry> {
        self.file_entries
            .iter()
            .filter_map(FileEntry::as_entry)
            .filter(|entry| entry.parent_id() == Some(parent_id))
            .collect()
    }

    pub fn get_label(&self, id: &str) -> Option<&str> {
        self.labels_by_id.get(id).map(String::as_str)
    }

    /// Set or clear a label on an entry.
    pub fn append_label_change(
        &mut self,
        target_id: &str,
        label: Option<&str>,
    ) -> Result<String, SessionManagerError> {
        if !self.ids.contains(target_id) {
            return Err(SessionManagerError::EntryNotFound(target_id.to_owned()));
        }
        let timestamp = now_iso();
        let entry = LabelEntry {
            id: self.next_id(),
            parent_id: self.leaf_id.clone(),
            timestamp: timestamp.clone(),
            target_id: target_id.to_owned(),
            label: label.map(str::to_owned),
            extra: Map::new(),
        };
        let id = self.append_entry(SessionEntry::Label(entry))?;
        match label.filter(|label| !label.is_empty()) {
            Some(label) => {
                self.labels_by_id
                    .insert(target_id.to_owned(), label.to_owned());
                self.label_timestamps_by_id
                    .insert(target_id.to_owned(), timestamp);
            }
            None => {
                self.labels_by_id.remove(target_id);
                self.label_timestamps_by_id.remove(target_id);
            }
        }
        Ok(id)
    }

    /// Walk from an entry to the root, returning the path in root-first order.
    pub fn get_branch(&self, from_id: Option<&str>) -> Vec<&SessionEntry> {
        let start_id = from_id.or(self.leaf_id.as_deref());
        let mut path: Vec<&SessionEntry> = Vec::new();
        let mut current = start_id.and_then(|id| self.find_entry(id));
        while let Some(entry) = current {
            path.push(entry);
            current = entry.parent_id().and_then(|id| self.find_entry(id));
        }
        path.reverse();
        path
    }

    /// The active, compaction-aware entry list for context and rendering.
    pub fn build_context_entries(&self) -> Vec<SessionEntry> {
        let entries: Vec<SessionEntry> = self.get_entries();
        let leaf = match &self.leaf_id {
            Some(id) => LeafSelector::Id(id),
            None => LeafSelector::Null,
        };
        build_context_entries(&entries, leaf)
            .into_iter()
            .cloned()
            .collect()
    }

    /// The session context that gets sent to the LLM.
    pub fn build_session_context(&self) -> SessionContext {
        let entries: Vec<SessionEntry> = self.get_entries();
        let leaf = match &self.leaf_id {
            Some(id) => LeafSelector::Id(id),
            None => LeafSelector::Null,
        };
        build_session_context(&entries, leaf)
    }

    pub fn get_header(&self) -> Option<&SessionHeader> {
        self.file_entries.iter().find_map(FileEntry::as_header)
    }

    /// All session entries (without the header), as a defensive copy.
    pub fn get_entries(&self) -> Vec<SessionEntry> {
        self.file_entries
            .iter()
            .filter_map(FileEntry::as_entry)
            .cloned()
            .collect()
    }

    /// The session as a tree. Orphaned entries become roots.
    pub fn get_tree(&self) -> Vec<SessionTreeNode> {
        let entries = self.entries_ref();
        let known: HashSet<&str> = entries.iter().map(|entry| entry.id()).collect();
        let mut nodes: Vec<SessionTreeNode> = Vec::with_capacity(entries.len());
        let mut index_of: HashMap<&str, usize> = HashMap::new();
        for (index, entry) in entries.iter().enumerate() {
            index_of.insert(entry.id(), index);
            nodes.push(SessionTreeNode {
                entry: (*entry).clone(),
                children: Vec::new(),
                label: self.labels_by_id.get(entry.id()).cloned(),
                label_timestamp: self.label_timestamps_by_id.get(entry.id()).cloned(),
            });
        }

        // Children are collected by index first so the tree can be assembled bottom-up.
        let mut child_indices: Vec<Vec<usize>> = vec![Vec::new(); entries.len()];
        let mut root_indices: Vec<usize> = Vec::new();
        for (index, entry) in entries.iter().enumerate() {
            match entry.parent_id() {
                None => root_indices.push(index),
                Some(parent_id) if parent_id == entry.id() => root_indices.push(index),
                Some(parent_id) if known.contains(parent_id) => {
                    child_indices[index_of[parent_id]].push(index);
                }
                // Orphan — treat as root.
                Some(_) => root_indices.push(index),
            }
        }

        fn assemble(
            index: usize,
            nodes: &mut Vec<Option<SessionTreeNode>>,
            child_indices: &[Vec<usize>],
        ) -> SessionTreeNode {
            let mut node = nodes[index].take().expect("each entry is assembled once");
            let mut children: Vec<SessionTreeNode> = child_indices[index]
                .iter()
                .map(|child| assemble(*child, nodes, child_indices))
                .collect();
            // Oldest first, newest at the bottom.
            children.sort_by_key(|child| parse_timestamp_ms(child.entry.timestamp()).unwrap_or(0));
            node.children = children;
            node
        }

        let mut slots: Vec<Option<SessionTreeNode>> = nodes.into_iter().map(Some).collect();
        root_indices
            .into_iter()
            .map(|index| assemble(index, &mut slots, &child_indices))
            .collect()
    }

    // =========================================================================
    // Branching
    // =========================================================================

    /// Move the leaf pointer to an earlier entry; the next append starts a branch.
    pub fn branch(&mut self, branch_from_id: &str) -> Result<(), SessionManagerError> {
        if !self.ids.contains(branch_from_id) {
            return Err(SessionManagerError::EntryNotFound(
                branch_from_id.to_owned(),
            ));
        }
        self.leaf_id = Some(branch_from_id.to_owned());
        Ok(())
    }

    /// Reset the leaf pointer so the next append creates a new root entry.
    pub fn reset_leaf(&mut self) {
        self.leaf_id = None;
    }

    /// Start a new branch and record a summary of the abandoned path.
    pub fn branch_with_summary(
        &mut self,
        branch_from_id: Option<&str>,
        summary: &str,
        details: Option<Value>,
        from_hook: Option<bool>,
        usage: Option<Usage>,
    ) -> Result<String, SessionManagerError> {
        if let Some(branch_from_id) = branch_from_id
            && !self.ids.contains(branch_from_id)
        {
            return Err(SessionManagerError::EntryNotFound(
                branch_from_id.to_owned(),
            ));
        }
        self.leaf_id = branch_from_id.map(str::to_owned);
        let entry = BranchSummaryEntry {
            id: self.next_id(),
            parent_id: branch_from_id.map(str::to_owned),
            timestamp: now_iso(),
            from_id: branch_from_id.unwrap_or("root").to_owned(),
            summary: summary.to_owned(),
            details,
            usage,
            from_hook,
            extra: Map::new(),
        };
        self.append_entry(SessionEntry::BranchSummary(entry))
    }

    /// Create a new session containing only the path from root to `leaf_id`.
    pub fn create_branched_session(
        &mut self,
        leaf_id: &str,
    ) -> Result<Option<String>, SessionManagerError> {
        let previous_session_file = self.session_file.clone();
        let path: Vec<SessionEntry> = self
            .get_branch(Some(leaf_id))
            .into_iter()
            .cloned()
            .collect();
        if path.is_empty() {
            return Err(SessionManagerError::EntryNotFound(leaf_id.to_owned()));
        }

        // Labels are real tree entries, so removing them requires re-chaining the
        // retained path; the labels are recreated from the resolved map below.
        let mut path_without_labels: Vec<SessionEntry> = Vec::new();
        let mut path_parent_id: Option<String> = None;
        for entry in path {
            if matches!(entry, SessionEntry::Label(_)) {
                continue;
            }
            let mut entry = entry;
            let id = entry.id().to_owned();
            entry.set_parent_id(path_parent_id.clone());
            path_without_labels.push(entry);
            path_parent_id = Some(id);
        }

        let new_session_id = create_session_id();
        let timestamp = now_iso();
        let file_timestamp = timestamp.replace([':', '.'], "-");
        let new_session_file = Path::new(&self.session_dir)
            .join(format!("{file_timestamp}_{new_session_id}.jsonl"))
            .to_string_lossy()
            .into_owned();

        let header = SessionHeader {
            version: Some(CURRENT_SESSION_VERSION),
            id: new_session_id.clone(),
            timestamp,
            cwd: Value::from(self.cwd.clone()),
            parent_session: if self.persist {
                previous_session_file
            } else {
                None
            },
            extra: Map::new(),
        };

        let mut path_entry_ids: HashSet<String> = path_without_labels
            .iter()
            .map(|entry| entry.id().to_owned())
            .collect();
        let labels_to_write: Vec<(String, String, String)> = self
            .labels_by_id
            .iter()
            .filter(|(target_id, _)| path_entry_ids.contains(*target_id))
            .map(|(target_id, label)| {
                (
                    target_id.clone(),
                    label.clone(),
                    self.label_timestamps_by_id
                        .get(target_id)
                        .cloned()
                        .unwrap_or_default(),
                )
            })
            .collect();

        let mut label_entries: Vec<SessionEntry> = Vec::new();
        let mut parent_id = path_without_labels
            .last()
            .map(|entry| entry.id().to_owned());
        for (target_id, label, label_timestamp) in labels_to_write {
            let id = generate_id(&|id| path_entry_ids.contains(id));
            path_entry_ids.insert(id.clone());
            label_entries.push(SessionEntry::Label(LabelEntry {
                id: id.clone(),
                parent_id: parent_id.clone(),
                timestamp: label_timestamp,
                target_id,
                label: Some(label),
                extra: Map::new(),
            }));
            parent_id = Some(id);
        }

        self.file_entries = std::iter::once(FileEntry::Session(header))
            .chain(path_without_labels.into_iter().map(FileEntry::Entry))
            .chain(label_entries.into_iter().map(FileEntry::Entry))
            .collect();
        self.session_id = new_session_id;
        if self.persist {
            self.session_file = Some(new_session_file.clone());
        }
        self.build_index();

        if !self.persist {
            return Ok(None);
        }

        // Write only if the path already contains an assistant message; otherwise
        // `persist_entry` creates the file with the first assistant response, which
        // matches the `new_session` contract.
        let has_assistant = self
            .file_entries
            .iter()
            .filter_map(FileEntry::as_entry)
            .any(SessionEntry::is_assistant_message);
        if has_assistant {
            self.rewrite_file()?;
            self.flushed = true;
        } else {
            self.flushed = false;
        }
        Ok(Some(new_session_file))
    }

    // =========================================================================
    // Constructors
    // =========================================================================

    /// Create a new session.
    pub fn create(
        cwd: &str,
        session_dir: Option<&str>,
        options: Option<NewSessionOptions>,
    ) -> Result<Self, SessionManagerError> {
        let dir = match session_dir {
            Some(dir) => normalize_or_raw(dir),
            None => get_default_session_dir(cwd, None),
        };
        Self::construct(cwd, &dir, None, true, options, None)
    }

    /// Open a specific session file.
    pub fn open(
        path: &str,
        session_dir: Option<&str>,
        cwd_override: Option<&str>,
    ) -> Result<Self, SessionManagerError> {
        let resolved_path = resolve_or_raw(path);
        let mut header: Option<SessionHeader> = None;
        let mut preloaded_file_entries: Option<Vec<FileEntry>> = None;
        if cwd_override.is_none() && Path::new(&resolved_path).exists() {
            match read_session_header(&resolved_path) {
                Ok(found) => header = found,
                Err(SessionFileError::HeaderScanLimit(_)) => {
                    // The bounded scan is only a discovery optimization; a full load
                    // stays authoritative for legacy files with very large headers.
                    let entries = load_entries_from_file(&resolved_path);
                    header = entries.first().and_then(FileEntry::as_header).cloned();
                    preloaded_file_entries = Some(entries);
                }
                Err(SessionFileError::Io(error)) => return Err(SessionManagerError::Io(error)),
            }
        }
        let cwd = cwd_override
            .map(str::to_owned)
            .or_else(|| {
                header
                    .as_ref()
                    .and_then(get_session_header_cwd)
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| {
                std::env::current_dir()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            });
        let dir = match session_dir {
            Some(dir) => normalize_or_raw(dir),
            None => Path::new(&resolved_path)
                .parent()
                .map(|parent| parent.to_string_lossy().into_owned())
                .unwrap_or_default(),
        };
        Self::construct(
            &cwd,
            &dir,
            Some(&resolved_path),
            true,
            None,
            preloaded_file_entries,
        )
    }

    /// Continue the most recent session, or create a new one.
    pub fn continue_recent(
        cwd: &str,
        session_dir: Option<&str>,
    ) -> Result<Self, SessionManagerError> {
        let dir = match session_dir {
            Some(dir) => normalize_or_raw(dir),
            None => get_default_session_dir(cwd, None),
        };
        let filter_cwd = session_dir.is_some() && dir != get_default_session_dir_path(cwd, None);
        let most_recent = find_most_recent_session(&dir, filter_cwd.then_some(cwd));
        Self::construct(cwd, &dir, most_recent.as_deref(), true, None, None)
    }

    /// Create an in-memory session (no file persistence).
    pub fn in_memory(
        cwd: Option<&str>,
        options: Option<NewSessionOptions>,
    ) -> Result<Self, SessionManagerError> {
        let cwd = cwd.map(str::to_owned).unwrap_or_else(|| {
            std::env::current_dir()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        });
        Self::construct(&cwd, "", None, false, options, None)
    }

    /// Fork a session from another project directory into the current project.
    pub fn fork_from(
        source_path: &str,
        target_cwd: &str,
        session_dir: Option<&str>,
        options: Option<NewSessionOptions>,
    ) -> Result<Self, SessionManagerError> {
        let resolved_source_path = resolve_or_raw(source_path);
        let resolved_target_cwd = resolve_or_raw(target_cwd);
        let source_entries = load_entries_from_file(&resolved_source_path);
        if source_entries.is_empty() {
            return Err(SessionManagerError::ForkSourceEmpty(resolved_source_path));
        }
        if !source_entries
            .iter()
            .any(|entry| entry.as_header().is_some())
        {
            return Err(SessionManagerError::ForkSourceWithoutHeader(
                resolved_source_path,
            ));
        }

        let dir = match session_dir {
            Some(dir) => normalize_or_raw(dir),
            None => get_default_session_dir(&resolved_target_cwd, None),
        };
        if !Path::new(&dir).exists() {
            std::fs::create_dir_all(&dir).map_err(io_error)?;
        }

        let options = options.unwrap_or_default();
        if let Some(id) = &options.id {
            assert_valid_session_id(id).map_err(SessionManagerError::InvalidSessionId)?;
        }
        let new_session_id = options.id.unwrap_or_else(create_session_id);
        let timestamp = now_iso();
        let file_timestamp = timestamp.replace([':', '.'], "-");
        let new_session_file = Path::new(&dir)
            .join(format!("{file_timestamp}_{new_session_id}.jsonl"))
            .to_string_lossy()
            .into_owned();

        let new_header = FileEntry::Session(SessionHeader {
            version: Some(CURRENT_SESSION_VERSION),
            id: new_session_id,
            timestamp,
            cwd: Value::from(resolved_target_cwd.clone()),
            parent_session: Some(resolved_source_path),
            extra: Map::new(),
        });
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&new_session_file)
            .map_err(io_error)?;
        writeln!(file, "{}", new_header.to_json()).map_err(io_error)?;
        for entry in &source_entries {
            if let FileEntry::Entry(_) = entry {
                writeln!(file, "{}", entry.to_json()).map_err(io_error)?;
            }
        }
        drop(file);

        Self::construct(
            &resolved_target_cwd,
            &dir,
            Some(&new_session_file),
            true,
            None,
            None,
        )
    }

    /// List all sessions for a directory.
    pub async fn list(
        cwd: &str,
        session_dir: Option<&str>,
        on_progress: Option<SessionListProgress<'_>>,
    ) -> Vec<SessionInfo> {
        let dir = match session_dir {
            Some(dir) => normalize_or_raw(dir),
            None => get_default_session_dir(cwd, None),
        };
        let filter_cwd = session_dir.is_some() && dir != get_default_session_dir_path(cwd, None);
        let resolved_cwd = resolve_or_raw(cwd);
        let mut sessions: Vec<SessionInfo> = list_sessions_from_dir(&dir, on_progress, 0, None)
            .await
            .into_iter()
            .filter(|session| {
                !filter_cwd || session_cwd_matches(Some(session.cwd.as_str()), &resolved_cwd)
            })
            .collect();
        sort_sessions_by_modified(&mut sessions);
        sessions
    }

    /// List all sessions across all project directories.
    pub async fn list_all(
        session_dir: Option<&str>,
        on_progress: Option<SessionListProgress<'_>>,
    ) -> Vec<SessionInfo> {
        if let Some(session_dir) = session_dir {
            let mut sessions =
                list_sessions_from_dir(&normalize_or_raw(session_dir), on_progress, 0, None).await;
            sort_sessions_by_modified(&mut sessions);
            return sessions;
        }

        let sessions_dir = get_sessions_dir();
        if !sessions_dir.exists() {
            return Vec::new();
        }
        let Ok(mut dir_entries) = tokio::fs::read_dir(&sessions_dir).await else {
            return Vec::new();
        };
        let mut dirs: Vec<PathBuf> = Vec::new();
        while let Ok(Some(entry)) = dir_entries.next_entry().await {
            let Ok(file_type) = entry.file_type().await else {
                continue;
            };
            if file_type.is_dir() || file_type.is_symlink() {
                dirs.push(entry.path());
            }
        }

        // Count the files first so progress reflects the whole run.
        let mut all_files: Vec<String> = Vec::new();
        for dir in dirs {
            let Ok(mut files) = tokio::fs::read_dir(&dir).await else {
                continue;
            };
            while let Ok(Some(file)) = files.next_entry().await {
                let path = file.path();
                if path
                    .extension()
                    .is_some_and(|extension| extension == "jsonl")
                {
                    all_files.push(path.to_string_lossy().into_owned());
                }
            }
        }
        let total = all_files.len();
        let loaded = std::sync::atomic::AtomicUsize::new(0);
        let results = build_session_infos_with_concurrency(&all_files, &|| {
            let loaded = loaded.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            if let Some(on_progress) = on_progress {
                on_progress(loaded, total);
            }
        })
        .await;
        let mut sessions: Vec<SessionInfo> = results.into_iter().flatten().collect();
        sort_sessions_by_modified(&mut sessions);
        sessions
    }
}
