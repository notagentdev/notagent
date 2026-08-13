//! Port of `packages/session-backends/sqlite-node/src/sqlite/types.ts`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::sql::SqlQuery;

/// Row of a query result: column name to value.
pub type Row = Map<String, Value>;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("{0}")]
pub struct SqliteError(pub String);

impl From<rusqlite::Error> for SqliteError {
    fn from(error: rusqlite::Error) -> Self {
        Self(error.to_string())
    }
}

/// Result of a prepared SQLite statement execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SqliteRunResult {
    /// Number of rows changed by the statement.
    pub changes: u64,
    /// Inserted row id when the backend exposes one.
    pub last_insert_rowid: Option<i64>,
}

/// SQLite database capability used by the SQLite session backend.
///
/// Deviation class 1: TS exposes prepared statements; Rust runs `SqlQuery`
/// values directly and returns rows as JSON objects, which keeps the typed
/// `get<TRow>`/`all<TRow>` surface of the original.
pub trait SqliteDatabase: Send + Sync {
    fn exec(&self, sql: &str) -> Result<(), SqliteError>;
    fn run(&self, query: &SqlQuery) -> Result<SqliteRunResult, SqliteError>;
    fn get(&self, query: &SqlQuery) -> Result<Option<Row>, SqliteError>;
    fn all(&self, query: &SqlQuery) -> Result<Vec<Row>, SqliteError>;
    fn close(&self);
}

/// Runs a write transaction. TS wraps this in `db.transaction(fn)`; Rust keeps
/// the trait object safe by exposing the three statements separately.
pub fn with_transaction<T>(
    db: &dyn SqliteDatabase,
    body: impl FnOnce() -> Result<T, SqliteError>,
) -> Result<T, SqliteError> {
    db.exec("BEGIN IMMEDIATE")?;
    match body() {
        Ok(value) => {
            db.exec("COMMIT")?;
            Ok(value)
        }
        Err(error) => {
            // Ignore rollback errors to rethrow the original error.
            let _ = db.exec("ROLLBACK");
            Err(error)
        }
    }
}

#[async_trait::async_trait]
pub trait SqliteDatabaseFactory: Send + Sync {
    async fn open(&self, path: &str) -> Result<std::sync::Arc<dyn SqliteDatabase>, SqliteError>;
}

/// `SessionMetadata` extension of the SQLite backend.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SqliteSessionMetadata {
    pub id: String,
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    pub cwd: String,
    pub path: String,
    /// Current session name projected from SQLite global facts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Opaque application-owned metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<BTreeMap<String, Value>>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SqliteSessionCreateOptions {
    pub id: Option<String>,
    pub parent_session_id: Option<String>,
    pub cwd: String,
    pub metadata: Option<BTreeMap<String, Value>>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SqliteSessionListOptions {
    pub cwd: Option<String>,
}
