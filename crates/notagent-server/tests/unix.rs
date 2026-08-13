//! Port of `packages/server/test/unix.test.ts`.

mod support;

use std::os::unix::fs::{FileTypeExt, PermissionsExt};

use notagent_server::PiServer;
use notagent_server::testing::{TestServerService, connect_unix_test_client};
use notagent_server::transports::unix::{UnixServerOptions, create_unix_server};
use support::*;

fn make_server(path: &str) -> PiServer {
    create_unix_server(
        TestServerService::new().as_service(),
        UnixServerOptions::new(path),
    )
    .expect("server")
}

#[tokio::test]
async fn rejects_a_live_listener_without_unlinking_it() {
    let directory = temp_directory();
    let path = socket_path(&directory);
    let first = make_server(&path);
    first.start().await.expect("starts");
    let first_identity = std::fs::symlink_metadata(&path).expect("stat");

    let second = make_server(&path);
    let error = second.start().await.expect_err("rejects");
    assert!(error.to_string().contains("already running"), "{error}");
    let current = std::fs::symlink_metadata(&path).expect("stat");
    assert!(current.file_type().is_socket());
    assert_eq!(
        (
            std::os::unix::fs::MetadataExt::dev(&current),
            std::os::unix::fs::MetadataExt::ino(&current)
        ),
        (
            std::os::unix::fs::MetadataExt::dev(&first_identity),
            std::os::unix::fs::MetadataExt::ino(&first_identity)
        )
    );

    let client = connect_unix_test_client(&path).await.expect("connects");
    assert!(matches!(
        client.hello().await,
        notagent_protocol::ServerMessage::Hello(_)
    ));
    client.close().await.expect("closes");
    first.close().await.expect("closes");
}

#[tokio::test]
async fn never_unlinks_a_regular_file_at_the_configured_path() {
    let directory = temp_directory();
    let path = socket_path(&directory);
    std::fs::write(&path, "do not remove").expect("writes");
    let server = make_server(&path);
    let error = server.start().await.expect_err("rejects");
    assert!(error.to_string().contains("non-socket"), "{error}");
    assert_eq!(
        std::fs::read_to_string(&path).expect("reads"),
        "do not remove"
    );
}

#[tokio::test]
async fn creates_nested_temp_parents_restricts_permissions_and_removes_its_own_socket() {
    let directory = temp_directory();
    let path = directory
        .path()
        .join("p")
        .join("n")
        .join("server.sock")
        .to_string_lossy()
        .into_owned();
    let server = make_server(&path);
    server.start().await.expect("starts");
    let stats = std::fs::symlink_metadata(&path).expect("stat");
    assert!(stats.file_type().is_socket());
    assert_eq!(stats.permissions().mode() & 0o777, 0o600);

    server.close().await.expect("closes");
    assert!(!std::path::Path::new(&path).exists());
}

#[tokio::test]
async fn does_not_remove_a_replacement_inode_during_shutdown() {
    let directory = temp_directory();
    let path = socket_path(&directory);
    let server = make_server(&path);
    server.start().await.expect("starts");
    std::fs::remove_file(&path).expect("unlinks");
    std::fs::write(&path, "replacement").expect("writes");

    server.close().await.expect("closes");
    assert_eq!(
        std::fs::read_to_string(&path).expect("reads"),
        "replacement"
    );
}

#[tokio::test]
async fn removes_a_genuinely_stale_socket_before_binding() {
    let directory = temp_directory();
    let path = socket_path(&directory);
    // TS forks a helper process and kills it; dropping a bound std listener
    // leaves exactly the same artefact: a socket file nobody listens on.
    {
        let stale = std::os::unix::net::UnixListener::bind(&path).expect("binds");
        drop(stale);
    }
    let stale_identity = std::fs::symlink_metadata(&path).expect("stat");
    assert!(stale_identity.file_type().is_socket());

    let server = make_server(&path);
    server.start().await.expect("starts");
    let live_identity = std::fs::symlink_metadata(&path).expect("stat");
    assert!(live_identity.file_type().is_socket());
    let client = connect_unix_test_client(&path).await.expect("connects");
    assert!(matches!(
        client.hello().await,
        notagent_protocol::ServerMessage::Hello(_)
    ));
    client.close().await.expect("closes");
    server.close().await.expect("closes");
}
