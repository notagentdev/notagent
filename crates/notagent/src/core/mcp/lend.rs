//! Lending this agent's own tools to an external one (v0.1.22).
//! Taken from `../notagent-main-rust`'s `mcp_lend.rs`, and the transport choice
//! is the load-bearing part of it. MCP's stdio flavour makes the client *spawn*
//! a server, and a freshly spawned process knows nothing of the session the user
//! is sitting in — no permission chain, no leases, no file tracking. Serving
//! over HTTP from inside the live process instead means a borrowed call lands
//! exactly where one of ours does, through the same registry.
//! The endpoint binds to loopback on a port the operating system picks, and
//! every request must carry a token. Both matter: loopback is not a boundary —
//! every process on the machine shares it — and this socket runs our tools with
//! our permissions.
//! One server for the process, one token per session. A server per session dies
//! with its session, and an external agent outliving the endpoint it was started
//! with is exactly the failure that reads as "cannot connect" on every call.
//! What may be lent is not decided here, and neither is whether a borrowed call
//! may run. The session's active tool list decides the first and its permission
//! chain decides the second, both the same ones a turn of ours meets. A second
//! list or a second gate beside them would be a rule to keep in step, and it
//! would drift.
//! That is not a detail. The permission chain is installed on the agent loop
//! (`crate::core::sdk`), not on the tool, so an endpoint that reaches straight
//! for a tool definition meets no gate at all — which is what this module did
//! until v0.1.22, and it meant a borrowed agent could run `bash` and `write`
//! with no approval in a session whose mode required one. The reference says so
//! in as many words: a borrowed call goes through the same door a turn uses "so
//! the permission gate, file tracking and hooks apply unchanged"
//! (`../notagent-main-rust/crates/notagent_app/src/mcp_lend.rs:158`).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

use notagent_agent::types::BoxFuture;
use rmcp::model::{
    CallToolRequestParam, CallToolResult, Content, Implementation, ListToolsResult,
    PaginatedRequestParam, ProtocolVersion, ServerCapabilities, ServerInfo, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler};
use tokio_util::sync::CancellationToken;

use crate::core::tools::tool_definition::ToolDefinition;

/// What a token grants: one session's tools, run in that session.
#[derive(Clone)]
struct Scope {
    tools: Arc<dyn LentToolSource>,
}

/// Where the lent tools come from and how they run.
/// A trait rather than the session itself, so this module does not reach back
/// into the session's internals — and so the tests can lend something small.
/// Running a borrowed tool is the source's job, not this module's. The
/// reference is explicit that a borrowed call goes through the same door a turn
/// of ours uses, "so the permission gate, file tracking and hooks apply
/// unchanged" (`../notagent-main-rust/crates/notagent_app/src/mcp_lend.rs:158`).
/// This module calling `execute` on a definition itself is exactly how that
/// property gets lost: the permission chain is installed on the agent loop
/// (`crates/notagent/src/core/sdk.rs:204`), not on the tool, so a call that
/// reaches past the loop meets no gate at all.
pub trait LentToolSource: Send + Sync {
    /// The tools that may be borrowed right now.
    fn lendable(&self) -> Vec<Arc<dyn ToolDefinition>>;

    /// Runs one of them, through everything a turn's call goes through.
    /// `signal` is the borrowing client's own cancellation, so an agent that
    /// gives up stops the work it started — and so a permission prompt raised
    /// for this call is settled when the asker goes away, rather than waiting
    /// on a turn that may not be running.
    fn call<'a>(
        &'a self,
        tool: &'a str,
        arguments: serde_json::Value,
        signal: CancellationToken,
    ) -> BoxFuture<'a, Result<LentCallOutcome, String>>;
}

/// What running a borrowed tool produced.
#[derive(Debug, Clone, PartialEq)]
pub struct LentCallOutcome {
    pub text: String,
    /// Set when the tool failed or the call was refused. The borrowing agent
    /// reads the text and adapts; it does not lose its connection over it.
    pub is_error: bool,
}

/// The tokens in force and what each grants.
fn scopes() -> &'static Mutex<BTreeMap<String, Scope>> {
    static SCOPES: OnceLock<Mutex<BTreeMap<String, Scope>>> = OnceLock::new();
    SCOPES.get_or_init(Default::default)
}

/// The one endpoint, started on first use and kept for the life of the process.
static ENDPOINT: tokio::sync::OnceCell<String> = tokio::sync::OnceCell::const_new();

