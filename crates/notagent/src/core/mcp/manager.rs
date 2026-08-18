//! The configured servers and their live connections (port addition, v0.1.22).
//!
//! Servers connect on first need rather than at startup, so a session that asks
//! nothing of them pays for nothing, and a server that is broken costs only the
//! calls that reach it.
//!
//! Six states, after `../kimi-code-main`'s connection manager. Two of them do
//! real work: `NeedsAuth` is what makes a server contribute an `authenticate`
//! tool instead of its own, and `Removed` is a tombstone, so a tool the model
//! still holds fails with "that server is gone" rather than with a transport
//! error that reads like a bug.
//!
//! Each server has its own lock. Connecting one never blocks a call to another,
//! and two callers racing to first-use the same server produce one connection.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::core::mcp::auth::{McpAuthPrompt, authorize, stored_access_token};
use crate::core::mcp::client::{McpCallError, McpConnection};
use crate::core::mcp::{McpServerConfig, ServerName};

/// A server's tools are capped so its manifest cannot become a standing tax:
/// names and descriptions are sent on every turn.
pub const MAX_TOOLS_PER_SERVER: usize = 128;

/// Longer descriptions are cut, for the same reason.
pub const MAX_TOOL_DESCRIPTION_CHARS: usize = 1024;

/// Where a server is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpServerStatus {
    /// Configured, not yet reached.
    Pending,
    /// Connected, tools discovered.
    Connected,
    /// The last attempt failed; the reason is on the entry.
    Failed,
    /// Switched off in the configuration.
    Disabled,
    /// Reachable, but it wants an OAuth login first.
    NeedsAuth,
    /// Gone from the configuration; its tools answer with a notice.
    Removed,
}

impl McpServerStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Connected => "connected",
            Self::Failed => "failed",
            Self::Disabled => "disabled",
            Self::NeedsAuth => "needs auth",
            Self::Removed => "removed",
        }
    }
}

/// One tool a server offers, as this port names it.
#[derive(Debug, Clone, PartialEq)]
pub struct McpToolInfo {
    /// The name the model sees: `mcp__<server>__<tool>`.
    pub qualified_name: String,
    /// The name the server knows.
    pub tool_name: String,
    pub server: ServerName,
    pub description: String,
    pub input_schema: serde_json::Value,
}

/// A server as the UI and `/mcp` report it.
#[derive(Debug, Clone, PartialEq)]
pub struct McpServerEntry {
    pub name: ServerName,
    pub transport: &'static str,
    pub status: McpServerStatus,
    pub tool_count: usize,
    pub error: Option<String>,
}

struct Server {
    config: McpServerConfig,
    status: McpServerStatus,
    connection: Option<Arc<McpConnection>>,
    tools: Vec<McpToolInfo>,
    error: Option<String>,
}

impl Server {
    fn new(config: McpServerConfig) -> Self {
        let status = if config.is_disabled() {
            McpServerStatus::Disabled
        } else {
            McpServerStatus::Pending
        };
        Self {
            config,
            status,
            connection: None,
            tools: Vec::new(),
            error: None,
        }
    }

    fn entry(&self, name: &ServerName) -> McpServerEntry {
        McpServerEntry {
            name: name.clone(),
            transport: self.config.transport(),
            status: self.status,
            tool_count: self.tools.len(),
            error: self.error.clone(),
        }
    }
}

/// Qualifies a server's tool name the way both references do.
pub fn qualify_tool_name(server: &ServerName, tool: &str) -> String {
    format!("mcp__{server}__{tool}")
}

/// The server and tool a qualified name refers to.
///
/// The tool half keeps any further separators, so a server whose tool is itself
/// called `a__b` round-trips; a server whose *name* contains the separator is
/// rejected at configuration time instead.
pub fn split_qualified_name(qualified: &str) -> Option<(ServerName, String)> {
    let rest = qualified.strip_prefix("mcp__")?;
    let (server, tool) = rest.split_once("__")?;
    (!server.is_empty() && !tool.is_empty()).then(|| (ServerName::from(server), tool.to_owned()))
}

