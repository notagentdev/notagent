use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use notagent_agent::uuidv7;
use serde_json::Value;

use crate::session_types::{
    BranchBounds, Entry, EntryOrder, EntryQuery, EntryType, ForkOptions, ForkPosition, ForkScope,
    LaneRecord, LogFact, LogItem, LogOptions, OperationStartedRecord, ProvisionedEntry,
    RecordQuery, SessionError, SessionErrorCode, SessionStats,
};
use crate::sqlite::branch_cache::{
    append_entry_to_branch_cache, build_cached_branch, delete_branch_cache, rebuild_branch_cache,
};
use crate::sqlite::migrations::apply_migrations;
use crate::sqlite::search_backend::configure_sqlite_database;
use crate::sqlite::storage::branch_entries::{
    CachedBranchQuery, query_cached_branch_rows, read_cached_branch,
};
use crate::sqlite::storage::branch_tips::read_branch_tip_ids;
use crate::sqlite::storage::entries::{
    EntryRow, EntryRowQuery, NewEntryRow, delete_entry_rows, entry_payload, id_exists_in_entries,
    insert_entry_row, read_entry_row, read_entry_rows,
};
use crate::sqlite::storage::facts::{
    append_fact, delete_fact_rows, read_fact_rows, read_latest_fact, read_latest_label_facts,
};
use crate::sqlite::storage::lanes::{
    create_initial_lane, create_lane, delete_lane_rows, finish_lane_operation, move_lane,
    read_lane, read_lane_head, read_lane_move_rows, read_lanes, set_lane_leaf,
    start_lane_operation,
};
use crate::sqlite::storage::records::{
    NewRecordRow, RecordRowQuery, append_record_row, delete_record_rows, id_exists_in_records,
    read_open_operation_rows, read_record_rows,
};
use crate::sqlite::storage::session_sequences::{
    advance_sequence, create_sequence, delete_sequence, get_next_sequence, set_next_sequence,
};
use crate::sqlite::storage::session_stats::{
    add_usage_to_stats, create_stats, delete_stats, increment_message_count, read_stats,
};
use crate::sqlite::storage::sessions::{
    NewSessionRow, SessionRow, decode_session_metadata, delete_session_row, insert_session_row,
    read_session_row, read_session_rows, session_exists,
};
use crate::sqlite::storage::writer_leases::{
    WriterLease, acquire_writer_lease, delete_writer_lease, release_writer_lease,
    renew_writer_lease,
};
use crate::sqlite::types::{
    SqliteDatabase, SqliteDatabaseFactory, SqliteError, SqliteSessionCreateOptions,
    SqliteSessionListOptions, SqliteSessionMetadata, with_transaction,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SqliteWriterLeaseOptions {
    /// Time without a successful heartbeat before another writer may take over. Default: 30 seconds.
    pub ttl_ms: Option<u64>,
    /// Idle heartbeat cadence. Default: 10 seconds. Must be less than `ttl_ms`.
    pub heartbeat_interval_ms: Option<u64>,
}

pub struct SqliteSessionRepositoryOptions {
    pub sqlite: Arc<dyn SqliteDatabaseFactory>,
    pub database_path: String,
    pub writer_lease: Option<SqliteWriterLeaseOptions>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ResolvedWriterLeaseOptions {
    ttl_ms: u64,
    heartbeat_interval_ms: u64,
}

fn resolve_writer_lease_options(
    options: Option<SqliteWriterLeaseOptions>,
) -> Result<ResolvedWriterLeaseOptions, SessionError> {
    let options = options.unwrap_or_default();
    let ttl_ms = options.ttl_ms.unwrap_or(30_000);
    let heartbeat_interval_ms = options.heartbeat_interval_ms.unwrap_or(10_000);
    if ttl_ms == 0 {
        return Err(SessionError::storage("writerLease.ttlMs must be positive"));
    }
    if heartbeat_interval_ms == 0 || heartbeat_interval_ms >= ttl_ms {
        return Err(SessionError::storage(
            "writerLease.heartbeatIntervalMs must be positive and less than ttlMs",
        ));
    }
    Ok(ResolvedWriterLeaseOptions {
        ttl_ms,
        heartbeat_interval_ms,
    })
}

fn active_writer_error(session_id: &str) -> SessionError {
    SessionError::storage(format!(
        "SQLite session {session_id} already has an active writer"
    ))
}

fn lost_writer_error(session_id: &str) -> SessionError {
    SessionError::storage(format!("SQLite session {session_id} writer lease was lost"))
}

pub(crate) fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn claim_writer_lease(
    db: &dyn SqliteDatabase,
    session_id: &str,
    options: ResolvedWriterLeaseOptions,
) -> Result<WriterLease, SessionError> {
    let now = now_ms();
    let lease = acquire_writer_lease(db, session_id, &uuidv7(), now, now + options.ttl_ms as i64)?;
    lease.ok_or_else(|| active_writer_error(session_id))
}

fn get_parent_path(path: &str) -> String {
    Path::new(path).parent().map_or_else(
        || ".".to_owned(),
        |parent| parent.to_string_lossy().into_owned(),
    )
}

/// Runs a write transaction and preserves the original `SessionError` code:
fn in_transaction<T>(
    db: &dyn SqliteDatabase,
    body: impl FnOnce() -> Result<T, SessionError>,
) -> Result<T, SessionError> {
    let captured: std::cell::RefCell<Option<SessionError>> = std::cell::RefCell::new(None);
    let result = with_transaction(db, || {
        body().map_err(|error| {
            *captured.borrow_mut() = Some(error.clone());
            SqliteError(error.message)
        })
    });
    match result {
        Ok(value) => Ok(value),
        Err(error) => Err(captured
            .into_inner()
            .unwrap_or_else(|| SessionError::storage(error.0))),
    }
}

fn require_session_row(
    db: &dyn SqliteDatabase,
    session_id: &str,
) -> Result<SessionRow, SessionError> {
    read_session_row(db, session_id)?
        .ok_or_else(|| SessionError::not_found(format!("Session not found: {session_id}")))
}

pub(crate) fn decode_entry(row: &EntryRow) -> Result<Entry, SessionError> {
    let invalid = || {
        SessionError::new(
            SessionErrorCode::InvalidEntry,
            format!(
                "Invalid SQLite session entry {}: failed to decode entry {}",
                row.id, row.id
            ),
        )
    };
    let payload: Value = serde_json::from_str(&row.payload).map_err(|_| invalid())?;
    let Value::Object(mut object) = payload else {
        return Err(invalid());
    };
    object.insert("type".to_owned(), Value::from(row.entry_type.clone()));
    object.insert("id".to_owned(), Value::from(row.id.clone()));
    object.insert("seq".to_owned(), Value::from(row.seq));
    object.insert(
        "parentId".to_owned(),
        row.parent_id.clone().map_or(Value::Null, Value::from),
    );
    object.insert("timestamp".to_owned(), Value::from(row.timestamp));
    serde_json::from_value(Value::Object(object)).map_err(|_| invalid())
}

fn decode_record(seq: i64, timestamp: i64, payload: &str) -> Result<LaneRecord, SessionError> {
    let invalid = || {
        SessionError::storage(format!(
            "Invalid SQLite session record at sequence {seq}: failed to decode payload"
        ))
    };
    let parsed: Value = serde_json::from_str(payload).map_err(|_| invalid())?;
    let Value::Object(mut object) = parsed else {
        return Err(invalid());
    };
    object.insert("seq".to_owned(), Value::from(seq));
    object.insert("timestamp".to_owned(), Value::from(timestamp));
    serde_json::from_value(Value::Object(object)).map_err(|_| invalid())
}

fn matches_entry_query(entry: &Entry, query: &EntryQuery) -> bool {
    let type_matches = query
        .entry_type
        .is_none_or(|entry_type| entry.entry_type() == entry_type);
    let custom_matches = query
        .custom_type
        .as_ref()
        .is_none_or(|custom_type| entry.custom_type() == Some(custom_type.as_str()));
    let cursor_matches = query.cursor.is_none_or(|cursor| {
        if query.order == Some(EntryOrder::OldestFirst) {
            entry.seq() > cursor.after_seq
        } else {
            entry.seq() < cursor.after_seq
        }
    });
    type_matches && custom_matches && cursor_matches
}

fn validate_cached_branch_rows(
    rows: &[EntryRow],
    query: &EntryQuery,
    bounds: &BranchBounds,
) -> Result<(), SessionError> {
    if rows.is_empty() || query.entry_type.is_some() || query.custom_type.is_some() {
        return Ok(());
    }
    let mut path: Vec<&EntryRow> = rows.iter().collect();
    path.sort_by_key(|row| row.seq);
    let should_include_root = bounds.stop_at_id.is_none()
        && bounds.stop_at_type.is_none()
        && query.cursor.is_none()
        && (query.order == Some(EntryOrder::OldestFirst) || query.limit.is_none());
    if should_include_root && path[0].parent_id.is_some() {
        let parent = path[0].parent_id.clone().unwrap_or_default();
        return Err(SessionError::new(
            SessionErrorCode::InvalidEntry,
            format!("Entry {parent} not found"),
        ));
    }
    for index in 1..path.len() {
        let previous = path[index - 1];
        let current = path[index];
        if current.parent_id.as_deref() != Some(previous.id.as_str()) {
            let parent = current.parent_id.clone().unwrap_or_default();
            return Err(SessionError::new(
                SessionErrorCode::InvalidEntry,
                format!("Entry {parent} not found"),
            ));
        }
    }
    Ok(())
}

fn assert_unused_id(
    db: &dyn SqliteDatabase,
    session_id: &str,
    id: &str,
) -> Result<(), SessionError> {
    if id_exists_in_entries(db, session_id, id)? || id_exists_in_records(db, session_id, id)? {
        return Err(SessionError::new(
            SessionErrorCode::AlreadyExists,
            format!("ID already exists: {id}"),
        ));
    }
    Ok(())
}

fn record_run_id(record: &LaneRecord) -> Option<String> {
    record.run_id().map(str::to_owned)
}

fn record_op_kind(record: &LaneRecord) -> Option<String> {
    record.operation_kind().map(|kind| {
        match kind {
            crate::session_types::OperationKind::Run => "run",
            crate::session_types::OperationKind::Compaction => "compaction",
            crate::session_types::OperationKind::Navigation => "navigation",
        }
        .to_owned()
    })
}

pub struct SqliteSessionStorage {
    db: Arc<dyn SqliteDatabase>,
    metadata: SqliteSessionMetadata,
    lease: Mutex<WriterLease>,
    lease_options: ResolvedWriterLeaseOptions,
    operations: tokio::sync::Mutex<()>,
    lease_error: Mutex<Option<SessionError>>,
    closing: AtomicBool,
    heartbeat: Mutex<Option<tokio::task::JoinHandle<()>>>,
    released: Mutex<bool>,
}

impl std::fmt::Debug for SqliteSessionStorage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SqliteSessionStorage")
            .field("session", &self.metadata.id)
            .finish_non_exhaustive()
    }
}

