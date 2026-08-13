//! SQLite session backend (SessionRepo/SessionStorage).
//!
//! 1:1 port of `packages/session-backends/sqlite-node` (see
//! crates/notagent-session-sqlite/PARITY.md).
//! Port of `packages/session-backends/sqlite-node/src/index.ts`.

pub mod session;
pub mod session_types;
pub mod sqlite;

pub use session::{Session, Uuidv7IdGenerator, assert_json_serializable};
pub use sqlite::database::{RusqliteDatabase, RusqliteFactory, create_sqlite_factory};
pub use sqlite::migrations::{SqliteMigration, apply_migrations, load_migrations};
pub use sqlite::repo::{
    SqliteSessionRepository, SqliteSessionRepositoryOptions, SqliteSessionStorage,
    SqliteWriterLeaseOptions,
};
pub use sqlite::sql::{SqlPart, SqlQuery, SqlValue, fragment, join_sql_fragments, param, text};
pub use sqlite::types::{
    Row, SqliteDatabase, SqliteDatabaseFactory, SqliteError, SqliteRunResult,
    SqliteSessionCreateOptions, SqliteSessionListOptions, SqliteSessionMetadata, with_transaction,
};