/// Whether a server may be configured under this name.
///
/// A name containing the separator would make its qualified tool names
/// ambiguous, and one with whitespace or a leading `mcp__` would confuse the
/// same lookup.
pub fn is_usable_server_name(name: &ServerName) -> bool {
    let value = name.as_str();
    !value.is_empty() && !value.contains("__") && !value.contains(char::is_whitespace)
}

/// The configured servers, their connections and their tools.
pub struct McpManager {
    servers: Mutex<BTreeMap<ServerName, Arc<Mutex<Server>>>>,
    env: BTreeMap<String, String>,
    /// Where OAuth tokens are kept. Absent means this manager never stores or
    /// presents one, which is what the tests and the stdio-only cases want.
    credentials: Option<PathBuf>,
}

impl McpManager {
    pub fn new(
        configs: BTreeMap<ServerName, McpServerConfig>,
        env: BTreeMap<String, String>,
    ) -> Self {
        let servers = configs
            .into_iter()
            .filter(|(name, _)| is_usable_server_name(name))
            .map(|(name, config)| (name, Arc::new(Mutex::new(Server::new(config)))))
            .collect();
        Self {
            servers: Mutex::new(servers),
            env,
            credentials: None,
        }
    }

    /// The same manager, with a credential file behind it.
    pub fn with_credentials(mut self, path: impl Into<PathBuf>) -> Self {
        self.credentials = Some(path.into());
        self
    }

    pub fn empty() -> Self {
        Self::new(BTreeMap::new(), BTreeMap::new())
    }

    async fn server(&self, name: &ServerName) -> Option<Arc<Mutex<Server>>> {
        self.servers.lock().await.get(name).cloned()
    }

    async fn names(&self) -> Vec<ServerName> {
        self.servers.lock().await.keys().cloned().collect()
    }

    /// Every configured server, for `/mcp` and the footer.
    pub async fn entries(&self) -> Vec<McpServerEntry> {
        let mut entries = Vec::new();
        for name in self.names().await {
            let Some(server) = self.server(&name).await else {
                continue;
            };
            let server = server.lock().await;
            entries.push(server.entry(&name));
        }
        entries
    }

    /// Every configured server, without waiting for a lock.
    ///
    /// For callers that must not block — the footer renders on the UI thread,
    /// and a summary one render out of date costs nothing next to a stalled
    /// frame. `None` means someone else held a lock, not that anything is wrong.
    pub fn try_entries(&self) -> Option<Vec<McpServerEntry>> {
        let servers = self.servers.try_lock().ok()?;
        let mut entries = Vec::new();
        for (name, handle) in servers.iter() {
            let server = handle.try_lock().ok()?;
            entries.push(server.entry(name));
        }
        Some(entries)
    }

    /// Connects a server if it is not connected yet and returns its tools.
    ///
    /// A failure is recorded on the entry and returned; it never propagates
    /// past the server it belongs to.
    pub async fn ensure_connected(&self, name: &ServerName) -> Vec<McpToolInfo> {
        let Some(handle) = self.server(name).await else {
            return Vec::new();
        };
        let mut server = handle.lock().await;
        match server.status {
            McpServerStatus::Connected => return server.tools.clone(),
            McpServerStatus::Disabled | McpServerStatus::Removed => return Vec::new(),
            // A previous failure is not retried on its own; `/mcp reconnect`
            // is how a user says the cause is gone.
            McpServerStatus::Failed | McpServerStatus::NeedsAuth => return server.tools.clone(),
            McpServerStatus::Pending => {}
        }
        Self::connect_locked(name, &mut server, &self.env, self.credentials.as_deref()).await;
        server.tools.clone()
    }

    /// Connects every server that has not been tried yet.
    pub async fn ensure_all_connected(&self) -> Vec<McpToolInfo> {
        let mut tools = Vec::new();
        for name in self.names().await {
            tools.extend(self.ensure_connected(&name).await);
        }
        tools
    }

