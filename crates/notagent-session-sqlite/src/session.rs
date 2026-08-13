//! Port of `packages/agent/src/harness/session/session.ts`.
//!
//! Like `session_types.rs`, this lives in the SQLite crate: the agent harness
//! is excluded from the port, but `SessionRepo` returns a `Session` and the
//! backend's tests drive it (master plan exclusion table; WS-C plan task 4).

use std::sync::Arc;

use notagent_agent::{AgentMessage, uuidv7};
use serde_json::Value;

use crate::session_types::{
    BranchBounds, Entry, EntryQuery, IdGenerator, LanePointer, LaneRecord, LogItem, LogOptions,
    OperationStartedRecord, ProvisionedEntry, RecordQuery, RecordType, SessionError,
    SessionErrorCode, SessionStats,
};
use crate::sqlite::repo::SqliteSessionStorage;
use crate::sqlite::types::SqliteSessionMetadata;

/// TS: `{ next: () => uuidv7() }`.
pub struct Uuidv7IdGenerator;

impl IdGenerator for Uuidv7IdGenerator {
    fn next(&self) -> String {
        uuidv7()
    }
}

fn assert_valid_limit(limit: Option<i64>) -> Result<(), SessionError> {
    match limit {
        Some(limit) if limit <= 0 => Err(SessionError::invalid_query(
            "limit must be a positive integer",
        )),
        _ => Ok(()),
    }
}

fn assert_valid_cursor(after_seq: Option<i64>) -> Result<(), SessionError> {
    match after_seq {
        Some(after_seq) if after_seq < 0 => Err(SessionError::invalid_query(
            "cursor sequence must be a non-negative integer",
        )),
        _ => Ok(()),
    }
}

/// Port of `assertJsonSerializable`. `serde_json::Value` cannot hold cycles,
/// accessors, symbols or sparse arrays; only non-finite numbers are checked
/// (deviation class 1).
pub fn assert_json_serializable(value: &Value) -> Result<(), SessionError> {
    match value {
        Value::Number(number) if number.as_f64().is_some_and(|value| !value.is_finite()) => {
            Err(SessionError::new(
                SessionErrorCode::InvalidPayload,
                "Durable payload contains a non-finite number",
            ))
        }
        Value::Array(items) => items.iter().try_for_each(assert_json_serializable),
        Value::Object(entries) => entries.values().try_for_each(assert_json_serializable),
        _ => Ok(()),
    }
}

/// Port of `class Session` — the validating view over a `SessionStorage`.
pub struct Session {
    storage: Arc<SqliteSessionStorage>,
    pub id_generator: Box<dyn IdGenerator>,
}

impl Session {
    pub fn new(storage: Arc<SqliteSessionStorage>) -> Self {
        Self {
            storage,
            id_generator: Box::new(Uuidv7IdGenerator),
        }
    }

    pub fn with_id_generator(
        storage: Arc<SqliteSessionStorage>,
        id_generator: Box<dyn IdGenerator>,
    ) -> Self {
        Self {
            storage,
            id_generator,
        }
    }

    pub fn storage(&self) -> &Arc<SqliteSessionStorage> {
        &self.storage
    }

    pub fn get_metadata(&self) -> Result<SqliteSessionMetadata, SessionError> {
        self.storage.get_metadata()
    }

    pub fn get_leaf_id(&self) -> Result<Option<String>, SessionError> {
        self.leaf_id_for_lane("main")
    }

    /// TS: `session.view(lane).getLeafId()`.
    pub fn get_leaf_id_in_lane(&self, lane: &str) -> Result<Option<String>, SessionError> {
        self.leaf_id_for_lane(lane)
    }

    /// TS: `session.view(lane).findEntriesOnBranch(query)`.
    pub fn find_entries_on_branch_in_lane(
        &self,
        lane: &str,
        query: &EntryQuery,
        bounds: &BranchBounds,
    ) -> Result<Vec<Entry>, SessionError> {
        self.query_branch_entries(lane, query, bounds, query.limit)
    }

