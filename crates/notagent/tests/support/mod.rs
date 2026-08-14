//! Shared test helpers.
//!
//! `vi.spyOn(globalThis, "fetch")` has no Rust counterpart, so the suites that pin
//! HTTP behavior run against a real loopback server instead (deviation class 3).

#![allow(dead_code)]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Notify;

/// One canned HTTP response.
#[derive(Clone, Debug)]
pub struct CannedResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
    /// When set, the response is only written after [`TestServer::release`].
    pub hold: bool,
}

impl CannedResponse {
    pub fn json(body: impl Into<String>) -> Self {
        CannedResponse {
            status: 200,
            headers: vec![("content-type".to_owned(), "application/json".to_owned())],
            body: body.into(),
            hold: false,
        }
    }

    pub fn status(status: u16, body: impl Into<String>) -> Self {
        CannedResponse {
            status,
            headers: Vec::new(),
            body: body.into(),
            hold: false,
        }
    }

    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }

    pub fn held(mut self) -> Self {
        self.hold = true;
        self
    }
}

/// A recorded request: method, path and headers (names lowercased).
#[derive(Clone, Debug)]
pub struct RecordedRequest {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
}

impl RecordedRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header == name)
            .map(|(_, value)| value.as_str())
    }
}

struct ServerState {
    responses: VecDeque<CannedResponse>,
    fallback: CannedResponse,
    requests: Vec<RecordedRequest>,
}

pub struct TestServer {
    pub base_url: String,
    state: Arc<Mutex<ServerState>>,
    release: Arc<Notify>,
}

impl TestServer {
    /// Serve `responses` in order; once exhausted, `fallback` repeats.
    pub async fn start(responses: Vec<CannedResponse>, fallback: CannedResponse) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("addr");
        let state = Arc::new(Mutex::new(ServerState {
            responses: responses.into(),
            fallback,
            requests: Vec::new(),
        }));
        let release = Arc::new(Notify::new());
        let accept_state = Arc::clone(&state);
        let accept_release = Arc::clone(&release);
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let state = Arc::clone(&accept_state);
                let release = Arc::clone(&accept_release);
                tokio::spawn(async move {
                    let mut buffer = Vec::new();
                    let mut chunk = [0_u8; 1024];
                    loop {
                        let Ok(read) = socket.read(&mut chunk).await else {
                            return;
                        };
                        if read == 0 {
                            return;
                        }
                        buffer.extend_from_slice(&chunk[..read]);
                        if buffer.windows(4).any(|window| window == b"\r\n\r\n") {
                            break;
                        }
                    }
                    let text = String::from_utf8_lossy(&buffer).into_owned();
                    let mut lines = text.split("\r\n");
                    let request_line = lines.next().unwrap_or_default();
                    let mut parts = request_line.split(' ');
                    let method = parts.next().unwrap_or_default().to_owned();
                    let path = parts.next().unwrap_or_default().to_owned();
                    let headers: Vec<(String, String)> = lines
                        .take_while(|line| !line.is_empty())
                        .filter_map(|line| {
                            line.split_once(": ")
                                .map(|(name, value)| (name.to_lowercase(), value.to_owned()))
                        })
                        .collect();
                    let response = {
                        let mut state = state.lock().expect("server state");
                        state.requests.push(RecordedRequest {
                            method,
                            path,
                            headers,
                        });
                        state
                            .responses
                            .pop_front()
                            .unwrap_or_else(|| state.fallback.clone())
                    };
                    if response.hold {
                        release.notified().await;
                    }
                    let mut head = format!(
                        "HTTP/1.1 {} {}\r\ncontent-length: {}\r\nconnection: close\r\n",
                        response.status,
                        reason(response.status),
                        response.body.len()
                    );
                    for (name, value) in &response.headers {
                        head.push_str(&format!("{name}: {value}\r\n"));
                    }
                    head.push_str("\r\n");
                    head.push_str(&response.body);
                    let _ = socket.write_all(head.as_bytes()).await;
                    let _ = socket.flush().await;
                });
            }
        });
        TestServer {
            base_url: format!("http://{address}"),
            state,
            release,
        }
    }

    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.state.lock().expect("server state").requests.clone()
    }

    pub fn request_count(&self) -> usize {
        self.state.lock().expect("server state").requests.len()
    }

    /// Let every held response through.
    pub fn release(&self) {
        self.release.notify_waiters();
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        304 => "Not Modified",
        404 => "Not Found",
        429 => "Too Many Requests",
        501 => "Not Implemented",
        _ => "Status",
    }
}
