//! BM25/Tantivy-based index for code chunk search.

use anyhow::{Context, Result};
use fs2::FileExt;
use std::fs::{File, OpenOptions};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use tantivy::TantivyError;
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::{Field, INDEXED, STORED, STRING, Schema, Value};
use tantivy::schema::{IndexRecordOption, TextFieldIndexing, TextOptions};
use tantivy::tokenizer::{Token, TokenStream, Tokenizer};
use tantivy::{Index, IndexReader, IndexWriter, TantivyDocument, Term, doc};
use twox_hash::XxHash64;

/// Hard caps for v1 chunking.
pub const MAX_CHUNK_LINES: usize = 300;
pub const MAX_CHUNK_BYTES: usize = 32 * 1024;
/// Returns whether an error was caused by Tantivy's writer lock being busy.
///
/// # Arguments
/// * `error` - Error chain produced while opening or writing the BM25 index.
pub fn is_writer_lock_busy(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        matches!(
            cause.downcast_ref::<TantivyError>(),
            Some(TantivyError::LockFailure(
                tantivy::directory::error::LockError::LockBusy,
                _
            ))
        )
    })
}
/// Deterministic chunk id (stable across re-index).
pub fn chunk_id(
    path: &str,
    language: &str,
    range_start: usize,
    range_end: usize,
    symbol: &str,
) -> String {
    let mut hasher = XxHash64::default();
    path.hash(&mut hasher);
    language.hash(&mut hasher);
    range_start.hash(&mut hasher);
    range_end.hash(&mut hasher);
    symbol.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

#[derive(Clone, Debug)]
pub struct ChunkDocument {
    pub id: String,
    pub path: String,
    pub language: String,
    pub symbol: String,
    pub kind: String,
    pub range_start: usize,
    pub range_end: usize,
    pub content: String,
    pub tags: Vec<String>,
}

#[derive(Clone)]
pub struct Bm25Schema {
    pub id: Field,
    pub path: Field,
    pub language: Field,
    pub symbol: Field,
    pub kind: Field,
    pub range_start: Field,
    pub range_end: Field,
    pub content: Field,
    pub tags: Field,
}

pub struct Bm25Index {
    index: Index,
    reader: IndexReader,
    schema: Schema,
    fields: Bm25Schema,
    index_path: PathBuf,
}

impl Bm25Index {
    pub fn open_or_create(index_path: impl AsRef<Path>) -> Result<Self> {
        let index_path = index_path.as_ref().to_path_buf();
        std::fs::create_dir_all(&index_path)?;

        let (schema, fields) = Self::build_schema();
        let index = match Index::open_in_dir(&index_path) {
            Ok(idx) => idx,
            Err(_) => Index::create_in_dir(&index_path, schema.clone())?,
        };
        index.tokenizers().register("code", CodeTokenizer);

        let reader = index.reader()?;
        Ok(Self {
            index,
            reader,
            schema,
            fields,
            index_path,
        })
    }

    pub fn index_path(&self) -> &Path {
        &self.index_path
    }

    fn build_schema() -> (Schema, Bm25Schema) {
        let mut builder = Schema::builder();
        let id = builder.add_text_field("id", STRING | STORED);
        let path = builder.add_text_field("path", STRING | STORED);
        let language = builder.add_text_field("language", STRING | STORED);
        let code_indexing = TextFieldIndexing::default()
            .set_tokenizer("code")
            .set_index_option(IndexRecordOption::WithFreqsAndPositions);
        let code_text_options = TextOptions::default()
            .set_indexing_options(code_indexing)
            .set_stored();
        let symbol = builder.add_text_field("symbol", code_text_options.clone());
        let kind = builder.add_text_field("kind", STRING | STORED);
        let range_start = builder.add_u64_field("range_start", STORED | INDEXED);
        let range_end = builder.add_u64_field("range_end", STORED | INDEXED);
        let content = builder.add_text_field("content", code_text_options);
        let tags = builder.add_text_field("tags", STRING | STORED);

        let schema = builder.build();
        let fields = Bm25Schema {
            id,
            path,
            language,
            symbol,
            kind,
            range_start,
            range_end,
            content,
            tags,
        };

        (schema, fields)
    }
    fn writer(&self) -> Result<(File, IndexWriter)> {
        let lock_path = self.index_path.join("notagent-writer.lock");
        let lock = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| format!("Failed to open BM25 writer lock {}", lock_path.display()))?;
        lock.lock_exclusive().with_context(|| {
            format!("Failed to acquire BM25 writer lock {}", lock_path.display())
        })?;
        let writer = self.index.writer(50_000_000)?;
        Ok((lock, writer))
    }
    pub fn add_chunks(&self, chunks: &[ChunkDocument]) -> Result<()> {
        let (_lock, mut writer) = self.writer()?;
        for chunk in chunks {
            let mut document = doc!(
                self.fields.id => chunk.id.clone(),
                self.fields.path => chunk.path.clone(),
                self.fields.language => chunk.language.clone(),
                self.fields.symbol => chunk.symbol.clone(),
                self.fields.kind => chunk.kind.clone(),
                self.fields.range_start => chunk.range_start as u64,
                self.fields.range_end => chunk.range_end as u64,
                self.fields.content => chunk.content.clone(),
            );
            for tag in &chunk.tags {
                document.add_text(self.fields.tags, tag);
            }
            writer.add_document(document)?;
        }
        writer.commit()?;
        Ok(())
    }

    /// Batch delete paths and add chunks in a single lock + single commit.
    pub fn batch_delete_and_add(
        &self,
        delete_paths: &[String],
        chunks: &[ChunkDocument],
    ) -> Result<()> {
        let (_lock, mut writer) = self.writer()?;
        for path in delete_paths {
            let term = Term::from_field_text(self.fields.path, path);
            writer.delete_term(term);
        }
        for chunk in chunks {
            let mut document = doc!(
                self.fields.id => chunk.id.clone(),
                self.fields.path => chunk.path.clone(),
                self.fields.language => chunk.language.clone(),
                self.fields.symbol => chunk.symbol.clone(),
                self.fields.kind => chunk.kind.clone(),
                self.fields.range_start => chunk.range_start as u64,
                self.fields.range_end => chunk.range_end as u64,
                self.fields.content => chunk.content.clone(),
            );
            for tag in &chunk.tags {
                document.add_text(self.fields.tags, tag);
            }
            writer.add_document(document)?;
        }
        writer.commit()?;
        Ok(())
    }

    pub fn delete_by_path(&self, path: &str) -> Result<()> {
        let term = Term::from_field_text(self.fields.path, path);
        let (_lock, mut writer) = self.writer()?;
        writer.delete_term(term);
        writer.commit()?;
        Ok(())
    }

    pub fn delete_by_id(&self, id: &str) -> Result<()> {
        let term = Term::from_field_text(self.fields.id, id);
        let (_lock, mut writer) = self.writer()?;
        writer.delete_term(term);
        writer.commit()?;
        Ok(())
    }

    /// Maintenance for v1: delete/re-add + periodic commit already handled.
    /// Optional compaction can be enabled behind a flag.
    pub fn maintenance_compact(&self, _enable: bool) -> Result<()> {
        Ok(())
    }

    pub fn reader(&self) -> &IndexReader {
        &self.reader
    }

    pub fn schema(&self) -> &Schema {
        &self.schema
    }

    pub fn fields(&self) -> &Bm25Schema {
        &self.fields
    }

    pub fn reload(&self) -> Result<()> {
        self.reader.reload().context("Failed to reload BM25 reader")
    }

    pub fn search(&self, query: &str, max_results: usize) -> Result<Vec<Bm25Hit>> {
        let reader = &self.reader;
        let searcher = reader.searcher();
        let parser = QueryParser::for_index(
            &self.index,
            vec![
                self.fields.content,
                self.fields.symbol,
                self.fields.path,
                self.fields.kind,
            ],
        );
        let (query, _errors) = parser.parse_query_lenient(query);
        let top_docs = searcher.search(&query, &TopDocs::with_limit(max_results))?;

        let mut results = Vec::new();
        for (score, doc_address) in top_docs {
            let retrieved = searcher.doc(doc_address)?;
            let hit = Bm25Hit::from_doc(&self.schema, &self.fields, retrieved, score)?;
            results.push(hit);
        }
        Ok(results)
    }
}

