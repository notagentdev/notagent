#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::broadcast;

/// Emits one SSE frame to every connected watcher.
pub type SseSend<'a> = &'a (dyn Fn(Value) + Send + Sync);

/// Work a handler defers until a watcher has subscribed.
pub type AfterSubscriber = Box<dyn FnOnce(SseSend<'_>) + Send>;

/// A recorded request.
#[derive(Clone, Debug)]
pub struct TestRequest {
    pub method: String,
    /// Path including the query string, as `request.url` in Node.
    pub path: String,
    pub headers: Vec<(String, String)>,
}

impl TestRequest {
    pub fn header(&self, name: &str) -> Option<String> {
        self.headers
            .iter()
            .find(|(header, _)| header == name)
            .map(|(_, value)| value.clone())
    }

    /// Decoded query parameters of [`Self::path`].
    pub fn query(&self) -> BTreeMap<String, String> {
        let Some((_, query)) = self.path.split_once('?') else {
            return BTreeMap::new();
        };
        query
            .split('&')
            .filter_map(|pair| pair.split_once('='))
            .map(|(name, value)| (decode_query(name), decode_query(value)))
            .collect()
    }
}

fn decode_query(value: &str) -> String {
    let bytes = value.replace('+', " ").into_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or_default();
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                decoded.push(byte);
                index += 3;
                continue;
            }
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

/// What the handler answers with.
pub enum Reply {
    Json {
        body: String,
        after: Option<AfterSubscriber>,
    },
    Status(u16),
    /// Keep the connection open as an event stream.
    Sse,
}

impl Reply {
    pub fn json(value: Value) -> Self {
        Reply::Json {
            body: value.to_string(),
            after: None,
        }
    }

    pub fn status(status: u16) -> Self {
        Reply::Status(status)
    }

    /// Run `after` once a watcher is connected, then 20 ms later — the delay the
    /// connection still being established.
    #[must_use]
    pub fn then_after_subscriber(self, after: AfterSubscriber) -> Self {
        match self {
            Reply::Json { body, .. } => Reply::Json {
                body,
                after: Some(after),
            },
            other => other,
        }
    }
}

type Handler = Arc<dyn Fn(&TestRequest) -> Reply + Send + Sync>;

pub struct TestHttpServer {
    pub base_url: String,
    requests: Arc<Mutex<Vec<TestRequest>>>,
}

impl TestHttpServer {
    /// Serve `handler` on a free loopback port.
    pub async fn start<H>(handler: H) -> Self
    where
        H: Fn(&TestRequest) -> Reply + Send + Sync + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let handler: Handler = Arc::new(handler);
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (events, _) = broadcast::channel::<String>(64);
        let watchers = Arc::new(AtomicUsize::new(0));

        let accept_handler = Arc::clone(&handler);
        let accept_requests = Arc::clone(&requests);
        let accept_events = events.clone();
        let accept_watchers = Arc::clone(&watchers);
        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let handler = Arc::clone(&accept_handler);
                let requests = Arc::clone(&accept_requests);
                let events = accept_events.clone();
                let watchers = Arc::clone(&accept_watchers);
                tokio::spawn(async move {
                    serve(socket, &handler, &requests, &events, &watchers).await;
                });
            }
        });

        TestHttpServer {
            base_url: format!("http://127.0.0.1:{}", address.port()),
            requests,
        }
    }

    /// Every request served so far, in order.
    pub fn requests(&self) -> Vec<TestRequest> {
        self.requests.lock().expect("poisoned").clone()
    }
}

async fn serve(
    mut socket: tokio::net::TcpStream,
    handler: &Handler,
    requests: &Mutex<Vec<TestRequest>>,
    events: &broadcast::Sender<String>,
    watchers: &Arc<AtomicUsize>,
) {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 1024];
    let head = loop {
        let Ok(read) = socket.read(&mut chunk).await else {
            return;
        };
        if read == 0 {
            return;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(end) = find_head_end(&buffer) {
            break String::from_utf8_lossy(&buffer[..end]).into_owned();
        }
    };

    let mut lines = head.split("\r\n");
    let mut request_line = lines.next().unwrap_or_default().split(' ');
    let request = TestRequest {
        method: request_line.next().unwrap_or_default().to_owned(),
        path: request_line.next().unwrap_or_default().to_owned(),
        headers: lines
            .filter_map(|line| line.split_once(':'))
            .map(|(name, value)| (name.to_lowercase(), value.trim().to_owned()))
            .collect(),
    };
    requests.lock().expect("poisoned").push(request.clone());

    match handler(&request) {
        Reply::Status(status) => {
            let _ = socket
                .write_all(
                    format!("HTTP/1.1 {status} \r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                        .as_bytes(),
                )
                .await;
        }
        Reply::Json { body, after } => {
            let _ = socket
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await;
            if let Some(after) = after {
                let events = events.clone();
                let watchers = Arc::clone(watchers);
                tokio::spawn(async move {
                    for _ in 0..2_000 {
                        if watchers.load(Ordering::SeqCst) > 0 {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(1)).await;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    after(&move |value: Value| {
                        let _ = events.send(format!("data: {value}\n\n"));
                    });
                });
            }
        }
        Reply::Sse => {
            let mut stream = events.subscribe();
            let _ = socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: keep-alive\r\n\r\n",
                )
                .await;
            watchers.fetch_add(1, Ordering::SeqCst);
            loop {
                tokio::select! {
                    frame = stream.recv() => {
                        let Ok(frame) = frame else { break };
                        if socket.write_all(frame.as_bytes()).await.is_err() {
                            break;
                        }
                    }
                    read = socket.read(&mut chunk) => {
                        // 0 bytes or an error means the watcher went away.
                        if !matches!(read, Ok(count) if count > 0) {
                            break;
                        }
                    }
                }
            }
            watchers.fetch_sub(1, Ordering::SeqCst);
        }
    }
    let _ = socket.shutdown().await;
}

fn find_head_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}
