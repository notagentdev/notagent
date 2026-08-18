//! MCP server configuration and the trust decision that gates it.
//!
//! Port addition (user decision 2026-08-18, v0.1.22), taken from
//! `../notagent-main-rust`'s `notagent_domain/src/mcp.rs` and following the
//! `.mcp.json` shape Claude Code established.
//!
//! Two files are read: a project-local `.mcp.json` beside the workspace and a
//! user-level one under the agent directory. On a name collision the project's
//! entry wins, because that is the one the person opening the repository chose
//! to see.
//!
//! A project-local file is not honoured until the user accepts it. It names
//! programs to run on their machine with their environment, and the acceptance
//! is bound to a hash of the file's contents, so editing an accepted file asks
//! again rather than inheriting the old answer.
//!
//! Scope: this is a client and nothing else. It connects to servers, discovers
//! their tools and calls them. MCP's prompts, resources, sampling and
//! elicitation are not implemented — a server offering them is used for its
//! tools and the rest is ignored, which is what both references do. Lending
//! this agent's own tools out over MCP is the opposite direction and a separate
//! feature; nothing here serves anything.

pub mod auth;
pub mod call;
pub mod client;
pub mod manager;
pub mod output;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::{CONFIG_DIR_NAME, get_agent_dir};

/// The file both configuration locations use.
pub const MCP_CONFIG_FILE: &str = ".mcp.json";

/// How long an operation against a server may take when its config says
/// nothing. The reference documents 300 seconds for this and never applies it;
/// here it is the default the deadline actually uses.
pub const DEFAULT_MCP_TIMEOUT_SECS: u64 = 300;

/// A server's name, as the config file spells it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
pub struct ServerName(String);

impl ServerName {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ServerName {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl From<&str> for ServerName {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl From<String> for ServerName {
    fn from(value: String) -> Self {
        Self(value)
    }
}

/// A configured server: a program to spawn, or a URL to reach.
///
/// Untagged, because `.mcp.json` distinguishes the two by which keys are
/// present rather than by a discriminator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum McpServerConfig {
    Stdio(McpStdioServer),
    Http(McpHttpServer),
}

impl McpServerConfig {
    pub fn new_stdio(
        command: impl Into<String>,
        args: Vec<String>,
        env: Option<BTreeMap<String, String>>,
    ) -> Self {
        Self::Stdio(McpStdioServer {
            command: command.into(),
            args,
            env: env.unwrap_or_default(),
            timeout: None,
            disable: false,
        })
    }

    pub fn new_http(url: impl Into<String>) -> Self {
        Self::Http(McpHttpServer {
            url: url.into(),
            headers: BTreeMap::new(),
            timeout: None,
            disable: false,
            oauth: McpOAuthSetting::AutoDetect,
        })
    }

    pub fn is_disabled(&self) -> bool {
        match self {
            Self::Stdio(server) => server.disable,
            Self::Http(server) => server.disable,
        }
    }

    /// How the UI names the transport.
    pub fn transport(&self) -> &'static str {
        match self {
            Self::Stdio(_) => "stdio",
            Self::Http(_) => "http",
        }
    }

    /// The deadline for this server's operations.
    pub fn timeout(&self) -> std::time::Duration {
        let seconds = match self {
            Self::Stdio(server) => server.timeout,
            Self::Http(server) => server.timeout,
        };
        std::time::Duration::from_secs(seconds.unwrap_or(DEFAULT_MCP_TIMEOUT_SECS))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct McpStdioServer {
    /// The program to run.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub command: String,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,

    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,

    /// Deadline in seconds for this server's operations; absent means
    /// [`DEFAULT_MCP_TIMEOUT_SECS`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,

    /// Switch the server off without removing it from the file.
    #[serde(default)]
    pub disable: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct McpHttpServer {
    /// The server's URL. Streamable HTTP is tried first, SSE second.
    #[serde(skip_serializing_if = "String::is_empty", alias = "serverUrl")]
    pub url: String,

    /// Headers sent with every request. Values may reference environment
    /// variables as `{{.env.NAME}}`, so a token stays out of a file that gets
    /// committed.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,

    #[serde(default)]
    pub disable: bool,

    /// Absent means auto-detect, `false` disables, an object configures.
    #[serde(
        default,
        skip_serializing_if = "McpOAuthSetting::is_default",
        deserialize_with = "McpOAuthSetting::deserialize_flexible",
        serialize_with = "McpOAuthSetting::serialize_flexible"
    )]
    pub oauth: McpOAuthSetting,
}

impl McpHttpServer {
    pub fn is_oauth_disabled(&self) -> bool {
        matches!(self.oauth, McpOAuthSetting::Disabled)
    }