    /// TS: `session.view(lane).findEntryOnBranch(query)`.
    pub fn find_entry_on_branch_in_lane(
        &self,
        lane: &str,
        query: &EntryQuery,
        bounds: &BranchBounds,
    ) -> Result<Option<Entry>, SessionError> {
        Ok(self
            .query_branch_entries(lane, query, bounds, Some(1))?
            .into_iter()
            .next())
    }

    /// TS: `session.view(lane).appendMessage(message)`.
    pub async fn append_message_in_lane(
        &self,
        lane: &str,
        message: AgentMessage,
    ) -> Result<String, SessionError> {
        self.append_message_to_lane(lane, message).await
    }

    /// TS: `session.view(lane).appendCustomEntry(customType, data)`.
    pub async fn append_custom_entry_in_lane(
        &self,
        lane: &str,
        custom_type: &str,
        data: Option<Value>,
    ) -> Result<String, SessionError> {
        self.append_custom_entry_to_lane(lane, custom_type, data)
            .await
    }

    pub fn get_entry(&self, id: &str) -> Result<Option<Entry>, SessionError> {
        self.storage.get_entry(id)
    }

    pub fn get_stats(&self) -> Result<SessionStats, SessionError> {
        self.storage.get_stats()
    }

    pub fn get_name(&self) -> Result<Option<String>, SessionError> {
        self.storage.get_name()
    }

    pub async fn set_name(&self, name: Option<&str>) -> Result<(), SessionError> {
        self.storage.set_name(name).await
    }

    pub fn get_label(&self, target_id: &str) -> Result<Option<String>, SessionError> {
        self.storage.get_label(target_id)
    }

    pub async fn set_label(
        &self,
        target_id: &str,
        label: Option<&str>,
    ) -> Result<(), SessionError> {
        self.storage.set_label(target_id, label).await
    }

    pub fn find_entries(&self, query: &EntryQuery) -> Result<Vec<Entry>, SessionError> {
        self.query_entries(query, query.limit)
    }

    pub fn find_entry(&self, query: &EntryQuery) -> Result<Option<Entry>, SessionError> {
        Ok(self.query_entries(query, Some(1))?.into_iter().next())
    }

    pub fn find_entries_on_branch(
        &self,
        query: &EntryQuery,
        bounds: &BranchBounds,
    ) -> Result<Vec<Entry>, SessionError> {
        self.query_branch_entries("main", query, bounds, query.limit)
    }

    pub fn find_entry_on_branch(
        &self,
        query: &EntryQuery,
        bounds: &BranchBounds,
    ) -> Result<Option<Entry>, SessionError> {
        Ok(self
            .query_branch_entries("main", query, bounds, Some(1))?
            .into_iter()
            .next())
    }

    pub async fn append_message(&self, message: AgentMessage) -> Result<String, SessionError> {
        self.append_message_to_lane("main", message).await
    }

    pub async fn append_custom_entry(
        &self,
        custom_type: &str,
        data: Option<Value>,
    ) -> Result<String, SessionError> {
        self.append_custom_entry_to_lane("main", custom_type, data)
            .await
    }

    pub fn get_lanes(&self) -> Result<Vec<LanePointer>, SessionError> {
        Ok(self
            .storage
            .get_lanes()?
            .into_iter()
            .map(|(lane, leaf_id)| LanePointer { lane, leaf_id })
            .collect())
    }

    pub async fn create_lane(&self, lane: &str, at: Option<&str>) -> Result<(), SessionError> {
        self.storage.create_lane(lane, at).await
    }

    pub async fn move_lane(&self, lane: &str, to: Option<&str>) -> Result<(), SessionError> {
        self.storage.move_lane(lane, to).await
    }

    pub async fn append_entry(
        &self,
        entry: ProvisionedEntry,
        lane: &str,
    ) -> Result<Entry, SessionError> {
        self.commit_entry(entry, lane).await
    }

    pub async fn append_record(&self, record: LaneRecord) -> Result<LaneRecord, SessionError> {
        self.commit_record(record).await
    }

    pub fn find_records(&self, query: &RecordQuery) -> Result<Vec<LaneRecord>, SessionError> {
        self.query_records(query)
    }

    pub fn find_open_operations(
        &self,
        lane: &str,
        limit: Option<i64>,
    ) -> Result<Vec<OperationStartedRecord>, SessionError> {
        assert_valid_limit(limit)?;
        self.storage.find_open_operations(lane)
    }