impl SqliteSessionStorage {
    fn new(
        db: Arc<dyn SqliteDatabase>,
        metadata: SqliteSessionMetadata,
        lease: WriterLease,
        lease_options: ResolvedWriterLeaseOptions,
    ) -> Arc<Self> {
        let storage = Arc::new(Self {
            db,
            metadata,
            lease: Mutex::new(lease),
            lease_options,
            operations: tokio::sync::Mutex::new(()),
            lease_error: Mutex::new(None),
            closing: AtomicBool::new(false),
            heartbeat: Mutex::new(None),
            released: Mutex::new(false),
        });
        storage.schedule_heartbeat();
        storage
    }

    pub fn metadata_id(&self) -> &str {
        &self.metadata.id
    }

    pub fn is_for_session(&self, session_id: &str) -> bool {
        self.metadata.id == session_id
    }

    pub async fn release(&self) {
        {
            let mut released = self.released.lock().expect("storage mutex");
            if *released {
                return;
            }
            *released = true;
        }
        self.closing.store(true, Ordering::SeqCst);
        if let Some(handle) = self.heartbeat.lock().expect("storage mutex").take() {
            handle.abort();
        }
        let _guard = self.operations.lock().await;
        let lease = self.lease.lock().expect("storage mutex").clone();
        let _ = with_transaction(self.db.as_ref(), || {
            release_writer_lease(self.db.as_ref(), &self.metadata.id, &lease)
                .map_err(|error| SqliteError(error.message))
        });
    }

