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
            .map(|item| log_kinds(std::slice::from_ref(item))[0].1)
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

#[tokio::test]
async fn persists_queue_cancellation_without_consuming_its_target() {
    let (fixture, session) = fixture("session").await;
    let enqueued = session
        .append_record(LaneRecord::QueueEnqueued(QueueEnqueuedRecord {
            id: "enqueue".to_owned(),
            seq: 0,
            lane: "main".to_owned(),
            timestamp: 0,
            queue: QueueKind::NextRun,
            run_id: None,
            target: message("queued-message", "queued"),
        }))
        .await
        .expect("appends");
    let cancelled = session
        .append_record(LaneRecord::QueueCancelled(
            notagent_session_sqlite::session_types::QueueCancelledRecord {
                id: "cancel".to_owned(),
                seq: 0,
                lane: "main".to_owned(),
                timestamp: 0,
                run_id: None,
                entry_id: "queued-message".to_owned(),
            },
        ))
        .await
        .expect("appends");
    let LaneRecord::QueueCancelled(cancelled_record) = &cancelled else {
        panic!("expected queue_cancelled")
    };
    assert_eq!(
        (cancelled_record.seq, cancelled_record.entry_id.as_str()),
        (2, "queued-message")
    );
    assert!(cancelled_record.run_id.is_none());
    assert!(
        session
            .get_entry("queued-message")
            .expect("entry")
            .is_none()
    );

    let cancellations = session
        .find_records(&RecordQuery {
            record_type: Some(RecordType::QueueCancelled),
            ..RecordQuery::default()
        })
        .expect("records");
    assert_eq!(cancellations, vec![cancelled.clone()]);
    let log = session.get_log(&Default::default()).expect("log");
    assert_eq!(
        log,
        vec![
            LogItem::Record {
                seq: enqueued.seq(),
                record: enqueued
            },
            LogItem::Record {
                seq: cancelled.seq(),
                record: cancelled
            },
        ]
    );
    fixture.repo.close().await;
}

fn step_attempt(id: &str, lane: &str, run_id: &str, result_entry_id: &str) -> LaneRecord {
    LaneRecord::StepAttempt(notagent_session_sqlite::session_types::StepAttemptRecord {
        id: id.to_owned(),
        seq: 0,
        lane: lane.to_owned(),
        timestamp: 0,
        run_id: run_id.to_owned(),
        step: notagent_session_sqlite::session_types::StepKind::Assistant,
        attempt: 1,
        result_entry_id: result_entry_id.to_owned(),
        compaction_reason: None,
    })
}

fn operation_finished(id: &str, lane: &str, run_id: &str) -> LaneRecord {
    LaneRecord::OperationFinished(OperationFinishedRecord {
        id: id.to_owned(),
        seq: 0,
        lane: lane.to_owned(),
        timestamp: 0,
        run_id: run_id.to_owned(),
        outcome: OperationOutcome::Completed,
        error: None,
    })
}

fn operation_started_with_kind(id: &str, lane: &str, intent: OperationIntent) -> LaneRecord {
    LaneRecord::OperationStarted(OperationStartedRecord {
        id: id.to_owned(),
        seq: 0,
        lane: lane.to_owned(),
        timestamp: 0,
        source_leaf_id: None,
        intent,
    })
}

fn record_ids(records: &[LaneRecord]) -> Vec<String> {
    records
        .iter()
        .map(|record| record.id().to_owned())
        .collect()
}

#[tokio::test]
async fn filters_records_by_lane_type_run_sequence_and_order() {
    let (fixture, session) = fixture("session").await;
    session
        .append_record(operation_started("run-1", "main"))
        .await
        .expect("appends");
    session
        .append_record(step_attempt("attempt-1", "main", "run-1", "assistant-1"))
        .await
        .expect("appends");
    session
        .create_lane("thread", None)
        .await
        .expect("creates lane");
    session
        .append_record(operation_started("run-2", "thread"))
        .await
        .expect("appends");
    session
        .append_record(step_attempt("attempt-2", "thread", "run-2", "assistant-2"))
        .await
        .expect("appends");

    assert_eq!(
        record_ids(
            &session
                .find_records(&RecordQuery {
                    lane: Some("thread".to_owned()),
                    ..RecordQuery::default()
                })
                .expect("records")
        ),
        vec!["attempt-2", "run-2"]
    );
    assert_eq!(
        record_ids(
            &session
                .find_records(&RecordQuery {
                    record_type: Some(RecordType::StepAttempt),
                    order: Some(EntryOrder::OldestFirst),
                    ..RecordQuery::default()
                })
                .expect("records")
        ),
        vec!["attempt-1", "attempt-2"]
    );
    assert_eq!(
        record_ids(
            &session
                .find_records(&RecordQuery {
                    run_id: Some("run-1".to_owned()),
                    after_seq: Some(1),
                    ..RecordQuery::default()
                })
                .expect("records")
        ),
        vec!["attempt-1"]
    );
    assert_eq!(
        record_ids(
            &session
                .find_records(&RecordQuery {
                    limit: Some(1),
                    ..RecordQuery::default()
                })
                .expect("records")
        ),
        vec!["attempt-2"]
    );
    fixture.repo.close().await;
}

