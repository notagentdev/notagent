//! Port of `packages/session-backends/sqlite-node/src/sqlite/storage/branch-entries.ts`.

use serde_json::Value;

use crate::session_types::{EntryType, SessionError, SessionErrorCode};
use crate::sql;
use crate::sqlite::sql::{SqlQuery, fragment, join_sql_fragments, param, text};
use crate::sqlite::types::{Row, SqliteDatabase};

use super::entries::{EntryRow, integer_column, string_column};

/// Derived root-to-tip branch cache membership. Canonical parent links remain in entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedBranch {
    pub branch_id: String,
    pub leaf_seq: i64,
}

#[derive(Debug, Clone, Default)]
pub struct CachedBranchQuery {
    pub entry_type: Option<EntryType>,
    pub custom_type: Option<String>,
    pub stop_at_type: Option<EntryType>,
    pub stop_at_id: Option<String>,
    pub cursor_after_seq: Option<i64>,
    pub order: Option<crate::session_types::EntryOrder>,
    pub limit: Option<i64>,
}

struct BranchPathEntryRow {
    id: String,
    seq: i64,
    entry_type: String,
    payload: String,
}

pub fn read_cached_branch(
    db: &dyn SqliteDatabase,
    session_id: &str,
    leaf_id: &str,
) -> Result<Option<CachedBranch>, SessionError> {
    let row = db.get(&sql![
        text("SELECT branch_id, entry_seq\n\t\tFROM branch_entries\n\t\tWHERE session_id = "),
        param(session_id),
        text(" AND entry_id = "),
        param(leaf_id),
        text("\n\t\tORDER BY branch_id\n\t\tLIMIT 1"),
    ])?;
    Ok(row.map(|row| CachedBranch {
        branch_id: string_column(&row, "branch_id"),
        leaf_seq: integer_column(&row, "entry_seq"),
    }))
}

pub fn query_cached_branch_rows(
    db: &dyn SqliteDatabase,
    session_id: &str,
    branch: &CachedBranch,
    query: &CachedBranchQuery,
) -> Result<Vec<EntryRow>, SessionError> {
    let oldest_first = query.order == Some(crate::session_types::EntryOrder::OldestFirst);
    let mut stop_predicates: Vec<SqlQuery> = Vec::new();
    if let Some(stop_at_type) = query.stop_at_type {
        stop_predicates.push(sql![
            text("stop.entry_type = "),
            param(stop_at_type.as_str())
        ]);
    }
    if let Some(stop_at_id) = &query.stop_at_id {
        stop_predicates.push(sql![text("stop.entry_id = "), param(stop_at_id.as_str())]);
    }

    let aggregate = if oldest_first { "MIN" } else { "MAX" };
    let boundary_comparison = if oldest_first { "<=" } else { ">=" };
    let cursor_comparison = if oldest_first { ">" } else { "<" };
    let direction = if oldest_first { "ASC" } else { "DESC" };
    let boundary = if stop_predicates.is_empty() {
        SqlQuery::default()
    } else {
        sql![
            text("SELECT "),
            text(aggregate),
            text(
                "(stop.entry_seq)\n\t\t\t\tFROM branch_entries AS stop\n\t\t\t\tWHERE stop.session_id = "
            ),
            param(session_id),
            text("\n\t\t\t\t\tAND stop.branch_id = "),
            param(branch.branch_id.as_str()),
            text("\n\t\t\t\t\tAND stop.entry_seq <= "),
            param(branch.leaf_seq),
            text("\n\t\t\t\t\tAND ("),
            fragment(join_sql_fragments(&stop_predicates, " OR ")),
            text(")"),
        ]
    };

    let mut predicates = vec![
        sql![text("b.session_id = "), param(session_id)],
        sql![text("b.branch_id = "), param(branch.branch_id.as_str())],
        sql![text("b.entry_seq <= "), param(branch.leaf_seq)],
    ];
    if !stop_predicates.is_empty() {
        predicates.push(sql![
            text("b.entry_seq "),
            text(boundary_comparison),
            text(" COALESCE(("),
            fragment(boundary),
            text("), "),
            param(if oldest_first { branch.leaf_seq } else { 0 }),
            text(")"),
        ]);
    }
    if let Some(after_seq) = query.cursor_after_seq {
        predicates.push(sql![
            text("b.entry_seq "),
            text(cursor_comparison),
            text(" "),
            param(after_seq)
        ]);
    }
    if let Some(entry_type) = query.entry_type {
        predicates.push(sql![text("b.entry_type = "), param(entry_type.as_str())]);
    }
    if let Some(custom_type) = &query.custom_type {
        predicates.push(sql![text("b.custom_type = "), param(custom_type.as_str())]);
    }
    let limit = match query.limit {
        None => SqlQuery::default(),
        Some(limit) => sql![text(" LIMIT "), param(limit)],
    };

    let rows = db.all(&sql![
        text(
            "SELECT e.session_id, e.id, e.seq AS entry_seq, e.parent_id, e.type, e.timestamp, e.payload\n\t\tFROM branch_entries AS b\n\t\tJOIN entries AS e ON e.session_id = b.session_id AND e.id = b.entry_id\n\t\tWHERE "
        ),
        fragment(join_sql_fragments(&predicates, " AND ")),
        text("\n\t\tORDER BY b.entry_seq "),
        text(direction),
        fragment(limit),
    ])?;
    Ok(rows.iter().map(cached_entry_row).collect())
}