#[derive(Clone)]
struct CodeTokenizer;

impl Tokenizer for CodeTokenizer {
    type TokenStream<'a> = CodeTokenStream;

    fn token_stream<'a>(&mut self, text: &'a str) -> Self::TokenStream<'a> {
        CodeTokenStream::new(text)
    }
}

struct CodeTokenStream {
    tokens: Vec<Token>,
    idx: usize,
}

impl CodeTokenStream {
    fn new(text: &str) -> Self {
        let tokens = split_code_tokens(text);
        Self { tokens, idx: 0 }
    }
}

impl TokenStream for CodeTokenStream {
    fn advance(&mut self) -> bool {
        if self.idx >= self.tokens.len() {
            return false;
        }
        self.idx += 1;
        true
    }

    fn token(&self) -> &Token {
        &self.tokens[self.idx.saturating_sub(1)]
    }

    fn token_mut(&mut self) -> &mut Token {
        &mut self.tokens[self.idx.saturating_sub(1)]
    }
}

/// Splits an identifier into search tokens, on non-alphanumerics, on `_`/`-`
/// and at camelCase humps. Offsets point back at the byte range each token
/// came from — highlighting and snippet extraction read them.
fn split_code_tokens(text: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut token_start = 0usize;
    let mut byte_index = 0usize;
    let mut prev: Option<char> = None;

    let push_current = |tokens: &mut Vec<Token>, current: &mut String, start: usize, end: usize| {
        if current.is_empty() {
            return;
        }
        let token = Token {
            text: current.clone(),
            offset_from: start,
            offset_to: end,
            position: tokens.len(),
            ..Default::default()
        };
        tokens.push(token);
        current.clear();
    };

    for (i, ch) in text.char_indices() {
        let is_boundary = !ch.is_alphanumeric();
        let camel_boundary = prev
            .map(|p| p.is_lowercase() && ch.is_uppercase())
            .unwrap_or(false);
        let underscore_boundary = ch == '_' || ch == '-';

        if current.is_empty() {
            token_start = i;
        }

        if is_boundary || underscore_boundary || camel_boundary {
            push_current(&mut tokens, &mut current, token_start, i);
            if is_boundary {
                prev = None;
                continue;
            }
            if underscore_boundary {
                prev = None;
                continue;
            }
            // A camel boundary does not consume its character — it opens the
            // next token with it. The emptiness check above has already run
            // for this iteration and will not run again while the token
            // grows, so the new start has to be recorded here. Without it
            // every token after the first camel split inherits the previous
            // token's `offset_from`: `WorkspaceLockManager` reported `lock`
            // at 0..13 instead of 9..13.
            token_start = i;
        }

        current.push(ch.to_ascii_lowercase());
        prev = Some(ch);
        byte_index = i + ch.len_utf8();
    }

    push_current(&mut tokens, &mut current, token_start, byte_index);

    // Lowercase filter
    tokens
}

