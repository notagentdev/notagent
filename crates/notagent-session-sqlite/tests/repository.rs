use std::collections::BTreeMap;
use std::sync::Arc;

use notagent_agent::AgentMessage;
use notagent_ai::{TextContent, UserContent, UserMessage};
use notagent_session_sqlite::session_types::{
    Entry, EntryCursor, EntryOrder, EntryQuery, EntryType, ForkOptions, ForkScope, LaneRecord,
    LogItem, OperationIntent, OperationStartedRecord, ProvisionedEntry, RecordQuery,
};
use notagent_session_sqlite::sqlite::repo::{
    SqliteSessionRepository, SqliteSessionRepositoryOptions,
};
use notagent_session_sqlite::{
    SqliteSessionCreateOptions, SqliteSessionListOptions, SqliteSessionMetadata,
    create_sqlite_factory,
};
use serde_json::json;

struct Harness {
    directory: tempfile::TempDir,
    repo: SqliteSessionRepository,
}

fn harness() -> Harness {
    let directory = tempfile::Builder::new()
        .prefix("notagent-session-backend-")
        .tempdir()
        .expect("temp dir");
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
    Harness { directory, repo }
}

fn create_options(cwd: &str, id: &str) -> SqliteSessionCreateOptions {
    SqliteSessionCreateOptions {
        id: Some(id.to_owned()),
        parent_session_id: None,
        cwd: cwd.to_owned(),
        metadata: None,
    }
}

fn user_message(text: &str) -> AgentMessage {
    AgentMessage::User(UserMessage {
        content: UserContent::Blocks(vec![notagent_ai::TextOrImageContent::Text(
            TextContent::new(text),
        )]),
        timestamp: 1,
    })
}

fn message_entry(id: &str, text: &str) -> ProvisionedEntry {
    ProvisionedEntry::Message {
        id: id.to_owned(),
        message: user_message(text),
        terminate: None,
    }
}

#[tokio::test]
async fn persists_session_metadata_through_create_list_open_and_fork() {
    let harness = harness();
    let cwd = harness.directory.path().to_string_lossy().into_owned();
    let mut metadata_map = BTreeMap::new();
    metadata_map.insert("profile".to_owned(), json!("reviewer"));
    let source = harness
        .repo
        .create(SqliteSessionCreateOptions {
            metadata: Some(metadata_map.clone()),
            ..create_options(&cwd, "session-1")
        })
        .await
        .expect("creates");
    let source_metadata = source.get_metadata().expect("metadata");
    assert_eq!(source_metadata.metadata, Some(metadata_map.clone()));

    let listed = harness
        .repo
        .list(&SqliteSessionListOptions {
            cwd: Some(cwd.clone()),
        })
        .await
        .expect("lists");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].metadata, Some(metadata_map.clone()));

    let reopened = harness.repo.open(&source_metadata).await.expect("opens");
    assert_eq!(
        reopened.get_metadata().expect("metadata").metadata,
        Some(metadata_map.clone())
    );

    let fork = harness
        .repo
        .fork(
            &source_metadata,
            &ForkOptions::default(),
            create_options(&cwd, "session-2"),
        )
        .await
        .expect("forks");
    assert_eq!(
        fork.get_metadata().expect("metadata").metadata,
        Some(metadata_map)
    );

    let mut overridden_metadata = BTreeMap::new();
    overridden_metadata.insert("profile".to_owned(), json!("writer"));
    let overridden = harness
        .repo
        .fork(
            &source_metadata,
            &ForkOptions::default(),
            SqliteSessionCreateOptions {
                metadata: Some(overridden_metadata.clone()),
                ..create_options(&cwd, "session-3")
            },
        )
        .await
        .expect("forks");
    assert_eq!(
        overridden.get_metadata().expect("metadata").metadata,
        Some(overridden_metadata)
    );
    harness.repo.close().await;
}

