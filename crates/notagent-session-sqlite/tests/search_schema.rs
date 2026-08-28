use notagent_session_sqlite::{RusqliteDatabase, SqlQuery, SqliteDatabase, apply_migrations};

#[test]
fn creates_the_trigram_fts_index_and_matches_entry_payloads() {
    let db = RusqliteDatabase::open(":memory:").expect("opens");
    apply_migrations(&db).expect("migrates");
    db.exec("INSERT INTO sessions (id, created_at, cwd) VALUES ('session-1', 1, '/work')")
        .expect("session");
    db.exec(
        "INSERT INTO entries (session_id, seq, id, parent_id, type, timestamp, payload)
         VALUES ('session-1', 1, 'entry-1', NULL, 'message', 1, '{\"message\":{\"role\":\"user\",\"content\":\"hello haystack\"}}')",
    )
    .expect("entry");

    notagent_session_sqlite::sqlite::search_backend::ensure_search_schema(&db)
        .expect("search schema");

    let rows = db
        .all(&SqlQuery::new(
            "SELECT se.id AS entry_id, bm25(session_search_fts) AS score
             FROM session_search_fts
             JOIN entries AS se ON se.rowid = session_search_fts.rowid
             WHERE session_search_fts MATCH ?
             ORDER BY score",
            vec![notagent_session_sqlite::SqlValue::Text(
                "\"haystack\"".to_owned(),
            )],
        ))
        .expect("searches");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["entry_id"], "entry-1");
    db.close();
}
