use serde_json::{Map, Value};

use crate::session_types::{Entry, EntryOrder, EntryType, SessionError};
use crate::sql;
use crate::sqlite::sql::{SqlQuery, fragment, param, text};
use crate::sqlite::types::{Row, SqliteDatabase};

#[derive(Debug, Clone, PartialEq)]
pub struct EntryRow {
    pub session_id: String,
    pub seq: i64,
    pub id: String,
    pub parent_id: Option<String>,
    pub entry_type: String,
    pub timestamp: i64,
    pub payload: String,
}

pub struct NewEntryRow {
    pub seq: i64,
    pub id: String,
    pub parent_id: Option<String>,
    pub entry_type: EntryType,
    pub timestamp: i64,
    pub payload: String,
}

pub(crate) fn entry_row(row: &Row) -> EntryRow {
    EntryRow {
        session_id: string_column(row, "session_id"),
        seq: integer_column(row, "seq"),
        id: string_column(row, "id"),
        parent_id: row
            .get("parent_id")
            .and_then(|value| value.as_str())
            .map(str::to_owned),
        entry_type: string_column(row, "type"),
        timestamp: integer_column(row, "timestamp"),
        payload: string_column(row, "payload"),
    }
}

pub(crate) fn string_column(row: &Row, name: &str) -> String {
    row.get(name)
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_owned()
}

pub(crate) fn integer_column(row: &Row, name: &str) -> i64 {
    row.get(name).and_then(Value::as_i64).unwrap_or_default()
}

pub fn entry_payload(entry: &Entry) -> Result<Map<String, Value>, SessionError> {
    let value = serde_json::to_value(entry).map_err(|error| {
        SessionError::new(
            crate::session_types::SessionErrorCode::InvalidPayload,
            error.to_string(),
        )
    })?;
    let Value::Object(mut object) = value else {
        return Err(SessionError::storage(
            "Entry did not serialize to an object",
        ));
    };
    for key in ["type", "id", "seq", "parentId", "timestamp"] {
        object.remove(key);
    }
    Ok(object)
}

const ENTRY_SELECT: &str = "SELECT session_id, seq, id, parent_id, type, timestamp, payload\n\t\tFROM entries\n\t\tWHERE session_id = ";

pub fn insert_entry_row(
    db: &dyn SqliteDatabase,
    session_id: &str,
    entry: &NewEntryRow,
) -> Result<(), SessionError> {
    db.run(&sql![
        text("INSERT INTO entries (session_id, id, seq, parent_id, type, timestamp, payload)\n\t\tVALUES ("),
        param(session_id),
        text(", "),
        param(entry.id.as_str()),
        text(", "),
        param(entry.seq),
        text(", "),
        param(entry.parent_id.clone()),
        text(", "),
        param(entry.entry_type.as_str()),
        text(", "),
        param(entry.timestamp),
        text(", "),
        param(entry.payload.as_str()),
        text(")"),
    ])?;
    Ok(())
}

pub fn read_entry_row(
    db: &dyn SqliteDatabase,
    session_id: &str,
    entry_id: &str,
) -> Result<Option<EntryRow>, SessionError> {
    let row = db.get(&sql![
        text(ENTRY_SELECT),
        param(session_id),
        text(" AND id = "),
        param(entry_id)
    ])?;
    Ok(row.as_ref().map(entry_row))
}

#[derive(Debug, Clone, Default)]
pub struct EntryRowQuery {
    pub after_seq: Option<i64>,
    pub cursor_after_seq: Option<i64>,
    pub entry_type: Option<EntryType>,
    pub order: Option<EntryOrder>,
    pub limit: Option<i64>,
}

pub fn read_entry_rows(
    db: &dyn SqliteDatabase,
    session_id: &str,
    options: &EntryRowQuery,
) -> Result<Vec<EntryRow>, SessionError> {
    let oldest_first = options.order == Some(EntryOrder::OldestFirst);
    let after = match options.after_seq {
        None => SqlQuery::default(),
        Some(after_seq) => sql![text(" AND seq > "), param(after_seq)],
    };
    let cursor = match options.cursor_after_seq {
        None => SqlQuery::default(),
        Some(after_seq) if oldest_first => sql![text(" AND seq > "), param(after_seq)],
        Some(after_seq) => sql![text(" AND seq < "), param(after_seq)],
    };
    let entry_type = match options.entry_type {
        None => SqlQuery::default(),
        Some(entry_type) => sql![text(" AND type = "), param(entry_type.as_str())],
    };
    let direction = if oldest_first { "ASC" } else { "DESC" };
    let limit = match options.limit {
        None => SqlQuery::default(),
        Some(limit) => sql![text(" LIMIT "), param(limit)],
    };
    let rows = db.all(&sql![
        text(ENTRY_SELECT),
        param(session_id),
        fragment(after),
        fragment(cursor),
        fragment(entry_type),
        text("\n\t\tORDER BY seq "),
        text(direction),
        fragment(limit),
    ])?;
    Ok(rows.iter().map(entry_row).collect())
}

pub fn id_exists_in_entries(
    db: &dyn SqliteDatabase,
    session_id: &str,
    id: &str,
) -> Result<bool, SessionError> {
    Ok(db
        .get(&sql![
            text("SELECT 1 AS found FROM entries WHERE session_id = "),
            param(session_id),
            text(" AND id = "),
            param(id),
            text(" LIMIT 1"),
        ])?
        .is_some())
}

pub fn delete_entry_rows(db: &dyn SqliteDatabase, session_id: &str) -> Result<(), SessionError> {
    db.run(&sql![
        text("DELETE FROM entries WHERE session_id = "),
        param(session_id)
    ])?;
    Ok(())
}