    /// Retries a server whatever state it is in, as `/mcp reconnect` does.
    pub async fn reconnect(&self, name: &ServerName) -> Result<usize, String> {
        let Some(handle) = self.server(name).await else {
            return Err(format!("no server named `{name}` is configured"));
        };
        let mut server = handle.lock().await;
        if server.status == McpServerStatus::Removed {
            return Err(format!("`{name}` is no longer in the configuration"));
        }
        if let Some(connection) = server.connection.take()
            && let Ok(connection) = Arc::try_unwrap(connection)
        {
            connection.close().await;
        }
        server.status = McpServerStatus::Pending;
        server.tools.clear();
        server.error = None;
        Self::connect_locked(name, &mut server, &self.env, self.credentials.as_deref()).await;
        match server.status {
            McpServerStatus::Connected => Ok(server.tools.len()),
            _ => Err(server
                .error
                .clone()
                .unwrap_or_else(|| "the server could not be reached".to_owned())),
        }
    }

    async fn connect_locked(
        name: &ServerName,
        server: &mut Server,
        env: &BTreeMap<String, String>,
        credentials: Option<&std::path::Path>,
    ) {
        let token = stored_token_for(&server.config, credentials).await;
        let connection =
            match McpConnection::connect_authorized(&server.config, env, token.as_deref()).await {
                Ok(connection) => connection,
                Err(error) => {
                    server.status = if is_unauthorized(&error) {
                        McpServerStatus::NeedsAuth
                    } else {
                        McpServerStatus::Failed
                    };
                    server.error = Some(describe(&error, "connecting"));
                    return;
                }
            };
        match connection.list_tools().await {
            Ok(tools) => {
                server.tools = accept_tools(name, tools);
                server.status = McpServerStatus::Connected;
                server.error = None;
                server.connection = Some(Arc::new(connection));
            }
            Err(error) => {
                server.status = if is_unauthorized(&error) {
                    McpServerStatus::NeedsAuth
                } else {
                    McpServerStatus::Failed
                };
                server.error = Some(describe(&error, "listing tools"));
                connection.close().await;
            }
        }
    }

    /// The live connection for a server, when it has one.
    pub async fn connection(&self, name: &ServerName) -> Option<Arc<McpConnection>> {
        let handle = self.server(name).await?;
        let server = handle.lock().await;
        server.connection.clone()
    }

    pub async fn status(&self, name: &ServerName) -> Option<McpServerStatus> {
        let handle = self.server(name).await?;
        let status = handle.lock().await.status;
        Some(status)
    }

    /// Marks a server gone, keeping the entry so its tools can say so.
    pub async fn mark_removed(&self, name: &ServerName) {
        let Some(handle) = self.server(name).await else {
            return;
        };
        let mut server = handle.lock().await;
        if let Some(connection) = server.connection.take()
            && let Ok(connection) = Arc::try_unwrap(connection)
        {
            connection.close().await;
        }
        server.status = McpServerStatus::Removed;
        server.tools.clear();
        server.error = Some("the server was removed from the configuration".to_owned());
    }

    /// Logs into a server and reconnects it on the new token.
    ///
    /// The per-server lock is deliberately not held across the browser flow:
    /// that wait is measured in minutes, and holding it would stall `/mcp`, the
    /// footer and every call to every other server for its duration. What the
    /// flow needs from the server — its URL and its OAuth settings — is copied
    /// out first, and the reconnect takes the lock again afterwards.
    pub async fn authenticate<A>(&self, name: &ServerName, announce: A) -> Result<usize, String>
    where
        A: FnOnce(McpAuthPrompt),
    {
        let Some(credentials) = self.credentials.clone() else {
            return Err("this session keeps no MCP credentials, so it cannot log in".to_owned());
        };
        let Some(handle) = self.server(name).await else {
            return Err(format!("no server named `{name}` is configured"));
        };
        let config = {
            let server = handle.lock().await;
            if server.status == McpServerStatus::Removed {
                return Err(format!("`{name}` is no longer in the configuration"));
            }
            server.config.clone()
        };

        let McpServerConfig::Http(http) = &config else {
            return Err(format!(
                "`{name}` runs as a local process, which has no login to perform"
            ));
        };
        if http.is_oauth_disabled() {
            return Err(format!(
                "`{name}` has OAuth switched off in its configuration; it expects a header instead"
            ));
        }

        authorize(&http.url, http.oauth_config(), &credentials, announce).await?;
        self.reconnect(name).await
    }

