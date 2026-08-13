//! Port of the session backend conformance suite
//! `packages/agent/src/harness/session/testing/conformance.ts`, driven by the
//! SQLite backend as in
//! `packages/session-backends/sqlite-node/test/conformance.test.ts`.
//!
//! Deviation class 1: TS generates the cases from a fixture factory so several
//! backends can share them; the port has one backend, so the cases are plain
//! tests against `SqliteSessionRepository`. Group and case names are kept.

use notagent_agent::AgentMessage;
use notagent_ai::{TextContent, TextOrImageContent, UserContent, UserMessage};
use notagent_session_sqlite::session_types::{
    BranchBounds, Entry, EntryCursor, EntryOrder, EntryQuery, EntryType, LaneRecord, LogItem,
    OperationFinishedRecord, OperationIntent, OperationKind, OperationOutcome,
    OperationStartedRecord, ProvisionedEntry, QueueEnqueuedRecord, QueueKind, RecordQuery,
    RecordType, SessionErrorCode,
};
use notagent_session_sqlite::sqlite::repo::{
    SqliteSessionRepository, SqliteSessionRepositoryOptions,
};
use notagent_session_sqlite::{Session, SqliteSessionCreateOptions, create_sqlite_factory};

struct Fixture {
    _directory: tempfile::TempDir,
    repo: SqliteSessionRepository,
}

async fn fixture(id: &str) -> (Fixture, Session) {
    let directory = tempfile::Builder::new()
        .prefix("notagent-conformance-")
        .tempdir()
        .expect("temp dir");
    let cwd = directory.path().to_string_lossy().into_owned();
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
            id: Some(id.to_owned()),
            cwd,
            ..SqliteSessionCreateOptions::default()
        })
        .await
        .expect("creates");
    (
        Fixture {
            _directory: directory,
            repo,
        },
        Session::new(storage),
    )
}

fn user_message(text: &str) -> AgentMessage {
    AgentMessage::User(UserMessage {
        content: UserContent::Blocks(vec![TextOrImageContent::Text(TextContent::new(text))]),
        timestamp: 1,
    })
}

fn message(id: &str, text: &str) -> ProvisionedEntry {
    ProvisionedEntry::Message {
        id: id.to_owned(),
        message: user_message(text),
        terminate: None,
    }
}

fn custom(id: &str, custom_type: &str, data: Option<serde_json::Value>) -> ProvisionedEntry {
    ProvisionedEntry::Custom {
        id: id.to_owned(),
        custom_type: custom_type.to_owned(),
        data,
    }
}

fn operation_started(id: &str, lane: &str) -> LaneRecord {
    LaneRecord::OperationStarted(OperationStartedRecord {
        id: id.to_owned(),
        seq: 0,
        lane: lane.to_owned(),
        timestamp: 0,
        source_leaf_id: None,
        intent: OperationIntent::Run {
            original_prompt: vec![],
            initial_messages: vec![],
            system_prompt_override: None,
            resume_data: None,
        },
    })
}

fn entry_ids(entries: &[Entry]) -> Vec<String> {
    entries.iter().map(|entry| entry.id().to_owned()).collect()
}

fn log_kinds(log: &[LogItem]) -> Vec<(&'static str, i64)> {
    log.iter()
        .map(|item| match item {
            LogItem::Entry { seq, .. } => ("entry", *seq),
            LogItem::Record { seq, .. } => ("record", *seq),
            LogItem::Lane { seq, .. } => ("lane", *seq),
            LogItem::Fact(fact) => (
                "fact",
                match fact {
                    notagent_session_sqlite::session_types::LogFact::Name { seq, .. } => *seq,
                    notagent_session_sqlite::session_types::LogFact::Label { seq, .. } => *seq,
                },
            ),
        })
        .collect()
}

// --- entries and lanes -------------------------------------------------------

