//! OAuth for HTTP transports and persistent MCP credentials.
//!
//! The callback listener normally uses an operating-system-assigned loopback
//! port. Explicit redirect URIs are honored for providers that require a
//! registered address. Credential updates use an inter-process lock, a
//! temporary file, an atomic rename, and mode 0600 so concurrent sessions do
//! not overwrite each other or expose bearer tokens.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rmcp::transport::auth::{AuthError, CredentialStore, OAuthState, StoredCredentials};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Mutex;

use crate::core::mcp::McpOAuthConfig;
use crate::utils::atomic_write::write_secret_file_atomic;
use crate::utils::lockfile::{LockOptions, lock_with_retry};

/// The mode the credential file is written with; it holds bearer tokens.
const CREDENTIAL_FILE_MODE: u32 = 0o600;

/// How long the browser has to come back before the login is abandoned.
/// Long enough for a login that needs a password manager and a second factor,
/// short enough that an abandoned tab does not hold a port for the session.
pub const CALLBACK_TIMEOUT: Duration = Duration::from_secs(300);

/// The name this client registers itself under.
const CLIENT_NAME: &str = "Notagent";

/// One server's tokens.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpOAuthTokens {
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    /// Seconds since the epoch, absolute rather than relative: a lifetime is
    /// only meaningful next to the moment it was issued, and that moment is not
    /// in the file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

/// What a dynamic registration returned, kept so the next login reuses it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpClientRegistration {
    pub client_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id_issued_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret_expires_at: Option<u64>,
}

/// Everything stored for one server URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpCredentialEntry {
    /// Tokens are bound to the URL they were issued for. A server that moves is
    /// a server that has to be logged into again, which is the safe reading.
    pub server_url: String,
    pub tokens: McpOAuthTokens,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_registration: Option<McpClientRegistration>,
}

/// The credential file's contents.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpCredentialStore {
    #[serde(default)]
    pub credentials: BTreeMap<String, McpCredentialEntry>,
}

impl McpCredentialStore {
    /// Reads the file, treating anything unreadable as empty.
    /// A corrupt credential file must not stop a session from starting: the
    /// worst it costs is a login, and refusing to start costs the whole run.
    pub fn load(path: &Path) -> Self {
        let Ok(content) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        serde_json::from_str(&content).unwrap_or_default()
    }

    /// Applies a change against the file's current contents, under the lock.
    /// The read happens inside the lock, so a change to one server's entry
    /// cannot roll back another's — the failure mode of every caller holding
    /// its own snapshot and writing the whole file back.
    pub async fn update<F>(path: &Path, mutate: F) -> Result<Self, String>
    where
        F: FnOnce(&mut Self) + Send + 'static,
    {
        let path = path.to_path_buf();
        tokio::task::spawn_blocking(move || Self::update_blocking(&path, mutate))
            .await
            .map_err(|error| format!("the credential store could not be written: {error}"))?
    }

    fn update_blocking<F>(path: &Path, mutate: F) -> Result<Self, String>
    where
        F: FnOnce(&mut Self),
    {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
            && !parent.exists()
        {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("{}: {error}", parent.display()))?;
        }
        let guard = lock_with_retry(
            path,
            &LockOptions::default(),
            LOCK_ATTEMPTS,
            LOCK_RETRY_DELAY,
        )
        .map_err(|error| format!("the credential store is locked: {error}"))?;

        let result = (|| {
            let mut store = Self::load(path);
            mutate(&mut store);
            let content = serde_json::to_string_pretty(&store)
                .map_err(|error| format!("the credential store could not be encoded: {error}"))?;
            write_secret_file_atomic(path, &content, CREDENTIAL_FILE_MODE)
                .map_err(|error| format!("{}: {error}", path.display()))?;
            Ok(store)
        })();
        guard.release();
        result
    }

    pub fn get(&self, server_url: &str) -> Option<&McpCredentialEntry> {
        self.credentials.get(server_url)
    }

    pub fn set(&mut self, entry: McpCredentialEntry) {
        self.credentials.insert(entry.server_url.clone(), entry);
    }

    pub fn remove(&mut self, server_url: &str) {
        self.credentials.remove(server_url);
    }

    pub fn has_credentials(&self, server_url: &str) -> bool {
        self.credentials.contains_key(server_url)
    }
}

