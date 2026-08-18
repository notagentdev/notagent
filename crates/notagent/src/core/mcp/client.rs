//! One connection to one MCP server (port addition, v0.1.22).
//!
//! Adapted from `../notagent-main-rust`'s `notagent_infra/src/mcp_client.rs`,
//! with the deadlines it documents but never applies.
//!
//! Every request goes out through the SDK's cancellable path with the server's
//! configured deadline attached, so an expiry both frees this side and tells the
//! server the call is gone. The reference uses the plain path throughout and has
//! neither.
//!
//! The connection surface kept here is deliberately narrow — connect, list,
//! call, cancel, close — so an SDK upgrade lands in one file.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use rmcp::model::{
    CallToolRequest, CallToolRequestParam, CallToolResult, ClientInfo, ClientRequest,
    Implementation, ListToolsRequest, ListToolsResult, PaginatedRequestParam,
};
use rmcp::model::{CancelledNotification, CancelledNotificationMethod, CancelledNotificationParam};
use rmcp::service::{PeerRequestOptions, RequestHandle, RunningService, ServiceError};
use rmcp::transport::sse_client::SseClientConfig;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::{SseClientTransport, StreamableHttpClientTransport, TokioChildProcess};
use rmcp::{RoleClient, ServiceExt};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use crate::core::mcp::{McpHttpServer, McpServerConfig, McpStdioServer, resolve_headers};

type Running = RunningService<RoleClient, rmcp::model::InitializeRequestParam>;

/// What went wrong, in the three shapes the caller has to tell apart.
///
/// The distinction is the whole basis of the recovery decision: a server that
/// answered will answer the same way again, an ambiguous failure may be a blip,
/// and a closed transport can only be fixed by reconnecting.
#[derive(Debug, thiserror::Error)]
pub enum McpCallError {
    /// The server answered, with an error or with something unreadable.
    /// Reconnecting cannot change this.
    #[error("{0}")]
    Answered(String),
    /// The deadline for this operation expired.
    #[error("the server did not answer within {0:?}")]
    TimedOut(Duration),
    /// The transport is provably gone.
    #[error("the connection to the server is closed")]
    Closed,
    /// Something failed at the transport that may or may not be fatal.
    #[error("{0}")]
    Ambiguous(String),
    /// The connection could not be established at all.
    #[error("{0}")]
    Connect(String),
    /// The turn was cancelled while the call was in flight.
    #[error("the call was cancelled")]
    Cancelled,
}

impl McpCallError {
    /// Whether reconnecting could plausibly change the outcome.
    pub fn is_transport(&self) -> bool {
        matches!(self, Self::Closed | Self::Ambiguous(_))
    }

    fn from_service(error: ServiceError) -> Self {
        match error {
            ServiceError::Timeout { timeout } => Self::TimedOut(timeout),
            ServiceError::TransportClosed => Self::Closed,
            ServiceError::TransportSend(error) => Self::Ambiguous(error.to_string()),
            // A McpError is the server answering; everything else that is
            // neither a timeout nor a transport fault is treated the same way,
            // because retrying it has no reason to help.
            other => Self::Answered(other.to_string()),
        }
    }
}

/// What the server is told when a turn ends mid-call.
const CANCELLED_REASON: &str = "the client cancelled the turn";

const CLIENT_NAME: &str = "notagent";

fn client_info() -> ClientInfo {
    ClientInfo {
        protocol_version: Default::default(),
        capabilities: Default::default(),
        client_info: Implementation {
            name: CLIENT_NAME.to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            icons: None,
            title: None,
            website_url: None,
        },
    }
}

/// A live connection to one server.
#[derive(Debug)]
pub struct McpConnection {
    service: Arc<Running>,
    timeout: Duration,
}

impl McpConnection {
    /// Connects, bounded by the server's deadline.
    ///
    /// The handshake is inside the deadline because it runs before any tool
    /// call: a server that hangs here would otherwise hang tool discovery for
    /// every later call in the session.
    pub async fn connect(
        config: &McpServerConfig,
        env: &BTreeMap<String, String>,
    ) -> Result<Self, McpCallError> {
        Self::connect_authorized(config, env, None).await
    }

