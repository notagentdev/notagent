//! Minimal one-route HTTP server for OAuth redirects.
//!
//! Substitution class 3 of the master plan: `node:http.createServer` becomes a small
//! tokio listener. It accepts exactly the redirect route, answers with the success or
//! error page and hands the authorization code back to the flow.

use std::collections::BTreeMap;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::auth::oauth::oauth_page::{oauth_error_html, oauth_success_html};

/// What the redirect delivered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallbackResult {
    pub code: String,
    pub state: String,
}

/// A running callback server.
pub struct CallbackServer {
    pub redirect_uri: String,
    receiver: oneshot::Receiver<Option<CallbackResult>>,
    cancel: CancellationToken,
}

/// Failure while starting the server.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct CallbackServerError(pub String);

/// Configuration of [`start_callback_server`].
pub struct CallbackServerOptions {
    pub host: String,
    pub port: u16,
    pub path: String,
    pub redirect_uri: String,
    pub expected_state: String,
    pub success_message: String,
    /// Prefix of the error page, e.g. "Anthropic authentication did not complete."
    pub error_message: String,
}

impl CallbackServer {
    /// Waits for the redirect; `None` when the wait was cancelled.
    pub async fn wait_for_code(self) -> Option<CallbackResult> {
        self.receiver.await.ok().flatten()
    }

    /// `cancelWait()` — stops waiting, e.g. because a manual code arrived first.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel.clone()
    }
}

/// Starts the server and returns once it is listening.
pub async fn start_callback_server(
    options: CallbackServerOptions,
) -> Result<CallbackServer, CallbackServerError> {
    let listener = TcpListener::bind((options.host.as_str(), options.port))
        .await
        .map_err(|error| CallbackServerError(error.to_string()))?;
    let (sender, receiver) = oneshot::channel();
    let cancel = CancellationToken::new();
    let cancel_child = cancel.clone();
    let redirect_uri = options.redirect_uri.clone();

    tokio::spawn(async move {
        let mut sender = Some(sender);
        loop {
            let accepted = tokio::select! {
                accepted = listener.accept() => accepted,
                _ = cancel_child.cancelled() => {
                    if let Some(sender) = sender.take() {
                        let _ = sender.send(None);
                    }
                    return;
                }
            };
            let Ok((mut socket, _)) = accepted else {
                continue;
            };

            let mut buffer = vec![0u8; 8192];
            let Ok(read) = socket.read(&mut buffer).await else {
                continue;
            };
            let request = String::from_utf8_lossy(&buffer[..read]).to_string();
            let target = request
                .lines()
                .next()
                .and_then(|line| line.split(' ').nth(1))
                .unwrap_or("/");
            let (path, query) = match target.split_once('?') {
                Some((path, query)) => (path, query),
                None => (target, ""),
            };
            let params = parse_query(query);

            let (status, body) = if path != options.path {
                (404, oauth_error_html("Callback route not found.", None))
            } else if let Some(error) = params.get("error") {
                (
                    400,
                    oauth_error_html(&options.error_message, Some(&format!("Error: {error}"))),
                )
            } else {
                match (params.get("code"), params.get("state")) {
                    (None, _) | (_, None) => (
                        400,
                        oauth_error_html("Missing code or state parameter.", None),
                    ),
                    (Some(_), Some(state)) if *state != options.expected_state => {
                        (400, oauth_error_html("State mismatch.", None))
                    }
                    (Some(code), Some(state)) => {
                        let result = CallbackResult {
                            code: code.clone(),
                            state: state.clone(),
                        };
                        if let Some(sender) = sender.take() {
                            let _ = sender.send(Some(result));
                        }
                        (200, oauth_success_html(&options.success_message))
                    }
                }
            };

            let response = format!(
                "HTTP/1.1 {status} {}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                if status == 200 {
                    "OK"
                } else if status == 404 {
                    "Not Found"
                } else {
                    "Bad Request"
                },
                body.len()
            );
            let _ = socket.write_all(response.as_bytes()).await;
            let _ = socket.flush().await;

            if status == 200 {
                return;
            }
        }
    });

    Ok(CallbackServer {
        redirect_uri,
        receiver,
        cancel,
    })
}

/// Percent-decoding query parser; the TS side uses `URL.searchParams`.
fn parse_query(query: &str) -> BTreeMap<String, String> {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .filter_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            Some((percent_decode(key), percent_decode(value)))
        })
        .collect()
}

fn percent_decode(value: &str) -> String {
    let bytes = value.replace('+', " ").into_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("");
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                decoded.push(byte);
                index += 3;
                continue;
            }
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&decoded).to_string()
}
