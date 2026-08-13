//! Port of `packages/session-backends/sqlite-node/test/branch-cache.test.ts`.

use notagent_agent::AgentMessage;
use notagent_ai::{TextContent, TextOrImageContent, UserContent, UserMessage};
use notagent_session_sqlite::session_types::{
    BranchBounds, EntryOrder, EntryQuery, EntryType, ForkOptions, ForkPosition, ForkScope,
    ProvisionedEntry, SessionErrorCode,
};
use notagent_session_sqlite::sqlite::repo::{
    SqliteSessionRepository, SqliteSessionRepositoryOptions,
};
use notagent_session_sqlite::{
    RusqliteDatabase, Session, SqlQuery, SqlValue, SqliteDatabase, SqliteSessionCreateOptions,
    create_sqlite_factory,
};

struct Harness {
    directory: tempfile::TempDir,
    database_path: String,
    repo: SqliteSessionRepository,
}

fn harness() -> Harness {
    let directory = tempfile::Builder::new()
        .prefix("notagent-branch-cache-")
        .tempdir()
        .expect("temp dir");
    let database_path = directory
        .path()
        .join("sessions.sqlite")
        .to_string_lossy()
        .into_owned();
    let repo = SqliteSessionRepository::new(SqliteSessionRepositoryOptions {
        sqlite: create_sqlite_factory(),
        database_path: database_path.clone(),
        writer_lease: None,
    })
    .expect("repository");
    Harness {
        directory,
        database_path,
        repo,
    }
}

async fn create_session(harness: &Harness, id: &str) -> Session {
    let storage = harness
        .repo
        .create(SqliteSessionCreateOptions {
            id: Some(id.to_owned()),
            cwd: harness.directory.path().to_string_lossy().into_owned(),
            ..SqliteSessionCreateOptions::default()
        })
        .await
        .expect("creates");
    Session::new(storage)
}

fn user_message(text: &str) -> AgentMessage {
    AgentMessage::User(UserMessage {
        content: UserContent::Blocks(vec![TextOrImageContent::Text(TextContent::new(text))]),
        timestamp: 1,
    })
}

async fn append_compaction(session: &Session, summary: &str, tokens_before: i64) -> String {
    let entry = session
        .append_entry(
            ProvisionedEntry::Compaction {
                id: session.id_generator.next(),
                summary: summary.to_owned(),
                retained_tail: vec![],
                tokens_before,
                details: None,
                usage: None,
            },
            "main",
        )
        .await
        .expect("appends");
    entry.id().to_owned()
}

/// Port of `getSqliteBranch`: the newest window up to the first compaction, oldest first.
fn branch(
    session: &Session,
) -> Result<Vec<String>, notagent_session_sqlite::session_types::SessionError> {
    let entries = session.find_entries_on_branch(
        &EntryQuery::default(),
        &BranchBounds {
            stop_at_type: Some(EntryType::Compaction),
            ..BranchBounds::default()
        },
    )?;
    Ok(entries
        .iter()
        .rev()
        .map(|entry| entry.id().to_owned())
        .collect())
}

fn inspect(harness: &Harness) -> RusqliteDatabase {
    RusqliteDatabase::open(&harness.database_path).expect("opens")
}