/// TS reuses the row shape with `entry_seq` instead of `seq`.
fn cached_entry_row(row: &Row) -> EntryRow {
    EntryRow {
        session_id: string_column(row, "session_id"),
        seq: integer_column(row, "entry_seq"),
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

pub fn delete_branch_entries(
    db: &dyn SqliteDatabase,
    session_id: &str,
) -> Result<(), SessionError> {
    db.run(&sql![
        text("DELETE FROM branch_entries WHERE session_id = "),
        param(session_id)
    ])?;
    Ok(())
}

pub fn insert_branch_entry(
    db: &dyn SqliteDatabase,
    session_id: &str,
    branch_id: &str,
    entry_id: &str,
    entry_seq: i64,
    entry_type: &str,
    custom_type: Option<&str>,
) -> Result<(), SessionError> {
    db.run(&sql![
        text("INSERT INTO branch_entries\n\t\t\t(session_id, branch_id, entry_id, entry_seq, entry_type, custom_type)\n\t\t\tVALUES ("),
        param(session_id),
        text(", "),
        param(branch_id),
        text(", "),
        param(entry_id),
        text(", "),
        param(entry_seq),
        text(", "),
        param(entry_type),
        text(", "),
        param(custom_type.map(str::to_owned)),
        text(")"),
    ])?;
    Ok(())
}

fn custom_type_from_payload(row: &BranchPathEntryRow) -> Result<Option<String>, SessionError> {
    if row.entry_type != "custom" {
        return Ok(None);
    }
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
    match payload.get("customType").and_then(|value| value.as_str()) {
        Some(custom_type) => Ok(Some(custom_type.to_owned())),
        None => Err(invalid()),
    }
}

pub fn insert_branch_entries_for_path(
    db: &dyn SqliteDatabase,
    session_id: &str,
    branch_id: &str,
    leaf_id: &str,
) -> Result<(), SessionError> {
    let mut path: Vec<BranchPathEntryRow> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let mut entry_id = Some(leaf_id.to_owned());

    while let Some(current) = entry_id {
        if seen.contains(&current) {
            return Err(SessionError::new(
                SessionErrorCode::InvalidEntry,
                format!("Entry parent cycle at {current}"),
            ));
        }
        seen.push(current.clone());
        let row = db.get(&sql![
            text("SELECT id, seq, parent_id, type, payload\n\t\t\tFROM entries\n\t\t\tWHERE session_id = "),
            param(session_id),
            text(" AND id = "),
            param(current.as_str()),
        ])?;
        let row = row.ok_or_else(|| {
            SessionError::new(
                SessionErrorCode::InvalidEntry,
                format!("Entry {current} not found"),
            )
        })?;
        let parent_id = row
            .get("parent_id")
            .and_then(|value| value.as_str())
            .map(str::to_owned);
        path.push(BranchPathEntryRow {
            id: string_column(&row, "id"),
            seq: integer_column(&row, "seq"),
            entry_type: string_column(&row, "type"),
            payload: string_column(&row, "payload"),
        });
        entry_id = parent_id;
    }

    for row in path.iter().rev() {
        insert_branch_entry(
            db,
            session_id,
            branch_id,
            &row.id,
            row.seq,
            &row.entry_type,
            custom_type_from_payload(row)?.as_deref(),
        )?;
    }
    Ok(())
}

pub fn read_branch_containing_entry(
    db: &dyn SqliteDatabase,
    session_id: &str,
    entry_id: &str,
) -> Result<Option<(String, i64)>, SessionError> {
    let row = db.get(&sql![
        text("SELECT b.branch_id, b.entry_seq\n\t\tFROM branch_entries AS b\n\t\tWHERE b.session_id = "),
        param(session_id),
        text(" AND b.entry_id = "),
        param(entry_id),
        text("\n\t\tORDER BY b.branch_id\n\t\tLIMIT 1"),
    ])?;
    Ok(row.map(|row| {
        (
            string_column(&row, "branch_id"),
            integer_column(&row, "entry_seq"),
        )
    }))
}

pub fn copy_branch_entries_through_seq(
    db: &dyn SqliteDatabase,
    session_id: &str,
    target_branch_id: &str,
    source_branch_id: &str,
    through_seq: i64,
) -> Result<(), SessionError> {
    db.run(&sql![
        text("INSERT INTO branch_entries (session_id, branch_id, entry_id, entry_seq, entry_type, custom_type)\n\t\tSELECT session_id, "),
        param(target_branch_id),
        text(", entry_id, entry_seq, entry_type, custom_type\n\t\tFROM branch_entries\n\t\tWHERE session_id = "),
        param(session_id),
        text(" AND branch_id = "),
        param(source_branch_id),
        text(" AND entry_seq <= "),
        param(through_seq),
    ])?;
    Ok(())
}