    pub fn oauth_config(&self) -> Option<&McpOAuthConfig> {
        match &self.oauth {
            McpOAuthSetting::Configured(config) => Some(config),
            _ => None,
        }
    }
}

/// What the `oauth` key says, in its three possible shapes.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum McpOAuthSetting {
    /// No `oauth` key: try unauthenticated, fall back on a 401.
    #[default]
    AutoDetect,
    /// `oauth: false`: never attempt OAuth, use headers instead.
    Disabled,
    /// `oauth: { … }`: use exactly this.
    Configured(McpOAuthConfig),
}

impl McpOAuthSetting {
    pub fn is_default(&self) -> bool {
        matches!(self, Self::AutoDetect)
    }

    /// Accepts `true` and an absent value as auto-detect, `false` as disabled,
    /// and an object as an explicit configuration.
    fn deserialize_flexible<'de, D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de;

        struct Visitor;

        impl<'de> de::Visitor<'de> for Visitor {
            type Value = McpOAuthSetting;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a boolean or an OAuth config object")
            }

            fn visit_bool<E: de::Error>(self, value: bool) -> Result<Self::Value, E> {
                Ok(if value {
                    McpOAuthSetting::AutoDetect
                } else {
                    McpOAuthSetting::Disabled
                })
            }

            fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(McpOAuthSetting::AutoDetect)
            }

            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(McpOAuthSetting::AutoDetect)
            }

            fn visit_map<M: de::MapAccess<'de>>(self, map: M) -> Result<Self::Value, M::Error> {
                McpOAuthConfig::deserialize(de::value::MapAccessDeserializer::new(map))
                    .map(McpOAuthSetting::Configured)
            }
        }

        deserializer.deserialize_any(Visitor)
    }

    fn serialize_flexible<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::AutoDetect => serializer.serialize_none(),
            Self::Disabled => serializer.serialize_bool(false),
            Self::Configured(config) => config.serialize(serializer),
        }
    }
}

/// An explicit OAuth configuration. Everything absent is discovered from the
/// server's metadata.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpOAuthConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scopes: Vec<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_url: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_url: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redirect_uri: Option<String>,
}

/// One `.mcp.json` file's contents.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpConfig {
    #[serde(default)]
    pub mcp_servers: BTreeMap<ServerName, McpServerConfig>,
}

impl McpConfig {
    pub fn is_empty(&self) -> bool {
        self.mcp_servers.is_empty()
    }

    /// Merges `other` underneath this one: entries already here win.
    pub fn merge_under(&mut self, other: McpConfig) {
        for (name, config) in other.mcp_servers {
            self.mcp_servers.entry(name).or_insert(config);
        }
    }

    /// A stable identifier for these contents.
    ///
    /// Stable across restarts, which the trust store depends on: a hasher with
    /// a per-process seed would make every remembered decision expire on
    /// restart. The `BTreeMap` gives the serialization a fixed order.
    pub fn content_hash(&self) -> u64 {
        use sha2::{Digest, Sha256};

        let canonical = serde_json::to_vec(self).unwrap_or_default();
        let digest = Sha256::digest(&canonical);
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&digest[..8]);
        u64::from_be_bytes(bytes)
    }
}

/// Where a configuration came from, which decides whether it needs trusting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpConfigScope {
    /// The user's own file under the agent directory. Always honoured.
    User,
    /// A file inside the workspace. Honoured only once accepted.
    Project,
}

/// A configuration file that was found and parsed.
#[derive(Debug, Clone, PartialEq)]
pub struct McpConfigFile {
    pub path: PathBuf,
    pub scope: McpConfigScope,
    pub config: McpConfig,
}

