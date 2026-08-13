//! Port of `packages/session-backends/sqlite-node/src/sqlite/storage/facts.ts`.

use crate::session_types::SessionError;
use crate::sql;
use crate::sqlite::sql::{SqlQuery, fragment, param, text};
use crate::sqlite::types::{Row, SqliteDatabase};

use super::entries::{integer_column, string_column};

#[derive(Debug, Clone, PartialEq)]
pub struct FactRow {
    pub session_id: String,
    pub seq: i64,
    pub kind: String,
    pub key: Option<String>,
    pub value: Option<String>,
}

fn fact_row(row: &Row) -> FactRow {
    FactRow {
        session_id: string_column(row, "session_id"),
        seq: integer_column(row, "seq"),
        kind: string_column(row, "kind"),
        key: row
            .get("key")
            .and_then(|value| value.as_str())
            .map(str::to_owned),
        value: row
            .get("value")
            .and_then(|value| value.as_str())
            .map(str::to_owned),
    }
}

pub fn append_fact(
    db: &dyn SqliteDatabase,
    session_id: &str,
    seq: i64,
    kind: &str,
    key: Option<&str>,
    value: Option<&str>,
) -> Result<(), SessionError> {
    db.run(&sql![
        text("INSERT INTO facts (session_id, seq, kind, key, value) VALUES ("),
        param(session_id),
        text(", "),
        param(seq),
        text(", "),
        param(kind),
        text(", "),
        param(key.map(str::to_owned)),
        text(", "),
        param(value.map(str::to_owned)),
        text(")"),
    ])?;
    Ok(())
}

pub fn read_latest_fact(
    db: &dyn SqliteDatabase,
    session_id: &str,
    kind: &str,
    key: Option<&str>,
) -> Result<Option<FactRow>, SessionError> {
    let row = db.get(&sql![
        text(
            "SELECT session_id, seq, kind, key, value\n\t\tFROM facts INDEXED BY idx_facts_session_kind_key_seq\n\t\tWHERE session_id = "
        ),
        param(session_id),
        text(" AND kind = "),
        param(kind),
        text(" AND key IS "),
        param(key.map(str::to_owned)),
        text("\n\t\tORDER BY seq DESC\n\t\tLIMIT 1"),
    ])?;
    Ok(row.as_ref().map(fact_row))
}

pub fn read_latest_label_facts(
    db: &dyn SqliteDatabase,
    session_id: &str,
) -> Result<Vec<(String, String)>, SessionError> {
    let rows = db.all(&sql![
        text(
            "SELECT f.key, f.value\n\t\tFROM facts AS f INDEXED BY idx_facts_session_kind_key_seq\n\t\tWHERE f.session_id = "
        ),
        param(session_id),
        text(
            "\n\t\t\tAND f.kind = 'label'\n\t\t\tAND f.value IS NOT NULL\n\t\t\tAND f.seq = (\n\t\t\t\tSELECT MAX(candidate.seq)\n\t\t\t\tFROM facts AS candidate INDEXED BY idx_facts_session_kind_key_seq\n\t\t\t\tWHERE candidate.session_id = f.session_id\n\t\t\t\t\tAND candidate.kind = f.kind\n\t\t\t\t\tAND candidate.key IS f.key\n\t\t\t)\n\t\tORDER BY f.key"
        ),
    ])?;
    Ok(rows
        .iter()
        .map(|row| (string_column(row, "key"), string_column(row, "value")))
        .collect())
}

pub fn read_fact_rows(
    db: &dyn SqliteDatabase,
    session_id: &str,
    after_seq: Option<i64>,
    limit: Option<i64>,
) -> Result<Vec<FactRow>, SessionError> {
    let after = match after_seq {
        None => SqlQuery::default(),
        Some(after_seq) => sql![text(" AND seq > "), param(after_seq)],
    };
    let limit = match limit {
        None => SqlQuery::default(),
        Some(limit) => sql![text(" LIMIT "), param(limit)],
    };
    let rows = db.all(&sql![
        text("SELECT session_id, seq, kind, key, value\n\t\tFROM facts\n\t\tWHERE session_id = "),
        param(session_id),
        fragment(after),
        text("\n\t\tORDER BY seq"),
        fragment(limit),
    ])?;
    Ok(rows.iter().map(fact_row).collect())
}

pub fn delete_fact_rows(db: &dyn SqliteDatabase, session_id: &str) -> Result<(), SessionError> {
    db.run(&sql![
        text("DELETE FROM facts WHERE session_id = "),
        param(session_id)
    ])?;
    Ok(())
}