    /// Every server waiting for a login.
    pub async fn servers_needing_auth(&self) -> Vec<ServerName> {
        let mut waiting = Vec::new();
        for name in self.names().await {
            if self.status(&name).await == Some(McpServerStatus::NeedsAuth) {
                waiting.push(name);
            }
        }
        waiting
    }

    /// Whether a login is possible at all for this server.
    ///
    /// The synthetic `authenticate` tool is only offered where the answer is
    /// yes; a tool that can only ever report "this cannot be done" is worse than
    /// no tool, because the model will spend a turn finding out.
    pub async fn can_authenticate(&self, name: &ServerName) -> bool {
        if self.credentials.is_none() {
            return false;
        }
        let Some(handle) = self.server(name).await else {
            return false;
        };
        let server = handle.lock().await;
        match &server.config {
            McpServerConfig::Http(http) => !http.is_oauth_disabled(),
            McpServerConfig::Stdio(_) => false,
        }
    }

    /// Closes every connection. Called when the session ends.
    pub async fn shutdown(&self) {
        for name in self.names().await {
            let Some(handle) = self.server(&name).await else {
                continue;
            };
            let mut server = handle.lock().await;
            if let Some(connection) = server.connection.take()
                && let Ok(connection) = Arc::try_unwrap(connection)
            {
                connection.close().await;
            }
            if server.status == McpServerStatus::Connected {
                server.status = McpServerStatus::Pending;
            }
        }
    }
}

/// The token to present when connecting, when one is stored and usable.
async fn stored_token_for(
    config: &McpServerConfig,
    credentials: Option<&std::path::Path>,
) -> Option<String> {
    let credentials = credentials?;
    let McpServerConfig::Http(http) = config else {
        return None;
    };
    if http.is_oauth_disabled() {
        return None;
    }
    stored_access_token(&http.url, credentials).await
}

/// Whether the server is asking to be logged into.
fn is_unauthorized(error: &McpCallError) -> bool {
    crate::core::mcp::client::is_unauthorized_text(&error.to_string())
}

fn describe(error: &McpCallError, phase: &str) -> String {
    format!("{phase} failed: {error}")
}

/// Keeps the tools that can be offered and drops the rest.
///
/// A server is remote input. An unnamed tool has nothing to call, a duplicate
/// would shadow its twin, and a manifest without a ceiling is a cost paid on
/// every turn for as long as the server is connected.
fn accept_tools(name: &ServerName, tools: Vec<rmcp::model::Tool>) -> Vec<McpToolInfo> {
    let mut accepted: Vec<McpToolInfo> = Vec::new();
    for tool in tools {
        if accepted.len() >= MAX_TOOLS_PER_SERVER {
            break;
        }
        let tool_name = tool.name.to_string();
        if tool_name.trim().is_empty() {
            continue;
        }
        let qualified_name = qualify_tool_name(name, &tool_name);
        if accepted
            .iter()
            .any(|existing| existing.qualified_name == qualified_name)
        {
            continue;
        }
        let description = tool
            .description
            .as_deref()
            .map(|text| truncate_chars(text, MAX_TOOL_DESCRIPTION_CHARS))
            .unwrap_or_default();
        accepted.push(McpToolInfo {
            qualified_name,
            tool_name,
            server: name.clone(),
            description,
            input_schema: serde_json::Value::Object((*tool.input_schema).clone()),
        });
    }
    accepted
}

