use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use notagent::core::mcp::auth::{
    McpClientRegistration, McpCredentialEntry, McpCredentialStore, McpOAuthTokens,
};
use notagent::core::mcp::manager::{McpManager, McpServerStatus};
use notagent::core::mcp::{McpHttpServer, McpOAuthSetting, McpServerConfig, ServerName};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// One request the fake server was sent.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Seen {
    target: String,
    authorization: Option<String>,
}

/// An HTTP server that refuses everything and remembers who asked.
struct FakeServer {
    url: String,
    seen: Arc<Mutex<Vec<Seen>>>,
    task: tokio::task::JoinHandle<()>,
}

impl FakeServer {
    async fn start() -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("binds");
        let port = listener.local_addr().expect("has an address").port();
        let seen: Arc<Mutex<Vec<Seen>>> = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&seen);
        let issuer = format!("http://127.0.0.1:{port}");

        let task = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let recorder = Arc::clone(&recorder);
                let issuer = issuer.clone();
                tokio::spawn(async move {
                    let mut buffer = vec![0u8; 8192];
                    let Ok(read) = stream.read(&mut buffer).await else {
                        return;
                    };
                    let request = String::from_utf8_lossy(&buffer[..read]).to_string();
                    let target = request
                        .lines()
                        .next()
                        .unwrap_or_default()
                        .split_whitespace()
                        .nth(1)
                        .unwrap_or("/")
                        .to_owned();
                    let authorization = request
                        .lines()
                        .find(|line| line.to_ascii_lowercase().starts_with("authorization:"))
                        .map(|line| line[line.find(':').unwrap_or(0) + 1..].trim().to_owned());
                    recorder.lock().expect("poisoned").push(Seen {
                        target: target.clone(),
                        authorization,
                    });

                    let response = if target.contains(".well-known") {
                        let body = serde_json::json!({
                            "issuer": issuer,
                            "authorization_endpoint": format!("{issuer}/authorize"),
                            "token_endpoint": format!("{issuer}/token"),
                            "registration_endpoint": format!("{issuer}/register"),
                            "response_types_supported": ["code"],
                            "grant_types_supported": ["authorization_code", "refresh_token"],
                            "code_challenge_methods_supported": ["S256"],
                        })
                        .to_string();
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        )
                    } else {
                        let body = "{\"error\":\"unauthorized\"}";
                        format!(
                            "HTTP/1.1 401 Unauthorized\r\n\
                             WWW-Authenticate: Bearer realm=\"mcp\"\r\n\
                             Content-Type: application/json\r\nContent-Length: {}\r\n\
                             Connection: close\r\n\r\n{body}",
                            body.len()
                        )
                    };
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.shutdown().await;
                });
            }
        });

        Self {
            url: format!("http://127.0.0.1:{port}/mcp"),
            seen,
            task,
        }
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().expect("poisoned").clone()
    }

    /// Every request that was not the metadata discovery.
    fn mcp_requests(&self) -> Vec<Seen> {
        self.seen()
            .into_iter()
            .filter(|request| !request.target.contains(".well-known"))
            .collect()
    }
}

impl Drop for FakeServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn credential_path(name: &str) -> std::path::PathBuf {
    let directory = std::env::temp_dir().join(format!("notagent-mcp-auth-{name}"));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("creates the directory");
    directory.join("mcp-credentials.json")
}

fn manager_for(server: &FakeServer, credentials: Option<&std::path::Path>) -> Arc<McpManager> {
    let mut configs = BTreeMap::new();
    configs.insert(
        ServerName::from("remote"),
        McpServerConfig::Http(McpHttpServer {
            url: server.url.clone(),
            timeout: Some(10),
            oauth: McpOAuthSetting::AutoDetect,
            ..McpHttpServer::default()
        }),
    );
    let manager = McpManager::new(configs, BTreeMap::new());
    Arc::new(match credentials {
        Some(path) => manager.with_credentials(path),
        None => manager,
    })
}

async fn store_token(path: &std::path::Path, url: &str, token: &str, lifetime: u64) {
    let url = url.to_owned();
    let token = token.to_owned();
    McpCredentialStore::update(path, move |store| {
        store.set(McpCredentialEntry {
            server_url: url,
            tokens: McpOAuthTokens {
                access_token: token,
                refresh_token: Some("refresh".to_owned()),
                expires_at: Some(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .expect("after the epoch")
                        .as_secs()
                        + lifetime,
                ),
                scope: None,
            },
            client_registration: Some(McpClientRegistration {
                client_id: "test-client".to_owned(),
                client_secret: None,
                client_id_issued_at: None,
                client_secret_expires_at: None,
            }),
        });
    })
    .await
    .expect("writes the credential file");
}

#[tokio::test]
async fn a_server_that_answers_401_is_reported_as_needing_a_login() {
    let server = FakeServer::start().await;
    let path = credential_path("needs-login");
    let manager = manager_for(&server, Some(&path));
    let name = ServerName::from("remote");

    let tools = manager.ensure_connected(&name).await;

    assert!(tools.is_empty());
    assert_eq!(
        manager.status(&name).await,
        Some(McpServerStatus::NeedsAuth),
        "the entry says: {:?}",
        manager.entries().await
    );
    assert_eq!(manager.servers_needing_auth().await, vec![name]);
}

#[tokio::test]
async fn a_server_waiting_on_a_login_can_be_offered_one() {
    let server = FakeServer::start().await;
    let path = credential_path("can-login");
    let manager = manager_for(&server, Some(&path));
    let name = ServerName::from("remote");
    manager.ensure_connected(&name).await;

    assert!(manager.can_authenticate(&name).await);
}