/// What an external agent is handed to reach our tools.
pub struct LentToolsEndpoint {
    /// Where it connects.
    pub url: String,
    /// What it must send as `Authorization`.
    pub authorization: String,
    token: String,
}

impl LentToolsEndpoint {
    /// The `.mcp.json` entry an external agent needs to use this.
    pub fn as_config_entry(&self, name: &str) -> String {
        let entry = serde_json::json!({
            "mcpServers": {
                name: {
                    "url": self.url,
                    "headers": { "Authorization": self.authorization },
                }
            }
        });
        serde_json::to_string_pretty(&entry).unwrap_or_default()
    }
}

impl Drop for LentToolsEndpoint {
    fn drop(&mut self) {
        // The session is over; the token stops meaning anything. The endpoint
        // outlives it, so a token that kept working would let a stale agent
        // back in to run our tools.
        if let Ok(mut scopes) = scopes().lock() {
            scopes.remove(&self.token);
        }
    }
}

/// Starts lending `source`'s tools and issues the token that reaches them.
pub async fn lend_tools(source: Arc<dyn LentToolSource>) -> Result<LentToolsEndpoint, String> {
    let url = ENDPOINT.get_or_try_init(serve).await?.clone();
    let token = uuid::Uuid::new_v4().simple().to_string();
    scopes()
        .lock()
        .map_err(|_| "the lent-tool scopes are poisoned".to_owned())?
        .insert(token.clone(), Scope { tools: source });
    Ok(LentToolsEndpoint {
        url,
        authorization: format!("Bearer {token}"),
        token,
    })
}

/// Starts the process-wide endpoint and returns its address.
async fn serve() -> Result<String, String> {
    use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
    use rmcp::transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService,
    };

    // Each request builds its handler from the scope its token resolved to, so
    // one server serves every session without knowing about any of them.
    let service = StreamableHttpService::new(
        || Ok(LentTools { scope: None }),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );

    let app = axum::Router::new()
        .route_service("/mcp", service.clone())
        .nest_service("/mcp/", service)
        .layer(axum::middleware::from_fn(bind_to_session));

    // Loopback only: this runs our tools with our permissions and has no
    // business being reachable from another machine.
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(|error| format!("the lending endpoint could not be started: {error}"))?;
    let address = listener
        .local_addr()
        .map_err(|error| format!("the lending endpoint has no address: {error}"))?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Ok(format!("http://{address}/mcp"))
}

/// Rejects a request without a usable token and hands the rest their scope.
async fn bind_to_session(
    mut request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse as _;

    let Some(scope) = resolve(request.headers()) else {
        return axum::http::StatusCode::UNAUTHORIZED.into_response();
    };
    request.extensions_mut().insert(scope);
    next.run(request).await
}

/// Resolves a request's bearer token to what it grants.
fn resolve(headers: &axum::http::HeaderMap) -> Option<Scope> {
    let presented = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())?
        .strip_prefix("Bearer ")?
        .trim();
    if presented.is_empty() {
        return None;
    }
    scopes().lock().ok()?.get(presented).cloned()
}

/// Our tools, as an MCP server serving every session.
#[derive(Clone)]
struct LentTools {
    /// Unused placeholder so the handler is constructible per connection; the
    /// scope that matters arrives with the request.
    scope: Option<Scope>,
}

impl LentTools {
    fn scope_of(&self, context: &RequestContext<RoleServer>) -> Result<Scope, ErrorData> {
        context
            .extensions
            .get::<axum::http::request::Parts>()
            .and_then(|parts| parts.extensions.get::<Scope>().cloned())
            .or_else(|| self.scope.clone())
            .ok_or_else(|| ErrorData::invalid_request("no session is bound to this request", None))
    }
}

/// Turns one of our tool definitions into the shape MCP advertises.
fn advertised(definition: &Arc<dyn ToolDefinition>) -> Option<Tool> {
    // A schema that is not an object has no parameters MCP can describe, and a
    // tool advertised without them would be called without them.
    let object = definition.parameters().as_object()?.clone();
    Some(Tool {
        name: definition.name().to_owned().into(),
        title: None,
        description: Some(definition.description().to_owned().into()),
        input_schema: Arc::new(object),
        output_schema: None,
        annotations: None,
        icons: None,
        meta: None,
    })
}