fn truncate_chars(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let kept: String = text.chars().take(limit.saturating_sub(1)).collect();
    format!("{kept}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str, description: Option<&str>) -> rmcp::model::Tool {
        rmcp::model::Tool {
            name: name.to_owned().into(),
            description: description.map(|text| text.to_owned().into()),
            input_schema: Arc::new(serde_json::Map::new()),
            annotations: None,
            output_schema: None,
            icons: None,
            title: None,
            meta: None,
        }
    }

    #[test]
    fn a_qualified_name_round_trips() {
        let server = ServerName::from("files");
        let qualified = qualify_tool_name(&server, "read");
        assert_eq!(qualified, "mcp__files__read");
        assert_eq!(
            split_qualified_name(&qualified),
            Some((server, "read".to_owned()))
        );
    }

    #[test]
    fn a_tool_whose_own_name_contains_the_separator_still_round_trips() {
        let server = ServerName::from("files");
        let qualified = qualify_tool_name(&server, "read__all");
        assert_eq!(
            split_qualified_name(&qualified),
            Some((server, "read__all".to_owned()))
        );
    }

    #[test]
    fn a_name_that_is_not_ours_does_not_split() {
        for value in ["read", "mcp__", "mcp__files", "mcp____read", "mcp__files__"] {
            assert_eq!(split_qualified_name(value), None, "{value}");
        }
    }

    #[test]
    fn a_server_name_that_would_make_lookups_ambiguous_is_refused() {
        for value in ["files", "my-files", "files.v2"] {
            assert!(is_usable_server_name(&ServerName::from(value)), "{value}");
        }
        for value in ["", "my__files", "my files"] {
            assert!(!is_usable_server_name(&ServerName::from(value)), "{value}");
        }
    }

    #[test]
    fn an_unusable_server_name_is_dropped_at_construction() {
        let configs = BTreeMap::from([
            (
                ServerName::from("good"),
                McpServerConfig::new_http("https://a.test"),
            ),
            (
                ServerName::from("bad__name"),
                McpServerConfig::new_http("https://b.test"),
            ),
        ]);
        let manager = McpManager::new(configs, BTreeMap::new());
        let entries = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime")
            .block_on(manager.entries());

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, ServerName::from("good"));
    }

    #[test]
    fn a_disabled_server_starts_disabled() {
        let mut server = crate::core::mcp::McpStdioServer {
            command: "true".to_owned(),
            ..crate::core::mcp::McpStdioServer::default()
        };
        server.disable = true;
        let manager = McpManager::new(
            BTreeMap::from([(ServerName::from("off"), McpServerConfig::Stdio(server))]),
            BTreeMap::new(),
        );
        let entries = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime")
            .block_on(manager.entries());

        assert_eq!(entries[0].status, McpServerStatus::Disabled);
    }

    #[test]
    fn a_nameless_tool_and_a_duplicate_are_dropped() {
        let name = ServerName::from("s");
        let accepted = accept_tools(
            &name,
            vec![
                tool("read", None),
                tool("   ", None),
                tool("read", Some("a second one under the same name")),
                tool("write", None),
            ],
        );

        let names: Vec<&str> = accepted
            .iter()
            .map(|tool| tool.tool_name.as_str())
            .collect();
        assert_eq!(names, vec!["read", "write"]);
    }

    #[test]
    fn a_manifest_without_a_ceiling_is_given_one() {
        let name = ServerName::from("s");
        let many: Vec<rmcp::model::Tool> = (0..MAX_TOOLS_PER_SERVER + 50)
            .map(|index| tool(&format!("tool_{index}"), None))
            .collect();

        assert_eq!(accept_tools(&name, many).len(), MAX_TOOLS_PER_SERVER);
    }

    #[test]
    fn an_oversized_description_is_cut() {
        let name = ServerName::from("s");
        let long = "d".repeat(MAX_TOOL_DESCRIPTION_CHARS * 2);
        let accepted = accept_tools(&name, vec![tool("read", Some(&long))]);

        assert_eq!(
            accepted[0].description.chars().count(),
            MAX_TOOL_DESCRIPTION_CHARS
        );
        assert!(accepted[0].description.ends_with('…'));
    }

    #[test]
    fn an_unauthorized_answer_is_told_from_an_ordinary_failure() {
        assert!(is_unauthorized(&McpCallError::Connect(
            "HTTP status 401 Unauthorized".into()
        )));
        assert!(is_unauthorized(&McpCallError::Answered(
            "Authentication required".into()
        )));
        assert!(!is_unauthorized(&McpCallError::Closed));
    }
}
