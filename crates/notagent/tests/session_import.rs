mod app_runtime;

use app_runtime::HeadlessApp;
use notagent::core::session_manager::SessionManager;
use serde_json::json;
use std::path::Path;

async fn import_fixture() -> (HeadlessApp, String, String, String) {
    let app = HeadlessApp::create().await;
    let directory = app.path("sessions");
    let mut manager = SessionManager::create(&app.cwd(), Some(&directory), None).expect("manager");
    manager
        .append_message_value(json!({"role":"assistant", "content":"original"}))
        .expect("persist");
    let target = manager.get_session_file().expect("file").to_owned();
    let original = std::fs::read_to_string(&target).expect("original");
    app.session()
        .with_session_manager(|current| *current = manager);
    (app, directory, target, original)
}

#[tokio::test]
async fn an_invalid_import_never_overwrites_a_same_named_session() {
    let (app, directory, target, original) = import_fixture().await;
    let source = Path::new(&app.cwd()).join(Path::new(&target).file_name().expect("name"));
    std::fs::write(&source, "not a session").expect("source");
    app.runtime()
        .import_from_jsonl(&source.to_string_lossy(), None)
        .await
        .expect_err("reject invalid import");
    assert_eq!(
        std::fs::read_to_string(&target).expect("target"),
        original,
        "the existing session must survive rejection"
    );
    assert_eq!(
        app.session().session_file().as_deref(),
        Some(target.as_str())
    );
    assert_eq!(
        std::fs::read_to_string(&source).expect("source"),
        "not a session"
    );
    assert_eq!(
        std::fs::read_dir(directory).expect("directory").count(),
        1,
        "rejected imports must clean up staging files"
    );
    app.runtime().dispose().await;
}

#[tokio::test]
async fn a_valid_import_with_a_name_collision_keeps_both_sessions() {
    let (app, directory, target, original) = import_fixture().await;
    let source = Path::new(&app.cwd()).join(Path::new(&target).file_name().expect("name"));
    let imported = original.replace("original", "imported");
    std::fs::write(&source, &imported).expect("source");
    app.runtime()
        .import_from_jsonl(&source.to_string_lossy(), None)
        .await
        .expect("import");
    let active = app.session().session_file().expect("active");
    assert_ne!(active, target, "a collision must receive a new path");
    assert_eq!(std::fs::read_to_string(&target).expect("target"), original);
    assert!(
        std::fs::read_to_string(&active)
            .expect("active")
            .starts_with(&imported),
        "startup may append settings but must preserve imported entries"
    );
    assert_eq!(std::fs::read_to_string(&source).expect("source"), imported);
    assert_eq!(std::fs::read_dir(directory).expect("directory").count(), 2);
    app.runtime().dispose().await;
}

#[tokio::test]
async fn an_import_with_a_missing_working_directory_is_not_published() {
    let (app, directory, target, original) = import_fixture().await;
    let source = app.path("external.jsonl");
    std::fs::write(&source, &original).expect("source");
    app.runtime()
        .import_from_jsonl(&source, Some(&app.path("missing")))
        .await
        .expect_err("reject missing cwd");
    assert_eq!(std::fs::read_to_string(target).expect("target"), original);
    assert_eq!(std::fs::read_dir(directory).expect("directory").count(), 1);
    app.runtime().dispose().await;
}

#[tokio::test]
async fn a_streamed_persistence_failure_is_reported_without_locking_out_listeners() {
    use notagent::core::agent_session::{AgentSessionEvent, PromptOptions};
    use std::sync::{Arc, Mutex};
    let (app, _, target, _) = import_fixture().await;
    let backup = app.path("backup.jsonl");
    std::fs::rename(&target, &backup).expect("backup");
    std::fs::create_dir(&target).expect("block writes");
    let reported = Arc::new(Mutex::new(Vec::new()));
    let output = Arc::clone(&reported);
    let session = Arc::downgrade(&app.session());
    let _listener = app.session().subscribe(Arc::new(move |event| {
        if let AgentSessionEvent::PersistenceError { error_message } = event {
            session
                .upgrade()
                .expect("session")
                .with_session_manager(|manager| {
                    assert!(
                        manager.get_leaf_id().is_some(),
                        "the pending entry must remain available"
                    );
                });
            output.lock().expect("output").push(error_message);
        }
    }));
    app.faux()
        .set_responses(vec![app_runtime::reply("pending response")]);
    app.session()
        .prompt("pending user", PromptOptions::default())
        .await
        .expect("prompt");
    assert!(
        !reported.lock().expect("reported").is_empty(),
        "discarded append Results must still reach the UI event stream"
    );
    std::fs::remove_dir(&target).expect("unblock");
    std::fs::rename(&backup, &target).expect("restore");
    app.session().set_session_name("recovered");
    let reopened = SessionManager::open(&target, None, None).expect("reopen");
    app.session().with_session_manager(|manager| {
        assert_eq!(
            reopened.build_session_context().messages,
            manager.build_session_context().messages
        );
    });
    app.runtime().dispose().await;
}
