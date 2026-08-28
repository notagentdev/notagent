use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::core::mcp::auth::{
    McpAuthPrompt, McpAuthStatus, auth_status, authorize, forget, stored_access_token,
};
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

/// What a provider accepts as a tool name.
/// Anthropic's is `^[a-zA-Z0-9_-]{1,128}$` and the others are no wider in
/// practice. This matters more than it looks: the tool list goes out whole, so
/// one name a provider will not take fails the entire request, not the one tool
/// it belongs to.
pub const MAX_QUALIFIED_NAME_CHARS: usize = 128;

/// Qualifies a server's tool name, in an alphabet a provider will accept.
/// A server names its tools for people — `Add comment`, `read.channel`,
/// `search:web` are all ordinary — and none of those survive the round trip.
/// Every character outside the alphabet becomes an underscore.
/// Unlike `../notagent-main-rust`, which lowercases as well, case is kept:
/// the providers accept it, and folding it would collide two tools that a
/// server deliberately told apart.
pub fn qualify_tool_name(server: &ServerName, tool: &str) -> String {
    let qualified = format!(
        "mcp__{}__{}",
        sanitize_name(server.as_str()),
        sanitize_name(tool)
    );
    shorten_to_limit(&qualified)
}

/// Replaces everything a provider will not take, and tidies what is left.
/// The result can be empty — a tool named entirely in characters that do not
/// survive has no usable name, and the caller drops it.
fn sanitize_name(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut gap = false;
    for character in value.chars() {
        if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
            // A run of rejected characters becomes one underscore, so `a - b`
            // does not turn into `a___b`, and a leading or trailing run
            // disappears instead of leaving a bare separator. A name that
            // already carries its own underscores keeps them exactly, so
            // `read__all` stays the two-word name the server chose.
            if gap && !out.is_empty() && character != '_' {
                out.push('_');
            }
            gap = false;
            out.push(character);
        } else {
            gap = true;
        }
    }
    out
}

/// Cuts an over-long name down, keeping it unique.
/// The tail is what distinguishes two tools of the same server, so it is the
/// tail that is kept along with a digest of the whole: truncating alone would
/// map two long names onto one.
fn shorten_to_limit(qualified: &str) -> String {
    if qualified.len() <= MAX_QUALIFIED_NAME_CHARS {
        return qualified.to_owned();
    }
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(qualified.as_bytes());
    let suffix: String = std::iter::once('_')
        .chain(
            digest
                .iter()
                .take(4)
                .flat_map(|byte| format!("{byte:02x}").chars().collect::<Vec<_>>()),
        )
        .collect();
    let keep = MAX_QUALIFIED_NAME_CHARS - suffix.len();
    let mut head: String = qualified.chars().take(keep).collect();
    // The truncation may land mid-run and leave a trailing underscore; the
    // suffix supplies its own separator.
    while head.ends_with('_') {
        head.pop();
    }
    format!("{head}{suffix}")
}

/// The server and tool a qualified name refers to.
/// The tool half keeps any further separators, so a server whose tool is itself
/// called `a__b` round-trips; a server whose *name* contains the separator is
/// rejected at configuration time instead.
pub fn split_qualified_name(qualified: &str) -> Option<(ServerName, String)> {
    let rest = qualified.strip_prefix("mcp__")?;
    let (server, tool) = rest.split_once("__")?;
    (!server.is_empty() && !tool.is_empty()).then(|| (ServerName::from(server), tool.to_owned()))
}

