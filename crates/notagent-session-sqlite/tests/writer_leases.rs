//! Port of `packages/session-backends/sqlite-node/test/writer-leases.test.ts`.

use notagent_agent::AgentMessage;
use notagent_ai::{TextContent, TextOrImageContent, UserContent, UserMessage};
use notagent_session_sqlite::session_types::{Entry, EntryOrder, EntryQuery, SessionErrorCode};
use notagent_session_sqlite::sqlite::repo::{
    SqliteSessionRepository, SqliteSessionRepositoryOptions, SqliteWriterLeaseOptions,
};
use notagent_session_sqlite::{
    RusqliteDatabase, Session, SqlQuery, SqlValue, SqliteDatabase, SqliteSessionCreateOptions,
    create_sqlite_factory,
};

fn repository(
    database_path: &str,
    lease: Option<SqliteWriterLeaseOptions>,
) -> SqliteSessionRepository {
    SqliteSessionRepository::new(SqliteSessionRepositoryOptions {
        sqlite: create_sqlite_factory(),
        database_path: database_path.to_owned(),
        writer_lease: lease,
    })
    .expect("repository")
}

fn user_message(text: &str) -> AgentMessage {
    AgentMessage::User(UserMessage {
        content: UserContent::Blocks(vec![TextOrImageContent::Text(TextContent::new(text))]),
        timestamp: 1,
    })
}

async fn create(repo: &SqliteSessionRepository, cwd: &str, id: &str) -> Session {
    Session::new(
        repo.create(SqliteSessionCreateOptions {
            id: Some(id.to_owned()),
            cwd: cwd.to_owned(),
            ..SqliteSessionCreateOptions::default()
        })
        .await
        .expect("creates"),
    )
}

#[tokio::test]
async fn shares_one_write_queue_across_repeated_opens_in_one_repository() {
    let directory = tempfile::Builder::new()
        .prefix("notagent-lease-")
        .tempdir()
        .expect("temp dir");
    let cwd = directory.path().to_string_lossy().into_owned();
    let database_path = directory
        .path()
        .join("sessions.sqlite")
        .to_string_lossy()
        .into_owned();
    let repo = repository(&database_path, None);
    let session = create(&repo, &cwd, "session").await;
    let reopened = Session::new(
        repo.open(&session.get_metadata().expect("metadata"))
            .await
            .expect("opens"),
    );

    let first = session
        .append_message(user_message("first"))
        .await
        .expect("appends");
    let second = reopened
        .append_message(user_message("second"))
        .await
        .expect("appends");

    let entries = session
        .find_entries(&EntryQuery {
            order: Some(EntryOrder::OldestFirst),
            ..EntryQuery::default()
        })
        .expect("finds");
    assert_eq!(
        entries.iter().map(Entry::id).collect::<Vec<_>>(),
        vec![first.as_str(), second.as_str()]
    );
    repo.close().await;
}

#[test]
fn rejects_invalid_lease_timing() {
    for (lease, message) in [
        (
            SqliteWriterLeaseOptions {
                ttl_ms: Some(0),
                heartbeat_interval_ms: Some(1),
            },
            "writerLease.ttlMs must be positive",
        ),
        (
            SqliteWriterLeaseOptions {
                ttl_ms: Some(100),
                heartbeat_interval_ms: Some(100),
            },
            "writerLease.heartbeatIntervalMs must be positive and less than ttlMs",
        ),
    ] {
        let error = SqliteSessionRepository::new(SqliteSessionRepositoryOptions {
            sqlite: create_sqlite_factory(),
            database_path: "/tmp/notagent-lease-test.sqlite".to_owned(),
            writer_lease: Some(lease),
        })
        .err()
        .expect("rejects");
        assert_eq!(error.message, message);
    }
}

