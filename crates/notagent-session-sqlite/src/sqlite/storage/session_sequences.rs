//! Port of `packages/session-backends/sqlite-node/src/sqlite/storage/session-sequences.ts`.

use crate::session_types::SessionError;
use crate::sql;
use crate::sqlite::sql::{param, text};
use crate::sqlite::types::SqliteDatabase;

pub fn create_sequence(
    db: &dyn SqliteDatabase,
    session_id: &str,
    next_seq: i64,
) -> Result<(), SessionError> {
    db.run(&sql![
        text("INSERT INTO session_sequences (session_id, next_seq) VALUES ("),
        param(session_id),
        text(", "),
        param(next_seq),
        text(")"),
    ])?;
    Ok(())
}

pub fn get_next_sequence(db: &dyn SqliteDatabase, session_id: &str) -> Result<i64, SessionError> {
    let row = db.get(&sql![
        text("SELECT next_seq FROM session_sequences WHERE session_id = "),
        param(session_id),
    ])?;
    let row = row.ok_or_else(|| {
        SessionError::storage(format!("Missing sequence row for session {session_id}"))
    })?;
    Ok(row
        .get("next_seq")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or_default())
}

pub fn set_next_sequence(
    db: &dyn SqliteDatabase,
    session_id: &str,
    next_seq: i64,
) -> Result<(), SessionError> {
    db.run(&sql![
        text("UPDATE session_sequences SET next_seq = "),
        param(next_seq),
        text(" WHERE session_id = "),
        param(session_id),
    ])?;
    Ok(())
}

pub fn advance_sequence(
    db: &dyn SqliteDatabase,
    session_id: &str,
    seq: i64,
) -> Result<(), SessionError> {
    set_next_sequence(db, session_id, seq + 1)
}

pub fn delete_sequence(db: &dyn SqliteDatabase, session_id: &str) -> Result<(), SessionError> {
    db.run(&sql![
        text("DELETE FROM session_sequences WHERE session_id = "),
        param(session_id)
    ])?;
    Ok(())
}