#[tokio::test]
async fn filters_operation_starts_by_operation_kind() {
    let (fixture, session) = fixture("session").await;
    let run_intent = || OperationIntent::Run {
        original_prompt: vec![],
        initial_messages: vec![],
        system_prompt_override: None,
        resume_data: None,
    };
    session
        .append_record(operation_started_with_kind("run-old", "main", run_intent()))
        .await
        .expect("appends");
    session
        .append_record(operation_finished("run-old-finished", "main", "run-old"))
        .await
        .expect("appends");
    session
        .append_record(operation_started_with_kind(
            "compaction",
            "main",
            OperationIntent::Compaction {
                custom_instructions: None,
                result_entry_id: "result".to_owned(),
            },
        ))
        .await
        .expect("appends");
    session
        .append_record(operation_finished(
            "compaction-finished",
            "main",
            "compaction",
        ))
        .await
        .expect("appends");
    session
        .append_record(operation_started_with_kind(
            "navigation",
            "main",
            OperationIntent::Navigation {
                target_id: None,
                summarize: false,
                custom_instructions: None,
                label: None,
                summary_entry_id: None,
            },
        ))
        .await
        .expect("appends");
    session
        .append_record(operation_finished(
            "navigation-finished",
            "main",
            "navigation",
        ))
        .await
        .expect("appends");
    session
        .append_record(operation_started_with_kind("run-new", "main", run_intent()))
        .await
        .expect("appends");

    let started = |kind, order, limit| RecordQuery {
        record_type: Some(RecordType::OperationStarted),
        operation_kind: Some(kind),
        order,
        limit,
        ..RecordQuery::default()
    };
    assert_eq!(
        record_ids(
            &session
                .find_records(&started(
                    OperationKind::Run,
                    Some(EntryOrder::OldestFirst),
                    None
                ))
                .expect("records")
        ),
        vec!["run-old", "run-new"]
    );
    assert_eq!(
        record_ids(
            &session
                .find_records(&started(OperationKind::Compaction, None, None))
                .expect("records")
        ),
        vec!["compaction"]
    );
    assert_eq!(
        record_ids(
            &session
                .find_records(&started(OperationKind::Navigation, None, None))
                .expect("records")
        ),
        vec!["navigation"]
    );
    assert_eq!(
        record_ids(
            &session
                .find_records(&started(OperationKind::Run, None, Some(1)))
                .expect("records")
        ),
        vec!["run-new"]
    );
    fixture.repo.close().await;
}

#[tokio::test]
async fn tracks_and_enforces_one_open_operation_per_lane() {
    let (fixture, session) = fixture("session").await;
    assert!(
        session
            .find_open_operations("main", Some(2))
            .expect("open")
            .is_empty()
    );

    let first = session
        .append_record(operation_started("first", "main"))
        .await
        .expect("appends");
    let open = session.find_open_operations("main", Some(2)).expect("open");
    assert_eq!(open.len(), 1);
    assert_eq!(LaneRecord::OperationStarted(open[0].clone()), first);
    let error = session
        .append_record(operation_started("second", "main"))
        .await
        .expect_err("rejects");
    assert_eq!(error.code, SessionErrorCode::Storage);
    assert_eq!(
        session
            .find_open_operations("main", Some(2))
            .expect("open")
            .len(),
        1
    );

    session
        .append_record(operation_finished("finish-first", "main", "first"))
        .await
        .expect("appends");
    assert!(
        session
            .find_open_operations("main", Some(2))
            .expect("open")
            .is_empty()
    );
    fixture.repo.close().await;
}

#[tokio::test]
async fn does_not_let_an_earlier_finish_close_a_later_start() {
    let (fixture, session) = fixture("session").await;
    session
        .append_record(operation_finished("finish-before-start", "main", "run"))
        .await
        .expect("appends");
    let started = session
        .append_record(operation_started("run", "main"))
        .await
        .expect("appends");
    let open = session.find_open_operations("main", Some(2)).expect("open");
    assert_eq!(open.len(), 1);
    assert_eq!(LaneRecord::OperationStarted(open[0].clone()), started);
    fixture.repo.close().await;
}

