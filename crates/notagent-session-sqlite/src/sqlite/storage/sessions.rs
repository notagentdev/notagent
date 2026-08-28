use std::collections::BTreeMap;

use serde_json::Value;

use crate::session_types::{SessionError, SessionErrorCode};
use crate::sql;
use crate::sqlite::sql::{SqlQuery, fragment, param, text};
use crate::sqlite::types::{Row, SqliteDatabase, SqliteSessionMetadata};

pub struct SessionRow {
    pub id: String,
    pub created_at: i64,
    pub metadata: Option<String>,
    pub cwd: String,
    pub parent_session_id: Option<String>,
    pub has_session_name: bool,
    pub session_name: Option<String>,
}

pub struct NewSessionRow {
    pub id: String,
    pub created_at: i64,
    pub cwd: String,
    pub parent_session_id: Option<String>,
    pub metadata: Option<BTreeMap<String, Value>>,
}

fn text_column(row: &Row, name: &str) -> Option<String> {
    row.get(name)
        .and_then(|value| value.as_str())
        .map(str::to_owned)
}

fn integer_column(row: &Row, name: &str) -> i64 {
    row.get(name)
        .and_then(serde_json::Value::as_i64)
        .unwrap_or_default()
}

pub fn session_row_from(row: &Row) -> SessionRow {
    SessionRow {
        id: text_column(row, "id").unwrap_or_default(),
        created_at: integer_column(row, "created_at"),
        metadata: text_column(row, "metadata"),
        cwd: text_column(row, "cwd").unwrap_or_default(),
        parent_session_id: text_column(row, "parent_session_id"),
        has_session_name: integer_column(row, "has_session_name") != 0,
        session_name: text_column(row, "session_name"),
    }
}

fn parse_metadata(
    metadata: Option<&str>,
    session_id: &str,
) -> Result<Option<BTreeMap<String, Value>>, SessionError> {
    let Some(metadata) = metadata else {
        return Ok(None);
    };
    let parsed: Value = serde_json::from_str(metadata).map_err(|_| {
        SessionError::storage(format!(
            "Invalid SQLite session {session_id}: metadata is not valid JSON"
        ))
    })?;
    match parsed {
        Value::Object(map) => Ok(Some(map.into_iter().collect())),
        _ => Err(SessionError::storage(format!(
            "Invalid SQLite session {session_id}: metadata must be an object"
        ))),
    }
}

fn serialize_metadata(
    metadata: Option<&BTreeMap<String, Value>>,
) -> Result<Option<String>, SessionError> {
    let Some(metadata) = metadata else {
        return Ok(None);
    };
    serde_json::to_string(metadata)
        .map(Some)
        .map_err(|error| SessionError::new(SessionErrorCode::InvalidPayload, error.to_string()))
}

pub fn session_exists(db: &dyn SqliteDatabase, session_id: &str) -> Result<bool, SessionError> {
    Ok(db
        .get(&sql![
            text("SELECT 1 AS found FROM sessions WHERE id = "),
            param(session_id)
        ])?
        .is_some())
}

pub fn insert_session_row(
    db: &dyn SqliteDatabase,
    session: &NewSessionRow,
) -> Result<(), SessionError> {
    db.run(&sql![
        text(
            "INSERT INTO sessions (id, created_at, metadata, cwd, parent_session_id)\n\t\tVALUES ("
        ),
        param(session.id.as_str()),
        text(", "),
        param(session.created_at),
        text(", "),
        param(serialize_metadata(session.metadata.as_ref())?),
        text(", "),
        param(session.cwd.as_str()),
        text(", "),
        param(session.parent_session_id.clone()),
        text(")"),
    ])?;
    Ok(())
}

const SESSION_SELECT: &str = "SELECT s.id, s.created_at, s.metadata, s.cwd, s.parent_session_id,
			name_fact.seq IS NOT NULL AS has_session_name,
			name_fact.value AS session_name
		FROM sessions AS s
		LEFT JOIN facts AS name_fact
			ON name_fact.session_id = s.id
			AND name_fact.kind = 'name'
			AND name_fact.key IS NULL
			AND name_fact.seq = (
				SELECT MAX(f.seq)
				FROM facts AS f
				WHERE f.session_id = s.id AND f.kind = 'name' AND f.key IS NULL
			)
		";

pub fn read_session_row(
    db: &dyn SqliteDatabase,
    session_id: &str,
) -> Result<Option<SessionRow>, SessionError> {
    let row = db.get(&sql![
        text(SESSION_SELECT),
        text("WHERE s.id = "),
        param(session_id)
    ])?;
    Ok(row.as_ref().map(session_row_from))
}

pub fn read_session_rows(
    db: &dyn SqliteDatabase,
    cwd: Option<&str>,
) -> Result<Vec<SessionRow>, SessionError> {
    let where_clause = match cwd {
        None => SqlQuery::default(),
        Some(cwd) => sql![text("WHERE s.cwd = "), param(cwd)],
    };
    let rows = db.all(&sql![
        text(SESSION_SELECT),
        fragment(where_clause),
        text("\n\t\tORDER BY s.created_at DESC"),
    ])?;
    Ok(rows.iter().map(session_row_from).collect())
}

pub fn delete_session_row(db: &dyn SqliteDatabase, session_id: &str) -> Result<(), SessionError> {
    db.run(&sql![
        text("DELETE FROM sessions WHERE id = "),
        param(session_id)
    ])?;
    Ok(())
}

fn parse_session_name(
    value: Option<&str>,
    session_id: &str,
) -> Result<Option<String>, SessionError> {
    let Some(value) = value else { return Ok(None) };
    let parsed: Value = serde_json::from_str(value).map_err(|_| {
        SessionError::storage(format!(
            "Invalid SQLite session {session_id}: name is not valid JSON"
        ))
    })?;
    match parsed {
        Value::String(name) => Ok(Some(name)),
        _ => Err(SessionError::storage(format!(
            "Invalid SQLite session {session_id}: name must be a string"
        ))),
    }
}

pub fn decode_session_metadata(
    row: &SessionRow,
    path: &str,
) -> Result<SqliteSessionMetadata, SessionError> {
    let metadata = parse_metadata(row.metadata.as_deref(), &row.id)?;
    let name = if row.has_session_name {
        parse_session_name(row.session_name.as_deref(), &row.id)?
    } else {
        None
    };
    Ok(SqliteSessionMetadata {
        id: row.id.clone(),
        created_at: row.created_at,
        name,
        cwd: row.cwd.clone(),
        path: path.to_owned(),
        parent_session_id: row.parent_session_id.clone(),
        metadata,
    })
}
