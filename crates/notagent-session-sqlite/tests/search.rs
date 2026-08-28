use notagent_agent::AgentMessage;
use notagent_ai::{TextContent, TextOrImageContent, UserContent, UserMessage};
use notagent_session_sqlite::session_types::{EntryType, SessionSearchOptions};
use notagent_session_sqlite::sqlite::repo::{
    SqliteSessionRepository, SqliteSessionRepositoryOptions,
};
use notagent_session_sqlite::sqlite::search_backend::{
    SqliteSessionSearch, SqliteSessionSearchHit, SqliteSessionSearchOptions,
    create_sqlite_session_search,
};
use notagent_session_sqlite::{Session, SqliteSessionCreateOptions, create_sqlite_factory};

struct Fixture {
    directory: tempfile::TempDir,
    repo: SqliteSessionRepository,
    search: SqliteSessionSearch,
}

fn fixture() -> Fixture {
    let directory = tempfile::Builder::new()
        .prefix("notagent-search-")
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
    let search = create_sqlite_session_search(SqliteSessionSearchOptions {
        sqlite: create_sqlite_factory(),
        database_path,
    });
    Fixture {
        directory,
        repo,
        search,
    }
}

async fn create_session(fixture: &Fixture, id: &str, cwd: Option<&str>) -> Session {
    let storage = fixture
        .repo
        .create(SqliteSessionCreateOptions {
            id: Some(id.to_owned()),
            cwd: cwd.map_or_else(
                || fixture.directory.path().to_string_lossy().into_owned(),
                str::to_owned,
            ),
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

async fn search(
    fixture: &Fixture,
    text: &str,
    options: SessionSearchOptions,
) -> Vec<SqliteSessionSearchHit> {
    fixture
        .search
        .search(text, &options, None)
        .await
        .expect("searches")
}

#[tokio::test]
async fn matches_trigrams() {
    let fixture = fixture();
    let included = create_session(&fixture, "included", None).await;
    let other_cwd = fixture
        .directory
        .path()
        .join("other")
        .to_string_lossy()
        .into_owned();
    let excluded = create_session(&fixture, "excluded", Some(&other_cwd)).await;
    let entry_id = included
        .append_message(user_message("Find the auth defect"))
        .await
        .expect("appends");
    included
        .set_name(Some("Canonical name"))
        .await
        .expect("sets name");
    let excluded_entry_id = excluded
        .append_message(user_message("Find the auth defect"))
        .await
        .expect("appends");

    let hits = search(&fixture, "auth", SessionSearchOptions::default()).await;
    assert_eq!(hits.len(), 2);
    let included_hit = hits
        .iter()
        .find(|hit| hit.session_id == "included")
        .expect("included hit");
    assert_eq!(included_hit.entry_id, entry_id);
    assert_eq!(
        included_hit.metadata.name.as_deref(),
        Some("Canonical name")
    );
    let excluded_hit = hits
        .iter()
        .find(|hit| hit.session_id == "excluded")
        .expect("excluded hit");
    assert_eq!(excluded_hit.entry_id, excluded_entry_id);

    // A trigram substring matches as well.
    let substring_hits = search(&fixture, "uth", SessionSearchOptions::default()).await;
    assert_eq!(substring_hits.len(), 2);
    fixture.repo.close().await;
}

#[tokio::test]
async fn omits_a_cleared_session_name_from_search_metadata() {
    let fixture = fixture();
    let session = create_session(&fixture, "session-1", None).await;
    let entry_id = session
        .append_message(user_message("Find the auth defect"))
        .await
        .expect("appends");
    session
        .set_name(Some("Temporary"))
        .await
        .expect("sets name");
    session.set_name(None).await.expect("clears name");

    let hits = search(&fixture, "auth", SessionSearchOptions::default()).await;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].session_id, "session-1");
    assert_eq!(hits[0].entry_id, entry_id);
    assert_eq!(hits[0].metadata.name, None);
    fixture.repo.close().await;
}

#[tokio::test]
async fn handles_quoted_search_text_without_exposing_fts_syntax() {
    let fixture = fixture();
    let hits = search(
        &fixture,
        "missing \"phrase\"",
        SessionSearchOptions::default(),
    )
    .await;
    assert!(hits.is_empty());
    fixture.repo.close().await;
}

#[tokio::test]
async fn rebuilds_existing_entries_when_fts_is_first_initialized() {
    let fixture = fixture();
    let session = create_session(&fixture, "session-1", None).await;
    let entry_id = session
        .append_message(user_message("Find the auth defect"))
        .await
        .expect("appends");

    let hits = search(&fixture, "auth", SessionSearchOptions::default()).await;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].entry_id, entry_id);
    fixture.repo.close().await;
}