    async fn enqueue_write<T>(
        &self,
        operation: impl FnOnce() -> Result<T, SessionError>,
    ) -> Result<T, SessionError> {
        if self.closing.load(Ordering::SeqCst) {
            return Err(SessionError::storage(format!(
                "SQLite session {} is closed",
                self.metadata.id
            )));
        }
        let _guard = self.operations.lock().await;
        if let Some(error) = self.lease_error.lock().expect("storage mutex").clone() {
            return Err(error);
        }
        in_transaction(self.db.as_ref(), || {
            let now = now_ms();
            let renewed = {
                let mut lease = self.lease.lock().expect("storage mutex");
                renew_writer_lease(
                    self.db.as_ref(),
                    &self.metadata.id,
                    &mut lease,
                    now,
                    now + self.lease_options.ttl_ms as i64,
                )?
            };
            if !renewed {
                let error = lost_writer_error(&self.metadata.id);
                *self.lease_error.lock().expect("storage mutex") = Some(error.clone());
                if let Some(handle) = self.heartbeat.lock().expect("storage mutex").take() {
                    handle.abort();
                }
                return Err(error);
            }
            operation()
        })
    }

    fn schedule_heartbeat(self: &Arc<Self>) {
        if self.closing.load(Ordering::SeqCst)
            || self.lease_error.lock().expect("storage mutex").is_some()
        {
            return;
        }
        let storage = Arc::downgrade(self);
        let interval = self.lease_options.heartbeat_interval_ms;
        let handle = tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(interval)).await;
                let Some(storage) = storage.upgrade() else {
                    return;
                };
                if storage.closing.load(Ordering::SeqCst)
                    || storage.lease_error.lock().expect("storage mutex").is_some()
                {
                    return;
                }
                let _guard = storage.operations.lock().await;
                // A transient heartbeat failure is retried. Every write still
                // verifies ownership transactionally.
                let _ = with_transaction(storage.db.as_ref(), || {
                    let now = now_ms();
                    let mut lease = storage.lease.lock().expect("storage mutex");
                    let renewed = renew_writer_lease(
                        storage.db.as_ref(),
                        &storage.metadata.id,
                        &mut lease,
                        now,
                        now + storage.lease_options.ttl_ms as i64,
                    )
                    .map_err(|error| SqliteError(error.message))?;
                    if !renewed {
                        *storage.lease_error.lock().expect("storage mutex") =
                            Some(lost_writer_error(&storage.metadata.id));
                    }
                    Ok(())
                });
            }
        });
        *self.heartbeat.lock().expect("storage mutex") = Some(handle);
    }

    pub fn get_metadata(&self) -> Result<SqliteSessionMetadata, SessionError> {
        decode_session_metadata(
            &require_session_row(self.db.as_ref(), &self.metadata.id)?,
            &self.metadata.path,
        )
    }

    pub fn get_lanes(&self) -> Result<Vec<(String, Option<String>)>, SessionError> {
        Ok(read_lanes(self.db.as_ref(), &self.metadata.id)?
            .into_iter()
            .map(|row| (row.lane, row.leaf_id))
            .collect())
    }

    pub async fn create_lane(&self, lane: &str, at: Option<&str>) -> Result<(), SessionError> {
        self.enqueue_write(|| {
            if read_lane(self.db.as_ref(), &self.metadata.id, lane)?.is_some() {
                return Err(SessionError::new(
                    SessionErrorCode::AlreadyExists,
                    format!("Lane already exists: {lane}"),
                ));
            }
            if let Some(at) = at
                && read_entry_row(self.db.as_ref(), &self.metadata.id, at)?.is_none()
            {
                return Err(SessionError::not_found(format!("Entry not found: {at}")));
            }
            let seq = get_next_sequence(self.db.as_ref(), &self.metadata.id)?;
            create_lane(self.db.as_ref(), &self.metadata.id, seq, lane, at)?;
            advance_sequence(self.db.as_ref(), &self.metadata.id, seq)
        })
        .await
    }

    pub async fn move_lane(&self, lane: &str, to: Option<&str>) -> Result<(), SessionError> {
        self.enqueue_write(|| {
            if read_lane(self.db.as_ref(), &self.metadata.id, lane)?.is_none() {
                return Err(SessionError::new(
                    SessionErrorCode::InvalidLane,
                    format!("Lane not found: {lane}"),
                ));
            }
            if let Some(to) = to
                && read_entry_row(self.db.as_ref(), &self.metadata.id, to)?.is_none()
            {
                return Err(SessionError::not_found(format!("Entry not found: {to}")));
            }
            let seq = get_next_sequence(self.db.as_ref(), &self.metadata.id)?;
            move_lane(self.db.as_ref(), &self.metadata.id, seq, lane, to)?;
            advance_sequence(self.db.as_ref(), &self.metadata.id, seq)
        })
        .await
    }

    pub async fn append_entry(
        &self,
        entry: ProvisionedEntry,
        lane: &str,
    ) -> Result<Entry, SessionError> {
        self.enqueue_write(|| {
            let parent_id = read_lane_head(self.db.as_ref(), &self.metadata.id, lane)?;
            assert_unused_id(self.db.as_ref(), &self.metadata.id, entry.id())?;
            let seq = get_next_sequence(self.db.as_ref(), &self.metadata.id)?;
            let timestamp = now_ms();
            let committed = provisioned_to_entry(&entry, parent_id.clone(), seq, timestamp)?;
            let payload = serde_json::to_string(&entry_payload(&committed)?).map_err(|error| {
                SessionError::new(SessionErrorCode::InvalidPayload, error.to_string())
            })?;
            insert_entry_row(
                self.db.as_ref(),
                &self.metadata.id,
                &NewEntryRow {
                    seq,
                    id: committed.id().to_owned(),
                    parent_id: committed.parent_id().map(str::to_owned),
                    entry_type: committed.entry_type(),
                    timestamp,
                    payload,
                },
            )?;
            set_lane_leaf(
                self.db.as_ref(),
                &self.metadata.id,
                lane,
                Some(committed.id()),
            )?;
            append_entry_to_branch_cache(
                self.db.as_ref(),
                &self.metadata.id,
                committed.id(),
                seq,
                committed.entry_type().as_str(),
                committed.custom_type(),
                committed.parent_id(),
            )?;
            if committed.entry_type() == EntryType::Message {
                increment_message_count(self.db.as_ref(), &self.metadata.id)?;
            }
            advance_sequence(self.db.as_ref(), &self.metadata.id, seq)?;
            Ok(committed)
        })
        .await
    }

    pub async fn append_record(&self, record: LaneRecord) -> Result<LaneRecord, SessionError> {
        self.enqueue_write(|| {
            let lane = record.lane().to_owned();
            if read_lane(self.db.as_ref(), &self.metadata.id, &lane)?.is_none() {
                return Err(SessionError::new(
                    SessionErrorCode::InvalidLane,
                    format!("Lane not found: {lane}"),
                ));
            }
            assert_unused_id(self.db.as_ref(), &self.metadata.id, record.id())?;
            let seq = get_next_sequence(self.db.as_ref(), &self.metadata.id)?;
            let timestamp = now_ms();
            let committed = with_seq_and_timestamp(&record, seq, timestamp)?;
            if let LaneRecord::OperationStarted(started) = &record {
                start_lane_operation(self.db.as_ref(), &self.metadata.id, &lane, &started.id)?;
            }
            let payload = serde_json::to_string(&record).map_err(|error| {
                SessionError::new(SessionErrorCode::InvalidPayload, error.to_string())
            })?;
            append_record_row(
                self.db.as_ref(),
                &self.metadata.id,
                &NewRecordRow {
                    seq,
                    id: record.id().to_owned(),
                    lane: lane.clone(),
                    run_id: record_run_id(&record),
                    record_type: record.record_type().as_str().to_owned(),
                    op_kind: record_op_kind(&record),
                    timestamp,
                    payload,
                },
            )?;
            if let LaneRecord::OperationFinished(finished) = &record {
                finish_lane_operation(
                    self.db.as_ref(),
                    &self.metadata.id,
                    &lane,
                    &finished.run_id,
                )?;
            }
            if let LaneRecord::Usage(usage) = &record {
                add_usage_to_stats(self.db.as_ref(), &self.metadata.id, &usage.usage)?;
            }
            advance_sequence(self.db.as_ref(), &self.metadata.id, seq)?;
            Ok(committed)
        })
        .await
    }

    pub fn get_entry(&self, id: &str) -> Result<Option<Entry>, SessionError> {
        match read_entry_row(self.db.as_ref(), &self.metadata.id, id)? {
            Some(row) => Ok(Some(decode_entry(&row)?)),
            None => Ok(None),
        }
    }

    pub fn find_entries(&self, query: &EntryQuery) -> Result<Vec<Entry>, SessionError> {
        let sql_type = query
            .entry_type
            .or(query.custom_type.as_ref().map(|_| EntryType::Custom));
        let sql_limit = if query.custom_type.is_none() {
            query.limit
        } else {
            None
        };
        let rows = read_entry_rows(
            self.db.as_ref(),
            &self.metadata.id,
            &EntryRowQuery {
                after_seq: None,
                cursor_after_seq: query.cursor.map(|cursor| cursor.after_seq),
                entry_type: sql_type,
                order: query.order,
                limit: sql_limit,
            },
        )?;
        let mut entries = Vec::new();
        for row in &rows {
            let entry = decode_entry(row)?;
            if matches_entry_query(&entry, query) {
                entries.push(entry);
            }
        }
        truncate(entries, query.limit)
    }

    pub fn find_entries_on_branch(
        &self,
        start: &str,
        query: &EntryQuery,
        bounds: &BranchBounds,
    ) -> Result<Vec<Entry>, SessionError> {
        let Some(cached) = read_cached_branch(self.db.as_ref(), &self.metadata.id, start)? else {
            if read_entry_row(self.db.as_ref(), &self.metadata.id, start)?.is_none() {
                return Err(SessionError::not_found(format!("Entry not found: {start}")));
            }
            return Err(SessionError::new(
                SessionErrorCode::InvalidEntry,
                format!("Branch cache missing entry {start}"),
            ));
        };
        let rows = query_cached_branch_rows(
            self.db.as_ref(),
            &self.metadata.id,
            &cached,
            &CachedBranchQuery {
                entry_type: query.entry_type,
                custom_type: query.custom_type.clone(),
                stop_at_type: bounds.stop_at_type,
                stop_at_id: bounds.stop_at_id.clone(),
                cursor_after_seq: query.cursor.map(|cursor| cursor.after_seq),
                order: query.order,
                limit: query.limit,
            },
        )?;
        validate_cached_branch_rows(&rows, query, bounds)?;
        let mut entries = Vec::new();
        for row in &rows {
            let entry = decode_entry(row)?;
            if matches_entry_query(&entry, query) {
                entries.push(entry);
            }
        }
        truncate(entries, query.limit)
    }

    pub fn find_records(&self, query: &RecordQuery) -> Result<Vec<LaneRecord>, SessionError> {
        let rows = read_record_rows(
            self.db.as_ref(),
            &self.metadata.id,
            &RecordRowQuery {
                lane: query.lane.clone(),
                record_type: query.record_type.map(|value| value.as_str().to_owned()),
                run_id: query.run_id.clone(),
                operation_kind: query.operation_kind.map(|kind| {
                    match kind {
                        crate::session_types::OperationKind::Run => "run",
                        crate::session_types::OperationKind::Compaction => "compaction",
                        crate::session_types::OperationKind::Navigation => "navigation",
                    }
                    .to_owned()
                }),
                after_seq: query.after_seq,
                order: query.order,
                limit: query.limit,
            },
        )?;
        rows.iter()
            .map(|row| decode_record(row.seq, row.timestamp, &row.payload))
            .collect()
    }

    pub fn find_open_operations(
        &self,
        lane: &str,
    ) -> Result<Vec<OperationStartedRecord>, SessionError> {
        let rows = read_open_operation_rows(self.db.as_ref(), &self.metadata.id, lane)?;
        rows.iter()
            .map(
                |row| match decode_record(row.seq, row.timestamp, &row.payload)? {
                    LaneRecord::OperationStarted(record) => Ok(record),
                    _ => Err(SessionError::storage("Expected operation_started record")),
                },
            )
            .collect()
    }

    pub fn get_log(&self, options: &LogOptions) -> Result<Vec<LogItem>, SessionError> {
        let after_seq = options.after_seq.unwrap_or(0);
        let limit = options.limit;
        let entry_rows = read_entry_rows(
            self.db.as_ref(),
            &self.metadata.id,
            &EntryRowQuery {
                after_seq: Some(after_seq),
                order: Some(EntryOrder::OldestFirst),
                limit,
                ..EntryRowQuery::default()
            },
        )?;
        let record_rows = read_record_rows(
            self.db.as_ref(),
            &self.metadata.id,
            &RecordRowQuery {
                after_seq: Some(after_seq),
                order: Some(EntryOrder::OldestFirst),
                limit,
                ..RecordRowQuery::default()
            },
        )?;
        let lane_rows =
            read_lane_move_rows(self.db.as_ref(), &self.metadata.id, Some(after_seq), limit)?;
        let fact_rows =
            read_fact_rows(self.db.as_ref(), &self.metadata.id, Some(after_seq), limit)?;

        let mut items: Vec<(i64, LogItem)> = Vec::new();
        for row in &entry_rows {
            items.push((
                row.seq,
                LogItem::Entry {
                    seq: row.seq,
                    entry: decode_entry(row)?,
                },
            ));
        }
        for row in &record_rows {
            items.push((
                row.seq,
                LogItem::Record {
                    seq: row.seq,
                    record: decode_record(row.seq, row.timestamp, &row.payload)?,
                },
            ));
        }
        for row in &lane_rows {
            items.push((
                row.seq,
                LogItem::Lane {
                    seq: row.seq,
                    lane: row.lane.clone(),
                    leaf_id: row.leaf_id.clone(),
                },
            ));
        }
        for row in &fact_rows {
            let value = match &row.value {
                None => None,
                Some(value) => Some(serde_json::from_str::<String>(value).map_err(|_| {
                    SessionError::storage(format!("Invalid fact value at sequence {}", row.seq))
                })?),
            };
            let item = if row.kind == "name" {
                LogItem::Fact(LogFact::Name {
                    seq: row.seq,
                    name: value,
                })
            } else {
                LogItem::Fact(LogFact::Label {
                    seq: row.seq,
                    target_id: row.key.clone().unwrap_or_default(),
                    label: value,
                })
            };
            items.push((row.seq, item));
        }
        items.sort_by_key(|(seq, _)| *seq);
        let selected: Vec<LogItem> = match options.limit {
            None => items.into_iter().map(|(_, item)| item).collect(),
            Some(limit) => items
                .into_iter()
                .take(limit.max(0) as usize)
                .map(|(_, item)| item)
                .collect(),
        };
        Ok(selected)
    }

    pub fn get_name(&self) -> Result<Option<String>, SessionError> {
        decode_fact_value(read_latest_fact(
            self.db.as_ref(),
            &self.metadata.id,
            "name",
            None,
        )?)
    }

    pub async fn set_name(&self, name: Option<&str>) -> Result<(), SessionError> {
        let encoded = encode_fact_value(name)?;
        self.enqueue_write(|| {
            let seq = get_next_sequence(self.db.as_ref(), &self.metadata.id)?;
            append_fact(
                self.db.as_ref(),
                &self.metadata.id,
                seq,
                "name",
                None,
                encoded.as_deref(),
            )?;
            advance_sequence(self.db.as_ref(), &self.metadata.id, seq)
        })
        .await
    }

    pub fn get_label(&self, id: &str) -> Result<Option<String>, SessionError> {
        decode_fact_value(read_latest_fact(
            self.db.as_ref(),
            &self.metadata.id,
            "label",
            Some(id),
        )?)
    }

    pub async fn set_label(&self, id: &str, label: Option<&str>) -> Result<(), SessionError> {
        let encoded = encode_fact_value(label)?;
        self.enqueue_write(|| {
            if read_entry_row(self.db.as_ref(), &self.metadata.id, id)?.is_none() {
                return Err(SessionError::not_found(format!("Entry not found: {id}")));
            }
            let seq = get_next_sequence(self.db.as_ref(), &self.metadata.id)?;
            append_fact(
                self.db.as_ref(),
                &self.metadata.id,
                seq,
                "label",
                Some(id),
                encoded.as_deref(),
            )?;
            advance_sequence(self.db.as_ref(), &self.metadata.id, seq)
        })
        .await
    }

    pub fn get_stats(&self) -> Result<SessionStats, SessionError> {
        read_stats(self.db.as_ref(), &self.metadata.id)
    }
}

