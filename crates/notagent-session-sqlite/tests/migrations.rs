//! Port of `packages/session-backends/sqlite-node/test/migrations.test.ts`.

use notagent_session_sqlite::{RusqliteDatabase, SqlQuery, SqliteDatabase, apply_migrations};

fn names(db: &RusqliteDatabase, query: &str, column: &str) -> Vec<String> {
    db.all(&SqlQuery::raw(query))
        .expect("queries")
        .iter()
        .filter_map(|row| {
            row.get(column)
                .and_then(|value| value.as_str())
                .map(str::to_owned)
        })
        .collect()
}

#[test]
fn applies_the_current_schema_once_and_records_its_migration() {
    let directory = tempfile::Builder::new()
        .prefix("notagent-session-backend-")
        .tempdir()
        .expect("temp dir");
    let database_path = directory.path().join("sessions.sqlite");
    let db = RusqliteDatabase::open(&database_path.to_string_lossy()).expect("opens");

    apply_migrations(&db).expect("applies");
    apply_migrations(&db).expect("applies twice");

    assert_eq!(
        names(&db, "SELECT id FROM migrations ORDER BY id", "id"),
        vec!["001_initial.sql".to_owned()]
    );
    let tables = names(
        &db,
        "SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name",
        "name",
    );
    for expected in [
        "migrations",
        "sessions",
        "entries",
        "session_sequences",
        "session_stats",
        "branch_entries",
        "branch_tips",
        "lanes",
        "records",
        "lane_moves",
        "facts",
        "writer_leases",
    ] {
        assert!(
            tables.iter().any(|name| name == expected),
            "missing table {expected} in {tables:?}"
        );
    }

    let session_columns = names(&db, "PRAGMA table_info(sessions)", "name");
    assert!(!session_columns.iter().any(|name| name == "leaf_id"));
    let session_indexes = names(&db, "PRAGMA index_list(sessions)", "name");
    assert!(
        session_indexes
            .iter()
            .any(|name| name == "idx_sessions_cwd_created_at")
    );
    assert!(
        !session_indexes
            .iter()
            .any(|name| name == "idx_sessions_parent")
    );
    let lane_columns = names(&db, "PRAGMA table_info(lanes)", "name");
    assert!(lane_columns.iter().any(|name| name == "open_operation_id"));
    let entry_indexes = names(&db, "PRAGMA index_list(entries)", "name");
    assert!(
        !entry_indexes
            .iter()
            .any(|name| name == "idx_entries_session_seq")
    );
    let branch_entry_indexes = names(&db, "PRAGMA index_list(branch_entries)", "name");
    assert!(
        branch_entry_indexes
            .iter()
            .any(|name| name == "idx_branch_entries_session_entry")
    );
    let record_indexes = names(&db, "PRAGMA index_list(records)", "name");
    for expected in [
        "idx_records_session_lane_seq",
        "idx_records_session_type_seq",
        "idx_records_session_type_op_kind_seq",
    ] {
        assert!(
            record_indexes.iter().any(|name| name == expected),
            "missing index {expected}"
        );
    }
    assert!(
        !record_indexes
            .iter()
            .any(|name| name == "idx_records_session_seq")
    );
    let lane_move_indexes = names(&db, "PRAGMA index_list(lane_moves)", "name");
    assert!(
        !lane_move_indexes
            .iter()
            .any(|name| name == "idx_lane_moves_session_lane_seq")
    );
    db.close();
}
