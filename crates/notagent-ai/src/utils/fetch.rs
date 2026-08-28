use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tokio::sync::mpsc::Receiver;

#[derive(Debug, Clone, Default)]
pub struct FetchRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
}

pub enum FetchBody {
    Bytes(Vec<u8>),
    Stream(Receiver<Result<Vec<u8>, FetchError>>),
}

impl std::fmt::Debug for FetchBody {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchBody::Bytes(bytes) => formatter.debug_tuple("Bytes").field(&bytes.len()).finish(),
            FetchBody::Stream(_) => formatter.write_str("Stream"),
        }
    }
}

/// HTTP-Antwort.
#[derive(Debug)]
pub struct FetchResponse {
    pub status: u16,
    pub status_text: String,
    pub headers: Vec<(String, String)>,
    pub body: FetchBody,
}

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct FetchError {
    pub message: String,
}

impl FetchError {
    pub fn new(message: impl Into<String>) -> Self {
        FetchError {
            message: message.into(),
        }
    }
}

/// Return type of [`FetchFn::fetch`].
pub type FetchFuture = Pin<Box<dyn Future<Output = Result<FetchResponse, FetchError>> + Send>>;

pub trait FetchFn: Send + Sync {
    fn fetch(&self, request: FetchRequest) -> FetchFuture;
}

/// `fetch?: FetchFunction`
pub type FetchFunction = Arc<dyn FetchFn>;

/// The default [`FetchFn`], backed by reqwest.
/// the SSE decoder can consume them incrementally.
pub struct ReqwestFetch {
    client: reqwest::Client,
}

impl ReqwestFetch {
    /// Builds a client with the given timeout and optional proxy.
    pub fn new(
        timeout: Option<std::time::Duration>,
        proxy: Option<&str>,
    ) -> Result<Self, FetchError> {
        let mut builder = reqwest::Client::builder();
        if let Some(timeout) = timeout {
            builder = builder.timeout(timeout);
        }
        if let Some(proxy) = proxy {
            builder = builder.proxy(
                reqwest::Proxy::all(proxy).map_err(|error| FetchError::new(error.to_string()))?,
            );
        }
        Ok(ReqwestFetch {
            client: builder
                .build()
                .map_err(|error| FetchError::new(error.to_string()))?,
        })
    }
}

impl Default for ReqwestFetch {
    fn default() -> Self {
        ReqwestFetch {
            client: reqwest::Client::new(),
        }
    }
}

impl FetchFn for ReqwestFetch {
    fn fetch(&self, request: FetchRequest) -> FetchFuture {
        let client = self.client.clone();
        Box::pin(async move {
            let method = reqwest::Method::from_bytes(request.method.as_bytes())
                .map_err(|error| FetchError::new(error.to_string()))?;
            let mut builder = client.request(method, &request.url);
            for (name, value) in &request.headers {
                builder = builder.header(name, value);
            }
            if let Some(body) = request.body {
                builder = builder.body(body);
            }

            let response = builder
                .send()
                .await
                .map_err(|error| FetchError::new(error.to_string()))?;
            let status = response.status().as_u16();
            let status_text = response
                .status()
                .canonical_reason()
                .unwrap_or("")
                .to_string();
            let headers = response
                .headers()
                .iter()
                .map(|(name, value)| {
                    (
                        name.to_string(),
                        value.to_str().unwrap_or_default().to_string(),
                    )
                })
                .collect();

            // The body is forwarded as a chunk stream; SSE consumers decode incrementally.
            let (sender, receiver) = tokio::sync::mpsc::channel(32);
            tokio::spawn(async move {
                let mut response = response;
                loop {
                    match response.chunk().await {
                        Ok(Some(chunk)) => {
                            if sender.send(Ok(chunk.to_vec())).await.is_err() {
                                break;
                            }
                        }
                        Ok(None) => break,
                        Err(error) => {
                            let _ = sender.send(Err(FetchError::new(error.to_string()))).await;
                            break;
                        }
                    }
                }
            });

            Ok(FetchResponse {
                status,
                status_text,
                headers,
                body: FetchBody::Stream(receiver),
            })
        })
    }
}
