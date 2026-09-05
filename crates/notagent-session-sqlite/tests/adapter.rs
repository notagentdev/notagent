use notagent_session_sqlite::{
    RusqliteDatabase, SqlQuery, SqlValue, SqliteDatabase, with_transaction,
};

#[test]
fn commits_a_transaction_and_returns_its_result() {
    let db = RusqliteDatabase::open(":memory:").expect("opens");
    db.exec("CREATE TABLE values_table (value INTEGER NOT NULL)")
        .expect("creates");
    let result = with_transaction(&db, |db| {
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
    let error = with_transaction(&db, |db| {
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

#[test]
fn competing_transactions_wait_until_the_connection_is_released() {
    use std::sync::{Arc, mpsc};
    use std::time::Duration;
    let db = Arc::new(RusqliteDatabase::open(":memory:").expect("open"));
    let (entered, wait_entered) = mpsc::channel();
    let (release, wait_release) = mpsc::channel();
    let first = Arc::clone(&db);
    let holder = std::thread::spawn(move || {
        with_transaction(first.as_ref(), |db| {
            db.exec("CREATE TABLE isolated (value INTEGER)")?;
            entered.send(()).expect("entered");
            wait_release
                .recv_timeout(Duration::from_secs(5))
                .expect("release");
            Ok(())
        })
    });
    wait_entered
        .recv_timeout(Duration::from_secs(5))
        .expect("first transaction");
    let (finished, wait_finished) = mpsc::channel();
    let second = Arc::clone(&db);
    let waiter = std::thread::spawn(move || {
        let result = with_transaction(second.as_ref(), |db| {
            db.exec("INSERT INTO isolated VALUES (1)")
        });
        finished.send(result).expect("result");
    });
    let early = wait_finished.recv_timeout(Duration::from_millis(100));
    release.send(()).expect("release holder");
    holder.join().expect("holder").expect("first commit");
    waiter.join().expect("waiter");
    assert!(
        matches!(early, Err(mpsc::RecvTimeoutError::Timeout)),
        "a competing BEGIN must wait rather than fail or enter the current transaction: {early:?}"
    );
    wait_finished
        .recv_timeout(Duration::from_secs(5))
        .expect("completed")
        .expect("second commit");
}

#[test]
fn a_failed_commit_rolls_back_before_the_next_transaction() {
    let db = RusqliteDatabase::open(":memory:").expect("open");
    db.exec("PRAGMA foreign_keys=ON; CREATE TABLE parent(id INTEGER PRIMARY KEY); CREATE TABLE child(id INTEGER REFERENCES parent(id) DEFERRABLE INITIALLY DEFERRED)").expect("schema");
    with_transaction(&db, |db| db.exec("INSERT INTO child VALUES (1)"))
        .expect_err("deferred constraint must reject commit");
    with_transaction(&db, |db| {
        db.exec("INSERT INTO parent VALUES (1); INSERT INTO child VALUES (1)")
    })
    .expect("failed commit must not leave a transaction open");
    assert_eq!(
        db.all(&SqlQuery::raw("SELECT * FROM child"))
            .expect("rows")
            .len(),
        1
    );
}