fn truncate(entries: Vec<Entry>, limit: Option<i64>) -> Result<Vec<Entry>, SessionError> {
    Ok(match limit {
        None => entries,
        Some(limit) => entries.into_iter().take(limit.max(0) as usize).collect(),
    })
}

fn decode_fact_value(
    row: Option<crate::sqlite::storage::facts::FactRow>,
) -> Result<Option<String>, SessionError> {
    let Some(value) = row.and_then(|row| row.value) else {
        return Ok(None);
    };
    serde_json::from_str::<String>(&value)
        .map(Some)
        .map_err(|_| SessionError::storage("Invalid fact value"))
}

fn encode_fact_value(value: Option<&str>) -> Result<Option<String>, SessionError> {
    match value {
        None => Ok(None),
        Some(value) => serde_json::to_string(value).map(Some).map_err(|error| {
            SessionError::new(SessionErrorCode::InvalidPayload, error.to_string())
        }),
    }
}

/// Adds the storage-assigned columns to a provisioned entry.
fn provisioned_to_entry(
    entry: &ProvisionedEntry,
    parent_id: Option<String>,
    seq: i64,
    timestamp: i64,
) -> Result<Entry, SessionError> {
    let value = serde_json::to_value(entry)
        .map_err(|error| SessionError::new(SessionErrorCode::InvalidPayload, error.to_string()))?;
    let Value::Object(mut object) = value else {
        return Err(SessionError::storage(
            "Entry did not serialize to an object",
        ));
    };
    object.insert(
        "parentId".to_owned(),
        parent_id.map_or(Value::Null, Value::from),
    );
    object.insert("seq".to_owned(), Value::from(seq));
    object.insert("timestamp".to_owned(), Value::from(timestamp));
    serde_json::from_value(Value::Object(object))
        .map_err(|error| SessionError::new(SessionErrorCode::InvalidEntry, error.to_string()))
}

