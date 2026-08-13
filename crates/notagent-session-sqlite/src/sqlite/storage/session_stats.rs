//! Port of `packages/session-backends/sqlite-node/src/sqlite/storage/session-stats.ts`.

use notagent_ai::Usage;

use crate::session_types::{SessionError, SessionStats};
use crate::sql;
use crate::sqlite::sql::{param, text};
use crate::sqlite::types::SqliteDatabase;

pub fn create_stats(
    db: &dyn SqliteDatabase,
    session_id: &str,
    message_count: i64,
) -> Result<(), SessionError> {
    db.run(&sql![
        text(
            "INSERT INTO session_stats\n\t\t\t(session_id, message_count, cached_tokens, uncached_tokens, total_tokens, cost_total)\n\t\t\tVALUES ("
        ),
        param(session_id),
        text(", "),
        param(message_count),
        text(", 0, 0, 0, 0)"),
    ])?;
    Ok(())
}

pub fn read_stats(db: &dyn SqliteDatabase, session_id: &str) -> Result<SessionStats, SessionError> {
    let row = db.get(&sql![
        text(
            "SELECT session_id, message_count, cached_tokens, uncached_tokens, total_tokens, cost_total\n\t\tFROM session_stats\n\t\tWHERE session_id = "
        ),
        param(session_id),
    ])?;
    let row = row.ok_or_else(|| {
        SessionError::storage(format!("Missing stats row for session {session_id}"))
    })?;
    let number = |name: &str| {
        row.get(name)
            .and_then(serde_json::Value::as_f64)
            .unwrap_or_default()
    };
    Ok(SessionStats {
        message_count: row
            .get("message_count")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or_default(),
        cached_tokens: number("cached_tokens"),
        uncached_tokens: number("uncached_tokens"),
        total_tokens: number("total_tokens"),
        cost_total: number("cost_total"),
    })
}

pub fn increment_message_count(
    db: &dyn SqliteDatabase,
    session_id: &str,
) -> Result<(), SessionError> {
    let result = db.run(&sql![
        text("UPDATE session_stats SET message_count = message_count + 1 WHERE session_id = "),
        param(session_id),
    ])?;
    if result.changes != 1 {
        return Err(SessionError::storage(format!(
            "Missing stats row for session {session_id}"
        )));
    }
    Ok(())
}

pub fn add_usage_to_stats(
    db: &dyn SqliteDatabase,
    session_id: &str,
    usage: &Usage,
) -> Result<(), SessionError> {
    let result = db.run(&sql![
        text("UPDATE session_stats\n\t\tSET cached_tokens = cached_tokens + "),
        param(usage.cache_read as i64),
        text(",\n\t\t\tuncached_tokens = uncached_tokens + "),
        param((usage.input + usage.cache_write) as i64),
        text(",\n\t\t\ttotal_tokens = total_tokens + "),
        param(usage.total_tokens.unwrap_or(0) as i64),
        text(",\n\t\t\tcost_total = cost_total + "),
        param(usage.cost.total),
        text("\n\t\tWHERE session_id = "),
        param(session_id),
    ])?;
    if result.changes != 1 {
        return Err(SessionError::storage(format!(
            "Missing stats row for session {session_id}"
        )));
    }
    Ok(())
}

pub fn delete_stats(db: &dyn SqliteDatabase, session_id: &str) -> Result<(), SessionError> {
    db.run(&sql![
        text("DELETE FROM session_stats WHERE session_id = "),
        param(session_id)
    ])?;
    Ok(())
}