#[tokio::test]
async fn appends_entries_and_queries_them_in_sequence_order() {
    let harness = harness();
    let cwd = harness.directory.path().to_string_lossy().into_owned();
    let session = harness
        .repo
        .create(create_options(&cwd, "session-1"))
        .await
        .expect("creates");

    let first = session
        .append_entry(message_entry("entry-1", "one"), "main")
        .await
        .expect("appends");
    let second = session
        .append_entry(message_entry("entry-2", "two"), "main")
        .await
        .expect("appends");
    assert_eq!(first.parent_id(), None);
    assert_eq!(second.parent_id(), Some("entry-1"));
    assert_eq!(first.seq() + 1, second.seq());

    let entries = session
        .find_entries(&EntryQuery {
            order: Some(EntryOrder::OldestFirst),
            ..EntryQuery::default()
        })
        .expect("finds");
    assert_eq!(
        entries.iter().map(Entry::id).collect::<Vec<_>>(),
        vec!["entry-1", "entry-2"]
    );

    let limited = session
        .find_entries(&EntryQuery {
            order: Some(EntryOrder::OldestFirst),
            limit: Some(1),
            ..EntryQuery::default()
        })
        .expect("finds");
    assert_eq!(limited.len(), 1);

    let after_first = session
        .find_entries(&EntryQuery {
            order: Some(EntryOrder::OldestFirst),
            cursor: Some(EntryCursor {
                after_seq: first.seq(),
            }),
            ..EntryQuery::default()
        })
        .expect("finds");
    assert_eq!(
        after_first.iter().map(Entry::id).collect::<Vec<_>>(),
        vec!["entry-2"]
    );

    // The branch cache resolves the path from the leaf back to the root.
    let branch = session
        .find_entries_on_branch("entry-2", &EntryQuery::default(), &Default::default())
        .expect("branch");
    assert_eq!(
        branch.iter().map(Entry::id).collect::<Vec<_>>(),
        vec!["entry-2", "entry-1"]
    );

    let stats = session.get_stats().expect("stats");
    assert_eq!(stats.message_count, 2);
    assert_eq!(
        session
            .get_entry("entry-1")
            .expect("entry")
            .map(|entry| entry.id().to_owned()),
        Some("entry-1".to_owned())
    );
    harness.repo.close().await;
}

#[tokio::test]
async fn manages_lanes_records_and_open_operations() {
    let harness = harness();
    let cwd = harness.directory.path().to_string_lossy().into_owned();
    let session = harness
        .repo
        .create(create_options(&cwd, "session-1"))
        .await
        .expect("creates");
    session
        .append_entry(message_entry("entry-1", "one"), "main")
        .await
        .expect("appends");

    session
        .create_lane("side", Some("entry-1"))
        .await
        .expect("creates lane");
    let lanes = session.get_lanes().expect("lanes");
    assert_eq!(
        lanes,
        vec![
            ("main".to_owned(), Some("entry-1".to_owned())),
            ("side".to_owned(), Some("entry-1".to_owned()))
        ]
    );
    session.move_lane("side", None).await.expect("moves lane");
    assert_eq!(session.get_lanes().expect("lanes")[1].1, None);

    let started = LaneRecord::OperationStarted(OperationStartedRecord {
        id: "run-1".to_owned(),
        seq: 0,
        lane: "main".to_owned(),
        timestamp: 0,
        source_leaf_id: Some("entry-1".to_owned()),
        intent: OperationIntent::Run {
            original_prompt: vec![],
            initial_messages: vec![],
            system_prompt_override: None,
            resume_data: None,
        },
    });
    let committed = session
        .append_record(started)
        .await
        .expect("appends record");
    assert_eq!(committed.id(), "run-1");
    assert!(committed.seq() > 0);

    let open = session
        .find_open_operations("main")
        .expect("open operations");
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].id, "run-1");

    let records = session
        .find_records(&RecordQuery {
            order: Some(EntryOrder::OldestFirst),
            ..RecordQuery::default()
        })
        .expect("records");
    assert_eq!(records.len(), 1);

    let finished = LaneRecord::OperationFinished(
        notagent_session_sqlite::session_types::OperationFinishedRecord {
            id: "finish-1".to_owned(),
            seq: 0,
            lane: "main".to_owned(),
            timestamp: 0,
            run_id: "run-1".to_owned(),
            outcome: notagent_session_sqlite::session_types::OperationOutcome::Completed,
            error: None,
        },
    );
    session
        .append_record(finished)
        .await
        .expect("appends record");
    assert!(
        session
            .find_open_operations("main")
            .expect("open operations")
            .is_empty()
    );
    harness.repo.close().await;
}