#[tokio::test]
async fn scopes_open_operations_by_lane_and_limit() {
    let (fixture, session) = fixture("session").await;
    session
        .create_lane("thread", None)
        .await
        .expect("creates lane");
    let main_run = session
        .append_record(operation_started("main-run", "main"))
        .await
        .expect("appends");
    let thread_navigation = session
        .append_record(operation_started_with_kind(
            "thread-navigation",
            "thread",
            OperationIntent::Navigation {
                target_id: None,
                summarize: false,
                custom_instructions: None,
                label: None,
                summary_entry_id: None,
            },
        ))
        .await
        .expect("appends");

    for open in [
        session.find_open_operations("main", None).expect("open"),
        session.find_open_operations("main", Some(1)).expect("open"),
    ] {
        assert_eq!(LaneRecord::OperationStarted(open[0].clone()), main_run);
    }
    let thread_open = session
        .find_open_operations("thread", Some(2))
        .expect("open");
    assert_eq!(
        LaneRecord::OperationStarted(thread_open[0].clone()),
        thread_navigation
    );
    fixture.repo.close().await;
}

/// Rust returns owned values, so mutation cannot reach the storage at all.
#[tokio::test]
async fn returns_immutable_open_operation_records() {
    let (fixture, session) = fixture("session").await;
    let committed = session
        .append_record(operation_started("run", "main"))
        .await
        .expect("appends");
    let mut read = session.find_open_operations("main", None).expect("open");
    if let OperationIntent::Run {
        original_prompt, ..
    } = &mut read[0].intent
    {
        original_prompt.push(user_message("mutated"));
    }
    let reread = session.find_open_operations("main", None).expect("open");
    assert_eq!(LaneRecord::OperationStarted(reread[0].clone()), committed);
    fixture.repo.close().await;
}

/// `notagent_ai::Usage` counts tokens as `u64` (B's contract), so negative
/// token counts are not representable — only the negative cost survives. The
fn usage(
    input: i64,
    output: i64,
    cache_read: i64,
    cache_write: i64,
    total_tokens: i64,
    cost_total: f64,
) -> notagent_ai::Usage {
    notagent_ai::Usage {
        input: input.max(0) as u64,
        output: output.max(0) as u64,
        cache_read: cache_read.max(0) as u64,
        cache_write: cache_write.max(0) as u64,
        cache_write1h: None,
        reasoning: None,
        total_tokens: Some(total_tokens.max(0) as u64),
        cost: notagent_ai::UsageCost {
            input: 0.0,
            output: 0.0,
            cache_read: 0.0,
            cache_write: 0.0,
            total: cost_total,
        },
    }
}