fn with_seq_and_timestamp(
    record: &LaneRecord,
    seq: i64,
    timestamp: i64,
) -> Result<LaneRecord, SessionError> {
    let value = serde_json::to_value(record)
        .map_err(|error| SessionError::new(SessionErrorCode::InvalidPayload, error.to_string()))?;
    let Value::Object(mut object) = value else {
        return Err(SessionError::storage(
            "Record did not serialize to an object",
        ));
    };
    object.insert("seq".to_owned(), Value::from(seq));
    object.insert("timestamp".to_owned(), Value::from(timestamp));
    serde_json::from_value(Value::Object(object))
        .map_err(|error| SessionError::new(SessionErrorCode::InvalidPayload, error.to_string()))
}

struct RepositoryInner {
    options: SqliteSessionRepositoryOptions,
    lease_options: ResolvedWriterLeaseOptions,
    database: tokio::sync::Mutex<Option<Arc<dyn SqliteDatabase>>>,
    operations: tokio::sync::Mutex<()>,
    active_storages: Mutex<Vec<Arc<SqliteSessionStorage>>>,
}

pub struct SqliteSessionRepository {
    inner: Arc<RepositoryInner>,
}

impl SqliteSessionRepository {
    pub fn new(options: SqliteSessionRepositoryOptions) -> Result<Self, SessionError> {
        let lease_options = resolve_writer_lease_options(options.writer_lease)?;
        Ok(Self {
            inner: Arc::new(RepositoryInner {
                options,
                lease_options,
                database: tokio::sync::Mutex::new(None),
                operations: tokio::sync::Mutex::new(()),
                active_storages: Mutex::new(Vec::new()),
            }),
        })
    }