#[tokio::test]
async fn projects_name_and_label_facts_and_the_log() {
    let harness = harness();
    let cwd = harness.directory.path().to_string_lossy().into_owned();
    let session = harness
        .repo
        .create(create_options(&cwd, "session-1"))
        .await
        .expect("creates");
    session
        .append_entry(message_entry("entry-1", "one"), "main")
        .await
        .expect("appends");

    assert_eq!(session.get_name().expect("name"), None);
    session.set_name(Some("Review")).await.expect("sets name");
    assert_eq!(session.get_name().expect("name"), Some("Review".to_owned()));
    session
        .set_label("entry-1", Some("start"))
        .await
        .expect("sets label");
    assert_eq!(
        session.get_label("entry-1").expect("label"),
        Some("start".to_owned())
    );
    session
        .set_label("entry-1", None)
        .await
        .expect("clears label");
    assert_eq!(session.get_label("entry-1").expect("label"), None);

    // The name is projected into the session metadata.
    assert_eq!(
        session.get_metadata().expect("metadata").name,
        Some("Review".to_owned())
    );

    let log = session.get_log(&Default::default()).expect("log");
    let kinds: Vec<&str> = log
        .iter()
        .map(|item| match item {
            LogItem::Entry { .. } => "entry",
            LogItem::Record { .. } => "record",
            LogItem::Lane { .. } => "lane",
            LogItem::Fact(_) => "fact",
        })
        .collect();
    assert_eq!(kinds, vec!["entry", "fact", "fact", "fact"]);
    harness.repo.close().await;
}

#[tokio::test]
async fn forks_a_branch_and_deletes_a_session() {
    let harness = harness();
    let cwd = harness.directory.path().to_string_lossy().into_owned();
    let session = harness
        .repo
        .create(create_options(&cwd, "session-1"))
        .await
        .expect("creates");
    session
        .append_entry(message_entry("entry-1", "one"), "main")
        .await
        .expect("appends");
    session
        .append_entry(message_entry("entry-2", "two"), "main")
        .await
        .expect("appends");
    let metadata = session.get_metadata().expect("metadata");

    let forked = harness
        .repo
        .fork(
            &metadata,
            &ForkOptions {
                scope: ForkScope::Branch,
                entry_id: Some("entry-2".to_owned()),
                position: None,
            },
            create_options(&cwd, "session-fork"),
        )
        .await
        .expect("forks");
    let forked_entries = forked
        .find_entries(&EntryQuery {
            order: Some(EntryOrder::OldestFirst),
            ..EntryQuery::default()
        })
        .expect("finds");
    // position defaults to "before" when an entryId is given: entry-2 is excluded.
    assert_eq!(
        forked_entries.iter().map(Entry::id).collect::<Vec<_>>(),
        vec!["entry-1"]
    );
    assert_eq!(
        forked.get_metadata().expect("metadata").parent_session_id,
        Some("session-1".to_owned())
    );

    let tree_fork = harness
        .repo
        .fork(
            &metadata,
            &ForkOptions {
                scope: ForkScope::Tree,
                entry_id: None,
                position: None,
            },
            create_options(&cwd, "session-tree"),
        )
        .await
        .expect("forks");
    let tree_entries = tree_fork
        .find_entries(&EntryQuery {
            order: Some(EntryOrder::OldestFirst),
            ..EntryQuery::default()
        })
        .expect("finds");
    assert_eq!(
        tree_entries.iter().map(Entry::id).collect::<Vec<_>>(),
        vec!["entry-1", "entry-2"]
    );

    harness.repo.delete(&metadata).await.expect("deletes");
    let remaining = harness
        .repo
        .list(&SqliteSessionListOptions::default())
        .await
        .expect("lists");
    assert!(
        !remaining.iter().any(|session| session.id == "session-1"),
        "{remaining:?}"
    );
    harness.repo.close().await;
}

