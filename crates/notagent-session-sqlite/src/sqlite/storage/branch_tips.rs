//! Port of `packages/session-backends/sqlite-node/src/sqlite/storage/branch-tips.ts`.

use crate::session_types::SessionError;
use crate::sql;
use crate::sqlite::sql::{param, text};
use crate::sqlite::types::SqliteDatabase;

use super::entries::string_column;

pub fn read_branch_tip_ids(
    db: &dyn SqliteDatabase,
    session_id: &str,
) -> Result<Vec<String>, SessionError> {
    let rows = db.all(&sql![
        text("SELECT tip_id FROM branch_tips WHERE session_id = "),
        param(session_id),
        text(" ORDER BY tip_id"),
    ])?;
    Ok(rows
        .iter()
        .map(|row| string_column(row, "tip_id"))
        .collect())
}

pub fn read_branch_tip_branch_id(
    db: &dyn SqliteDatabase,
    session_id: &str,
    tip_id: &str,
) -> Result<Option<String>, SessionError> {
    let row = db.get(&sql![
        text("SELECT branch_id FROM branch_tips WHERE session_id = "),
        param(session_id),
        text(" AND tip_id = "),
        param(tip_id),
    ])?;
    Ok(row.as_ref().map(|row| string_column(row, "branch_id")))
}

pub fn insert_branch_tip(
    db: &dyn SqliteDatabase,
    session_id: &str,
    tip_id: &str,
    branch_id: &str,
) -> Result<(), SessionError> {
    db.run(&sql![
        text("INSERT INTO branch_tips (session_id, tip_id, branch_id) VALUES ("),
        param(session_id),
        text(", "),
        param(tip_id),
        text(", "),
        param(branch_id),
        text(")"),
    ])?;
    Ok(())
}

pub fn update_branch_tip(
    db: &dyn SqliteDatabase,
    session_id: &str,
    branch_id: &str,
    old_tip_id: &str,
    new_tip_id: &str,
) -> Result<bool, SessionError> {
    let result = db.run(&sql![
        text("UPDATE branch_tips SET tip_id = "),
        param(new_tip_id),
        text("\n\t\tWHERE session_id = "),
        param(session_id),
        text(" AND branch_id = "),
        param(branch_id),
        text(" AND tip_id = "),
        param(old_tip_id),
    ])?;
    Ok(result.changes == 1)
}

pub fn delete_branch_tips(db: &dyn SqliteDatabase, session_id: &str) -> Result<(), SessionError> {
    db.run(&sql![
        text("DELETE FROM branch_tips WHERE session_id = "),
        param(session_id)
    ])?;
    Ok(())
}