/// Why a configuration file could not be used.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum McpConfigError {
    #[error("could not read {path}: {message}")]
    Read { path: String, message: String },
    #[error("{path} is not valid MCP configuration: {message}")]
    Parse { path: String, message: String },
}

/// Reads one configuration file, or `None` when it does not exist.
///
/// A file that exists and does not parse is an error rather than an empty
/// config: a typo that silently switches off every server is worse than being
/// told about it.
pub fn read_mcp_config(
    path: &Path,
    scope: McpConfigScope,
) -> Result<Option<McpConfigFile>, McpConfigError> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(McpConfigError::Read {
                path: path.display().to_string(),
                message: error.to_string(),
            });
        }
    };
    let config: McpConfig = serde_json::from_str(&raw).map_err(|error| McpConfigError::Parse {
        path: path.display().to_string(),
        message: error.to_string(),
    })?;
    Ok(Some(McpConfigFile {
        path: path.to_path_buf(),
        scope,
        config,
    }))
}

/// The two files, project first.
pub fn mcp_config_paths(cwd: &Path) -> [(PathBuf, McpConfigScope); 2] {
    [
        (cwd.join(MCP_CONFIG_FILE), McpConfigScope::Project),
        (get_agent_dir().join(MCP_CONFIG_FILE), McpConfigScope::User),
    ]
}

/// Reads both files. The project one comes first, so a caller merging in order
/// gets the project's entry on a name collision.
pub fn read_all_mcp_configs(cwd: &Path) -> Result<Vec<McpConfigFile>, McpConfigError> {
    let mut found = Vec::new();
    for (path, scope) in mcp_config_paths(cwd) {
        if let Some(file) = read_mcp_config(&path, scope)? {
            found.push(file);
        }
    }
    Ok(found)
}

/// Resolves `{{.env.NAME}}` in a header value against the given variables.
///
/// An unknown variable leaves its reference in place rather than producing an
/// empty header: a request that goes out with a literal placeholder fails
/// visibly, where one with an empty credential fails as an authorization error
/// that reads like the server's fault.
pub fn resolve_env_template(value: &str, env: &BTreeMap<String, String>) -> String {
    let mut resolved = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find("{{") {
        resolved.push_str(&rest[..start]);
        let after = &rest[start..];
        let Some(end) = after.find("}}") else {
            // No closing braces: the remainder is literal text.
            resolved.push_str(after);
            return resolved;
        };
        let name = after[2..end].trim().strip_prefix(".env.");
        match name.and_then(|name| env.get(name)) {
            Some(replacement) => resolved.push_str(replacement),
            None => resolved.push_str(&after[..end + 2]),
        }
        rest = &after[end + 2..];
    }
    resolved.push_str(rest);
    resolved
}

/// Applies [`resolve_env_template`] to every header of an HTTP server.
pub fn resolve_headers(server: &McpHttpServer, env: &BTreeMap<String, String>) -> McpHttpServer {
    let mut resolved = server.clone();
    for value in resolved.headers.values_mut() {
        *value = resolve_env_template(value, env);
    }
    resolved
}

/// What the user answered about a project-local configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpTrustResponse {
    /// Run these servers, and remember that for this exact file.
    Accept,
    /// Run none of them.
    Reject,
}

/// Remembered trust decisions, keyed by path and bound to a content hash.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct McpTrustStore {
    #[serde(default)]
    trusted: BTreeMap<String, u64>,
    #[serde(default)]
    rejected: BTreeMap<String, u64>,
}

impl McpTrustStore {
    pub fn is_trusted(&self, path: &Path, content_hash: u64) -> bool {
        self.trusted
            .get(&path.to_string_lossy().into_owned())
            .is_some_and(|&stored| stored == content_hash)
    }

    pub fn is_rejected(&self, path: &Path, content_hash: u64) -> bool {
        self.rejected
            .get(&path.to_string_lossy().into_owned())
            .is_some_and(|&stored| stored == content_hash)
    }

