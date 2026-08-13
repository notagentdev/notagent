//! Port of `packages/session-backends/sqlite-node/test/adapter.test.ts`.
//!
//! TS wraps `node:sqlite`; Rust wraps rusqlite (tech substitution). The
//! "rejects asynchronous transaction callbacks" case has no equivalent:
//! `with_transaction` takes a synchronous closure, so an async body cannot be
//! passed at all (deviation class 1).

use notagent_session_sqlite::{
    RusqliteDatabase, SqlQuery, SqlValue, SqliteDatabase, with_transaction,
};

#[test]
fn commits_a_transaction_and_returns_its_result() {
    let db = RusqliteDatabase::open(":memory:").expect("opens");
    db.exec("CREATE TABLE values_table (value INTEGER NOT NULL)")
        .expect("creates");
    let result = with_transaction(&db, || {
        db.run(&SqlQuery::new(
            "INSERT INTO values_table (value) VALUES (?)",
            vec![SqlValue::Integer(42)],
        ))?;
        Ok("committed")
    })
    .expect("commits");

    assert_eq!(result, "committed");
    let row = db
        .get(&SqlQuery::raw("SELECT value FROM values_table"))
        .expect("queries")
        .expect("row");
    assert_eq!(row["value"], 42);
    db.close();
}

#[test]
fn rolls_back_a_failed_transaction() {
    let db = RusqliteDatabase::open(":memory:").expect("opens");
    db.exec("CREATE TABLE values_table (value INTEGER NOT NULL)")
        .expect("creates");
    let error = with_transaction(&db, || {
        db.run(&SqlQuery::new(
            "INSERT INTO values_table (value) VALUES (?)",
            vec![SqlValue::Integer(42)],
        ))?;
        Err::<(), _>(notagent_session_sqlite::SqliteError(
            "rolled back".to_owned(),
        ))
    })
    .expect_err("rolls back");
    assert_eq!(error.0, "rolled back");
    assert!(
        db.all(&SqlQuery::raw("SELECT value FROM values_table"))
            .expect("queries")
            .is_empty()
    );
    db.close();
}

#[test]
fn forwards_positional_statement_parameters_and_reports_changes() {
    let db = RusqliteDatabase::open(":memory:").expect("opens");
    db.exec("CREATE TABLE values_table (id INTEGER PRIMARY KEY, value TEXT NOT NULL)")
        .expect("creates");
    let first = db
        .run(&SqlQuery::new(
            "INSERT INTO values_table (value) VALUES (?)",
            vec![SqlValue::Text("positional".to_owned())],
        ))
        .expect("inserts");
    assert_eq!(first.changes, 1);
    assert_eq!(first.last_insert_rowid, Some(1));
    let second = db
        .run(&SqlQuery::new(
            "INSERT INTO values_table (value) VALUES (?)",
            vec![SqlValue::Text("second".to_owned())],
        ))
        .expect("inserts");
    assert_eq!(second.last_insert_rowid, Some(2));

    let row = db
        .get(&SqlQuery::new(
            "SELECT value FROM values_table WHERE id = ?",
            vec![SqlValue::Integer(1)],
        ))
        .expect("queries")
        .expect("row");
    assert_eq!(row["value"], "positional");
    let rows = db
        .all(&SqlQuery::new(
            "SELECT value FROM values_table WHERE id >= ? ORDER BY id",
            vec![SqlValue::Integer(2)],
        ))
        .expect("queries");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["value"], "second");
    db.close();
}

#[test]
fn reports_a_closed_database() {
    let db = RusqliteDatabase::open(":memory:").expect("opens");
    db.close();
    let error = db.exec("SELECT 1").expect_err("rejects");
    assert_eq!(error.0, "SQLite database is closed");
}