#[tokio::test]
async fn assigns_parents_and_one_sequence_across_every_mutation() {
    let (fixture, session) = fixture("session").await;
    let root = session
        .append_entry(message("root", "root"), "main")
        .await
        .expect("appends");
    session
        .create_lane("thread", Some(root.id()))
        .await
        .expect("creates lane");
    let child = session
        .append_entry(
            custom("child", "note", Some(serde_json::json!({ "value": 1 }))),
            "thread",
        )
        .await
        .expect("appends");
    let record = session
        .append_record(operation_started("run", "thread"))
        .await
        .expect("appends record");
    session.set_name(Some("Example")).await.expect("sets name");
    session
        .set_label(root.id(), Some("checkpoint"))
        .await
        .expect("sets label");
    session
        .move_lane("main", Some(child.id()))
        .await
        .expect("moves lane");

    assert_eq!((root.parent_id(), root.seq()), (None, 1));
    assert_eq!((child.parent_id(), child.seq()), (Some("root"), 3));
    assert_eq!(record.seq(), 4);
    for timestamp in [root.timestamp(), child.timestamp(), record.seq()] {
        assert!(
            timestamp >= 0,
            "storage-assigned timestamps must be Unix milliseconds"
        );
    }
    assert_eq!(
        log_kinds(&session.get_log(&Default::default()).expect("log")),
        vec![
            ("entry", 1),
            ("lane", 2),
            ("entry", 3),
            ("record", 4),
            ("fact", 5),
            ("fact", 6),
            ("lane", 7)
        ]
    );
    let lanes = session.get_lanes().expect("lanes");
    assert_eq!(
        lanes
            .iter()
            .map(|lane| (lane.lane.as_str(), lane.leaf_id.as_deref()))
            .collect::<Vec<_>>(),
        vec![("main", Some("child")), ("thread", Some("child"))]
    );
    fixture.repo.close().await;
}