const LOCK_ATTEMPTS: u32 = 20;
const LOCK_RETRY_DELAY: Duration = Duration::from_millis(100);

/// The file-backed store as `rmcp` wants to see it.
pub struct McpTokenStorage {
    server_url: String,
    path: PathBuf,
    cached: Arc<Mutex<Option<McpCredentialStore>>>,
}

impl McpTokenStorage {
    pub fn new(server_url: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        Self {
            server_url: server_url.into(),
            path: path.into(),
            cached: Arc::new(Mutex::new(None)),
        }
    }

    async fn store(&self) -> McpCredentialStore {
        let mut cached = self.cached.lock().await;
        if cached.is_none() {
            let path = self.path.clone();
            let loaded = tokio::task::spawn_blocking(move || McpCredentialStore::load(&path))
                .await
                .unwrap_or_default();
            *cached = Some(loaded);
        }
        cached.clone().unwrap_or_default()
    }

    /// Forgets this server's tokens, keeping any registration.
    /// A rejected token and an unregistered client are different problems: the
    /// first needs a login, the second needs the provider to be asked for a new
    /// client, which is slower and which some providers rate-limit.
    pub async fn forget_tokens(&self) -> Result<(), String> {
        let server_url = self.server_url.clone();
        let store = McpCredentialStore::update(&self.path, move |store| {
            if let Some(entry) = store.get(&server_url).cloned() {
                store.set(McpCredentialEntry {
                    server_url: entry.server_url,
                    tokens: McpOAuthTokens::default(),
                    client_registration: entry.client_registration,
                });
            }
        })
        .await?;
        *self.cached.lock().await = Some(store);
        Ok(())
    }
}

#[async_trait::async_trait]
impl CredentialStore for McpTokenStorage {
    async fn load(&self) -> Result<Option<StoredCredentials>, AuthError> {
        let store = self.store().await;
        let Some(entry) = store.get(&self.server_url) else {
            return Ok(None);
        };
        if entry.tokens.access_token.is_empty() {
            // An entry that holds only a registration has nothing to present.
            // Reporting it as credentials would make `rmcp` try an empty bearer
            // token and read the 401 as a rejected login rather than as none.
            return Ok(None);
        }

        use oauth2::basic::BasicTokenType;
        use oauth2::{AccessToken, RefreshToken};
        use rmcp::transport::auth::OAuthTokenResponse;

        let mut response = OAuthTokenResponse::new(
            AccessToken::new(entry.tokens.access_token.clone()),
            BasicTokenType::Bearer,
            oauth2::EmptyExtraTokenFields {},
        );
        if let Some(refresh) = &entry.tokens.refresh_token {
            response.set_refresh_token(Some(RefreshToken::new(refresh.clone())));
        }
        if let Some(expires_at) = entry.tokens.expires_at {
            // A lifetime already spent is reported as zero rather than left out:
            // absent reads as "never expires", and the refresh that should have
            // happened never would.
            let remaining = expires_at.saturating_sub(now_seconds());
            response.set_expires_in(Some(&Duration::from_secs(remaining)));
        }

        Ok(Some(StoredCredentials {
            client_id: entry
                .client_registration
                .as_ref()
                .map(|registration| registration.client_id.clone())
                .unwrap_or_default(),
            token_response: Some(response),
        }))
    }

