use notagent_agent::AgentMessage;
use notagent_ai::{TextContent, TextOrImageContent, UserContent, UserMessage};
use notagent_session_sqlite::session_types::{
    BranchBounds, EntryCursor, EntryOrder, EntryQuery, EntryType, OperationKind, RecordQuery,
    RecordType,
};
use notagent_session_sqlite::sqlite::repo::{
    SqliteSessionRepository, SqliteSessionRepositoryOptions,
};
use notagent_session_sqlite::{Session, SqliteSessionCreateOptions, create_sqlite_factory};

async fn session(directory: &tempfile::TempDir) -> (SqliteSessionRepository, Session) {
    let database_path = directory
        .path()
        .join("sessions.sqlite")
        .to_string_lossy()
        .into_owned();
    let repo = SqliteSessionRepository::new(SqliteSessionRepositoryOptions {
        sqlite: create_sqlite_factory(),
        database_path,
        writer_lease: None,
    })
    .expect("repository");
    let storage = repo
        .create(SqliteSessionCreateOptions {
            id: Some("session-1".to_owned()),
            cwd: directory.path().to_string_lossy().into_owned(),
            ..SqliteSessionCreateOptions::default()
        })
        .await
        .expect("creates");
    (repo, Session::new(storage))
}

fn user_message(text: &str) -> AgentMessage {
    AgentMessage::User(UserMessage {
        content: UserContent::Blocks(vec![TextOrImageContent::Text(TextContent::new(text))]),
        timestamp: 1,
    })
}

#[tokio::test]
async fn appends_messages_and_custom_entries_with_generated_ids() {
    let directory = tempfile::Builder::new()
        .prefix("notagent-session-")
        .tempdir()
        .expect("temp dir");
    let (repo, session) = session(&directory).await;

    let first = session
        .append_message(user_message("one"))
        .await
        .expect("appends");
    let second = session
        .append_custom_entry("mode", Some(serde_json::json!({ "name": "plan" })))
        .await
        .expect("appends");
    assert_ne!(first, second);
    assert_eq!(session.get_leaf_id().expect("leaf"), Some(second.clone()));

    let entries = session
        .find_entries(&EntryQuery {
            order: Some(EntryOrder::OldestFirst),
            ..EntryQuery::default()
        })
        .expect("finds");
    assert_eq!(
        entries
            .iter()
            .map(notagent_session_sqlite::session_types::Entry::entry_type)
            .collect::<Vec<_>>(),
        vec![EntryType::Message, EntryType::Custom]
    );

    // findEntry caps the query at one result without changing the caller's query.
    let single = session.find_entry(&EntryQuery::default()).expect("finds");
    assert_eq!(single.map(|entry| entry.id().to_owned()), Some(second));

    // findEntriesOnBranch defaults to the lane leaf.
    let branch = session
        .find_entries_on_branch(&EntryQuery::default(), &BranchBounds::default())
        .expect("branch");
    assert_eq!(branch.len(), 2);
    repo.close().await;
}

#[tokio::test]
async fn rejects_invalid_limits_cursors_and_operation_kind_queries() {
    let directory = tempfile::Builder::new()
        .prefix("notagent-session-")
        .tempdir()
        .expect("temp dir");
    let (repo, session) = session(&directory).await;

    let error = session
        .find_entries(&EntryQuery {
            limit: Some(0),
            ..EntryQuery::default()
        })
        .expect_err("rejects");
    assert_eq!(error.message, "limit must be a positive integer");

    let error = session
        .find_entries(&EntryQuery {
            cursor: Some(EntryCursor { after_seq: -1 }),
            ..EntryQuery::default()
        })
        .expect_err("rejects");
    assert_eq!(
        error.message,
        "cursor sequence must be a non-negative integer"
    );

    let error = session
        .find_records(&RecordQuery {
            operation_kind: Some(OperationKind::Run),
            record_type: Some(RecordType::Usage),
            ..RecordQuery::default()
        })
        .expect_err("rejects");
    assert_eq!(
        error.message,
        "operationKind requires type \"operation_started\""
    );

    // The same query with the right type is accepted.
    session
        .find_records(&RecordQuery {
            operation_kind: Some(OperationKind::Run),
            record_type: Some(RecordType::OperationStarted),
            ..RecordQuery::default()
        })
        .expect("accepts");
    repo.close().await;
}

#[tokio::test]
async fn reports_an_unknown_lane() {
    let directory = tempfile::Builder::new()
        .prefix("notagent-session-")
        .tempdir()
        .expect("temp dir");
    let (repo, session) = session(&directory).await;
    let error = session
        .find_entries_on_branch(
            &EntryQuery::default(),
            &BranchBounds {
                start: None,
                ..BranchBounds::default()
            },
        )
        .expect("empty branch");
    assert!(error.is_empty());
    let lanes = session.get_lanes().expect("lanes");
    assert_eq!(lanes.len(), 1);
    assert_eq!(lanes[0].lane, "main");
    repo.close().await;
}
