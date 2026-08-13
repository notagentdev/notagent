//! Port of `packages/session-backends/sqlite-node/test/sql.test.ts`.

use notagent_session_sqlite::sql;
use notagent_session_sqlite::{
    RusqliteDatabase, SqlQuery, SqliteDatabase, join_sql_fragments, param, text,
};

#[test]
fn composes_sqlite_queries_without_renumbering_parameters() {
    let db = RusqliteDatabase::open(":memory:").expect("opens");
    db.exec(
        "CREATE TABLE entries (id TEXT PRIMARY KEY, kind TEXT NOT NULL, active INTEGER NOT NULL)",
    )
    .expect("creates");
    db.run(&sql![
        text("INSERT INTO entries (id, kind, active) VALUES ("),
        param("one"),
        text(", "),
        param("message"),
        text(", "),
        param(1),
        text(")"),
    ])
    .expect("inserts");
    db.run(&sql![
        text("INSERT INTO entries (id, kind, active) VALUES ("),
        param("two"),
        text(", "),
        param("message"),
        text(", "),
        param(0),
        text(")"),
    ])
    .expect("inserts");
    let filters = join_sql_fragments(
        &[
            sql![text("kind = "), param("message")],
            sql![text("active = "), param(1)],
        ],
        " AND ",
    );

    let rows = db
        .all(&sql![
            text("SELECT id FROM entries WHERE "),
            notagent_session_sqlite::fragment(filters),
            text(" LIMIT "),
            param(10),
        ])
        .expect("queries");
    let ids: Vec<&str> = rows.iter().filter_map(|row| row["id"].as_str()).collect();
    assert_eq!(ids, vec!["one"]);
    db.close();
}

#[test]
fn executes_parameterized_queries() {
    let db = RusqliteDatabase::open(":memory:").expect("opens");
    db.exec("CREATE TABLE values_table (id INTEGER PRIMARY KEY, value TEXT NOT NULL)")
        .expect("creates");
    for (id, value) in [(1, "one"), (2, "two")] {
        db.run(&sql![
            text("INSERT INTO values_table (id, value) VALUES ("),
            param(id),
            text(", "),
            param(value),
            text(")"),
        ])
        .expect("inserts");
    }

    let row = db
        .get(&sql![
            text("SELECT value FROM values_table WHERE id = "),
            param(1)
        ])
        .expect("queries")
        .expect("row");
    assert_eq!(row["value"], "one");
    let rows = db
        .all(&SqlQuery::raw("SELECT value FROM values_table ORDER BY id"))
        .expect("queries");
    let values: Vec<&str> = rows
        .iter()
        .filter_map(|row| row["value"].as_str())
        .collect();
    assert_eq!(values, vec!["one", "two"]);
    db.close();
}