    async fn save(&self, credentials: StoredCredentials) -> Result<(), AuthError> {
        use oauth2::TokenResponse;

        let server_url = self.server_url.clone();
        let store = McpCredentialStore::update(&self.path, move |store| {
            let existing = store.get(&server_url).cloned();

            let tokens = match &credentials.token_response {
                Some(response) => McpOAuthTokens {
                    access_token: response.access_token().secret().to_string(),
                    // A refresh grant that returns no new refresh token means
                    // the old one still stands. Dropping it here would turn the
                    // next expiry into a login the user did not need.
                    refresh_token: response
                        .refresh_token()
                        .map(|token| token.secret().to_string())
                        .or_else(|| {
                            existing
                                .as_ref()
                                .and_then(|entry| entry.tokens.refresh_token.clone())
                        }),
                    expires_at: response
                        .expires_in()
                        .map(|lifetime| now_seconds() + lifetime.as_secs()),
                    scope: response.scopes().map(|scopes| {
                        scopes
                            .iter()
                            .map(|scope| scope.to_string())
                            .collect::<Vec<_>>()
                            .join(" ")
                    }),
                },
                None => McpOAuthTokens::default(),
            };

            let client_registration = if credentials.client_id.is_empty() {
                existing.and_then(|entry| entry.client_registration)
            } else {
                let previous = existing
                    .as_ref()
                    .and_then(|entry| entry.client_registration.as_ref());
                Some(McpClientRegistration {
                    client_id: credentials.client_id.clone(),
                    // `rmcp` hands back only the id; the secret and the issue
                    // times came from the registration response and are not
                    // reissued, so they are carried across rather than lost.
                    client_secret: previous.and_then(|reg| reg.client_secret.clone()),
                    client_id_issued_at: previous.and_then(|reg| reg.client_id_issued_at),
                    client_secret_expires_at: previous.and_then(|reg| reg.client_secret_expires_at),
                })
            };

            store.set(McpCredentialEntry {
                server_url: server_url.clone(),
                tokens,
                client_registration,
            });
        })
        .await
        .map_err(AuthError::InternalError)?;

        *self.cached.lock().await = Some(store);
        Ok(())
    }

    async fn clear(&self) -> Result<(), AuthError> {
        let server_url = self.server_url.clone();
        let store = McpCredentialStore::update(&self.path, move |store| {
            store.remove(&server_url);
        })
        .await
        .map_err(AuthError::InternalError)?;
        *self.cached.lock().await = Some(store);
        Ok(())
    }
}

fn now_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// A token that can be presented now, refreshed if it had gone stale.
/// `None` means there is nothing stored, or that what was stored can no longer
/// be turned into a token — both of which are answered by logging in, so the
/// caller treats them alike.
pub async fn stored_access_token(server_url: &str, path: &Path) -> Option<String> {
    // Skipped before any network call: discovery against a server nobody has
    // ever logged into would cost a round trip on every connect.
    if !McpCredentialStore::load(path).has_credentials(server_url) {
        return None;
    }

    let mut manager = rmcp::transport::auth::AuthorizationManager::new(server_url)
        .await
        .ok()?;
    manager.set_credential_store(McpTokenStorage::new(server_url, path));
    if !manager.initialize_from_store().await.ok()? {
        return None;
    }
    manager.get_access_token().await.ok()
}

/// What is stored for a server, as a line a user reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpAuthStatus {
    /// Nothing stored; a call will be refused until there is.
    NotAuthenticated,
    /// A token that can be presented now.
    Authenticated,
    /// A token past its lifetime, with a refresh token to replace it. The next
    /// connect does that silently, so this is not a problem to act on.
    ExpiredWithRefresh,
    /// A token past its lifetime and nothing to renew it with; only a fresh
    /// login gets past this.
    Expired,
}

impl McpAuthStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotAuthenticated => "not signed in",
            Self::Authenticated => "signed in",
            Self::ExpiredWithRefresh => "signed in (token due for renewal)",
            Self::Expired => "sign-in expired",
        }
    }
}

/// What is on disk for a server, without touching the network.
pub fn auth_status(server_url: &str, path: &Path) -> McpAuthStatus {
    let store = McpCredentialStore::load(path);
    let Some(entry) = store.get(server_url) else {
        return McpAuthStatus::NotAuthenticated;
    };
    if entry.tokens.access_token.is_empty() {
        return McpAuthStatus::NotAuthenticated;
    }
    let Some(expires_at) = entry.tokens.expires_at else {
        // A token with no stated lifetime is one the provider did not bound.
        return McpAuthStatus::Authenticated;
    };
    if expires_at > now_seconds() {
        return McpAuthStatus::Authenticated;
    }
    match entry.tokens.refresh_token.is_some() {
        true => McpAuthStatus::ExpiredWithRefresh,
        false => McpAuthStatus::Expired,
    }
}

/// Forgets one server's credentials, registration included.
pub async fn forget(server_url: &str, path: &Path) -> Result<(), String> {
    McpTokenStorage::new(server_url, path)
        .clear()
        .await
        .map_err(|error| error.to_string())
}