    /// Connects with a bearer token attached to every HTTP request.
    ///
    /// The token goes on the streamable transport only. SSE carries no
    /// authorization header of its own here, so a server that needs a token and
    /// speaks only SSE answers 401 and is reported as needing a login rather
    /// than being retried on a transport that cannot present one.
    pub async fn connect_authorized(
        config: &McpServerConfig,
        env: &BTreeMap<String, String>,
        token: Option<&str>,
    ) -> Result<Self, McpCallError> {
        let timeout = config.timeout();
        let connecting = Self::establish(config, env, token);
        let service = tokio::time::timeout(timeout, connecting)
            .await
            .map_err(|_| McpCallError::TimedOut(timeout))??;
        Ok(Self {
            service: Arc::new(service),
            timeout,
        })
    }

    async fn establish(
        config: &McpServerConfig,
        env: &BTreeMap<String, String>,
        token: Option<&str>,
    ) -> Result<Running, McpCallError> {
        match config {
            McpServerConfig::Stdio(stdio) => Self::establish_stdio(stdio).await,
            McpServerConfig::Http(http) => {
                Self::establish_http(&resolve_headers(http, env), token).await
            }
        }
    }

    async fn establish_stdio(stdio: &McpStdioServer) -> Result<Running, McpCallError> {
        let mut command = Command::new(&stdio.command);
        for (key, value) in &stdio.env {
            command.env(key, value);
        }
        command.args(&stdio.args).kill_on_drop(true);

        let (transport, stderr) = TokioChildProcess::builder(command)
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|error| McpCallError::Connect(error.to_string()))?;