    async fn get_database(&self) -> Result<Arc<dyn SqliteDatabase>, SessionError> {
        let mut database = self.inner.database.lock().await;
        if let Some(db) = database.as_ref() {
            return Ok(Arc::clone(db));
        }
        let path = &self.inner.options.database_path;
        let directory = get_parent_path(path);
        std::fs::create_dir_all(&directory).map_err(|error| {
            SessionError::storage(format!(
                "Failed to create SQLite sessions directory {path}: {error}"
            ))
        })?;
        let db = self.inner.options.sqlite.open(path).await?;
        match (|| {
            configure_sqlite_database(db.as_ref())?;
            apply_migrations(db.as_ref()).map_err(SessionError::from)
        })() {
            Ok(()) => {
                *database = Some(Arc::clone(&db));
                Ok(db)
            }
            Err(error) => {
                db.close();
                Err(error)
            }
        }
    }

    fn track(&self, storage: Arc<SqliteSessionStorage>) -> Arc<SqliteSessionStorage> {
        self.inner
            .active_storages
            .lock()
            .expect("repository mutex")
            .push(Arc::clone(&storage));
        storage
    }

    fn find_active(&self, session_id: &str) -> Option<Arc<SqliteSessionStorage>> {
        self.inner
            .active_storages
            .lock()
            .expect("repository mutex")
            .iter()
            .find(|storage| storage.is_for_session(session_id))
            .cloned()
    }

    async fn release_storages_for_session(&self, session_id: &str) {
        let storages: Vec<Arc<SqliteSessionStorage>> = self
            .inner
            .active_storages
            .lock()
            .expect("repository mutex")
            .iter()
            .filter(|storage| storage.is_for_session(session_id))
            .cloned()
            .collect();
        for storage in storages {
            storage.release().await;
            self.forget(&storage);
        }
    }

    fn forget(&self, storage: &Arc<SqliteSessionStorage>) {
        self.inner
            .active_storages
            .lock()
            .expect("repository mutex")
            .retain(|candidate| !Arc::ptr_eq(candidate, storage));
    }

    pub async fn create(
        &self,
        options: SqliteSessionCreateOptions,
    ) -> Result<Arc<SqliteSessionStorage>, SessionError> {
        let _guard = self.inner.operations.lock().await;
        let db = self.get_database().await?;
        let path = self.inner.options.database_path.clone();
        let id = options.id.clone().unwrap_or_else(uuidv7);
        if session_exists(db.as_ref(), &id)? {
            return Err(SessionError::new(
                SessionErrorCode::AlreadyExists,
                format!("Session already exists: {id}"),
            ));
        }
        let created_at = now_ms();
        let lease_options = self.inner.lease_options;
        let lease = with_transaction(db.as_ref(), || {
            insert_session_row(
                db.as_ref(),
                &NewSessionRow {
                    id: id.clone(),
                    created_at,
                    cwd: options.cwd.clone(),
                    parent_session_id: options.parent_session_id.clone(),
                    metadata: options.metadata.clone(),
                },
            )
            .map_err(|error| SqliteError(error.message))?;
            create_sequence(db.as_ref(), &id, 1).map_err(|error| SqliteError(error.message))?;
            create_stats(db.as_ref(), &id, 0).map_err(|error| SqliteError(error.message))?;
            create_initial_lane(db.as_ref(), &id, "main", None)
                .map_err(|error| SqliteError(error.message))?;
            claim_writer_lease(db.as_ref(), &id, lease_options)
                .map_err(|error| SqliteError(error.message))
        })?;
        let row = require_session_row(db.as_ref(), &id)?;
        let metadata = decode_session_metadata(&row, &path)?;
        Ok(self.track(SqliteSessionStorage::new(
            db,
            metadata,
            lease,
            lease_options,
        )))
    }