#[tokio::test]
async fn keeps_latest_value_facts_and_computes_ledger_statistics_across_lanes() {
    let (fixture, session) = fixture("session").await;
    let assistant_usage = usage(10, 5, 3, 2, 20, 10.0);
    session
        .append_entry(message("user", "question"), "main")
        .await
        .expect("appends");
    session
        .append_entry(message("assistant", "answer"), "main")
        .await
        .expect("appends");
    session
        .append_record(LaneRecord::Usage(
            notagent_session_sqlite::session_types::UsageRecord {
                id: "assistant-usage".to_owned(),
                seq: 0,
                lane: "main".to_owned(),
                timestamp: 0,
                usage: assistant_usage,
                cause: notagent_session_sqlite::session_types::UsageCause::Assistant,
                run_id: Some("run".to_owned()),
                entry_id: Some("assistant".to_owned()),
                attempt: Some(1),
                stop_reason: Some(notagent_session_sqlite::session_types::SessionStopReason::Stop),
                tool_call_id: None,
                details: None,
            },
        ))
        .await
        .expect("appends");
    session
        .append_record(LaneRecord::Usage(
            notagent_session_sqlite::session_types::UsageRecord {
                id: "deferred-usage".to_owned(),
                seq: 0,
                lane: "main".to_owned(),
                timestamp: 0,
                usage: usage(0, 0, 0, 0, 0, 0.0),
                cause: notagent_session_sqlite::session_types::UsageCause::DeferredFetch,
                run_id: Some("run".to_owned()),
                entry_id: Some("deferred-result".to_owned()),
                attempt: Some(1),
                stop_reason: Some(
                    notagent_session_sqlite::session_types::SessionStopReason::Deferred,
                ),
                tool_call_id: None,
                details: None,
            },
        ))
        .await
        .expect("appends");
    session
        .create_lane("thread", Some("assistant"))
        .await
        .expect("creates lane");
    session
        .append_record(LaneRecord::Usage(
            notagent_session_sqlite::session_types::UsageRecord {
                id: "correction".to_owned(),
                seq: 0,
                lane: "thread".to_owned(),
                timestamp: 0,
                usage: usage(0, 0, 0, 0, 0, -0.5),
                cause: notagent_session_sqlite::session_types::UsageCause::Adjustment,
                run_id: None,
                entry_id: None,
                attempt: None,
                stop_reason: None,
                tool_call_id: None,
                details: Some(serde_json::json!({ "reason": "provider correction" })),
            },
        ))
        .await
        .expect("appends");
    session.set_name(Some("First")).await.expect("sets name");
    session.set_name(Some("Second")).await.expect("sets name");
    session
        .set_label("user", Some("keep"))
        .await
        .expect("sets label");
    session.set_label("user", None).await.expect("clears label");
    let error = session
        .set_label("missing", Some("checkpoint"))
        .await
        .expect_err("rejects");
    assert_eq!(error.code, SessionErrorCode::NotFound);

    assert_eq!(session.get_name().expect("name"), Some("Second".to_owned()));
    assert_eq!(session.get_label("user").expect("label"), None);
    let usage_records = session
        .find_records(&RecordQuery {
            record_type: Some(RecordType::Usage),
            order: Some(EntryOrder::OldestFirst),
            ..RecordQuery::default()
        })
        .expect("records");
    let causes: Vec<_> = usage_records
        .iter()
        .map(|record| match record {
            LaneRecord::Usage(usage) => usage.cause,
            _ => panic!("expected usage record"),
        })
        .collect();
    assert_eq!(
        causes,
        vec![
            notagent_session_sqlite::session_types::UsageCause::Assistant,
            notagent_session_sqlite::session_types::UsageCause::DeferredFetch,
            notagent_session_sqlite::session_types::UsageCause::Adjustment,
        ]
    );
    let stats = session.get_stats().expect("stats");
    assert_eq!(stats.message_count, 2);
    assert_eq!(stats.cached_tokens, 3.0);
    assert_eq!(stats.uncached_tokens, 12.0);
    assert_eq!(stats.total_tokens, 20.0);
    assert!(
        (stats.cost_total - 9.5).abs() < 1e-9,
        "cost_total: {}",
        stats.cost_total
    );
    fixture.repo.close().await;
}

#[tokio::test]
async fn clears_session_names_durably() {
    let (fixture, session) = fixture("session").await;
    session
        .set_name(Some("Temporary"))
        .await
        .expect("sets name");
    session.set_name(None).await.expect("clears name");

    assert_eq!(session.get_name().expect("name"), None);
    let expected_log = vec![
        LogItem::Fact(notagent_session_sqlite::session_types::LogFact::Name {
            seq: 1,
            name: Some("Temporary".to_owned()),
        }),
        LogItem::Fact(notagent_session_sqlite::session_types::LogFact::Name { seq: 2, name: None }),
    ];
    assert_eq!(
        session.get_log(&Default::default()).expect("log"),
        expected_log
    );

    let metadata = session.get_metadata().expect("metadata");
    let reopened = Session::new(fixture.repo.open(&metadata).await.expect("opens"));
    assert_eq!(reopened.get_name().expect("name"), None);
    assert_eq!(
        reopened.get_log(&Default::default()).expect("log"),
        expected_log
    );

    let fork = Session::new(
        fixture
            .repo
            .fork(
                &metadata,
                &notagent_session_sqlite::session_types::ForkOptions::default(),
                SqliteSessionCreateOptions {
                    id: Some("fork".to_owned()),
                    cwd: metadata.cwd.clone(),
                    ..SqliteSessionCreateOptions::default()
                },
            )
            .await
            .expect("forks"),
    );
    assert_eq!(fork.get_name().expect("name"), None);
    fixture.repo.close().await;
}

/// Rust returns owned values, so a mutation cannot reach the storage.
#[tokio::test]
async fn returns_immutable_copies_from_reads() {
    let (fixture, session) = fixture("immutable").await;
    let metadata = session.get_metadata().expect("metadata");
    session
        .append_entry(
            custom(
                "custom",
                "note",
                Some(serde_json::json!({ "nested": { "value": 1 } })),
            ),
            "main",
        )
        .await
        .expect("appends");

    let read = session
        .get_entry("custom")
        .expect("entry")
        .expect("custom entry");
    assert_eq!(session.get_metadata().expect("metadata"), metadata);
    let Entry::Custom(custom_entry) = &read else {
        panic!("expected a custom entry")
    };
    assert_eq!(
        custom_entry.data,
        Some(serde_json::json!({ "nested": { "value": 1 } }))
    );
    assert_eq!(custom_entry.parent_id, None);
    assert_eq!(custom_entry.seq, 1);
    fixture.repo.close().await;
}