#[tokio::test]
async fn honors_entry_type_filters() {
    let fixture = fixture();
    let session = create_session(&fixture, "session-1", None).await;
    let message_entry_id = session
        .append_message(user_message("Find the auth defect"))
        .await
        .expect("appends");
    session
        .append_custom_entry(
            "note",
            Some(serde_json::json!({ "text": "Find the auth custom entry" })),
        )
        .await
        .expect("appends");

    let hits = search(
        &fixture,
        "auth",
        SessionSearchOptions {
            entry_types: Some(vec![EntryType::Message]),
            limit: None,
        },
    )
    .await;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].entry_id, message_entry_id);
    fixture.repo.close().await;
}

#[tokio::test]
async fn honors_result_limits() {
    let fixture = fixture();
    let first = create_session(&fixture, "session-1", None).await;
    let second = create_session(&fixture, "session-2", None).await;
    first
        .append_message(user_message("Find the auth defect"))
        .await
        .expect("appends");
    second
        .append_message(user_message("Find the auth defect too"))
        .await
        .expect("appends");

    assert_eq!(
        search(
            &fixture,
            "auth",
            SessionSearchOptions {
                limit: Some(1),
                entry_types: None
            }
        )
        .await
        .len(),
        1
    );
    assert!(
        search(
            &fixture,
            "auth",
            SessionSearchOptions {
                limit: Some(0),
                entry_types: None
            }
        )
        .await
        .is_empty()
    );
    fixture.repo.close().await;
}

#[tokio::test]
async fn removes_deleted_session_entries_from_the_index() {
    let fixture = fixture();
    let session = create_session(&fixture, "session-1", None).await;
    session
        .append_message(user_message("Find the auth defect"))
        .await
        .expect("appends");
    assert_eq!(
        search(&fixture, "auth", SessionSearchOptions::default())
            .await
            .len(),
        1
    );

    let metadata = session.get_metadata().expect("metadata");
    fixture.repo.delete(&metadata).await.expect("deletes");

    assert!(
        search(&fixture, "auth", SessionSearchOptions::default())
            .await
            .is_empty()
    );
    fixture.repo.close().await;
}

#[tokio::test]
async fn indexes_and_removes_entries_through_triggers_after_fts_initialization() {
    let fixture = fixture();
    assert!(
        search(&fixture, "auth", SessionSearchOptions::default())
            .await
            .is_empty()
    );
    let session = create_session(&fixture, "session-1", None).await;
    let entry_id = session
        .append_message(user_message("Find the auth defect"))
        .await
        .expect("appends");
    let hits = search(&fixture, "auth", SessionSearchOptions::default()).await;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].entry_id, entry_id);

    let metadata = session.get_metadata().expect("metadata");
    fixture.repo.delete(&metadata).await.expect("deletes");
    assert!(
        search(&fixture, "auth", SessionSearchOptions::default())
            .await
            .is_empty()
    );
    fixture.repo.close().await;
}

/// The search opens its own connection, so it must not hold the writer lease.
#[tokio::test]
async fn searching_does_not_take_the_writer_lease() {
    let fixture = fixture();
    let session = create_session(&fixture, "session-1", None).await;
    session
        .append_message(user_message("Find the auth defect"))
        .await
        .expect("appends");
    let _ = search(&fixture, "auth", SessionSearchOptions::default()).await;
    // The session keeps writing after the search.
    session
        .append_message(user_message("still writing"))
        .await
        .expect("appends");
    fixture.repo.close().await;
}