/// Forgets every server's credentials.
/// The file is emptied rather than deleted, so its mode and its place stay put
/// and the next login does not have to re-create either.
pub async fn forget_all(path: &Path) -> Result<usize, String> {
    let count = McpCredentialStore::load(path).credentials.len();
    McpCredentialStore::update(path, |store| store.credentials.clear()).await?;
    Ok(count)
}

/// Where the browser is being sent, so the caller can show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpAuthPrompt {
    pub authorization_url: String,
    pub redirect_uri: String,
}

/// Runs the login from end to end and returns a usable access token.
/// `announce` is called once, with the URL, at the moment the browser is opened
/// — before the wait, so the user sees where to go even when the browser did not
/// come up.
pub async fn authorize<A>(
    server_url: &str,
    config: Option<&McpOAuthConfig>,
    path: &Path,
    announce: A,
) -> Result<String, String>
where
    A: FnOnce(McpAuthPrompt),
{
    let configured_redirect = config.and_then(|config| config.redirect_uri.clone());
    let listener = bind_callback_listener(configured_redirect.as_deref()).await?;
    let redirect_uri = match &configured_redirect {
        Some(uri) => uri.clone(),
        None => format!("http://127.0.0.1:{}/callback", listener.port),
    };

    let mut state = OAuthState::new(server_url, None)
        .await
        .map_err(|error| format!("the OAuth flow could not be started: {error}"))?;

    let scopes: Vec<&str> = config
        .map(|config| config.scopes.iter().map(String::as_str).collect())
        .unwrap_or_default();
    state
        .start_authorization(&scopes, &redirect_uri, Some(CLIENT_NAME))
        .await
        .map_err(|error| format!("the server would not start an authorization: {error}"))?;

    let authorization_url = state
        .get_authorization_url()
        .await
        .map_err(|error| format!("the authorization URL could not be built: {error}"))?;

    announce(McpAuthPrompt {
        authorization_url: authorization_url.clone(),
        redirect_uri: redirect_uri.clone(),
    });
    crate::utils::open_browser::open_browser(&authorization_url);

    let callback = listener.wait(CALLBACK_TIMEOUT).await?;
    // The CSRF state is checked inside `handle_callback` against the value it
    // generated, and the PKCE verifier goes with the exchange.
    state
        .handle_callback(&callback.code, &callback.state)
        .await
        .map_err(|error| format!("the authorization code was not accepted: {error}"))?;

    let token = state
        .get_access_token()
        .await
        .map_err(|error| format!("no access token came back: {error}"))?;

    let (client_id, token_response) = state
        .get_credentials()
        .await
        .map_err(|error| format!("the new credentials could not be read: {error}"))?;
    McpTokenStorage::new(server_url, path)
        .save(StoredCredentials {
            client_id,
            token_response,
        })
        .await
        .map_err(|error| format!("the new credentials could not be stored: {error}"))?;

    Ok(token)
}

/// The bound loopback socket the provider will redirect to.
#[derive(Debug)]
struct CallbackListener {
    listener: TcpListener,
    port: u16,
}

/// What came back on the redirect.
/// Deliberately not `Debug`: the code is a one-time credential, and a struct
/// that can be printed ends up printed.
struct Callback {
    code: String,
    state: String,
}

async fn bind_callback_listener(configured: Option<&str>) -> Result<CallbackListener, String> {
    // Port 0 asks the operating system for a free one. A fixed port is only
    // used when the configuration names it, because then the provider has that
    // exact URI registered and no other will be accepted.
    let port = match configured {
        Some(uri) => {
            let parsed: url::Url = uri
                .parse()
                .map_err(|error| format!("`{uri}` is not a usable redirect URI: {error}"))?;
            parsed.port().unwrap_or(80)
        }
        None => 0,
    };
    let listener =
        TcpListener::bind(("127.0.0.1", port))
            .await
            .map_err(|error| match configured {
                Some(uri) => format!(
                    "the callback listener for `{uri}` could not be started: {error}. \
                 Another process is likely holding that port."
                ),
                None => format!("the callback listener could not be started: {error}"),
            })?;
    let port = listener
        .local_addr()
        .map_err(|error| format!("the callback listener has no address: {error}"))?
        .port();
    Ok(CallbackListener { listener, port })
}

