use std::path::Path;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::session_types::{SessionError, SessionSearchHit, SessionSearchOptions};
use crate::sql;
use crate::sqlite::migrations::apply_migrations;
use crate::sqlite::sql::{SqlQuery, join_sql_fragments, param, text};
use crate::sqlite::storage::entries::{integer_column, string_column};
use crate::sqlite::storage::sessions::decode_session_metadata;
use crate::sqlite::types::{
    SqliteDatabase, SqliteDatabaseFactory, SqliteSessionMetadata, with_transaction,
};

pub struct SqliteSessionSearchOptions {
    pub sqlite: Arc<dyn SqliteDatabaseFactory>,
    pub database_path: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SqliteSessionSearchHit {
    pub session_id: String,
    pub metadata: SqliteSessionMetadata,
    pub entry_id: String,
    pub timestamp: i64,
    pub score: f64,
}

impl SessionSearchHit for SqliteSessionSearchHit {
    fn session_id(&self) -> &str {
        &self.session_id
    }

    fn entry_id(&self) -> &str {
        &self.entry_id
    }
}

pub(crate) fn configure_sqlite_database(db: &dyn SqliteDatabase) -> Result<(), SessionError> {
    db.exec("PRAGMA journal_mode=WAL")?;
    db.exec("PRAGMA synchronous=FULL")?;
    db.exec("PRAGMA busy_timeout=5000")?;
    Ok(())
}

fn table_exists(db: &dyn SqliteDatabase, name: &str) -> Result<bool, SessionError> {
    Ok(db
        .get(&sql![
            text("SELECT 1 AS found FROM sqlite_master WHERE type = 'table' AND name = "),
            param(name),
            text(" LIMIT 1"),
        ])?
        .is_some())
}

fn rebuild_search_index(db: &dyn SqliteDatabase) -> Result<(), SessionError> {
    db.run(&SqlQuery::raw(
        "INSERT INTO session_search_fts(session_search_fts) VALUES('rebuild')",
    ))?;
    Ok(())
}

const SEARCH_SCHEMA: &str = "
CREATE VIRTUAL TABLE IF NOT EXISTS session_search_fts USING fts5(
  payload,
  content = 'entries',
  content_rowid = 'rowid',
  tokenize = 'trigram remove_diacritics 1'
);
CREATE TRIGGER IF NOT EXISTS session_search_fts_ai AFTER INSERT ON entries BEGIN
  INSERT INTO session_search_fts(rowid, payload) VALUES (new.rowid, new.payload);
END;
CREATE TRIGGER IF NOT EXISTS session_search_fts_ad AFTER DELETE ON entries BEGIN
  INSERT INTO session_search_fts(session_search_fts, rowid, payload) VALUES('delete', old.rowid, old.payload);
END;
CREATE TRIGGER IF NOT EXISTS session_search_fts_au AFTER UPDATE OF payload ON entries BEGIN
  INSERT INTO session_search_fts(session_search_fts, rowid, payload) VALUES('delete', old.rowid, old.payload);
  INSERT INTO session_search_fts(rowid, payload) VALUES (new.rowid, new.payload);
END;
";

pub fn ensure_search_schema(db: &dyn SqliteDatabase) -> Result<(), SessionError> {
    let fts_exists = table_exists(db, "session_search_fts")?;
    let entries_exist = table_exists(db, "entries")?;
    with_transaction(db, |db| {
        db.exec(SEARCH_SCHEMA)?;
        if !fts_exists && entries_exist {
            rebuild_search_index(db)
                .map_err(|error| crate::sqlite::types::SqliteError(error.message))?;
        }
        Ok(())
    })?;
    Ok(())
}

/// SQLite FTS search over a co-located canonical session database.
pub struct SqliteSessionSearch {
    options: SqliteSessionSearchOptions,
}

impl SqliteSessionSearch {
    pub fn new(options: SqliteSessionSearchOptions) -> Self {
        Self { options }
    }

    async fn open_database(&self) -> Result<Arc<dyn SqliteDatabase>, SessionError> {
        let path = &self.options.database_path;
        if let Some(directory) = Path::new(path).parent() {
            std::fs::create_dir_all(directory).map_err(|error| {
                SessionError::storage(format!(
                    "Failed to create SQLite search directory {}: {error}",
                    directory.display()
                ))
            })?;
        }
        let db = self.options.sqlite.open(path).await?;
        match (|| {
            configure_sqlite_database(db.as_ref())?;
            apply_migrations(db.as_ref())?;
            ensure_search_schema(db.as_ref())
        })() {
            Ok(()) => Ok(db),
            Err(error) => {
                db.close();
                Err(error)
            }
        }
    }