#[tokio::test]
async fn lists_metadata_without_acquiring_active_writer_leases() {
    let directory = tempfile::Builder::new()
        .prefix("notagent-lease-")
        .tempdir()
        .expect("temp dir");
    let cwd = directory.path().to_string_lossy().into_owned();
    let database_path = directory
        .path()
        .join("sessions.sqlite")
        .to_string_lossy()
        .into_owned();
    let writer = repository(&database_path, None);
    let reader = repository(&database_path, None);
    let first = create(&writer, &cwd, "session-1").await;
    let second = create(&writer, &cwd, "session-2").await;
    first
        .set_name(Some("Review session"))
        .await
        .expect("sets name");
    second
        .set_name(Some("Write session"))
        .await
        .expect("sets name");

    let db = RusqliteDatabase::open(&database_path).expect("opens");
    let leases_before = db
        .all(&SqlQuery::raw(
            "SELECT session_id, owner_id, fence, expires_at_ms FROM writer_leases ORDER BY session_id",
        ))
        .expect("queries");
    db.close();

    let listed = reader
        .list(&notagent_session_sqlite::SqliteSessionListOptions {
            cwd: Some(cwd.clone()),
        })
        .await
        .expect("lists");
    let mut names: Vec<Option<String>> =
        listed.iter().map(|session| session.name.clone()).collect();
    names.sort();
    assert_eq!(
        names,
        vec![
            Some("Review session".to_owned()),
            Some("Write session".to_owned())
        ]
    );

    let db = RusqliteDatabase::open(&database_path).expect("opens");
    let leases_after = db
        .all(&SqlQuery::raw(
            "SELECT session_id, owner_id, fence, expires_at_ms FROM writer_leases ORDER BY session_id",
        ))
        .expect("queries");
    db.close();
    assert_eq!(leases_after, leases_before);

    let error = reader
        .open(&first.get_metadata().expect("metadata"))
        .await
        .expect_err("rejects");
    assert_eq!(error.code, SessionErrorCode::Storage);
    assert!(
        error.message.contains("already has an active writer"),
        "{}",
        error.message
    );
    writer.close().await;
    reader.close().await;
}

#[tokio::test]
async fn rejects_a_second_writer_until_the_first_session_releases_its_claim() {
    let directory = tempfile::Builder::new()
        .prefix("notagent-lease-")
        .tempdir()
        .expect("temp dir");
    let cwd = directory.path().to_string_lossy().into_owned();
    let database_path = directory
        .path()
        .join("sessions.sqlite")
        .to_string_lossy()
        .into_owned();
    let first_repository = repository(&database_path, None);
    let second_repository = repository(&database_path, None);
    let first = create(&first_repository, &cwd, "session-1").await;
    let metadata = first.get_metadata().expect("metadata");

    let error = second_repository
        .open(&metadata)
        .await
        .expect_err("rejects");
    assert!(
        error.message.contains("already has an active writer"),
        "{}",
        error.message
    );

    first_repository.close().await;
    let second = Session::new(second_repository.open(&metadata).await.expect("opens"));
    second
        .append_message(user_message("new owner"))
        .await
        .expect("appends");
    second_repository.close().await;
}