impl CallbackListener {
    async fn wait(self, timeout: Duration) -> Result<Callback, String> {
        let (mut stream, _) = tokio::time::timeout(timeout, self.listener.accept())
            .await
            .map_err(|_| {
                format!(
                    "the browser did not come back within {} seconds",
                    timeout.as_secs()
                )
            })?
            .map_err(|error| format!("the callback could not be accepted: {error}"))?;

        // One read of a bounded buffer. The request line carries the whole
        // answer, and a client that sends more than this is not the redirect.
        let mut buffer = vec![0u8; 8192];
        let read = stream
            .read(&mut buffer)
            .await
            .map_err(|error| format!("the callback could not be read: {error}"))?;
        let request = String::from_utf8_lossy(&buffer[..read]);
        let result = parse_callback(&request);

        let body = match &result {
            Ok(_) => SUCCESS_PAGE,
            Err(_) => FAILURE_PAGE,
        };
        let status = if result.is_ok() {
            "200 OK"
        } else {
            "400 Bad Request"
        };
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes()).await;
        let _ = stream.shutdown().await;

        result
    }
}

/// Reads the code and state out of the redirect's request line.
fn parse_callback(request: &str) -> Result<Callback, String> {
    let target = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .ok_or_else(|| "the callback was not an HTTP request".to_owned())?;
    let query = target.split_once('?').map(|(_, query)| query).unwrap_or("");
    let params: BTreeMap<String, String> = url::form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect();

    if let Some(error) = params.get("error") {
        let description = params
            .get("error_description")
            .map(String::as_str)
            .unwrap_or("no description");
        return Err(format!(
            "the provider refused the login: {error} ({description})"
        ));
    }
    let code = params
        .get("code")
        .cloned()
        .ok_or_else(|| "the callback carried no authorization code".to_owned())?;
    let state = params
        .get("state")
        .cloned()
        .ok_or_else(|| "the callback carried no state parameter".to_owned())?;
    Ok(Callback { code, state })
}

const SUCCESS_PAGE: &str = "<!doctype html><meta charset=\"utf-8\"><title>Signed in</title>\
<body style=\"font-family:system-ui,sans-serif;text-align:center;padding:4rem\">\
<h1>Signed in</h1><p>You can close this tab and go back to the terminal.</p>";