#[tokio::test]
async fn collects_complete_root_paths_for_branches_created_after_compaction() {
    let harness = harness();
    let session = create_session(&harness, "session-1").await;
    let root_id = session
        .append_message(user_message("root"))
        .await
        .expect("appends");
    let kept_id = session
        .append_message(user_message("kept"))
        .await
        .expect("appends");
    let compaction_id = append_compaction(&session, "summary", 100).await;
    session
        .append_message(user_message("first child"))
        .await
        .expect("appends");
    session
        .move_lane("main", Some(&compaction_id))
        .await
        .expect("moves");
    let branched_id = session
        .append_message(user_message("branched child"))
        .await
        .expect("appends");

    let db = inspect(&harness);
    let row = db
        .get(&SqlQuery::new(
            "SELECT branch_id FROM branch_entries WHERE session_id = ? AND entry_id = ?",
            vec![
                SqlValue::Text("session-1".to_owned()),
                SqlValue::Text(branched_id.clone()),
            ],
        ))
        .expect("queries")
        .expect("Missing branched entry cache row");
    let branch_id = row["branch_id"].as_str().expect("branch id").to_owned();
    let entries = db
        .all(&SqlQuery::new(
            "SELECT entry_id FROM branch_entries WHERE session_id = ? AND branch_id = ? ORDER BY entry_seq",
            vec![SqlValue::Text("session-1".to_owned()), SqlValue::Text(branch_id)],
        ))
        .expect("queries");
    let ids: Vec<&str> = entries
        .iter()
        .filter_map(|row| row["entry_id"].as_str())
        .collect();
    assert_eq!(
        ids,
        vec![
            root_id.as_str(),
            kept_id.as_str(),
            compaction_id.as_str(),
            branched_id.as_str()
        ]
    );
    db.close();
    harness.repo.close().await;
}

#[tokio::test]
async fn preserves_nested_compaction_boundaries_when_reading_the_cache() {
    let harness = harness();
    let session = create_session(&harness, "session-1").await;
    session
        .append_message(user_message("root"))
        .await
        .expect("appends");
    append_compaction(&session, "first summary", 100).await;
    session
        .append_message(user_message("middle"))
        .await
        .expect("appends");
    let second_compaction_id = append_compaction(&session, "second summary", 200).await;
    let leaf_id = session
        .append_message(user_message("new"))
        .await
        .expect("appends");

    assert_eq!(
        branch(&session).expect("branch"),
        vec![second_compaction_id, leaf_id]
    );
    harness.repo.close().await;
}

#[tokio::test]
async fn rejects_reads_and_writes_without_repairing_a_missing_branch_cache() {
    let harness = harness();
    let session = create_session(&harness, "session-1").await;
    session
        .append_message(user_message("root"))
        .await
        .expect("appends");
    session
        .append_message(user_message("child"))
        .await
        .expect("appends");

    let db = inspect(&harness);
    db.exec("DELETE FROM branch_tips WHERE session_id = 'session-1'")
        .expect("clears tips");
    db.exec("DELETE FROM branch_entries WHERE session_id = 'session-1'")
        .expect("clears entries");
    db.close();

    let error = branch(&session).expect_err("rejects");
    assert_eq!(error.code, SessionErrorCode::InvalidEntry);
    let error = session
        .append_message(user_message("later"))
        .await
        .expect_err("rejects");
    assert_eq!(
        error.code,
        SessionErrorCode::InvalidEntry,
        "message: {}",
        error.message
    );
    assert!(
        error
            .message
            .contains("has no branch containing parent entry"),
        "{}",
        error.message
    );

    let db = inspect(&harness);
    let rows = db
        .all(&SqlQuery::raw(
            "SELECT entry_id FROM branch_entries WHERE session_id = 'session-1'",
        ))
        .expect("queries");
    assert!(rows.is_empty());
    db.close();
    harness.repo.close().await;
}

#[tokio::test]
async fn repairs_the_private_branch_cache_explicitly() {
    let harness = harness();
    let session = create_session(&harness, "session-1").await;
    let root_id = session
        .append_message(user_message("root"))
        .await
        .expect("appends");
    let child_id = session
        .append_message(user_message("child"))
        .await
        .expect("appends");
    let metadata = session.get_metadata().expect("metadata");

    let db = inspect(&harness);
    db.exec("DELETE FROM branch_tips WHERE session_id = 'session-1'")
        .expect("clears tips");
    db.exec("DELETE FROM branch_entries WHERE session_id = 'session-1'")
        .expect("clears entries");
    db.close();

    assert_eq!(
        branch(&session).expect_err("rejects").code,
        SessionErrorCode::InvalidEntry
    );

    // repairBranchCache releases the active storage; reopen afterwards.
    harness
        .repo
        .repair_branch_cache(&metadata)
        .await
        .expect("repairs");
    let reopened = Session::new(harness.repo.open(&metadata).await.expect("opens"));
    assert_eq!(branch(&reopened).expect("branch"), vec![root_id, child_id]);
    harness.repo.close().await;
}