#[tokio::test]
async fn rejects_duplicate_ids_without_changing_state() {
    let (fixture, session) = fixture("session").await;
    session
        .append_entry(message("shared", "root"), "main")
        .await
        .expect("appends");
    let error = session
        .append_record(operation_started("shared", "main"))
        .await
        .expect_err("rejects");
    assert_eq!(error.code, SessionErrorCode::AlreadyExists);
    session
        .append_record(operation_started("run", "main"))
        .await
        .expect("appends");
    let error = session
        .append_entry(custom("run", "note", None), "main")
        .await
        .expect_err("rejects");
    assert_eq!(error.code, SessionErrorCode::AlreadyExists);
    assert_eq!(
        session
            .get_log(&Default::default())
            .expect("log")
            .iter()
            .map(|item| log_kinds(&[item.clone()])[0].1)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
    fixture.repo.close().await;
}

#[tokio::test]
async fn isolates_lanes_while_sharing_the_tree() {
    let (fixture, session) = fixture("session").await;
    session
        .append_entry(message("root", "root"), "main")
        .await
        .expect("appends");
    session
        .create_lane("thread", Some("root"))
        .await
        .expect("creates lane");
    session
        .append_entry(message("main-child", "main"), "main")
        .await
        .expect("appends");
    session
        .append_entry(message("thread-child", "thread"), "thread")
        .await
        .expect("appends");

    let lanes = session.get_lanes().expect("lanes");
    assert_eq!(
        lanes
            .iter()
            .map(|lane| (lane.lane.as_str(), lane.leaf_id.as_deref()))
            .collect::<Vec<_>>(),
        vec![
            ("main", Some("main-child")),
            ("thread", Some("thread-child"))
        ]
    );
    let oldest_first = EntryQuery {
        order: Some(EntryOrder::OldestFirst),
        ..EntryQuery::default()
    };
    assert_eq!(
        entry_ids(
            &session
                .find_entries_on_branch(
                    &oldest_first,
                    &BranchBounds {
                        start: Some("main-child".to_owned()),
                        ..BranchBounds::default()
                    }
                )
                .expect("branch")
        ),
        vec!["root", "main-child"]
    );
    assert_eq!(
        entry_ids(
            &session
                .find_entries_on_branch(
                    &oldest_first,
                    &BranchBounds {
                        start: Some("thread-child".to_owned()),
                        ..BranchBounds::default()
                    }
                )
                .expect("branch")
        ),
        vec!["root", "thread-child"]
    );
    fixture.repo.close().await;
}

// --- records and log ---------------------------------------------------------

#[tokio::test]
async fn commits_records_and_lane_moves_as_separate_mutations() {
    let (fixture, session) = fixture("session").await;
    session
        .append_entry(message("root", "root"), "main")
        .await
        .expect("appends");
    let finished = session
        .append_record(LaneRecord::OperationFinished(OperationFinishedRecord {
            id: "finish".to_owned(),
            seq: 0,
            lane: "main".to_owned(),
            timestamp: 0,
            run_id: "run".to_owned(),
            outcome: OperationOutcome::Completed,
            error: None,
        }))
        .await
        .expect("appends record");

    assert_eq!(finished.seq(), 2);
    assert_eq!(
        session.get_lanes().expect("lanes")[0].leaf_id.as_deref(),
        Some("root")
    );
    session.move_lane("main", None).await.expect("moves lane");
    assert_eq!(session.get_lanes().expect("lanes")[0].leaf_id, None);
    assert_eq!(
        log_kinds(&session.get_log(&Default::default()).expect("log")),
        vec![("entry", 1), ("record", 2), ("lane", 3)]
    );

    let error = session
        .move_lane("main", Some("missing"))
        .await
        .expect_err("rejects");
    assert_eq!(error.code, SessionErrorCode::NotFound);
    assert_eq!(
        session
            .find_records(&RecordQuery::default())
            .expect("records")
            .len(),
        1
    );
    assert_eq!(
        log_kinds(&session.get_log(&Default::default()).expect("log"))
            .iter()
            .map(|(_, seq)| *seq)
            .collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    fixture.repo.close().await;
}

#[tokio::test]
async fn keeps_lane_names_permanent_with_their_recovery_records() {
    let (fixture, session) = fixture("session").await;
    session
        .create_lane("thread", None)
        .await
        .expect("creates lane");
    session
        .append_record(operation_started("old-run", "thread"))
        .await
        .expect("appends");
    session
        .append_record(LaneRecord::QueueEnqueued(QueueEnqueuedRecord {
            id: "old-next-run".to_owned(),
            seq: 0,
            lane: "thread".to_owned(),
            timestamp: 0,
            queue: QueueKind::NextRun,
            run_id: None,
            target: message("queued-message", "queued"),
        }))
        .await
        .expect("appends");

    let records = session
        .find_records(&RecordQuery {
            lane: Some("thread".to_owned()),
            ..RecordQuery::default()
        })
        .expect("records");
    assert_eq!(
        records
            .iter()
            .map(|record| record.id().to_owned())
            .collect::<Vec<_>>(),
        vec!["old-next-run".to_owned(), "old-run".to_owned()]
    );
    let log_record_ids: Vec<String> = session
        .get_log(&Default::default())
        .expect("log")
        .iter()
        .filter_map(|item| match item {
            LogItem::Record { record, .. } => Some(record.id().to_owned()),
            _ => None,
        })
        .collect();
    assert_eq!(
        log_record_ids,
        vec!["old-run".to_owned(), "old-next-run".to_owned()]
    );
    fixture.repo.close().await;
}

// --- queries and facts -------------------------------------------------------

#[tokio::test]
async fn rejects_invalid_queries_before_empty_reads() {
    let (fixture, session) = fixture("invalid-queries").await;
    session
        .create_lane("thread", None)
        .await
        .expect("creates lane");

    let invalid_limit = EntryQuery {
        limit: Some(0),
        ..EntryQuery::default()
    };
    for error in [
        session.find_entries(&invalid_limit).expect_err("rejects"),
        session.find_entry(&invalid_limit).expect_err("rejects"),
        session
            .find_entries_on_branch(&invalid_limit, &BranchBounds::default())
            .expect_err("rejects"),
        session
            .find_entries_on_branch_in_lane(
                "thread",
                &EntryQuery {
                    cursor: Some(EntryCursor { after_seq: -1 }),
                    ..EntryQuery::default()
                },
                &BranchBounds::default(),
            )
            .expect_err("rejects"),
        session
            .find_entry_on_branch_in_lane("thread", &invalid_limit, &BranchBounds::default())
            .expect_err("rejects"),
        session
            .find_records(&RecordQuery {
                limit: Some(0),
                ..RecordQuery::default()
            })
            .expect_err("rejects"),
        session
            .find_records(&RecordQuery {
                operation_kind: Some(OperationKind::Run),
                ..RecordQuery::default()
            })
            .expect_err("rejects"),
        session
            .find_records(&RecordQuery {
                record_type: Some(RecordType::StepAttempt),
                operation_kind: Some(OperationKind::Run),
                ..RecordQuery::default()
            })
            .expect_err("rejects"),
        session
            .find_open_operations("main", Some(0))
            .expect_err("rejects"),
        session
            .find_open_operations("main", Some(-1))
            .expect_err("rejects"),
        session
            .get_log(&notagent_session_sqlite::session_types::LogOptions {
                after_seq: Some(-1),
                limit: None,
            })
            .expect_err("rejects"),
    ] {
        assert_eq!(
            error.code,
            SessionErrorCode::InvalidQuery,
            "{}",
            error.message
        );
    }
    fixture.repo.close().await;
}

#[tokio::test]
async fn supports_bounded_filtered_and_cursor_based_queries() {
    let (fixture, session) = fixture("session").await;
    session
        .append_entry(message("root", "root"), "main")
        .await
        .expect("appends");
    session
        .append_entry(
            custom("old-note", "note", Some(serde_json::json!(1))),
            "main",
        )
        .await
        .expect("appends");
    session
        .append_entry(
            ProvisionedEntry::Compaction {
                id: "compact".to_owned(),
                summary: "summary".to_owned(),
                retained_tail: vec![],
                tokens_before: 10,
                details: None,
                usage: None,
            },
            "main",
        )
        .await
        .expect("appends");
    session
        .append_entry(
            custom("new-note", "note", Some(serde_json::json!(2))),
            "main",
        )
        .await
        .expect("appends");
    session
        .append_entry(message("tail", "tail"), "main")
        .await
        .expect("appends");

    assert_eq!(
        entry_ids(&session.find_entries(&EntryQuery::default()).expect("finds")),
        vec!["tail", "new-note", "compact", "old-note", "root"]
    );
    assert_eq!(
        entry_ids(
            &session
                .find_entries(&EntryQuery {
                    order: Some(EntryOrder::OldestFirst),
                    cursor: Some(EntryCursor { after_seq: 2 }),
                    limit: Some(2),
                    ..EntryQuery::default()
                })
                .expect("finds")
        ),
        vec!["compact", "new-note"]
    );
    assert_eq!(
        entry_ids(
            &session
                .find_entries(&EntryQuery {
                    custom_type: Some("note".to_owned()),
                    ..EntryQuery::default()
                })
                .expect("finds")
        ),
        vec!["new-note", "old-note"]
    );
    assert_eq!(
        entry_ids(
            &session
                .find_entries_on_branch(
                    &EntryQuery {
                        custom_type: Some("note".to_owned()),
                        limit: Some(1),
                        ..EntryQuery::default()
                    },
                    &BranchBounds {
                        start: Some("tail".to_owned()),
                        ..BranchBounds::default()
                    }
                )
                .expect("finds")
        ),
        vec!["new-note"]
    );
    assert_eq!(
        entry_ids(
            &session
                .find_entries_on_branch(
                    &EntryQuery {
                        entry_type: Some(EntryType::Message),
                        ..EntryQuery::default()
                    },
                    &BranchBounds {
                        start: Some("tail".to_owned()),
                        stop_at_type: Some(EntryType::Compaction),
                        stop_at_id: None
                    }
                )
                .expect("finds")
        ),
        vec!["tail"]
    );
    assert!(
        session
            .find_entries_on_branch(
                &EntryQuery {
                    entry_type: Some(EntryType::Custom),
                    ..EntryQuery::default()
                },
                &BranchBounds {
                    start: Some("tail".to_owned()),
                    stop_at_type: None,
                    stop_at_id: Some("tail".to_owned())
                }
            )
            .expect("finds")
            .is_empty()
    );
    assert_eq!(
        entry_ids(
            &session
                .find_entries_on_branch(
                    &EntryQuery {
                        order: Some(EntryOrder::OldestFirst),
                        ..EntryQuery::default()
                    },
                    &BranchBounds {
                        start: Some("tail".to_owned()),
                        stop_at_type: Some(EntryType::Custom),
                        stop_at_id: None
                    }
                )
                .expect("finds")
        ),
        vec!["root", "old-note"]
    );
    assert_eq!(
        session
            .find_entries(&EntryQuery {
                limit: Some(0),
                ..EntryQuery::default()
            })
            .expect_err("rejects")
            .code,
        SessionErrorCode::InvalidQuery
    );
    assert_eq!(
        session
            .find_entries_on_branch(
                &EntryQuery::default(),
                &BranchBounds {
                    start: Some("missing".to_owned()),
                    ..BranchBounds::default()
                }
            )
            .expect_err("rejects")
            .code,
        SessionErrorCode::NotFound
    );
    fixture.repo.close().await;
}