#[tokio::test]
async fn validates_lane_lifecycle_and_targets() {
    let (fixture, session) = fixture("session").await;
    assert_eq!(
        session
            .create_lane("main", None)
            .await
            .expect_err("rejects")
            .code,
        SessionErrorCode::AlreadyExists
    );
    assert_eq!(
        session
            .create_lane("thread", Some("missing"))
            .await
            .expect_err("rejects")
            .code,
        SessionErrorCode::NotFound
    );
    assert_eq!(
        session
            .move_lane("missing", None)
            .await
            .expect_err("rejects")
            .code,
        SessionErrorCode::InvalidLane
    );
    fixture.repo.close().await;
}

#[tokio::test]
async fn binds_lane_views_without_caching_leaves() {
    let (fixture, session) = fixture("session").await;
    let root = session
        .append_message(user_message("root"))
        .await
        .expect("appends");
    session
        .create_lane("thread", Some(&root))
        .await
        .expect("creates lane");
    let main_child = session
        .append_message(user_message("main"))
        .await
        .expect("appends");
    let thread_child = session
        .append_message_in_lane("thread", user_message("thread"))
        .await
        .expect("appends");

    assert_eq!(
        session.get_leaf_id().expect("leaf"),
        Some(main_child.clone())
    );
    assert_eq!(
        session.get_leaf_id_in_lane("thread").expect("leaf"),
        Some(thread_child.clone())
    );
    let oldest_first = EntryQuery {
        order: Some(EntryOrder::OldestFirst),
        ..EntryQuery::default()
    };
    assert_eq!(
        entry_ids(
            &session
                .find_entries_on_branch(&oldest_first, &BranchBounds::default())
                .expect("branch")
        ),
        vec![root.clone(), main_child]
    );
    assert_eq!(
        entry_ids(
            &session
                .find_entries_on_branch_in_lane("thread", &oldest_first, &BranchBounds::default())
                .expect("branch")
        ),
        vec![root, thread_child]
    );

    let empty = Session::new(
        fixture
            .repo
            .create(SqliteSessionCreateOptions {
                id: Some("empty".to_owned()),
                cwd: session.get_metadata().expect("metadata").cwd,
                ..SqliteSessionCreateOptions::default()
            })
            .await
            .expect("creates"),
    );
    assert!(
        empty
            .find_entries_on_branch(&EntryQuery::default(), &BranchBounds::default())
            .expect("branch")
            .is_empty()
    );
    fixture.repo.close().await;
}

#[tokio::test]
async fn appends_provisioned_entries_with_their_existing_ids() {
    let (fixture, session) = fixture("session").await;
    let entry = session
        .append_entry(
            custom(
                "provisioned",
                "note",
                Some(serde_json::json!({ "value": 1 })),
            ),
            "main",
        )
        .await
        .expect("appends");

    assert_eq!(entry.custom_type(), Some("note"));
    assert_eq!(
        (entry.id(), entry.parent_id(), entry.seq()),
        ("provisioned", None, 1)
    );
    assert_eq!(
        session.get_leaf_id().expect("leaf"),
        Some("provisioned".to_owned())
    );
    fixture.repo.close().await;
}

#[tokio::test]
async fn persists_tool_result_termination_decisions() {
    let (fixture, session) = fixture("session").await;
    let tool_result = AgentMessage::ToolResult(notagent_ai::ToolResultMessage {
        tool_call_id: "call-1".to_owned(),
        tool_name: "example".to_owned(),
        content: vec![TextOrImageContent::Text(TextContent::new("done"))],
        details: None,
        usage: None,
        added_tool_names: None,
        is_error: false,
        timestamp: 1,
    });
    let entry = session
        .append_entry(
            ProvisionedEntry::Message {
                id: "tool-result".to_owned(),
                message: tool_result,
                terminate: Some(true),
            },
            "main",
        )
        .await
        .expect("appends");

    let Entry::Message(message_entry) = &entry else {
        panic!("expected a message entry")
    };
    assert_eq!(message_entry.terminate, Some(true));
    let stored = session
        .get_entry(entry.id())
        .expect("entry")
        .expect("stored");
    let Entry::Message(stored_message) = &stored else {
        panic!("expected a message entry")
    };
    assert_eq!(stored_message.terminate, Some(true));
    assert_eq!(
        session.find_entries(&EntryQuery::default()).expect("finds"),
        vec![entry.clone()]
    );
    assert_eq!(
        session.get_log(&Default::default()).expect("log"),
        vec![LogItem::Entry {
            seq: entry.seq(),
            entry
        }]
    );
    fixture.repo.close().await;
}

