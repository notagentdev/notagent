//! Injizierbare HTTP-Schicht (`ProviderRequestOptions.fetch`).
//!
//! Substitution Klasse 3 (Master-Plan: Provider-SDKs → reqwest): In TS ist `fetch`
//! die WHATWG-`fetch`-Funktion, die Tests durch eigene Implementierungen ersetzen.
//! In Rust tritt an ihre Stelle dieses Trait-Objekt; die Default-Implementierung
//! benutzt reqwest (siehe Task 8).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tokio::sync::mpsc::Receiver;

/// HTTP-Anfrage, wie sie an [`FetchFn`] übergeben wird.
#[derive(Debug, Clone, Default)]
pub struct FetchRequest {
    pub method: String,
    pub url: String,
    /// Reihenfolge-erhaltend wie `Headers` in JS.
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
}

/// Antwort-Körper: entweder vollständig gepuffert oder als Chunk-Strom (SSE).
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

/// Transportfehler (TS: geworfene `TypeError`/Netzwerkfehler aus `fetch`).
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

type FetchFuture = Pin<Box<dyn Future<Output = Result<FetchResponse, FetchError>> + Send>>;

/// Ersatz für `FetchFunction = typeof globalThis.fetch`.
pub trait FetchFn: Send + Sync {
    fn fetch(&self, request: FetchRequest) -> FetchFuture;
}

/// `fetch?: FetchFunction`
pub type FetchFunction = Arc<dyn FetchFn>;