#[tokio::test]
async fn fences_a_stale_owner_after_an_expired_lease_is_acquired_by_another_writer() {
    let directory = tempfile::Builder::new()
        .prefix("notagent-lease-")
        .tempdir()
        .expect("temp dir");
    let cwd = directory.path().to_string_lossy().into_owned();
    let database_path = directory
        .path()
        .join("sessions.sqlite")
        .to_string_lossy()
        .into_owned();
    let lease = SqliteWriterLeaseOptions {
        ttl_ms: Some(120_000),
        heartbeat_interval_ms: Some(60_000),
    };
    let first_repository = repository(&database_path, Some(lease));
    let second_repository = repository(&database_path, Some(lease));
    let first = create(&first_repository, &cwd, "session-1").await;
    let metadata = first.get_metadata().expect("metadata");

    let db = RusqliteDatabase::open(&database_path).expect("opens");
    db.run(&SqlQuery::new(
        "UPDATE writer_leases SET expires_at_ms = 0 WHERE session_id = ?",
        vec![SqlValue::Text(metadata.id.clone())],
    ))
    .expect("expires the lease");
    db.close();

    let second = Session::new(second_repository.open(&metadata).await.expect("opens"));
    let error = first
        .append_message(user_message("stale owner"))
        .await
        .expect_err("rejects");
    assert_eq!(error.code, SessionErrorCode::Storage);
    assert!(
        error.message.contains("writer lease was lost"),
        "{}",
        error.message
    );
    assert!(
        second
            .find_entries(&EntryQuery::default())
            .expect("finds")
            .is_empty()
    );

    let db = RusqliteDatabase::open(&database_path).expect("opens");
    let current = db
        .get(&SqlQuery::new(
            "SELECT owner_id, fence FROM writer_leases WHERE session_id = ?",
            vec![SqlValue::Text(metadata.id.clone())],
        ))
        .expect("queries")
        .expect("lease row");
    assert_eq!(current["fence"], 2);
    db.close();

    // Closing the fenced repository must not remove the new owner's lease.
    first_repository.close().await;
    let db = RusqliteDatabase::open(&database_path).expect("opens");
    let after_close = db
        .get(&SqlQuery::new(
            "SELECT owner_id, fence FROM writer_leases WHERE session_id = ?",
            vec![SqlValue::Text(metadata.id.clone())],
        ))
        .expect("queries")
        .expect("lease row");
    assert_eq!(after_close, current);
    db.close();

    second
        .append_message(user_message("current owner"))
        .await
        .expect("appends");
    second_repository.close().await;
}

#[tokio::test]
async fn serializes_lease_checked_writes_for_sessions_sharing_one_connection() {
    let directory = tempfile::Builder::new()
        .prefix("notagent-lease-")
        .tempdir()
        .expect("temp dir");
    let cwd = directory.path().to_string_lossy().into_owned();
    let database_path = directory
        .path()
        .join("sessions.sqlite")
        .to_string_lossy()
        .into_owned();
    let repo = repository(&database_path, None);
    let first = create(&repo, &cwd, "session-1").await;
    let second = create(&repo, &cwd, "session-2").await;

    let (first_id, second_id) = tokio::join!(
        first.append_message(user_message("first")),
        second.append_message(user_message("second"))
    );
    first_id.expect("appends");
    second_id.expect("appends");
    repo.close().await;
}

#[tokio::test]
async fn renews_an_idle_writer_lease_with_a_heartbeat() {
    let directory = tempfile::Builder::new()
        .prefix("notagent-lease-")
        .tempdir()
        .expect("temp dir");
    let cwd = directory.path().to_string_lossy().into_owned();
    let database_path = directory
        .path()
        .join("sessions.sqlite")
        .to_string_lossy()
        .into_owned();
    // TS advances fake timers; Rust uses a short real interval instead.
    let repo = repository(
        &database_path,
        Some(SqliteWriterLeaseOptions {
            ttl_ms: Some(300),
            heartbeat_interval_ms: Some(50),
        }),
    );
    let session = create(&repo, &cwd, "session-1").await;
    let metadata = session.get_metadata().expect("metadata");

    let read_expiry = || {
        let db = RusqliteDatabase::open(&database_path).expect("opens");
        let expiry = db
            .get(&SqlQuery::new(
                "SELECT expires_at_ms FROM writer_leases WHERE session_id = ?",
                vec![SqlValue::Text(metadata.id.clone())],
            ))
            .expect("queries")
            .and_then(|row| row["expires_at_ms"].as_i64());
        db.close();
        expiry
    };

    let initial = read_expiry().expect("initial expiry");
    tokio::time::sleep(std::time::Duration::from_millis(180)).await;
    let renewed = read_expiry().expect("renewed expiry");
    assert!(
        renewed > initial,
        "expiry was not renewed: {initial} -> {renewed}"
    );
    repo.close().await;
}