/// `serde_json::Value`, so the case checks the surviving invariant: a rejected
/// record leaves no trace and the next valid record still gets sequence 1.
#[tokio::test]
async fn rejects_invalid_records_before_storage_mutation() {
    let (fixture, session) = fixture("session").await;
    let error = session
        .append_record(operation_started("run", "missing-lane"))
        .await
        .expect_err("rejects");
    assert_eq!(error.code, SessionErrorCode::InvalidLane);

    assert!(
        session
            .find_records(&RecordQuery::default())
            .expect("records")
            .is_empty()
    );
    assert!(
        session
            .get_log(&Default::default())
            .expect("log")
            .is_empty()
    );
    assert_eq!(
        session
            .append_record(operation_started("valid-record", "main"))
            .await
            .expect("appends")
            .seq(),
        1
    );
    fixture.repo.close().await;
}

#[tokio::test]
async fn linearizes_concurrent_writes_across_two_lanes() {
    let (fixture, session) = fixture("session").await;
    session
        .append_entry(message("root", "root"), "main")
        .await
        .expect("appends");
    session
        .create_lane("thread", Some("root"))
        .await
        .expect("creates lane");

    let (main_1, thread_1, main_2, thread_2) = tokio::join!(
        session.append_entry(custom("main-1", "note", None), "main"),
        session.append_entry(custom("thread-1", "note", None), "thread"),
        session.append_entry(custom("main-2", "note", None), "main"),
        session.append_entry(custom("thread-2", "note", None), "thread"),
    );
    let entries = [
        main_1.expect("appends"),
        thread_1.expect("appends"),
        main_2.expect("appends"),
        thread_2.expect("appends"),
    ];

    let mut sequences: Vec<i64> = entries.iter().map(Entry::seq).collect();
    sequences.sort_unstable();
    sequences.dedup();
    assert_eq!(
        sequences.len(),
        entries.len(),
        "every write gets its own sequence"
    );

    let log = session.get_log(&Default::default()).expect("log");
    let log_sequences: Vec<i64> = log_kinds(&log).iter().map(|(_, seq)| *seq).collect();
    let mut sorted = log_sequences.clone();
    sorted.sort_unstable();
    assert_eq!(log_sequences, sorted, "the log stays in sequence order");
    fixture.repo.close().await;
}

// --- repository and forks ----------------------------------------------------

#[tokio::test]
async fn creates_lists_and_opens_sessions() {
    let (fixture, session) = fixture("one").await;
    let entry_id = session
        .append_message(user_message("persisted"))
        .await
        .expect("appends");
    let metadata = session.get_metadata().expect("metadata");

    let listed = fixture.repo.list(&Default::default()).await.expect("lists");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, metadata.id);
    assert_eq!(listed[0].created_at, metadata.created_at);
    assert_eq!(listed[0].parent_session_id, metadata.parent_session_id);

    let reopened = Session::new(fixture.repo.open(&metadata).await.expect("opens"));
    assert_eq!(
        entry_ids(
            &reopened
                .find_entries(&EntryQuery::default())
                .expect("finds")
        ),
        vec![entry_id]
    );

    let error = fixture
        .repo
        .create(SqliteSessionCreateOptions {
            id: Some("one".to_owned()),
            cwd: metadata.cwd.clone(),
            ..SqliteSessionCreateOptions::default()
        })
        .await
        .expect_err("rejects");
    assert_eq!(error.code, SessionErrorCode::AlreadyExists);
    fixture.repo.close().await;
}

#[tokio::test]
async fn deletes_sessions_idempotently() {
    let (fixture, session) = fixture("one").await;
    let metadata = session.get_metadata().expect("metadata");

    fixture.repo.delete(&metadata).await.expect("deletes");
    let error = fixture.repo.open(&metadata).await.expect_err("rejects");
    assert_eq!(error.code, SessionErrorCode::NotFound);
    fixture.repo.delete(&metadata).await.expect("deletes again");
    fixture.repo.close().await;
}

fn fork_options(
    scope: ForkScopeArg,
    entry_id: Option<&str>,
    position: Option<ForkPositionArg>,
) -> ForkOptionsArg {
    ForkOptionsArg {
        scope,
        entry_id: entry_id.map(str::to_owned),
        position,
    }
}