#[tokio::test]
async fn fails_when_forking_from_a_source_with_a_missing_branch_cache() {
    let harness = harness();
    let source = create_session(&harness, "source").await;
    let root_id = source
        .append_message(user_message("root"))
        .await
        .expect("appends");
    let child_id = source
        .append_message(user_message("child"))
        .await
        .expect("appends");
    assert_ne!(root_id, child_id);

    let db = inspect(&harness);
    db.exec("DELETE FROM branch_tips WHERE session_id = 'source'")
        .expect("clears tips");
    db.exec("DELETE FROM branch_entries WHERE session_id = 'source'")
        .expect("clears entries");
    db.close();

    let metadata = source.get_metadata().expect("metadata");
    let error = harness
        .repo
        .fork(
            &metadata,
            &ForkOptions {
                scope: ForkScope::Branch,
                entry_id: Some(child_id),
                position: Some(ForkPosition::At),
            },
            SqliteSessionCreateOptions {
                id: Some("fork".to_owned()),
                cwd: harness.directory.path().to_string_lossy().into_owned(),
                ..SqliteSessionCreateOptions::default()
            },
        )
        .await
        .expect_err("rejects");
    assert_eq!(error.code, SessionErrorCode::InvalidForkTarget);
    harness.repo.close().await;
}

#[tokio::test]
async fn fails_when_the_private_branch_cache_is_stale() {
    let harness = harness();
    let session = create_session(&harness, "session-1").await;
    let root_id = session
        .append_message(user_message("root"))
        .await
        .expect("appends");
    let stale_id = session
        .append_message(user_message("stale"))
        .await
        .expect("appends");
    let leaf_id = session
        .append_message(user_message("leaf"))
        .await
        .expect("appends");
    assert_ne!(stale_id, leaf_id);

    let db = inspect(&harness);
    db.run(&SqlQuery::new(
        "UPDATE entries SET parent_id = ? WHERE session_id = ? AND id = ?",
        vec![
            SqlValue::Text(root_id),
            SqlValue::Text("session-1".to_owned()),
            SqlValue::Text(leaf_id.clone()),
        ],
    ))
    .expect("updates");
    db.close();

    let error = session
        .storage()
        .find_entries_on_branch(
            &leaf_id,
            &EntryQuery {
                order: Some(EntryOrder::OldestFirst),
                ..EntryQuery::default()
            },
            &BranchBounds::default(),
        )
        .expect_err("rejects");
    assert_eq!(error.code, SessionErrorCode::InvalidEntry);
    harness.repo.close().await;
}

#[tokio::test]
async fn deletes_branch_entries_and_tips_with_the_session() {
    let harness = harness();
    let session = create_session(&harness, "session-1").await;
    session
        .append_message(user_message("root"))
        .await
        .expect("appends");
    let metadata = session.get_metadata().expect("metadata");

    harness.repo.delete(&metadata).await.expect("deletes");

    let db = inspect(&harness);
    assert!(
        db.all(&SqlQuery::raw(
            "SELECT entry_id FROM branch_entries WHERE session_id = 'session-1'"
        ))
        .expect("queries")
        .is_empty()
    );
    assert!(
        db.all(&SqlQuery::raw(
            "SELECT tip_id FROM branch_tips WHERE session_id = 'session-1'"
        ))
        .expect("queries")
        .is_empty()
    );
    db.close();
    harness.repo.close().await;
}
