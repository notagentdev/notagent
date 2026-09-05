use super::sql::{SqlQuery, param, text};
use super::types::{SqliteDatabase, SqliteError, with_transaction};
use crate::sql;

pub struct SqliteMigration {
    pub id: &'static str,
    pub order: u32,
    pub sql: &'static str,
}

/// distribution mechanics — the single binary has no migration directory).
const INITIAL_MIGRATION: &str = include_str!("migrations/001_initial.sql");

pub fn load_migrations() -> Vec<SqliteMigration> {
    vec![SqliteMigration {
        id: "001_initial.sql",
        order: 1,
        sql: INITIAL_MIGRATION,
    }]
}

fn ensure_migrations_table(db: &dyn SqliteDatabase) -> Result<(), SqliteError> {
    db.exec(
        "
CREATE TABLE IF NOT EXISTS migrations (
\tid TEXT PRIMARY KEY,
\tapplied_at TEXT NOT NULL
);
",
    )
}

pub fn apply_migrations(db: &dyn SqliteDatabase) -> Result<(), SqliteError> {
    ensure_migrations_table(db)?;
    let migrations = load_migrations();
    let applied_rows = db.all(&SqlQuery::raw(
        "SELECT id FROM migrations ORDER BY applied_at, id",
    ))?;
    let applied: Vec<String> = applied_rows
        .iter()
        .filter_map(|row| {
            row.get("id")
                .and_then(|value| value.as_str())
                .map(str::to_owned)
        })
        .collect();

    for migration in migrations {
        if applied.iter().any(|id| id == migration.id) {
            continue;
        }
        with_transaction(db, |db| {
            db.exec(migration.sql)?;
            db.run(&sql![
                text("INSERT INTO migrations (id, applied_at) VALUES ("),
                param(migration.id),
                text(", "),
                param(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
                text(")"),
            ])?;
            Ok(())
        })?;
    }
    Ok(())
}