    pub async fn open(
        &self,
        metadata: &SqliteSessionMetadata,
    ) -> Result<Arc<SqliteSessionStorage>, SessionError> {
        let _guard = self.inner.operations.lock().await;
        let db = self.get_database().await?;
        if let Some(active) = self.find_active(&metadata.id) {
            read_lanes(db.as_ref(), &metadata.id)?;
            return Ok(active);
        }
        require_session_row(db.as_ref(), &metadata.id)?;
        let lease_options = self.inner.lease_options;
        let (lease, row) = with_transaction(db.as_ref(), || {
            let lease = claim_writer_lease(db.as_ref(), &metadata.id, lease_options)
                .map_err(|error| SqliteError(error.message))?;
            let row = require_session_row(db.as_ref(), &metadata.id)
                .map_err(|error| SqliteError(error.message))?;
            read_lanes(db.as_ref(), &metadata.id).map_err(|error| SqliteError(error.message))?;
            Ok((lease, row))
        })?;
        let decoded = decode_session_metadata(&row, &metadata.path)?;
        Ok(self.track(SqliteSessionStorage::new(db, decoded, lease, lease_options)))
    }

    /// Rebuilds this session's private branch-read cache from canonical entry parent links.
    pub async fn repair_branch_cache(
        &self,
        metadata: &SqliteSessionMetadata,
    ) -> Result<(), SessionError> {
        self.release_storages_for_session(&metadata.id).await;
        let _guard = self.inner.operations.lock().await;
        let db = self.get_database().await?;
        let lease_options = self.inner.lease_options;
        with_transaction(db.as_ref(), || {
            let lease = claim_writer_lease(db.as_ref(), &metadata.id, lease_options)
                .map_err(|error| SqliteError(error.message))?;
            require_session_row(db.as_ref(), &metadata.id)
                .map_err(|error| SqliteError(error.message))?;
            rebuild_branch_cache(db.as_ref(), &metadata.id)
                .map_err(|error| SqliteError(error.message))?;
            release_writer_lease(db.as_ref(), &metadata.id, &lease)
                .map_err(|error| SqliteError(error.message))
        })?;
        Ok(())
    }

    /// Reads the session catalog without acquiring or renewing per-session writer leases.
    pub async fn list(
        &self,
        options: &SqliteSessionListOptions,
    ) -> Result<Vec<SqliteSessionMetadata>, SessionError> {
        let _guard = self.inner.operations.lock().await;
        let path = self.inner.options.database_path.clone();
        if !Path::new(&path).exists() {
            return Ok(Vec::new());
        }
        let db = self.get_database().await?;
        let rows = read_session_rows(db.as_ref(), options.cwd.as_deref())?;
        rows.iter()
            .map(|row| decode_session_metadata(row, &path))
            .collect()
    }

    pub async fn delete(&self, metadata: &SqliteSessionMetadata) -> Result<(), SessionError> {
        self.release_storages_for_session(&metadata.id).await;
        let _guard = self.inner.operations.lock().await;
        let db = self.get_database().await?;
        let lease_options = self.inner.lease_options;
        let session_id = metadata.id.clone();
        in_transaction(db.as_ref(), || {
            if !session_exists(db.as_ref(), &session_id)? {
                return delete_writer_lease(db.as_ref(), &session_id);
            }
            claim_writer_lease(db.as_ref(), &session_id, lease_options)?;
            delete_branch_cache(db.as_ref(), &session_id)?;
            delete_fact_rows(db.as_ref(), &session_id)?;
            delete_lane_rows(db.as_ref(), &session_id)?;
            delete_record_rows(db.as_ref(), &session_id)?;
            delete_entry_rows(db.as_ref(), &session_id)?;
            delete_writer_lease(db.as_ref(), &session_id)?;
            delete_stats(db.as_ref(), &session_id)?;
            delete_sequence(db.as_ref(), &session_id)?;
            delete_session_row(db.as_ref(), &session_id)
        })?;
        Ok(())
    }