#[tokio::test]
async fn a_session_without_a_credential_file_reports_it_rather_than_opening_a_browser() {
    let server = FakeServer::start().await;
    let manager = manager_for(&server, None);
    let name = ServerName::from("remote");
    manager.ensure_connected(&name).await;

    assert!(!manager.can_authenticate(&name).await);
    let error = manager
        .authenticate(&name, |_| panic!("no browser is opened"))
        .await
        .expect_err("there is nowhere to store a token");

    assert!(error.contains("keeps no MCP credentials"), "{error}");
}

#[tokio::test]
async fn nothing_is_sent_on_the_wire_when_nothing_is_stored() {
    let server = FakeServer::start().await;
    let path = credential_path("no-token");
    let manager = manager_for(&server, Some(&path));

    manager.ensure_connected(&ServerName::from("remote")).await;

    let requests = server.mcp_requests();
    assert!(!requests.is_empty(), "the server was reached");
    assert!(
        requests
            .iter()
            .all(|request| request.authorization.is_none()),
        "{requests:?}"
    );
    // Discovery costs a round trip and there is nothing to discover for a
    // server nobody has logged into.
    assert!(
        server
            .seen()
            .iter()
            .all(|request| !request.target.contains(".well-known")),
        "{:?}",
        server.seen()
    );
}

#[tokio::test]
async fn a_stored_token_is_put_on_the_wire() {
    let server = FakeServer::start().await;
    let path = credential_path("stored-token");
    store_token(&path, &server.url, "stored-token", 3600).await;
    let manager = manager_for(&server, Some(&path));

    manager.ensure_connected(&ServerName::from("remote")).await;

    let requests = server.mcp_requests();
    assert!(!requests.is_empty(), "the server was reached");
    assert!(
        requests
            .iter()
            .any(|request| request.authorization.as_deref() == Some("Bearer stored-token")),
        "{requests:?}"
    );
}

#[tokio::test]
async fn a_refused_token_is_not_chased_onto_the_older_transport() {
    let server = FakeServer::start().await;
    let path = credential_path("no-sse-fallback");
    store_token(&path, &server.url, "stored-token", 3600).await;
    let manager = manager_for(&server, Some(&path));
    let name = ServerName::from("remote");

    manager.ensure_connected(&name).await;

    // The streamable transport carried the token and the server refused it.
    // SSE cannot present one, so trying it would replace a 401 the user can act
    // on with a connection error they cannot.
    assert_eq!(
        manager.status(&name).await,
        Some(McpServerStatus::NeedsAuth)
    );
    assert_eq!(
        server.mcp_requests().len(),
        1,
        "{:?}",
        server.mcp_requests()
    );
}

#[tokio::test]
async fn a_login_a_local_process_cannot_perform_says_so_at_once() {
    let path = credential_path("stdio-login");
    let mut configs = BTreeMap::new();
    configs.insert(
        ServerName::from("local"),
        McpServerConfig::Stdio(notagent::core::mcp::McpStdioServer {
            command: "true".to_owned(),
            ..notagent::core::mcp::McpStdioServer::default()
        }),
    );
    let manager = McpManager::new(configs, BTreeMap::new()).with_credentials(&path);

    let error = manager
        .authenticate(&ServerName::from("local"), |_| {
            panic!("no browser is opened")
        })
        .await
        .expect_err("a local process has no login");

    assert!(error.contains("no login to perform"), "{error}");
}

#[tokio::test]
async fn a_login_for_a_server_that_is_not_configured_is_refused() {
    let path = credential_path("unknown-login");
    let manager = McpManager::empty().with_credentials(&path);

    let error = manager
        .authenticate(&ServerName::from("nobody"), |_| {
            panic!("no browser is opened")
        })
        .await
        .expect_err("there is no such server");

    assert!(error.contains("no server named"), "{error}");
}

#[tokio::test]
async fn a_server_with_oauth_switched_off_is_never_sent_a_token() {
    let server = FakeServer::start().await;
    let path = credential_path("oauth-off");
    store_token(&path, &server.url, "stored-token", 3600).await;

    let mut configs = BTreeMap::new();
    configs.insert(
        ServerName::from("remote"),
        McpServerConfig::Http(McpHttpServer {
            url: server.url.clone(),
            timeout: Some(10),
            oauth: McpOAuthSetting::Disabled,
            ..McpHttpServer::default()
        }),
    );
    let manager = McpManager::new(configs, BTreeMap::new()).with_credentials(&path);

    manager.ensure_connected(&ServerName::from("remote")).await;

    assert!(
        server
            .mcp_requests()
            .iter()
            .all(|request| request.authorization.is_none()),
        "{:?}",
        server.mcp_requests()
    );
    let error = manager
        .authenticate(&ServerName::from("remote"), |_| {
            panic!("no browser is opened")
        })
        .await
        .expect_err("OAuth is switched off");
    assert!(error.contains("OAuth switched off"), "{error}");
}

#[tokio::test]
async fn an_expired_token_is_refreshed_rather_than_presented() {
    let server = FakeServer::start().await;
    let path = credential_path("expired-token");
    // Already spent: the refresh grant is what should be attempted, and this
    // server refuses it, so nothing is presented at all.
    store_token(&path, &server.url, "stale-token", 0).await;
    let manager = manager_for(&server, Some(&path));

    manager.ensure_connected(&ServerName::from("remote")).await;

    assert!(
        server
            .mcp_requests()
            .iter()
            .all(|request| request.authorization.as_deref() != Some("Bearer stale-token")),
        "a spent token was presented anyway: {:?}",
        server.mcp_requests()
    );
    assert!(
        server
            .seen()
            .iter()
            .any(|request| request.target.contains(".well-known")),
        "the refresh needs the provider's endpoints: {:?}",
        server.seen()
    );
}
