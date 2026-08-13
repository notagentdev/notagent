//! Port of the node:sqlite adapter in
//! `packages/session-backends/sqlite-node/src/index.ts` — rusqlite instead of
//! `node:sqlite` (tech substitution from the master plan).

use std::sync::{Arc, Mutex};

use rusqlite::Connection;
use rusqlite::types::ValueRef;
use serde_json::{Map, Value};

use super::sql::SqlQuery;
use super::types::{Row, SqliteDatabase, SqliteDatabaseFactory, SqliteError, SqliteRunResult};

pub struct RusqliteDatabase {
    connection: Mutex<Option<Connection>>,
}

impl RusqliteDatabase {
    pub fn open(path: &str) -> Result<Self, SqliteError> {
        let connection = Connection::open(path)?;
        Ok(Self {
            connection: Mutex::new(Some(connection)),
        })
    }

    fn with_connection<T>(
        &self,
        body: impl FnOnce(&Connection) -> Result<T, SqliteError>,
    ) -> Result<T, SqliteError> {
        let guard = self.connection.lock().expect("sqlite mutex");
        let connection = guard
            .as_ref()
            .ok_or_else(|| SqliteError("SQLite database is closed".to_owned()))?;
        body(connection)
    }
}

fn row_to_json(row: &rusqlite::Row<'_>, columns: &[String]) -> Result<Row, SqliteError> {
    let mut result = Map::new();
    for (index, name) in columns.iter().enumerate() {
        let value = match row.get_ref(index)? {
            ValueRef::Null => Value::Null,
            ValueRef::Integer(value) => Value::from(value),
            ValueRef::Real(value) => Value::from(value),
            ValueRef::Text(value) => Value::from(String::from_utf8_lossy(value).into_owned()),
            ValueRef::Blob(value) => Value::from(value.to_vec()),
        };
        result.insert(name.clone(), value);
    }
    Ok(result)
}

impl SqliteDatabase for RusqliteDatabase {
    fn exec(&self, sql: &str) -> Result<(), SqliteError> {
        self.with_connection(|connection| {
            connection.execute_batch(sql)?;
            Ok(())
        })
    }

    fn run(&self, query: &SqlQuery) -> Result<SqliteRunResult, SqliteError> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(&query.query_text)?;
            let changes = statement.execute(rusqlite::params_from_iter(query.params.iter()))?;
            Ok(SqliteRunResult {
                changes: changes as u64,
                last_insert_rowid: Some(connection.last_insert_rowid()),
            })
        })
    }

    fn get(&self, query: &SqlQuery) -> Result<Option<Row>, SqliteError> {
        Ok(self.all(query)?.into_iter().next())
    }

    fn all(&self, query: &SqlQuery) -> Result<Vec<Row>, SqliteError> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(&query.query_text)?;
            let columns: Vec<String> = statement
                .column_names()
                .into_iter()
                .map(str::to_owned)
                .collect();
            let mut rows = statement.query(rusqlite::params_from_iter(query.params.iter()))?;
            let mut result = Vec::new();
            while let Some(row) = rows.next()? {
                result.push(row_to_json(row, &columns)?);
            }
            Ok(result)
        })
    }

    fn close(&self) {
        let mut guard = self.connection.lock().expect("sqlite mutex");
        if let Some(connection) = guard.take() {
            let _ = connection.close();
        }
    }
}

pub struct RusqliteFactory;

#[async_trait::async_trait]
impl SqliteDatabaseFactory for RusqliteFactory {
    async fn open(&self, path: &str) -> Result<Arc<dyn SqliteDatabase>, SqliteError> {
        Ok(Arc::new(RusqliteDatabase::open(path)?))
    }
}

/// Port of `createNodeSqliteFactory()`.
pub fn create_sqlite_factory() -> Arc<dyn SqliteDatabaseFactory> {
    Arc::new(RusqliteFactory)
}