/// Whether a server may be configured under this name.
/// A name that sanitizes to nothing has no tool names to give, and one
/// containing the separator would make the ones it gives ambiguous. Anything
/// else is accepted and cleaned up at qualification time — a server called
/// `claude.ai Slack` is a normal thing to write in a config file.
pub fn is_usable_server_name(name: &ServerName) -> bool {
    let value = name.as_str();
    !value.contains("__") && !sanitize_name(value).is_empty()
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

    /// Every configured server, with each connection checked before reporting.
    /// A server that died while nobody was calling it still reads as
    /// `connected` on the entry: `rmcp` gives a running service no way to say it
    /// ended without consuming the handle the calls need. A tool call finds out
    /// on its own — the triage reconnects and repeats it — but a status list
    /// that says `connected` about a dead process is simply wrong, and this is
    /// the one place a user reads it. The probe is a ping per connected server
    /// and only runs when asked.
    pub async fn probed_entries(&self) -> Vec<McpServerEntry> {
        let mut entries = Vec::new();
        for name in self.names().await {
            let Some(handle) = self.server(&name).await else {
                continue;
            };
            let mut server = handle.lock().await;
            if server.status == McpServerStatus::Connected
                && let Some(connection) = server.connection.clone()
                && !connection.is_alive().await
            {
                server.status = McpServerStatus::Failed;
                server.error = Some("the server stopped answering".to_owned());
                server.tools.clear();
                if let Some(connection) = server.connection.take()
                    && let Ok(connection) = Arc::try_unwrap(connection)
                {
                    connection.close().await;
                }
            }
            entries.push(server.entry(&name));
        }
        entries
    }

    /// Every configured server, without waiting for a lock.
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
                server.tools = accept_tools_filtered(name, tools, server.config.tool_filters());
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

    /// What is stored for a server, without touching the network.
    /// `None` means the question does not apply: a local process, a server with
    /// OAuth switched off, or a session that keeps no credentials.
    pub async fn auth_status(&self, name: &ServerName) -> Option<McpAuthStatus> {
        let credentials = self.credentials.as_ref()?;
        let handle = self.server(name).await?;
        let server = handle.lock().await;
        let McpServerConfig::Http(http) = &server.config else {
            return None;
        };
        if http.is_oauth_disabled() {
            return None;
        }
        Some(auth_status(&http.url, credentials))
    }

    /// Forgets one server's stored credentials and drops its connection.
    /// The connection goes with them: leaving it up would keep answering on a
    /// token the user just asked to be rid of.
    pub async fn sign_out(&self, name: &ServerName) -> Result<(), String> {
        let Some(credentials) = self.credentials.clone() else {
            return Err("this session keeps no MCP credentials".to_owned());
        };
        let Some(handle) = self.server(name).await else {
            return Err(format!("no server named `{name}` is configured"));
        };
        let url = {
            let server = handle.lock().await;
            match &server.config {
                McpServerConfig::Http(http) => http.url.clone(),
                McpServerConfig::Stdio(_) => {
                    return Err(format!(
                        "`{name}` runs as a local process, which has no sign-in to forget"
                    ));
                }
            }
        };
        forget(&url, &credentials).await?;

        let mut server = handle.lock().await;
        if let Some(connection) = server.connection.take()
            && let Ok(connection) = Arc::try_unwrap(connection)
        {
            connection.close().await;
        }
        server.tools.clear();
        server.status = McpServerStatus::Pending;
        server.error = None;
        Ok(())
    }

    /// Drops every connection that was made on a stored token.
    /// The credential file is the user's rather than the project's, so emptying
    /// it belongs to the command that owns that file; what belongs here is that
    /// no connection keeps answering on a sign-in that was just discarded.
    pub async fn drop_authenticated_connections(&self) {
        for name in self.names().await {
            let Some(handle) = self.server(&name).await else {
                continue;
            };
            let mut server = handle.lock().await;
            if !matches!(&server.config, McpServerConfig::Http(http) if !http.is_oauth_disabled()) {
                continue;
            }
            if let Some(connection) = server.connection.take()
                && let Ok(connection) = Arc::try_unwrap(connection)
            {
                connection.close().await;
            }
            server.tools.clear();
            server.status = McpServerStatus::Pending;
            server.error = None;
        }
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
/// A server is remote input. An unnamed tool has nothing to call, a duplicate
/// would shadow its twin, and a manifest without a ceiling is a cost paid on
/// every turn for as long as the server is connected.
#[cfg(test)]
fn accept_tools(name: &ServerName, tools: Vec<rmcp::model::Tool>) -> Vec<McpToolInfo> {
    accept_tools_filtered(name, tools, (None, None))
}

/// The same, with the server's own allow and deny lists applied.
/// The lists name tools the way the server does, because that is how its
/// nobody writing a config should have to know it.
fn accept_tools_filtered(
    name: &ServerName,
    tools: Vec<rmcp::model::Tool>,
    filters: (Option<&[String]>, Option<&[String]>),
) -> Vec<McpToolInfo> {
    let (enabled, disabled) = filters;
    let mut accepted: Vec<McpToolInfo> = Vec::new();
    for tool in tools {
        if accepted.len() >= MAX_TOOLS_PER_SERVER {
            break;
        }
        let tool_name = tool.name.to_string();
        if tool_name.trim().is_empty() {
            continue;
        }
        // An allow list that names nothing keeps nothing: an empty list is a
        // statement, not an oversight, and reading it as "everything" would be
        // the opposite of what it says.
        if enabled.is_some_and(|names| !names.iter().any(|allowed| allowed == &tool_name)) {
            continue;
        }
        if disabled.is_some_and(|names| names.iter().any(|denied| denied == &tool_name)) {
            continue;
        }
        let qualified_name = qualify_tool_name(name, &tool_name);
        // A name written entirely in characters no provider takes leaves
        // nothing to call it by.
        if qualified_name.ends_with("__") {
            continue;
        }
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
    fn an_allow_list_keeps_only_what_it_names() {
        let tools = accept_tools_filtered(
            &ServerName::from("s"),
            vec![
                tool("read", None),
                tool("write", None),
                tool("delete", None),
            ],
            (Some(&["read".to_owned(), "write".to_owned()]), None),
        );

        let names: Vec<&str> = tools.iter().map(|tool| tool.tool_name.as_str()).collect();
        assert_eq!(names, vec!["read", "write"]);
    }

    #[test]
    fn a_deny_list_removes_what_it_names() {
        let tools = accept_tools_filtered(
            &ServerName::from("s"),
            vec![tool("read", None), tool("delete", None)],
            (None, Some(&["delete".to_owned()])),
        );

        let names: Vec<&str> = tools.iter().map(|tool| tool.tool_name.as_str()).collect();
        assert_eq!(names, vec!["read"]);
    }

    #[test]
    fn a_denial_wins_over_an_allowance() {
        let tools = accept_tools_filtered(
            &ServerName::from("s"),
            vec![tool("read", None)],
            (Some(&["read".to_owned()]), Some(&["read".to_owned()])),
        );

        assert!(tools.is_empty());
    }

    #[test]
    fn an_empty_allow_list_keeps_nothing() {
        // An empty list is a statement, not an oversight; reading it as
        // "everything" would be the opposite of what it says.
        let tools = accept_tools_filtered(
            &ServerName::from("s"),
            vec![tool("read", None)],
            (Some(&[]), None),
        );

        assert!(tools.is_empty());
    }

    #[test]
    fn the_lists_use_the_server_s_own_naming() {
        // form, so the filter matches the name the server's documentation uses.
        let tools = accept_tools_filtered(
            &ServerName::from("s"),
            vec![tool("Add comment", None)],
            (Some(&["Add comment".to_owned()]), None),
        );

        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].qualified_name, "mcp__s__Add_comment");
    }

    #[test]
    fn a_name_a_provider_would_reject_is_made_acceptable() {
        // Anthropic takes `^[a-zA-Z0-9_-]{1,128}$`, and the tool list goes out
        // whole: one name outside it fails the request, not the tool.
        let server = ServerName::from("claude.ai Slack");

        let qualified = qualify_tool_name(&server, "Add comment");

        assert_eq!(qualified, "mcp__claude_ai_Slack__Add_comment");
        assert!(is_provider_safe(&qualified), "{qualified}");
    }

    #[test]
    fn every_shape_a_server_might_use_survives() {
        for (server, tool) in [
            ("github", "read-channel"),
            ("hugging face", "search:web"),
            ("a.b.c", "do/it"),
            ("Ünïcøde", "naïve"),
            ("srv", "  spaced  "),
            ("srv", "trailing---"),
        ] {
            let qualified = qualify_tool_name(&ServerName::from(server), tool);
            assert!(
                is_provider_safe(&qualified),
                "`{server}`/`{tool}` -> {qualified}"
            );
        }
    }

    #[test]
    fn a_hyphen_is_kept_because_the_providers_take_it() {
        // The reference folds it to an underscore; that is its own legacy
        // separator's business, not the alphabet's.
        assert_eq!(
            qualify_tool_name(&ServerName::from("gh"), "read-channel"),
            "mcp__gh__read-channel"
        );
    }

    #[test]
    fn case_is_kept_so_two_tools_do_not_become_one() {
        let lower = qualify_tool_name(&ServerName::from("s"), "getUser");
        let upper = qualify_tool_name(&ServerName::from("s"), "getuser");
        assert_ne!(lower, upper);
    }

    #[test]
    fn runs_of_rejected_characters_collapse() {
        assert_eq!(
            qualify_tool_name(&ServerName::from("s"), "a ... b"),
            "mcp__s__a_b"
        );
    }

    #[test]
    fn an_over_long_name_is_cut_to_the_limit() {
        let qualified = qualify_tool_name(&ServerName::from("s"), &"x".repeat(400));

        assert_eq!(qualified.len(), MAX_QUALIFIED_NAME_CHARS);
        assert!(is_provider_safe(&qualified), "{qualified}");
    }

    #[test]
    fn two_over_long_names_stay_apart() {
        // Truncation alone would map both onto the same head.
        let first = qualify_tool_name(&ServerName::from("s"), &format!("{}a", "x".repeat(400)));
        let second = qualify_tool_name(&ServerName::from("s"), &format!("{}b", "x".repeat(400)));

        assert_ne!(first, second);
    }

    #[test]
    fn a_name_with_nothing_usable_in_it_produces_no_tool() {
        let tools = accept_tools(
            &ServerName::from("s"),
            vec![tool("！！！", None), tool("ok", None)],
        );

        let names: Vec<&str> = tools.iter().map(|tool| tool.tool_name.as_str()).collect();
        assert_eq!(names, vec!["ok"]);
    }

    #[test]
    fn two_tools_that_sanitize_alike_keep_one_entry() {
        let tools = accept_tools(
            &ServerName::from("s"),
            vec![tool("read channel", None), tool("read.channel", None)],
        );

        assert_eq!(tools.len(), 1);
    }

    #[test]
    fn the_wire_name_is_never_the_sanitized_one() {
        // The server knows its tool by the name it gave; only the model sees
        // the cleaned-up one.
        let tools = accept_tools(&ServerName::from("s"), vec![tool("Add comment", None)]);

        assert_eq!(tools[0].tool_name, "Add comment");
        assert_eq!(tools[0].qualified_name, "mcp__s__Add_comment");
    }

    #[test]
    fn a_server_named_in_punctuation_alone_is_refused() {
        assert!(!is_usable_server_name(&ServerName::from("...")));
        assert!(!is_usable_server_name(&ServerName::from("")));
        assert!(!is_usable_server_name(&ServerName::from("a__b")));
        assert!(is_usable_server_name(&ServerName::from("claude.ai Slack")));
    }

    /// The alphabet every provider in use accepts.
    fn is_provider_safe(name: &str) -> bool {
        !name.is_empty()
            && name.len() <= MAX_QUALIFIED_NAME_CHARS
            && name.chars().all(|character| {
                character.is_ascii_alphanumeric() || character == '_' || character == '-'
            })
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
        // A name is only refused where it leaves nothing to build tool names
        // from, or where it would make them ambiguous. Anything else is
        // cleaned up at qualification time.
        for value in ["files.v2", "my files", "claude.ai Slack"] {
            assert!(is_usable_server_name(&ServerName::from(value)), "{value}");
        }
        for value in ["", "my__files", "..."] {
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
