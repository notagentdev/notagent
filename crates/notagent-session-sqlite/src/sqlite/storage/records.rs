//! Port of `packages/session-backends/sqlite-node/src/sqlite/storage/records.ts`.

use crate::session_types::{EntryOrder, SessionError};
use crate::sql;
use crate::sqlite::sql::{SqlQuery, fragment, join_sql_fragments, param, text};
use crate::sqlite::types::{Row, SqliteDatabase};

use super::entries::{integer_column, string_column};

#[derive(Debug, Clone, PartialEq)]
pub struct RecordRow {
    pub session_id: String,
    pub seq: i64,
    pub id: String,
    pub lane: String,
    pub run_id: Option<String>,
    pub record_type: String,
    pub op_kind: Option<String>,
    pub timestamp: i64,
    pub payload: String,
}

pub struct NewRecordRow {
    pub seq: i64,
    pub id: String,
    pub lane: String,
    pub run_id: Option<String>,
    pub record_type: String,
    pub op_kind: Option<String>,
    pub timestamp: i64,
    pub payload: String,
}

const RECORD_COLUMNS: &str = "SELECT session_id, seq, id, lane, run_id, type, op_kind, timestamp, payload\n\t\tFROM records\n\t\t";

fn record_row(row: &Row) -> RecordRow {
    RecordRow {
        session_id: string_column(row, "session_id"),
        seq: integer_column(row, "seq"),
        id: string_column(row, "id"),
        lane: string_column(row, "lane"),
        run_id: row
            .get("run_id")
            .and_then(|value| value.as_str())
            .map(str::to_owned),
        record_type: string_column(row, "type"),
        op_kind: row
            .get("op_kind")
            .and_then(|value| value.as_str())
            .map(str::to_owned),
        timestamp: integer_column(row, "timestamp"),
        payload: string_column(row, "payload"),
    }
}

pub fn append_record_row(
    db: &dyn SqliteDatabase,
    session_id: &str,
    record: &NewRecordRow,
) -> Result<(), SessionError> {
    db.run(&sql![
        text("INSERT INTO records\n\t\t\t(session_id, seq, id, lane, run_id, type, op_kind, timestamp, payload)\n\t\t\tVALUES ("),
        param(session_id),
        text(", "),
        param(record.seq),
        text(", "),
        param(record.id.as_str()),
        text(", "),
        param(record.lane.as_str()),
        text(", "),
        param(record.run_id.clone()),
        text(", "),
        param(record.record_type.as_str()),
        text(", "),
        param(record.op_kind.clone()),
        text(", "),
        param(record.timestamp),
        text(", "),
        param(record.payload.as_str()),
        text(")"),
    ])?;
    Ok(())
}

pub fn id_exists_in_records(
    db: &dyn SqliteDatabase,
    session_id: &str,
    id: &str,
) -> Result<bool, SessionError> {
    Ok(db
        .get(&sql![
            text("SELECT 1 AS found FROM records WHERE session_id = "),
            param(session_id),
            text(" AND id = "),
            param(id),
            text(" LIMIT 1"),
        ])?
        .is_some())
}

pub fn delete_record_rows(db: &dyn SqliteDatabase, session_id: &str) -> Result<(), SessionError> {
    db.run(&sql![
        text("DELETE FROM records WHERE session_id = "),
        param(session_id)
    ])?;
    Ok(())
}

#[derive(Debug, Clone, Default)]
pub struct RecordRowQuery {
    pub lane: Option<String>,
    pub record_type: Option<String>,
    pub run_id: Option<String>,
    pub operation_kind: Option<String>,
    pub after_seq: Option<i64>,
    pub order: Option<EntryOrder>,
    pub limit: Option<i64>,
}

pub fn read_record_rows(
    db: &dyn SqliteDatabase,
    session_id: &str,
    query: &RecordRowQuery,
) -> Result<Vec<RecordRow>, SessionError> {
    let mut predicates = vec![sql![text("session_id = "), param(session_id)]];
    if let Some(lane) = &query.lane {
        predicates.push(sql![text("lane = "), param(lane.as_str())]);
    }
    if let Some(record_type) = &query.record_type {
        predicates.push(sql![text("type = "), param(record_type.as_str())]);
    }
    if let Some(run_id) = &query.run_id {
        predicates.push(sql![text("run_id = "), param(run_id.as_str())]);
    }
    if let Some(operation_kind) = &query.operation_kind {
        predicates.push(sql![text("op_kind = "), param(operation_kind.as_str())]);
    }
    if let Some(after_seq) = query.after_seq {
        predicates.push(sql![text("seq > "), param(after_seq)]);
    }
    let direction = if query.order == Some(EntryOrder::OldestFirst) {
        "ASC"
    } else {
        "DESC"
    };
    let limit = match query.limit {
        None => SqlQuery::default(),
        Some(limit) => sql![text(" LIMIT "), param(limit)],
    };
    let rows = db.all(&sql![
        text(RECORD_COLUMNS),
        text("WHERE "),
        fragment(join_sql_fragments(&predicates, " AND ")),
        text("\n\t\tORDER BY seq "),
        text(direction),
        fragment(limit),
    ])?;
    Ok(rows.iter().map(record_row).collect())
}

/// TS ignores the `limit` option: a lane has at most one open operation.
pub fn read_open_operation_rows(
    db: &dyn SqliteDatabase,
    session_id: &str,
    lane: &str,
) -> Result<Vec<RecordRow>, SessionError> {
    let lane_row = db.get(&sql![
        text("SELECT open_operation_id FROM lanes WHERE session_id = "),
        param(session_id),
        text(" AND lane = "),
        param(lane),
    ])?;
    let Some(open_operation_id) = lane_row.and_then(|row| {
        row.get("open_operation_id")
            .and_then(|value| value.as_str())
            .map(str::to_owned)
    }) else {
        return Ok(Vec::new());
    };

    let record = db.get(&sql![
        text(RECORD_COLUMNS),
        text("WHERE session_id = "),
        param(session_id),
        text("\n\t\t\tAND id = "),
        param(open_operation_id.as_str()),
    ])?;
    let Some(record) = record.as_ref().map(record_row) else {
        return Err(SessionError::storage(format!(
            "Lane {lane} points at missing open operation {open_operation_id}"
        )));
    };
    if record.lane != lane || record.record_type != "operation_started" {
        return Err(SessionError::storage(format!(
            "Lane {lane} points at invalid open operation {open_operation_id}"
        )));
    }
    Ok(vec![record])
}