impl ServerHandler for LentTools {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::default(),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation {
                name: crate::config::APP_NAME.to_owned(),
                title: None,
                version: env!("CARGO_PKG_VERSION").to_owned(),
                icons: None,
                website_url: None,
            },
            instructions: None,
        }
    }

    /// The tools this session may run, and only those.
    /// Advertising more would invite a call that can only be refused, and a
    /// borrowing agent believes the schema it was given over the error it gets.
    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParam>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let scope = self.scope_of(&context)?;
        let tools = scope
            .tools
            .lendable()
            .iter()
            .filter_map(advertised)
            .collect();
        Ok(ListToolsResult {
            tools,
            next_cursor: None,
        })
    }

    /// Runs one tool in this session.
    /// A tool that fails reports it inside the result rather than as a protocol
    /// error: the borrowing agent should read the message and adapt, not lose
    /// the connection over it.
    async fn call_tool(
        &self,
        request: CallToolRequestParam,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let scope = self.scope_of(&context)?;
        let name = request.name.as_ref();
        // Checked against the same list that was advertised, so a tool the
        // session stopped offering between the two is refused rather than run.
        if !scope
            .tools
            .lendable()
            .iter()
            .any(|definition| definition.name() == name)
        {
            return Err(ErrorData::invalid_params(
                format!("`{name}` is not available"),
                None,
            ));
        }

        let arguments = request
            .arguments
            .map(serde_json::Value::Object)
            .unwrap_or(serde_json::Value::Null);
        // `context.ct` is cancelled when the borrowing client sends a
        // `CancelledNotification`. That is the right thing to stop this call by:
        // the session's own turn has nothing to do with it, and may not exist.
        match scope.tools.call(name, arguments, context.ct.clone()).await {
            Ok(outcome) => {
                let content = vec![Content::text(outcome.text)];
                Ok(match outcome.is_error {
                    true => CallToolResult::error(content),
                    false => CallToolResult::success(content),
                })
            }
            Err(error) => Ok(CallToolResult::error(vec![Content::text(error)])),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoTools;

    impl LentToolSource for NoTools {
        fn lendable(&self) -> Vec<Arc<dyn ToolDefinition>> {
            Vec::new()
        }

        fn call<'a>(
            &'a self,
            tool: &'a str,
            _arguments: serde_json::Value,
            _signal: CancellationToken,
        ) -> BoxFuture<'a, Result<LentCallOutcome, String>> {
            Box::pin(async move { Err(format!("`{tool}` is not available")) })
        }
    }

    fn header(value: Option<&str>) -> axum::http::HeaderMap {
        let mut headers = axum::http::HeaderMap::new();
        if let Some(value) = value {
            headers.insert(
                axum::http::header::AUTHORIZATION,
                value.parse().expect("a usable header value"),
            );
        }
        headers
    }

    #[tokio::test]
    async fn a_token_stops_meaning_anything_once_its_session_ends() {
        // The endpoint outlives every session, so a token that kept working
        // would let a finished agent back in to run our tools.
        let token = "test-token-ends".to_owned();
        scopes().lock().unwrap().insert(
            token.clone(),
            Scope {
                tools: Arc::new(NoTools),
            },
        );
        let endpoint = LentToolsEndpoint {
            url: "http://127.0.0.1:1/mcp".to_owned(),
            authorization: format!("Bearer {token}"),
            token,
        };
        let headers = header(Some(&endpoint.authorization));

        let while_open = resolve(&headers).is_some();
        drop(endpoint);
        let after_close = resolve(&headers).is_some();

        assert_eq!((while_open, after_close), (true, false));
    }

    #[test]
    fn nothing_without_a_usable_token_resolves() {
        // Loopback is not a boundary — every local process shares it.
        for value in [None, Some("Bearer "), Some("Bearer unknown"), Some("token")] {
            assert!(resolve(&header(value)).is_none(), "{value:?}");
        }
    }

    #[test]
    fn the_config_entry_carries_the_url_and_the_token() {
        let endpoint = LentToolsEndpoint {
            url: "http://127.0.0.1:9/mcp".to_owned(),
            authorization: "Bearer abc".to_owned(),
            token: String::new(),
        };

        let entry = endpoint.as_config_entry("notagent");

        assert!(entry.contains("http://127.0.0.1:9/mcp"), "{entry}");
        assert!(entry.contains("Bearer abc"), "{entry}");
        assert!(entry.contains("notagent"), "{entry}");
    }
}