type ForkOptionsArg = notagent_session_sqlite::session_types::ForkOptions;
type ForkScopeArg = notagent_session_sqlite::session_types::ForkScope;
type ForkPositionArg = notagent_session_sqlite::session_types::ForkPosition;

#[tokio::test]
async fn forks_one_branch_with_selected_facts_and_no_records() {
    let (fixture, source) = fixture("source").await;
    let cwd = source.get_metadata().expect("metadata").cwd;
    let root = source
        .append_message(user_message("root"))
        .await
        .expect("appends");
    let shared = source
        .append_message(user_message("shared"))
        .await
        .expect("appends");
    source
        .create_lane("thread", Some(&shared))
        .await
        .expect("creates lane");
    let thread_child = source
        .append_message_in_lane("thread", user_message("thread"))
        .await
        .expect("appends");
    let main_child = source
        .append_message(user_message("main"))
        .await
        .expect("appends");
    source.set_name(Some("Source")).await.expect("sets name");
    source
        .set_label(&shared, Some("copied"))
        .await
        .expect("sets label");
    source
        .set_label(&thread_child, Some("excluded"))
        .await
        .expect("sets label");
    source
        .append_record(operation_started("run", "main"))
        .await
        .expect("appends");

    let fork = Session::new(
        fixture
            .repo
            .fork(
                &source.get_metadata().expect("metadata"),
                &fork_options(
                    ForkScopeArg::Branch,
                    Some(&main_child),
                    Some(ForkPositionArg::At),
                ),
                SqliteSessionCreateOptions {
                    id: Some("branch-fork".to_owned()),
                    cwd: cwd.clone(),
                    ..SqliteSessionCreateOptions::default()
                },
            )
            .await
            .expect("forks"),
    );

    let oldest_first = EntryQuery {
        order: Some(EntryOrder::OldestFirst),
        ..EntryQuery::default()
    };
    assert_eq!(
        entry_ids(&fork.find_entries(&oldest_first).expect("finds")),
        vec![root, shared.clone(), main_child.clone()]
    );
    let lanes = fork.get_lanes().expect("lanes");
    assert_eq!(lanes.len(), 1);
    assert_eq!(
        (lanes[0].lane.as_str(), lanes[0].leaf_id.as_deref()),
        ("main", Some(main_child.as_str()))
    );
    assert_eq!(fork.get_name().expect("name"), Some("Source".to_owned()));
    assert_eq!(
        fork.get_label(&shared).expect("label"),
        Some("copied".to_owned())
    );
    assert_eq!(fork.get_label(&thread_child).expect("label"), None);
    assert!(
        fork.find_records(&RecordQuery::default())
            .expect("records")
            .is_empty()
    );
    let stats = fork.get_stats().expect("stats");
    assert_eq!(stats.message_count, 3);
    assert_eq!(stats.total_tokens, 0.0);
    fork.append_message(user_message("after fork"))
        .await
        .expect("appends");
    assert_eq!(fork.get_stats().expect("stats").message_count, 4);
    let metadata = fork.get_metadata().expect("metadata");
    assert_eq!(
        (metadata.id.as_str(), metadata.parent_session_id.as_deref()),
        ("branch-fork", Some("source"))
    );
    fixture.repo.close().await;
}

#[tokio::test]
async fn forks_a_complete_tree_with_lanes_and_facts() {
    let (fixture, source) = fixture("source").await;
    let cwd = source.get_metadata().expect("metadata").cwd;
    let root = source
        .append_message(user_message("root"))
        .await
        .expect("appends");
    source
        .create_lane("thread", Some(&root))
        .await
        .expect("creates lane");
    let main_child = source
        .append_message(user_message("main"))
        .await
        .expect("appends");
    let thread_child = source
        .append_message_in_lane("thread", user_message("thread"))
        .await
        .expect("appends");
    source
        .set_label(&thread_child, Some("thread-tip"))
        .await
        .expect("sets label");

    let fork = Session::new(
        fixture
            .repo
            .fork(
                &source.get_metadata().expect("metadata"),
                &fork_options(ForkScopeArg::Tree, None, None),
                SqliteSessionCreateOptions {
                    id: Some("tree-fork".to_owned()),
                    cwd,
                    ..SqliteSessionCreateOptions::default()
                },
            )
            .await
            .expect("forks"),
    );

    let oldest_first = EntryQuery {
        order: Some(EntryOrder::OldestFirst),
        ..EntryQuery::default()
    };
    assert_eq!(
        entry_ids(&fork.find_entries(&oldest_first).expect("finds")),
        vec![root, main_child.clone(), thread_child.clone()]
    );
    let lanes = fork.get_lanes().expect("lanes");
    assert_eq!(
        lanes
            .iter()
            .map(|lane| (lane.lane.as_str(), lane.leaf_id.as_deref()))
            .collect::<Vec<_>>(),
        vec![
            ("main", Some(main_child.as_str())),
            ("thread", Some(thread_child.as_str()))
        ]
    );
    assert_eq!(
        fork.get_label(&thread_child).expect("label"),
        Some("thread-tip".to_owned())
    );
    assert_eq!(fork.get_stats().expect("stats").message_count, 3);
    let lane_items: Vec<(i64, String, Option<String>)> = fork
        .get_log(&Default::default())
        .expect("log")
        .into_iter()
        .filter_map(|item| match item {
            LogItem::Lane { seq, lane, leaf_id } => Some((seq, lane, leaf_id)),
            _ => None,
        })
        .collect();
    assert_eq!(
        lane_items,
        vec![
            (4, "main".to_owned(), Some(main_child)),
            (5, "thread".to_owned(), Some(thread_child)),
        ]
    );
    fixture.repo.close().await;
}