    pub async fn fork(
        &self,
        source: &SqliteSessionMetadata,
        fork: &ForkOptions,
        options: SqliteSessionCreateOptions,
    ) -> Result<Arc<SqliteSessionStorage>, SessionError> {
        let _guard = self.inner.operations.lock().await;
        let db = self.get_database().await?;
        let path = self.inner.options.database_path.clone();
        let source_metadata =
            decode_session_metadata(&require_session_row(db.as_ref(), &source.id)?, &path)?;
        let id = options.id.clone().unwrap_or_else(uuidv7);
        if session_exists(db.as_ref(), &id)? {
            return Err(SessionError::new(
                SessionErrorCode::AlreadyExists,
                format!("Session already exists: {id}"),
            ));
        }

        let mut entries: Vec<EntryRow> = Vec::new();
        let mut lanes: Vec<(String, Option<String>)> = Vec::new();
        let mut branch_tips: Vec<String> = Vec::new();
        let mut branch_fork_target_id: Option<String> = None;

        if fork.scope == ForkScope::Tree {
            entries.extend(read_entry_rows(
                db.as_ref(),
                &source.id,
                &EntryRowQuery {
                    order: Some(EntryOrder::OldestFirst),
                    ..EntryRowQuery::default()
                },
            )?);
            lanes.extend(
                read_lanes(db.as_ref(), &source.id)?
                    .into_iter()
                    .map(|row| (row.lane, row.leaf_id)),
            );
            branch_tips.extend(read_branch_tip_ids(db.as_ref(), &source.id)?);
        } else {
            let main = read_lane(db.as_ref(), &source.id, "main")?.ok_or_else(|| {
                SessionError::new(SessionErrorCode::InvalidLane, "Lane not found: main")
            })?;
            let selected_entry_id = fork.entry_id.clone().or(main.leaf_id.clone());
            if let Some(selected_entry_id) = selected_entry_id {
                let target = read_entry_row(db.as_ref(), &source.id, &selected_entry_id)?;
                let target = match target {
                    Some(target) if target.entry_type == "message" => target,
                    _ => {
                        return Err(SessionError::new(
                            SessionErrorCode::InvalidForkTarget,
                            format!("Fork target is not a message entry: {selected_entry_id}"),
                        ));
                    }
                };
                let position = fork.position.unwrap_or(if fork.entry_id.is_none() {
                    ForkPosition::At
                } else {
                    ForkPosition::Before
                });
                branch_fork_target_id = if position == ForkPosition::At {
                    Some(target.id.clone())
                } else {
                    target.parent_id.clone()
                };
            }
            lanes.push(("main".to_owned(), branch_fork_target_id.clone()));
            if let Some(target_id) = &branch_fork_target_id {
                let cached =
                    read_cached_branch(db.as_ref(), &source.id, target_id)?.ok_or_else(|| {
                        SessionError::new(
                            SessionErrorCode::InvalidForkTarget,
                            format!("Fork target is not on a cached branch: {target_id}"),
                        )
                    })?;
                entries.extend(query_cached_branch_rows(
                    db.as_ref(),
                    &source.id,
                    &cached,
                    &CachedBranchQuery {
                        order: Some(EntryOrder::OldestFirst),
                        ..CachedBranchQuery::default()
                    },
                )?);
                branch_tips.push(target_id.clone());
            }
        }

        let copied_ids: Vec<String> = entries.iter().map(|entry| entry.id.clone()).collect();
        let latest_name = read_latest_fact(db.as_ref(), &source.id, "name", None)?;
        let latest_labels = read_latest_label_facts(db.as_ref(), &source.id)?;
        let labels_to_copy: Vec<(String, String)> = latest_labels
            .into_iter()
            .filter(|(key, _)| {
                fork.scope == ForkScope::Tree || copied_ids.iter().any(|id| id == key)
            })
            .collect();
        let created_at = now_ms();
        let metadata: Option<std::collections::BTreeMap<String, Value>> = options
            .metadata
            .clone()
            .or(source_metadata.metadata.clone());
        let lease_options = self.inner.lease_options;
        let scope = fork.scope;
        let parent_session_id = options
            .parent_session_id
            .clone()
            .or_else(|| Some(source.id.clone()));

        let lease = in_transaction(db.as_ref(), || {
            insert_session_row(
                db.as_ref(),
                &NewSessionRow {
                    id: id.clone(),
                    created_at,
                    cwd: options.cwd.clone(),
                    parent_session_id: parent_session_id.clone(),
                    metadata: metadata.clone(),
                },
            )?;
            create_sequence(db.as_ref(), &id, 1)?;
            create_stats(
                db.as_ref(),
                &id,
                entries
                    .iter()
                    .filter(|entry| entry.entry_type == "message")
                    .count() as i64,
            )?;

            let mut next_seq = 1i64;
            for entry in &entries {
                let seq = next_seq;
                next_seq += 1;
                insert_entry_row(
                    db.as_ref(),
                    &id,
                    &NewEntryRow {
                        seq,
                        id: entry.id.clone(),
                        parent_id: entry.parent_id.clone(),
                        entry_type: entry_type_from_str(&entry.entry_type)?,
                        timestamp: entry.timestamp,
                        payload: entry.payload.clone(),
                    },
                )?;
            }

            if scope == ForkScope::Tree {
                for (lane, leaf_id) in &lanes {
                    let seq = next_seq;
                    next_seq += 1;
                    create_lane(db.as_ref(), &id, seq, lane, leaf_id.as_deref())?;
                }
            } else {
                create_initial_lane(db.as_ref(), &id, "main", branch_fork_target_id.as_deref())?;
            }

            if let Some(name) = latest_name.as_ref().and_then(|fact| fact.value.as_ref()) {
                let seq = next_seq;
                next_seq += 1;
                append_fact(db.as_ref(), &id, seq, "name", None, Some(name))?;
            }
            for (key, value) in &labels_to_copy {
                let seq = next_seq;
                next_seq += 1;
                append_fact(db.as_ref(), &id, seq, "label", Some(key), Some(value))?;
            }

            set_next_sequence(db.as_ref(), &id, next_seq)?;
            for tip in &branch_tips {
                build_cached_branch(db.as_ref(), &id, tip)?;
            }
            claim_writer_lease(db.as_ref(), &id, lease_options)
        })?;

        let row = require_session_row(db.as_ref(), &id)?;
        let decoded = decode_session_metadata(&row, &path)?;
        Ok(self.track(SqliteSessionStorage::new(db, decoded, lease, lease_options)))
    }

    pub async fn close(&self) {
        {
            let _guard = self.inner.operations.lock().await;
        }
        let storages: Vec<Arc<SqliteSessionStorage>> = self
            .inner
            .active_storages
            .lock()
            .expect("repository mutex")
            .drain(..)
            .collect();
        for storage in storages {
            storage.release().await;
        }
        let mut database = self.inner.database.lock().await;
        if let Some(db) = database.take() {
            db.close();
        }
    }
}

fn entry_type_from_str(value: &str) -> Result<EntryType, SessionError> {
    Ok(match value {
        "message" => EntryType::Message,
        "model_change" => EntryType::ModelChange,
        "thinking_level_change" => EntryType::ThinkingLevelChange,
        "active_tools_change" => EntryType::ActiveToolsChange,
        "compaction" => EntryType::Compaction,
        "branch_summary" => EntryType::BranchSummary,
        "custom" => EntryType::Custom,
        other => {
            return Err(SessionError::storage(format!(
                "Unknown entry type: {other}"
            )));
        }
    })
}