#[tokio::test]
async fn rejects_a_second_writer_and_releases_the_lease() {
    let harness = harness();
    let cwd = harness.directory.path().to_string_lossy().into_owned();
    let session = harness
        .repo
        .create(create_options(&cwd, "session-1"))
        .await
        .expect("creates");
    let metadata = session.get_metadata().expect("metadata");

    // The same repository reuses its active storage instead of claiming twice.
    let reopened = harness.repo.open(&metadata).await.expect("opens");
    assert!(Arc::ptr_eq(&session, &reopened));

    // A second repository on the same database sees the active writer lease.
    let second = SqliteSessionRepository::new(SqliteSessionRepositoryOptions {
        sqlite: create_sqlite_factory(),
        database_path: harness
            .directory
            .path()
            .join("sessions.sqlite")
            .to_string_lossy()
            .into_owned(),
        writer_lease: None,
    })
    .expect("repository");
    let error = second.open(&metadata).await.expect_err("rejects");
    assert!(
        error.message.contains("already has an active writer"),
        "{}",
        error.message
    );

    session.release().await;
    let claimed = second.open(&metadata).await.expect("claims after release");
    assert_eq!(claimed.metadata_id(), "session-1");
    second.close().await;
    harness.repo.close().await;
}

#[tokio::test]
async fn reports_a_missing_session() {
    let harness = harness();
    let missing = SqliteSessionMetadata {
        id: "missing".to_owned(),
        created_at: 1,
        parent_session_id: None,
        cwd: "/tmp".to_owned(),
        path: harness
            .directory
            .path()
            .join("sessions.sqlite")
            .to_string_lossy()
            .into_owned(),
        name: None,
        metadata: None,
    };
    let error = harness.repo.open(&missing).await.expect_err("rejects");
    assert!(
        error.message.contains("Session not found"),
        "{}",
        error.message
    );
    harness.repo.close().await;
}

/// Entry types are decoded back into their typed form.
#[tokio::test]
async fn round_trips_every_entry_type() {
    let harness = harness();
    let cwd = harness.directory.path().to_string_lossy().into_owned();
    let session = harness
        .repo
        .create(create_options(&cwd, "session-1"))
        .await
        .expect("creates");

    session
        .append_entry(message_entry("entry-1", "one"), "main")
        .await
        .expect("appends");
    session
        .append_entry(
            ProvisionedEntry::ModelChange {
                id: "entry-2".to_owned(),
                provider: "anthropic".to_owned(),
                model_id: "claude".to_owned(),
            },
            "main",
        )
        .await
        .expect("appends");
    session
        .append_entry(
            ProvisionedEntry::Custom {
                id: "entry-3".to_owned(),
                custom_type: "mode".to_owned(),
                data: Some(json!({ "name": "plan" })),
            },
            "main",
        )
        .await
        .expect("appends");

    let entries = session
        .find_entries(&EntryQuery {
            order: Some(EntryOrder::OldestFirst),
            ..EntryQuery::default()
        })
        .expect("finds");
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.entry_type())
            .collect::<Vec<_>>(),
        vec![
            EntryType::Message,
            EntryType::ModelChange,
            EntryType::Custom
        ]
    );
    let custom = session
        .find_entries(&EntryQuery {
            custom_type: Some("mode".to_owned()),
            ..EntryQuery::default()
        })
        .expect("finds");
    assert_eq!(custom.len(), 1);
    assert_eq!(custom[0].id(), "entry-3");
    harness.repo.close().await;
}