const FAILURE_PAGE: &str = "<!doctype html><meta charset=\"utf-8\"><title>Sign-in failed</title>\
<body style=\"font-family:system-ui,sans-serif;text-align:center;padding:4rem\">\
<h1>Sign-in failed</h1><p>Go back to the terminal for the reason.</p>";

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn entry(url: &str, token: &str) -> McpCredentialEntry {
        McpCredentialEntry {
            server_url: url.to_owned(),
            tokens: McpOAuthTokens {
                access_token: token.to_owned(),
                ..McpOAuthTokens::default()
            },
            client_registration: None,
        }
    }

    #[tokio::test]
    async fn a_writer_with_a_stale_view_keeps_the_other_server() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("mcp-credentials.json");

        // Both writers start from the same, empty, view of the file.
        let stale = McpCredentialStore::load(&path);
        assert!(stale.credentials.is_empty());

        McpCredentialStore::update(&path, |store| store.set(entry("https://a/mcp", "token-a")))
            .await
            .unwrap();
        McpCredentialStore::update(&path, |store| store.set(entry("https://b/mcp", "token-b")))
            .await
            .unwrap();

        let loaded = McpCredentialStore::load(&path);
        assert_eq!(loaded.credentials.len(), 2);
        assert_eq!(
            loaded.get("https://a/mcp").unwrap().tokens.access_token,
            "token-a"
        );
        assert_eq!(
            loaded.get("https://b/mcp").unwrap().tokens.access_token,
            "token-b"
        );
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn the_credential_file_is_never_world_readable() {
        use std::os::unix::fs::PermissionsExt;
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("mcp-credentials.json");

        McpCredentialStore::update(&path, |store| store.set(entry("https://a/mcp", "t")))
            .await
            .unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "mode is {:o}", mode & 0o777);
    }

    #[tokio::test]
    async fn a_corrupt_file_reads_as_empty_rather_than_failing() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("mcp-credentials.json");
        std::fs::write(&path, "{ this is not json").unwrap();

        assert!(McpCredentialStore::load(&path).credentials.is_empty());
        // And it is repairable: the next write replaces it.
        McpCredentialStore::update(&path, |store| store.set(entry("https://a/mcp", "t")))
            .await
            .unwrap();
        assert!(McpCredentialStore::load(&path).has_credentials("https://a/mcp"));
    }

    #[tokio::test]
    async fn a_stored_token_round_trips_through_the_rmcp_shape() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("mcp-credentials.json");
        McpCredentialStore::update(&path, |store| {
            store.set(McpCredentialEntry {
                server_url: "https://a/mcp".to_owned(),
                tokens: McpOAuthTokens {
                    access_token: "access".to_owned(),
                    refresh_token: Some("refresh".to_owned()),
                    expires_at: Some(now_seconds() + 3600),
                    scope: Some("read".to_owned()),
                },
                client_registration: Some(McpClientRegistration {
                    client_id: "client".to_owned(),
                    client_secret: None,
                    client_id_issued_at: None,
                    client_secret_expires_at: None,
                }),
            });
        })
        .await
        .unwrap();

        let storage = McpTokenStorage::new("https://a/mcp", &path);
        let loaded = CredentialStore::load(&storage).await.unwrap().unwrap();

        assert_eq!(loaded.client_id, "client");
        let response = loaded.token_response.unwrap();
        use oauth2::TokenResponse;
        assert_eq!(response.access_token().secret(), "access");
        assert_eq!(response.refresh_token().unwrap().secret(), "refresh");
        assert!(
            response
                .expires_in()
                .is_some_and(|left| left.as_secs() > 3500)
        );
    }

    #[tokio::test]
    async fn an_expired_token_reports_no_time_left_rather_than_none() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("mcp-credentials.json");
        McpCredentialStore::update(&path, |store| {
            store.set(McpCredentialEntry {
                server_url: "https://a/mcp".to_owned(),
                tokens: McpOAuthTokens {
                    access_token: "access".to_owned(),
                    refresh_token: Some("refresh".to_owned()),
                    expires_at: Some(1),
                    scope: None,
                },
                client_registration: None,
            });
        })
        .await
        .unwrap();

        let storage = McpTokenStorage::new("https://a/mcp", &path);
        let loaded = CredentialStore::load(&storage).await.unwrap().unwrap();

        use oauth2::TokenResponse;
        // Zero, not absent: absent would read as a token that never expires.
        let response = loaded.token_response.unwrap();
        assert_eq!(response.expires_in().map(|left| left.as_secs()), Some(0));
    }

    #[tokio::test]
    async fn a_registration_without_a_token_is_not_offered_as_credentials() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("mcp-credentials.json");
        McpCredentialStore::update(&path, |store| {
            store.set(McpCredentialEntry {
                server_url: "https://a/mcp".to_owned(),
                tokens: McpOAuthTokens::default(),
                client_registration: Some(McpClientRegistration {
                    client_id: "client".to_owned(),
                    client_secret: None,
                    client_id_issued_at: None,
                    client_secret_expires_at: None,
                }),
            });
        })
        .await
        .unwrap();

        let storage = McpTokenStorage::new("https://a/mcp", &path);
        assert!(CredentialStore::load(&storage).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn forgetting_tokens_keeps_the_registration() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("mcp-credentials.json");
        McpCredentialStore::update(&path, |store| {
            store.set(McpCredentialEntry {
                server_url: "https://a/mcp".to_owned(),
                tokens: McpOAuthTokens {
                    access_token: "access".to_owned(),
                    ..McpOAuthTokens::default()
                },
                client_registration: Some(McpClientRegistration {
                    client_id: "client".to_owned(),
                    client_secret: Some("secret".to_owned()),
                    client_id_issued_at: None,
                    client_secret_expires_at: None,
                }),
            });
        })
        .await
        .unwrap();

        McpTokenStorage::new("https://a/mcp", &path)
            .forget_tokens()
            .await
            .unwrap();

        let store = McpCredentialStore::load(&path);
        let entry = store.get("https://a/mcp").unwrap();
        assert!(entry.tokens.access_token.is_empty());
        assert_eq!(
            entry.client_registration.as_ref().unwrap().client_secret,
            Some("secret".to_owned())
        );
    }

    #[tokio::test]
    async fn clearing_removes_the_server_and_leaves_the_others() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("mcp-credentials.json");
        McpCredentialStore::update(&path, |store| {
            store.set(entry("https://a/mcp", "a"));
            store.set(entry("https://b/mcp", "b"));
        })
        .await
        .unwrap();

        McpTokenStorage::new("https://a/mcp", &path)
            .clear()
            .await
            .unwrap();

        let store = McpCredentialStore::load(&path);
        assert!(!store.has_credentials("https://a/mcp"));
        assert!(store.has_credentials("https://b/mcp"));
    }

    #[tokio::test]
    async fn a_refresh_without_a_new_refresh_token_keeps_the_old_one() {
        use oauth2::AccessToken;
        use oauth2::basic::BasicTokenType;
        use rmcp::transport::auth::OAuthTokenResponse;

        let directory = TempDir::new().unwrap();
        let path = directory.path().join("mcp-credentials.json");
        McpCredentialStore::update(&path, |store| {
            store.set(McpCredentialEntry {
                server_url: "https://a/mcp".to_owned(),
                tokens: McpOAuthTokens {
                    access_token: "old".to_owned(),
                    refresh_token: Some("refresh".to_owned()),
                    expires_at: None,
                    scope: None,
                },
                client_registration: None,
            });
        })
        .await
        .unwrap();

        let storage = McpTokenStorage::new("https://a/mcp", &path);
        storage
            .save(StoredCredentials {
                client_id: String::new(),
                token_response: Some(OAuthTokenResponse::new(
                    AccessToken::new("new".to_owned()),
                    BasicTokenType::Bearer,
                    oauth2::EmptyExtraTokenFields {},
                )),
            })
            .await
            .unwrap();

        let store = McpCredentialStore::load(&path);
        let entry = store.get("https://a/mcp").unwrap();
        assert_eq!(entry.tokens.access_token, "new");
        assert_eq!(entry.tokens.refresh_token, Some("refresh".to_owned()));
    }

    #[tokio::test]
    async fn a_server_nobody_logged_into_costs_no_round_trip() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("mcp-credentials.json");
        // The URL is unreachable on purpose: reaching for it would hang or fail
        // slowly, and the point is that it is never reached.
        assert!(
            stored_access_token("https://127.0.0.1:1/mcp", &path)
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn the_stored_state_is_reported_as_it_stands() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("mcp-credentials.json");

        assert_eq!(
            auth_status("https://a/mcp", &path),
            McpAuthStatus::NotAuthenticated
        );

        McpCredentialStore::update(&path, |store| {
            store.set(entry("https://a/mcp", "token"));
        })
        .await
        .unwrap();
        // No stated lifetime means the provider did not bound it.
        assert_eq!(
            auth_status("https://a/mcp", &path),
            McpAuthStatus::Authenticated
        );

        McpCredentialStore::update(&path, |store| {
            store.set(McpCredentialEntry {
                server_url: "https://a/mcp".to_owned(),
                tokens: McpOAuthTokens {
                    access_token: "token".to_owned(),
                    refresh_token: Some("refresh".to_owned()),
                    expires_at: Some(1),
                    scope: None,
                },
                client_registration: None,
            });
        })
        .await
        .unwrap();
        // Renewable: the next connect fixes it without asking anyone.
        assert_eq!(
            auth_status("https://a/mcp", &path),
            McpAuthStatus::ExpiredWithRefresh
        );

        McpCredentialStore::update(&path, |store| {
            store.set(McpCredentialEntry {
                server_url: "https://a/mcp".to_owned(),
                tokens: McpOAuthTokens {
                    access_token: "token".to_owned(),
                    refresh_token: None,
                    expires_at: Some(1),
                    scope: None,
                },
                client_registration: None,
            });
        })
        .await
        .unwrap();
        assert_eq!(auth_status("https://a/mcp", &path), McpAuthStatus::Expired);
    }

    #[tokio::test]
    async fn signing_out_of_one_server_leaves_the_others() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("mcp-credentials.json");
        McpCredentialStore::update(&path, |store| {
            store.set(entry("https://a/mcp", "a"));
            store.set(entry("https://b/mcp", "b"));
        })
        .await
        .unwrap();

        forget("https://a/mcp", &path).await.unwrap();

        let store = McpCredentialStore::load(&path);
        assert!(!store.has_credentials("https://a/mcp"));
        assert!(store.has_credentials("https://b/mcp"));
    }

    #[tokio::test]
    async fn signing_out_of_everything_empties_the_file_without_removing_it() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("mcp-credentials.json");
        McpCredentialStore::update(&path, |store| {
            store.set(entry("https://a/mcp", "a"));
            store.set(entry("https://b/mcp", "b"));
        })
        .await
        .unwrap();

        assert_eq!(forget_all(&path).await.unwrap(), 2);

        // The file stays, so its mode and its place survive the next login.
        assert!(path.exists());
        assert!(McpCredentialStore::load(&path).credentials.is_empty());
    }

    #[test]
    fn a_callback_carries_its_code_and_state() {
        let callback =
            parse_callback("GET /callback?code=abc&state=xyz HTTP/1.1\r\nHost: localhost\r\n\r\n")
                .unwrap();
        assert_eq!(callback.code, "abc");
        assert_eq!(callback.state, "xyz");
    }

    #[test]
    fn a_refusal_is_reported_with_the_provider_s_reason() {
        let Err(error) = parse_callback(
            "GET /callback?error=access_denied&error_description=User+said+no HTTP/1.1\r\n\r\n",
        ) else {
            panic!("a refusal was read as a successful callback");
        };
        assert!(error.contains("access_denied"), "{error}");
        assert!(error.contains("User said no"), "{error}");
    }

    #[test]
    fn a_callback_without_a_state_is_refused() {
        // The state is the CSRF check; a redirect without one is not an answer
        // to the request that was sent.
        assert!(parse_callback("GET /callback?code=abc HTTP/1.1\r\n\r\n").is_err());
    }

    #[test]
    fn something_that_is_not_a_request_is_refused() {
        assert!(parse_callback("").is_err());
    }

    #[tokio::test]
    async fn the_listener_takes_a_port_the_system_picks() {
        let listener = bind_callback_listener(None).await.unwrap();
        assert_ne!(listener.port, 0);
    }

    #[tokio::test]
    async fn a_configured_redirect_keeps_its_port() {
        let any = bind_callback_listener(None).await.unwrap();
        let port = any.port;
        drop(any);

        let bound = bind_callback_listener(Some(&format!("http://127.0.0.1:{port}/callback")))
            .await
            .unwrap();
        assert_eq!(bound.port, port);
    }

    #[tokio::test]
    async fn a_configured_redirect_that_cannot_be_bound_says_which_one() {
        let held = bind_callback_listener(None).await.unwrap();
        let uri = format!("http://127.0.0.1:{}/callback", held.port);

        let error = bind_callback_listener(Some(&uri)).await.unwrap_err();

        assert!(error.contains(&uri), "{error}");
    }

    #[tokio::test]
    async fn the_wait_gives_up_rather_than_holding_the_port_forever() {
        let listener = bind_callback_listener(None).await.unwrap();
        let Err(error) = listener.wait(Duration::from_millis(50)).await else {
            panic!("the wait returned a callback nobody sent");
        };
        assert!(error.contains("did not come back"), "{error}");
    }

    #[tokio::test]
    async fn the_browser_gets_an_answer_it_can_show() {
        let listener = bind_callback_listener(None).await.unwrap();
        let port = listener.port;
        let waiting = tokio::spawn(async move { listener.wait(CALLBACK_TIMEOUT).await });

        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        stream
            .write_all(b"GET /callback?code=abc&state=xyz HTTP/1.1\r\nHost: x\r\n\r\n")
            .await
            .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap();

        assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
        assert!(response.contains("Signed in"), "{response}");
        let callback = waiting.await.unwrap().unwrap();
        assert_eq!(callback.code, "abc");
    }

    #[tokio::test]
    async fn a_refused_login_still_answers_the_browser() {
        let listener = bind_callback_listener(None).await.unwrap();
        let port = listener.port;
        let waiting = tokio::spawn(async move { listener.wait(CALLBACK_TIMEOUT).await });

        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        stream
            .write_all(b"GET /callback?error=access_denied HTTP/1.1\r\n\r\n")
            .await
            .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap();

        assert!(response.starts_with("HTTP/1.1 400"), "{response}");
        assert!(waiting.await.unwrap().is_err());
    }
}