#[derive(Clone, Debug)]
pub struct Bm25Hit {
    pub id: String,
    pub path: String,
    pub language: String,
    pub symbol: String,
    pub kind: String,
    pub range_start: usize,
    pub range_end: usize,
    pub content: String,
    pub score: f32,
}

impl Bm25Hit {
    fn from_doc(
        _schema: &Schema,
        fields: &Bm25Schema,
        doc: TantivyDocument,
        score: f32,
    ) -> Result<Self> {
        let get_text = |field: Field| -> Result<String> {
            doc.get_first(field)
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .context("Missing text field")
        };
        let get_u64 = |field: Field| -> Result<u64> {
            doc.get_first(field)
                .and_then(|v| v.as_u64())
                .context("Missing u64 field")
        };

        Ok(Self {
            id: get_text(fields.id)?,
            path: get_text(fields.path)?,
            language: get_text(fields.language)?,
            symbol: get_text(fields.symbol)?,
            kind: get_text(fields.kind)?,
            range_start: get_u64(fields.range_start)? as usize,
            range_end: get_u64(fields.range_end)? as usize,
            content: get_text(fields.content)?,
            score,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn every_token_points_at_the_bytes_it_came_from() {
        let cases = [
            (
                "WorkspaceLockManager",
                vec![("workspace", 0, 9), ("lock", 9, 13), ("manager", 13, 20)],
            ),
            // Underscores and non-alphanumerics were never affected — the
            // `continue` leaves `current` empty, so the start is recorded on
            // the next pass. Held here so the two paths stay in step.
            (
                "workspace_lock_manager",
                vec![("workspace", 0, 9), ("lock", 10, 14), ("manager", 15, 22)],
            ),
            (
                "fooBar.bazQux",
                vec![
                    ("foo", 0, 3),
                    ("bar", 3, 6),
                    ("baz", 7, 10),
                    ("qux", 10, 13),
                ],
            ),
        ];

        for (input, expected) in cases {
            let tokens = split_code_tokens(input);
            let actual: Vec<_> = tokens
                .iter()
                .map(|t| (t.text.as_str(), t.offset_from, t.offset_to))
                .collect();
            assert_eq!(actual, expected, "tokenizing {input:?}");
            for token in &tokens {
                assert_eq!(
                    input[token.offset_from..token.offset_to].to_ascii_lowercase(),
                    token.text,
                    "the range of {:?} in {input:?} must hold the token itself",
                    token.text
                );
            }
        }
    }

    fn sample_chunk(id: &str, path: &str, symbol: &str, content: &str) -> ChunkDocument {
        ChunkDocument {
            id: id.to_string(),
            path: path.to_string(),
            language: "rust".to_string(),
            symbol: symbol.to_string(),
            kind: "function".to_string(),
            range_start: 1,
            range_end: 5,
            content: content.to_string(),
            tags: vec!["core".to_string()],
        }
    }

    #[test]
    fn chunk_id_is_stable_and_sensitive_to_inputs() {
        let first = chunk_id("src/lib.rs", "rust", 1, 8, "WorkspaceLockManager");
        let second = chunk_id("src/lib.rs", "rust", 1, 8, "WorkspaceLockManager");
        let changed = chunk_id("src/lib.rs", "rust", 1, 9, "WorkspaceLockManager");

        assert_eq!(first, second);
        assert_ne!(first, changed);
    }

    #[test]
    fn split_code_tokens_handles_camel_case_and_delimiters() {
        let tokens = split_code_tokens("WorkspaceLockManager handles_file-changes");
        let texts: Vec<&str> = tokens.iter().map(|token| token.text.as_str()).collect();

        assert_eq!(
            texts,
            vec!["workspace", "lock", "manager", "handles", "file", "changes"]
        );
    }

    #[test]
    fn bm25_index_can_add_search_delete_and_replace_chunks() {
        let tempdir = TempDir::new().expect("tempdir");
        let index = Bm25Index::open_or_create(tempdir.path()).expect("index should open");
        let first = sample_chunk(
            "chunk-1",
            "src/workspace_lock.rs",
            "WorkspaceLockManager",
            "WorkspaceLockManager handles file changes",
        );
        index.add_chunks(&[first]).expect("chunk should be indexed");
        index.reload().expect("reader should reload");

        let initial = index
            .search("workspace lock changes", 10)
            .expect("search should work");
        assert_eq!(initial.len(), 1);
        assert_eq!(initial[0].path, "src/workspace_lock.rs");

        index
            .delete_by_path("src/workspace_lock.rs")
            .expect("delete should work");
        index.reload().expect("reader should reload");
        let deleted = index
            .search("workspace lock changes", 10)
            .expect("search should work");
        assert!(deleted.is_empty());

        let replacement = sample_chunk(
            "chunk-2",
            "src/workspace_lock.rs",
            "WorkspaceEvents",
            "Workspace events broadcast lock snapshots",
        );
        index
            .batch_delete_and_add(&["src/workspace_lock.rs".to_string()], &[replacement])
            .expect("batch update should work");
        index.reload().expect("reader should reload");

        let updated = index
            .search("broadcast snapshots", 10)
            .expect("search should work");
        assert_eq!(updated.len(), 1);
        assert_eq!(updated[0].id, "chunk-2");
        assert_eq!(updated[0].symbol, "WorkspaceEvents");
    }
    #[test]
    fn multiple_instances_share_the_same_index_without_holding_a_writer_lock() {
        let tempdir = TempDir::new().expect("tempdir");
        let first = Bm25Index::open_or_create(tempdir.path()).expect("first index should open");
        let second = Bm25Index::open_or_create(tempdir.path()).expect("second index should open");
        first
            .add_chunks(&[sample_chunk(
                "chunk-1",
                "src/first.rs",
                "FirstSymbol",
                "first shared symbol",
            )])
            .expect("first writer should commit");
        second.reload().expect("second reader should reload");
        let actual = second
            .search("shared symbol", 10)
            .expect("second search should work");
        assert_eq!(
            actual.first().map(|hit| hit.path.as_str()),
            Some("src/first.rs")
        );
    }
    #[test]
    fn writer_lock_busy_is_identified_through_anyhow_context() {
        let error = anyhow::Error::new(TantivyError::LockFailure(
            tantivy::directory::error::LockError::LockBusy,
            Some("fixture".to_string()),
        ))
        .context("refresh failed");
        assert!(is_writer_lock_busy(&error));
    }
}
