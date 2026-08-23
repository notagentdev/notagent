# notagent-index

Code symbol indexing using tree-sitter.

Parses source files into symbols, chunks them for retrieval, and answers
search queries with a BM25 ranking.

- `parser.rs` / `symbol.rs` — tree-sitter parsing into a symbol table across
  multiple languages
- `chunking.rs` — splitting files into retrievable chunks
- `bm25.rs` / `search.rs` / `query.rs` — scoring and query execution
- `persistence.rs` — the on-disk index
- `watcher.rs` — keeping the index fresh as files change