    pub fn get_log(&self, options: &LogOptions) -> Result<Vec<LogItem>, SessionError> {
        assert_valid_limit(options.limit)?;
        assert_valid_cursor(options.after_seq)?;
        self.storage.get_log(options)
    }

    /// Returns the lane's current leaf, or `None` when empty. Errors when the lane does not exist.
    fn leaf_id_for_lane(&self, lane: &str) -> Result<Option<String>, SessionError> {
        let lanes = self.get_lanes()?;
        let pointer = lanes
            .into_iter()
            .find(|pointer| pointer.lane == lane)
            .ok_or_else(|| {
                SessionError::new(
                    SessionErrorCode::InvalidLane,
                    format!("Lane not found: {lane}"),
                )
            })?;
        Ok(pointer.leaf_id)
    }

    fn query_entries(
        &self,
        query: &EntryQuery,
        result_limit: Option<i64>,
    ) -> Result<Vec<Entry>, SessionError> {
        assert_valid_limit(query.limit)?;
        assert_valid_cursor(query.cursor.map(|cursor| cursor.after_seq))?;
        let effective = EntryQuery {
            limit: result_limit,
            ..query.clone()
        };
        self.storage.find_entries(&effective)
    }

    /// Queries from `bounds.start` toward the root, defaulting to the lane's leaf.
    fn query_branch_entries(
        &self,
        default_lane: &str,
        query: &EntryQuery,
        bounds: &BranchBounds,
        result_limit: Option<i64>,
    ) -> Result<Vec<Entry>, SessionError> {
        assert_valid_limit(query.limit)?;
        assert_valid_cursor(query.cursor.map(|cursor| cursor.after_seq))?;
        let start = match &bounds.start {
            Some(start) => Some(start.clone()),
            None => self.leaf_id_for_lane(default_lane)?,
        };
        let Some(start) = start else {
            return Ok(Vec::new());
        };
        let effective = EntryQuery {
            limit: result_limit,
            ..query.clone()
        };
        self.storage
            .find_entries_on_branch(&start, &effective, bounds)
    }

    fn query_records(&self, query: &RecordQuery) -> Result<Vec<LaneRecord>, SessionError> {
        assert_valid_limit(query.limit)?;
        assert_valid_cursor(query.after_seq)?;
        if query.operation_kind.is_some() && query.record_type != Some(RecordType::OperationStarted)
        {
            return Err(SessionError::invalid_query(
                "operationKind requires type \"operation_started\"",
            ));
        }
        self.storage.find_records(query)
    }

    async fn append_message_to_lane(
        &self,
        lane: &str,
        message: AgentMessage,
    ) -> Result<String, SessionError> {
        let entry = self
            .commit_entry(
                ProvisionedEntry::Message {
                    id: self.id_generator.next(),
                    message,
                    terminate: None,
                },
                lane,
            )
            .await?;
        Ok(entry.id().to_owned())
    }

    async fn append_custom_entry_to_lane(
        &self,
        lane: &str,
        custom_type: &str,
        data: Option<Value>,
    ) -> Result<String, SessionError> {
        let entry = self
            .commit_entry(
                ProvisionedEntry::Custom {
                    id: self.id_generator.next(),
                    custom_type: custom_type.to_owned(),
                    data,
                },
                lane,
            )
            .await?;
        Ok(entry.id().to_owned())
    }

    async fn commit_entry(
        &self,
        entry: ProvisionedEntry,
        lane: &str,
    ) -> Result<Entry, SessionError> {
        let value = serde_json::to_value(&entry).map_err(|error| {
            SessionError::new(SessionErrorCode::InvalidPayload, error.to_string())
        })?;
        assert_json_serializable(&value)?;
        self.storage.append_entry(entry, lane).await
    }

    async fn commit_record(&self, record: LaneRecord) -> Result<LaneRecord, SessionError> {
        let value = serde_json::to_value(&record).map_err(|error| {
            SessionError::new(SessionErrorCode::InvalidPayload, error.to_string())
        })?;
        assert_json_serializable(&value)?;
        self.storage.append_record(record).await
    }
}