    /// Records an acceptance, clearing any earlier rejection of the same file.
    pub fn remember(&mut self, path: PathBuf, content_hash: u64) {
        let key = path.to_string_lossy().into_owned();
        self.rejected.remove(&key);
        self.trusted.insert(key, content_hash);
    }

    /// Records a rejection, clearing any earlier acceptance of the same file.
    pub fn reject(&mut self, path: PathBuf, content_hash: u64) {
        let key = path.to_string_lossy().into_owned();
        self.trusted.remove(&key);
        self.rejected.insert(key, content_hash);
    }

    /// Whether this file still needs an answer from the user.
    pub fn needs_decision(&self, file: &McpConfigFile) -> bool {
        if file.scope == McpConfigScope::User || file.config.is_empty() {
            return false;
        }
        let hash = file.config.content_hash();
        !self.is_trusted(&file.path, hash) && !self.is_rejected(&file.path, hash)
    }

    /// Whether this file's servers may run.
    pub fn allows(&self, file: &McpConfigFile) -> bool {
        match file.scope {
            McpConfigScope::User => true,
            McpConfigScope::Project => self.is_trusted(&file.path, file.config.content_hash()),
        }
    }
}

/// Where the trust store is kept.
pub fn mcp_trust_store_path() -> PathBuf {
    get_agent_dir().join("mcp-trust.json")
}