#[tokio::test]
async fn forks_before_an_entry_without_modifying_the_source() {
    let (fixture, source) = fixture("source").await;
    let cwd = source.get_metadata().expect("metadata").cwd;
    let root = source
        .append_message(user_message("root"))
        .await
        .expect("appends");
    let tail = source
        .append_message(user_message("tail"))
        .await
        .expect("appends");
    let metadata = source.get_metadata().expect("metadata");
    let options = |id: &str| SqliteSessionCreateOptions {
        id: Some(id.to_owned()),
        cwd: cwd.clone(),
        ..SqliteSessionCreateOptions::default()
    };
    let oldest_first = EntryQuery {
        order: Some(EntryOrder::OldestFirst),
        ..EntryQuery::default()
    };

    let fork = Session::new(
        fixture
            .repo
            .fork(
                &metadata,
                &fork_options(ForkScopeArg::Branch, Some(&tail), None),
                options("fork"),
            )
            .await
            .expect("forks"),
    );
    assert_eq!(
        entry_ids(&fork.find_entries(&oldest_first).expect("finds")),
        vec![root.clone()]
    );
    assert_eq!(fork.get_leaf_id().expect("leaf"), Some(root.clone()));
    assert_eq!(source.get_leaf_id().expect("leaf"), Some(tail.clone()));

    let before_default = Session::new(
        fixture
            .repo
            .fork(
                &metadata,
                &fork_options(ForkScopeArg::Branch, None, Some(ForkPositionArg::Before)),
                options("before-default-target"),
            )
            .await
            .expect("forks"),
    );
    assert_eq!(
        entry_ids(&before_default.find_entries(&oldest_first).expect("finds")),
        vec![root.clone()]
    );
    assert_eq!(
        before_default.get_leaf_id().expect("leaf"),
        Some(root.clone())
    );

    let at_default = Session::new(
        fixture
            .repo
            .fork(
                &metadata,
                &fork_options(ForkScopeArg::Branch, None, Some(ForkPositionArg::At)),
                options("at-default-target"),
            )
            .await
            .expect("forks"),
    );
    assert_eq!(
        entry_ids(&at_default.find_entries(&oldest_first).expect("finds")),
        vec![root, tail.clone()]
    );
    assert_eq!(at_default.get_leaf_id().expect("leaf"), Some(tail));

    let error = fixture
        .repo
        .fork(
            &metadata,
            &fork_options(ForkScopeArg::Branch, Some("missing"), None),
            options("missing-fork"),
        )
        .await
        .expect_err("rejects");
    assert_eq!(error.code, SessionErrorCode::InvalidForkTarget);
    fixture.repo.close().await;
}

#[tokio::test]
async fn validates_the_default_fork_target() {
    let (fixture, source) = fixture("source-with-custom-leaf").await;
    let cwd = source.get_metadata().expect("metadata").cwd;
    source
        .append_custom_entry("not-a-message", None)
        .await
        .expect("appends");

    let error = fixture
        .repo
        .fork(
            &source.get_metadata().expect("metadata"),
            &fork_options(ForkScopeArg::Branch, None, None),
            SqliteSessionCreateOptions {
                id: Some("fork".to_owned()),
                cwd,
                ..SqliteSessionCreateOptions::default()
            },
        )
        .await
        .expect_err("rejects");
    assert_eq!(error.code, SessionErrorCode::InvalidForkTarget);
    fixture.repo.close().await;
}
