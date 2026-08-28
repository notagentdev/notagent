//! Notagent Index - Code symbol indexing using tree-sitter
//! This crate provides:
//! - Tree-sitter based parsing for multiple languages
//! - Symbol extraction (functions, structs, traits, etc.)
//! - In-memory symbol index with fuzzy search
//! - Persistent cache with incremental updates
//! - File watcher for real-time index updates
//! - BM25 (tantivy) chunk search with ranked results (`cb_search`)

mod bm25;
mod chunking;
mod index;
mod parser;
mod persistence;
mod query;
mod search;
mod symbol;
mod watcher;

pub use bm25::{
    Bm25Hit, Bm25Index, Bm25Schema, ChunkDocument, MAX_CHUNK_BYTES, MAX_CHUNK_LINES, chunk_id,
    is_writer_lock_busy,
};
pub use chunking::{ChunkerOptions, chunk_file};
pub use index::SymbolIndex;
pub use parser::{MultiParser, RustParser, SupportedLanguage};
pub use persistence::{IndexCache, workspace_key};
pub use query::SearchResult;
pub use search::{CbSearchParams, CbSearchResult, cb_search};
pub use symbol::{Symbol, SymbolKind};
pub use watcher::{IndexManager, IndexProgress, IndexWatcher, IndexingPhase};