        // A child whose stderr nobody reads blocks once the pipe fills, which
        // looks from here like a server that stopped answering. The lines are
        // drained and dropped: this port has no logger, and the server's own
        // diagnostics are not the agent's output.
        if let Some(stderr) = stderr {
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(_line)) = lines.next_line().await {}
            });
        }

        client_info()
            .serve(transport)
            .await
            .map_err(|error| McpCallError::Connect(error.to_string()))
    }

    async fn establish_http(
        http: &McpHttpServer,
        token: Option<&str>,
    ) -> Result<Running, McpCallError> {
        let client = Self::http_client(http)?;
        let mut streamable_config = StreamableHttpClientTransportConfig::with_uri(http.url.clone());
        if let Some(token) = token {
            streamable_config = streamable_config.auth_header(token);
        }
        let streamable =
            StreamableHttpClientTransport::with_client(client.clone(), streamable_config);
        // Streamable HTTP first, SSE second: a server speaking only the older
        // transport answers the first attempt with a protocol error, not a
        // hang, so the fallback costs one round trip.
        let streamable_failure = match client_info().serve(streamable).await {
            Ok(service) => return Ok(service),
            Err(error) => error.to_string(),
        };
        // A token that was presented and refused is the answer, not a reason to
        // try a transport that cannot present it: the SSE attempt would fail
        // with a connection error and hide the 401 the caller needs to see.
        if token.is_some() && is_unauthorized_text(&streamable_failure) {
            return Err(McpCallError::Connect(streamable_failure));
        }

        let sse = SseClientTransport::start_with_client(
            client,
            SseClientConfig {
                sse_endpoint: http.url.clone().into(),
                ..SseClientConfig::default()
            },
        )
        .await
        .map_err(|error| McpCallError::Connect(error.to_string()))?;
        client_info()
            .serve(sse)
            .await
            .map_err(|error| McpCallError::Connect(error.to_string()))
    }

    fn http_client(http: &McpHttpServer) -> Result<reqwest::Client, McpCallError> {
        let mut headers = reqwest::header::HeaderMap::new();
        for (key, value) in &http.headers {
            let name = reqwest::header::HeaderName::try_from(key.as_str())
                .map_err(|_| McpCallError::Connect(format!("invalid header name `{key}`")))?;
            let value = reqwest::header::HeaderValue::from_str(value)
                .map_err(|_| McpCallError::Connect(format!("invalid value for header `{key}`")))?;
            headers.insert(name, value);
        }
        reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .map_err(|error| McpCallError::Connect(error.to_string()))
    }

    /// Sends one request with this server's deadline attached.
    async fn request(
        &self,
        request: ClientRequest,
    ) -> Result<rmcp::model::ServerResult, McpCallError> {
        self.request_until(request, None).await
    }

    /// Sends one request, giving up when the deadline expires or the turn ends.
    ///
    /// Both endings tell the server, so a call nobody is waiting for stops
    /// rather than running to completion against a client that has moved on.
    /// The deadline is applied here rather than through the SDK's own timeout
    /// so that the two endings take the same path and the notification is sent
    /// exactly once either way.
    async fn request_until(
        &self,
        request: ClientRequest,
        signal: Option<&CancellationToken>,
    ) -> Result<rmcp::model::ServerResult, McpCallError> {
        let handle = self
            .service
            .send_cancellable_request(
                request,
                PeerRequestOptions {
                    timeout: None,
                    meta: None,
                },
            )
            .await
            .map_err(McpCallError::from_service)?;
        // The fields are taken apart because `cancel` consumes the handle and
        // `await_response` borrows it; the notification below is what `cancel`
        // would have sent.
        let RequestHandle {
            mut rx, peer, id, ..
        } = handle;

        let cancelled = async {
            match signal {
                Some(signal) => signal.cancelled().await,
                // Nothing to wait for; the other two arms decide.
                None => std::future::pending().await,
            }
        };

        let (reason, error) = tokio::select! {
            received = &mut rx => {
                return received
                    .map_err(|_| McpCallError::Closed)?
                    .map_err(McpCallError::from_service);
            }
            () = cancelled => (CANCELLED_REASON, McpCallError::Cancelled),
            () = tokio::time::sleep(self.timeout) => (
                RequestHandle::<RoleClient>::REQUEST_TIMEOUT_REASON,
                McpCallError::TimedOut(self.timeout),
            ),
        };

        let notification = CancelledNotification {
            params: CancelledNotificationParam {
                request_id: id,
                reason: Some(reason.to_owned()),
            },
            method: CancelledNotificationMethod,
            extensions: Default::default(),
        };
        let _ = peer.send_notification(notification.into()).await;
        Err(error)
    }

    /// Every tool the server offers, following pagination.
    pub async fn list_tools(&self) -> Result<Vec<rmcp::model::Tool>, McpCallError> {
        let mut tools = Vec::new();
        let mut cursor = None;
        loop {
            let request = ClientRequest::ListToolsRequest(match cursor {
                Some(cursor) => ListToolsRequest::with_param(PaginatedRequestParam {
                    cursor: Some(cursor),
                }),
                None => ListToolsRequest::default(),
            });
            let result = match self.request(request).await? {
                rmcp::model::ServerResult::ListToolsResult(result) => result,
                // The SDK parses the whole result or none of it, so one tool
                // with a malformed schema costs the server all of its tools.
                // That is the server being broken, and saying so is more use
                // than a silently short list.
                _ => {
                    return Err(McpCallError::Answered(
                        "the server's tool list could not be read; one of its tools is malformed"
                            .to_owned(),
                    ));
                }
            };
            let ListToolsResult {
                tools: page,
                next_cursor,
                ..
            } = result;
            tools.extend(page);
            match next_cursor {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        Ok(tools)
    }

    /// Calls one tool.
    pub async fn call_tool(
        &self,
        name: &str,
        arguments: serde_json::Map<String, serde_json::Value>,
    ) -> Result<CallToolResult, McpCallError> {
        self.call_tool_until(name, arguments, None).await
    }

    /// Calls one tool, abandoning it if the turn is cancelled first.
    pub async fn call_tool_until(
        &self,
        name: &str,
        arguments: serde_json::Map<String, serde_json::Value>,
        signal: Option<&CancellationToken>,
    ) -> Result<CallToolResult, McpCallError> {
        let request = ClientRequest::CallToolRequest(CallToolRequest::new(CallToolRequestParam {
            name: name.to_owned().into(),
            arguments: Some(arguments),
        }));
        match self.request_until(request, signal).await? {
            rmcp::model::ServerResult::CallToolResult(result) => Ok(result),
            _ => Err(McpCallError::Answered(
                "the server's answer to a tool call could not be read".to_owned(),
            )),
        }
    }

    /// Whether the connection still answers, used to tell a blip from a death.
    pub async fn is_alive(&self) -> bool {
        let request = ClientRequest::PingRequest(Default::default());
        self.request(request).await.is_ok()
    }

    /// Closes the connection and lets the child be reaped.
    pub async fn close(self) {
        let Ok(service) = Arc::try_unwrap(self.service) else {
            // Another holder is still using it; dropping our handle is all we
            // can do without cancelling their call.
            return;
        };
        let _ = service.cancel().await;
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }
}

/// Whether a failure reads as "log in first".
///
/// The SDK folds the HTTP status into a message rather than surfacing it, so
/// this is a text match. It is deliberately narrow: a false positive would send
/// a user to a login that fixes nothing.
pub fn is_unauthorized_text(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    text.contains("401")
        || text.contains("unauthorized")
        // The SDK says "Auth required"; a server or a proxy may say either of
        // the longer forms. All three are pinned by tests, because this is a
        // text match against a message that is not part of any contract.
        || text.contains("auth required")
        || text.contains("authentication required")
        || text.contains("authorization required")
        || text.contains("invalid_token")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_server_answer_is_not_a_transport_failure() {
        assert!(!McpCallError::Answered("bad request".into()).is_transport());
        assert!(!McpCallError::TimedOut(Duration::from_secs(1)).is_transport());
        assert!(McpCallError::Closed.is_transport());
        assert!(McpCallError::Ambiguous("broken pipe".into()).is_transport());
    }

    #[test]
    fn a_closed_transport_is_told_from_an_ambiguous_send() {
        let closed = McpCallError::from_service(ServiceError::TransportClosed);
        let timeout = McpCallError::from_service(ServiceError::Timeout {
            timeout: Duration::from_secs(2),
        });

        assert!(matches!(closed, McpCallError::Closed));
        assert!(matches!(timeout, McpCallError::TimedOut(_)));
    }

    #[test]
    fn every_wording_a_refused_login_arrives_in_is_recognised() {
        // The first is the SDK's own, seen in `tests/mcp_auth.rs`; the rest are
        // what servers and proxies put in the body of a 401.
        assert!(is_unauthorized_text(
            "Transport error: Auth required, when send initialize request"
        ));
        assert!(is_unauthorized_text("HTTP status 401"));
        assert!(is_unauthorized_text("Unauthorized"));
        assert!(is_unauthorized_text("authentication required"));
        assert!(is_unauthorized_text("authorization required"));
        assert!(is_unauthorized_text("error=\"invalid_token\""));
    }

    #[test]
    fn an_ordinary_failure_is_not_read_as_a_missing_login() {
        // A false positive sends the user to a login that fixes nothing.
        assert!(!is_unauthorized_text("connection refused"));
        assert!(!is_unauthorized_text("HTTP status 500"));
        assert!(!is_unauthorized_text("no such file or directory"));
        assert!(!is_unauthorized_text(
            "the author required a newer protocol"
        ));
    }

    #[tokio::test]
    async fn a_command_that_does_not_exist_fails_to_connect_rather_than_hanging() {
        let config =
            McpServerConfig::new_stdio("notagent-no-such-mcp-server-binary", Vec::new(), None);
        let error = McpConnection::connect(&config, &BTreeMap::new())
            .await
            .expect_err("there is no such program");
        assert!(
            matches!(error, McpCallError::Connect(_) | McpCallError::Closed),
            "{error}"
        );
    }
}