/// Reads the trust store, treating an unreadable or unparsable one as empty.
///
/// Empty means every project-local file is asked about again, which is the safe
/// direction to fail in.
pub fn load_mcp_trust_store() -> McpTrustStore {
    std::fs::read_to_string(mcp_trust_store_path())
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

pub fn save_mcp_trust_store(store: &McpTrustStore) -> std::io::Result<()> {
    let path = mcp_trust_store_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let serialized = serde_json::to_vec_pretty(store)?;
    std::fs::write(path, serialized)
}

/// The workspace-local directory name, for callers building a config path.
pub fn workspace_config_dir(cwd: &Path) -> PathBuf {
    cwd.join(CONFIG_DIR_NAME)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    // ---- the schema --------------------------------------------------------

    #[test]
    fn a_stdio_and_an_http_server_are_told_apart_by_their_keys() {
        let raw = r#"{
            "mcpServers": {
                "local": { "command": "node", "args": ["server.js"] },
                "remote": { "url": "https://example.test/mcp" }
            }
        }"#;
        let config: McpConfig = serde_json::from_str(raw).expect("parses");

        let local = &config.mcp_servers[&ServerName::from("local")];
        let remote = &config.mcp_servers[&ServerName::from("remote")];

        assert_eq!(local.transport(), "stdio");
        assert_eq!(remote.transport(), "http");
    }

    #[test]
    fn the_oauth_key_reads_in_all_three_shapes() {
        let raw = r#"{
            "mcpServers": {
                "auto": { "url": "https://a.test" },
                "off": { "url": "https://b.test", "oauth": false },
                "on": { "url": "https://c.test", "oauth": true },
                "explicit": {
                    "url": "https://d.test",
                    "oauth": { "clientId": "abc", "scopes": ["read"] }
                }
            }
        }"#;
        let config: McpConfig = serde_json::from_str(raw).expect("parses");

        let setting = |name: &str| match &config.mcp_servers[&ServerName::from(name)] {
            McpServerConfig::Http(server) => server.oauth.clone(),
            McpServerConfig::Stdio(_) => panic!("{name} is an HTTP server"),
        };

        assert_eq!(setting("auto"), McpOAuthSetting::AutoDetect);
        assert_eq!(setting("off"), McpOAuthSetting::Disabled);
        assert_eq!(setting("on"), McpOAuthSetting::AutoDetect);
        assert_eq!(
            setting("explicit"),
            McpOAuthSetting::Configured(McpOAuthConfig {
                client_id: Some("abc".to_owned()),
                scopes: vec!["read".to_owned()],
                ..McpOAuthConfig::default()
            })
        );
    }

    #[test]
    fn an_absent_timeout_falls_back_to_the_documented_default() {
        let configured = McpServerConfig::Stdio(McpStdioServer {
            command: "node".to_owned(),
            timeout: Some(5),
            ..McpStdioServer::default()
        });
        let bare = McpServerConfig::new_stdio("node", Vec::new(), None);

        assert_eq!(configured.timeout(), std::time::Duration::from_secs(5));
        assert_eq!(
            bare.timeout(),
            std::time::Duration::from_secs(DEFAULT_MCP_TIMEOUT_SECS)
        );
    }

    #[test]
    fn an_unknown_key_is_rejected_rather_than_ignored() {
        let raw = r#"{ "mcpServers": {}, "mcpServerz": {} }"#;
        assert!(serde_json::from_str::<McpConfig>(raw).is_err());
    }

    // ---- merging -----------------------------------------------------------

    #[test]
    fn the_project_entry_wins_a_name_collision() {
        let mut project = McpConfig {
            mcp_servers: BTreeMap::from([(
                ServerName::from("shared"),
                McpServerConfig::new_http("https://project.test"),
            )]),
        };
        let user = McpConfig {
            mcp_servers: BTreeMap::from([
                (
                    ServerName::from("shared"),
                    McpServerConfig::new_http("https://user.test"),
                ),
                (
                    ServerName::from("only-user"),
                    McpServerConfig::new_http("https://other.test"),
                ),
            ]),
        };

        project.merge_under(user);

        assert_eq!(
            project.mcp_servers[&ServerName::from("shared")],
            McpServerConfig::new_http("https://project.test")
        );
        assert!(
            project
                .mcp_servers
                .contains_key(&ServerName::from("only-user"))
        );
    }

    // ---- the content hash --------------------------------------------------

    #[test]
    fn the_hash_ignores_insertion_order_and_follows_content() {
        let first = McpConfig {
            mcp_servers: BTreeMap::from([
                (
                    ServerName::from("a"),
                    McpServerConfig::new_http("https://a"),
                ),
                (
                    ServerName::from("z"),
                    McpServerConfig::new_http("https://z"),
                ),
            ]),
        };
        let same_in_other_order = McpConfig {
            mcp_servers: BTreeMap::from([
                (
                    ServerName::from("z"),
                    McpServerConfig::new_http("https://z"),
                ),
                (
                    ServerName::from("a"),
                    McpServerConfig::new_http("https://a"),
                ),
            ]),
        };
        let changed = McpConfig {
            mcp_servers: BTreeMap::from([
                (
                    ServerName::from("a"),
                    McpServerConfig::new_http("https://a"),
                ),
                (
                    ServerName::from("z"),
                    McpServerConfig::new_http("https://z-changed"),
                ),
            ]),
        };

        assert_eq!(first.content_hash(), same_in_other_order.content_hash());
        assert_ne!(first.content_hash(), changed.content_hash());
    }

    // ---- header templates --------------------------------------------------

    #[test]
    fn a_header_template_reads_the_environment() {
        let variables = env(&[("TOKEN", "s3cret")]);
        assert_eq!(
            resolve_env_template("Bearer {{.env.TOKEN}}", &variables),
            "Bearer s3cret"
        );
        assert_eq!(
            resolve_env_template("{{.env.TOKEN}}/{{.env.TOKEN}}", &variables),
            "s3cret/s3cret"
        );
    }

    #[test]
    fn an_unknown_variable_leaves_its_reference_standing() {
        // An empty credential would fail as an authorization error that reads
        // like the server's fault; the literal placeholder fails visibly.
        let variables = env(&[]);
        assert_eq!(
            resolve_env_template("Bearer {{.env.MISSING}}", &variables),
            "Bearer {{.env.MISSING}}"
        );
    }

    #[test]
    fn text_that_is_not_a_template_survives_unchanged() {
        let variables = env(&[("TOKEN", "s3cret")]);
        for value in ["plain", "{{ unclosed", "{{.other.TOKEN}}", "{}"] {
            assert_eq!(resolve_env_template(value, &variables), value, "{value}");
        }
    }

    #[test]
    fn resolving_headers_touches_only_the_values() {
        let mut server = McpHttpServer {
            url: "https://a.test".to_owned(),
            ..McpHttpServer::default()
        };
        server
            .headers
            .insert("Authorization".to_owned(), "Bearer {{.env.T}}".to_owned());

        let resolved = resolve_headers(&server, &env(&[("T", "abc")]));

        assert_eq!(resolved.headers["Authorization"], "Bearer abc");
        assert_eq!(resolved.url, server.url);
    }

    // ---- trust -------------------------------------------------------------

    fn project_file(hash_source: &str) -> McpConfigFile {
        McpConfigFile {
            path: PathBuf::from("/workspace/.mcp.json"),
            scope: McpConfigScope::Project,
            config: McpConfig {
                mcp_servers: BTreeMap::from([(
                    ServerName::from("s"),
                    McpServerConfig::new_http(hash_source),
                )]),
            },
        }
    }

    #[test]
    fn a_project_file_is_not_allowed_until_it_is_accepted() {
        let file = project_file("https://a.test");
        let mut store = McpTrustStore::default();

        assert!(store.needs_decision(&file));
        assert!(!store.allows(&file));

        store.remember(file.path.clone(), file.config.content_hash());

        assert!(!store.needs_decision(&file));
        assert!(store.allows(&file));
    }

    #[test]
    fn editing_an_accepted_file_asks_again() {
        let file = project_file("https://a.test");
        let mut store = McpTrustStore::default();
        store.remember(file.path.clone(), file.config.content_hash());

        let edited = project_file("https://somewhere-else.test");

        assert!(store.needs_decision(&edited));
        assert!(!store.allows(&edited));
    }

    #[test]
    fn a_rejection_is_remembered_and_reversible() {
        let file = project_file("https://a.test");
        let mut store = McpTrustStore::default();

        store.reject(file.path.clone(), file.config.content_hash());
        assert!(!store.needs_decision(&file), "a rejection is an answer");
        assert!(!store.allows(&file));

        store.remember(file.path.clone(), file.config.content_hash());
        assert!(store.allows(&file));
    }

    #[test]
    fn the_user_file_needs_no_decision() {
        let file = McpConfigFile {
            path: PathBuf::from("/home/user/.notagent/.mcp.json"),
            scope: McpConfigScope::User,
            config: project_file("https://a.test").config,
        };
        let store = McpTrustStore::default();

        assert!(!store.needs_decision(&file));
        assert!(store.allows(&file));
    }

    #[test]
    fn an_empty_project_file_needs_no_decision() {
        let file = McpConfigFile {
            path: PathBuf::from("/workspace/.mcp.json"),
            scope: McpConfigScope::Project,
            config: McpConfig::default(),
        };
        assert!(!McpTrustStore::default().needs_decision(&file));
    }

    // ---- reading files -----------------------------------------------------

    #[test]
    fn a_missing_file_is_not_an_error() {
        let actual = read_mcp_config(
            Path::new("/definitely/not/here/.mcp.json"),
            McpConfigScope::Project,
        );
        assert_eq!(actual, Ok(None));
    }

    #[test]
    fn a_file_that_does_not_parse_is_an_error_naming_it() {
        let directory = tempfile::Builder::new()
            .prefix("notagent-mcp-config-")
            .tempdir()
            .expect("temp dir");
        let path = directory.path().join(MCP_CONFIG_FILE);
        std::fs::write(&path, "{ not json").expect("write");

        let error = read_mcp_config(&path, McpConfigScope::Project)
            .expect_err("a broken file is reported, not silently emptied");

        assert!(
            matches!(&error, McpConfigError::Parse { path: named, .. } if named.contains(".mcp.json")),
            "{error}"
        );
    }

    #[test]
    fn a_file_that_parses_carries_its_path_and_scope() {
        let directory = tempfile::Builder::new()
            .prefix("notagent-mcp-config-")
            .tempdir()
            .expect("temp dir");
        let path = directory.path().join(MCP_CONFIG_FILE);
        std::fs::write(&path, r#"{"mcpServers":{"s":{"url":"https://a.test"}}}"#).expect("write");

        let file = read_mcp_config(&path, McpConfigScope::Project)
            .expect("reads")
            .expect("exists");

        assert_eq!(file.scope, McpConfigScope::Project);
        assert_eq!(file.path, path);
        assert_eq!(file.config.mcp_servers.len(), 1);
    }
}