    pub async fn search(
        &self,
        text_query: &str,
        options: &SessionSearchOptions,
        cancel: Option<&CancellationToken>,
    ) -> Result<Vec<SqliteSessionSearchHit>, SessionError> {
        let query_text = text_query.trim();
        if query_text.is_empty() || options.limit.is_some_and(|limit| limit <= 0) {
            return Ok(Vec::new());
        }
        if options
            .entry_types
            .as_ref()
            .is_some_and(std::vec::Vec::is_empty)
        {
            return Ok(Vec::new());
        }
        throw_if_aborted(cancel)?;
        let db = self.open_database().await?;
        let result = self.run_search(db.as_ref(), query_text, options, cancel);
        db.close();
        result
    }

    fn run_search(
        &self,
        db: &dyn SqliteDatabase,
        query_text: &str,
        options: &SessionSearchOptions,
        cancel: Option<&CancellationToken>,
    ) -> Result<Vec<SqliteSessionSearchHit>, SessionError> {
        let escaped = format!("\"{}\"", query_text.replace('"', "\"\""));
        let mut predicates = vec![sql![
            text("session_search_fts MATCH "),
            param(escaped.as_str())
        ]];
        if let Some(entry_types) = &options.entry_types {
            let mut parts = vec![crate::sqlite::sql::text("se.type IN (")];
            for (index, entry_type) in entry_types.iter().enumerate() {
                if index > 0 {
                    parts.push(crate::sqlite::sql::text(", "));
                }
                parts.push(param(entry_type.as_str()));
            }
            parts.push(crate::sqlite::sql::text(")"));
            predicates.push(SqlQuery::compose(parts));
        }
        let rows = db.all(&sql![
            text(
                "SELECT s.id, s.created_at, s.metadata, s.cwd, s.parent_session_id,\n\t\t\t\t\t\tname_fact.seq IS NOT NULL AS has_session_name,\n\t\t\t\t\t\tname_fact.value AS session_name,\n\t\t\t\t\t\tse.id AS entry_id, se.timestamp, bm25(session_search_fts) AS score\n\t\t\t\t\tFROM session_search_fts\n\t\t\t\t\tJOIN entries AS se ON se.rowid = session_search_fts.rowid\n\t\t\t\t\tJOIN sessions AS s ON s.id = se.session_id\n\t\t\t\t\tLEFT JOIN facts AS name_fact\n\t\t\t\t\t\tON name_fact.session_id = s.id\n\t\t\t\t\t\tAND name_fact.kind = 'name'\n\t\t\t\t\t\tAND name_fact.key IS NULL\n\t\t\t\t\t\tAND name_fact.seq = (\n\t\t\t\t\t\t\tSELECT MAX(f.seq)\n\t\t\t\t\t\t\tFROM facts AS f\n\t\t\t\t\t\t\tWHERE f.session_id = s.id AND f.kind = 'name' AND f.key IS NULL\n\t\t\t\t\t\t)\n\t\t\t\t\tWHERE "
            ),
            crate::sqlite::sql::fragment(join_sql_fragments(&predicates, " AND ")),
            text("\n\t\t\t\t\tORDER BY score\n\t\t\t\t\tLIMIT "),
            param(options.limit.unwrap_or(-1)),
        ])?;

        let mut hits = Vec::with_capacity(rows.len());
        for row in &rows {
            throw_if_aborted(cancel)?;
            let session_row = crate::sqlite::storage::sessions::session_row_from(row);
            hits.push(SqliteSessionSearchHit {
                session_id: session_row.id.clone(),
                metadata: decode_session_metadata(&session_row, &self.options.database_path)?,
                entry_id: string_column(row, "entry_id"),
                timestamp: integer_column(row, "timestamp"),
                score: row
                    .get("score")
                    .and_then(serde_json::Value::as_f64)
                    .unwrap_or_default(),
            });
        }
        Ok(hits)
    }
}

fn throw_if_aborted(cancel: Option<&CancellationToken>) -> Result<(), SessionError> {
    match cancel {
        Some(token) if token.is_cancelled() => {
            Err(SessionError::storage("The operation was aborted"))
        }
        _ => Ok(()),
    }
}

pub fn create_sqlite_session_search(options: SqliteSessionSearchOptions) -> SqliteSessionSearch {
    SqliteSessionSearch::new(options)
}
