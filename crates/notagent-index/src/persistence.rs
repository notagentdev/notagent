//! Persistence for symbol index - save/load to disk

use crate::symbol::Symbol;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::hash::Hasher;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use twox_hash::XxHash64;

/// Cache version - bump when format changes
const CACHE_VERSION: u32 = 2; // Bumped: String paths instead of PathBuf

/// Serializable index cache
/// Uses String paths for cross-platform compatibility (Windows/Linux/Mac)
#[derive(Serialize, Deserialize)]
pub struct IndexCache {
    /// Cache format version
    pub version: u32,
    /// All symbols indexed by file path (UTF-8 string for portability)
    pub files: HashMap<String, FileCache>,
}

/// Cached data for a single file
#[derive(Serialize, Deserialize)]
pub struct FileCache {
    /// Content hash for change detection (xxHash64 for speed)
    pub content_hash: u64,
    /// Symbols extracted from this file
    pub symbols: Vec<Symbol>,
}

impl IndexCache {
    /// Create a new empty cache
    pub fn new() -> Self {
        Self {
            version: CACHE_VERSION,
            files: HashMap::new(),
        }
    }

    /// Save cache to a file using bincode
    pub fn save(&self, path: &Path) -> Result<()> {
        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).context("Failed to create cache directory")?;
        }

        let bytes = bincode::serialize(self).context("Failed to serialize index cache")?;

        fs::write(path, bytes).context("Failed to write index cache file")?;

        tracing::info!(
            "Saved index cache: {} files, {} bytes",
            self.files.len(),
            path.metadata().map(|m| m.len()).unwrap_or(0)
        );

        Ok(())
    }

    /// Load cache from a file with graceful fallback on error
    pub fn load(path: &Path) -> Result<Self> {
        let bytes = fs::read(path).context("Failed to read index cache file")?;

        let cache: IndexCache =
            bincode::deserialize(&bytes).context("Failed to deserialize index cache")?;

        // Check version compatibility
        if cache.version != CACHE_VERSION {
            anyhow::bail!(
                "Cache version mismatch: expected {}, got {}",
                CACHE_VERSION,
                cache.version
            );
        }

        tracing::info!("Loaded index cache: {} files", cache.files.len());

        Ok(cache)
    }

    /// Load cache with graceful fallback - returns empty cache on any error
    pub fn load_or_default(path: &Path) -> Self {
        match Self::load(path) {
            Ok(cache) => cache,
            Err(e) => {
                tracing::warn!("Failed to load index cache, starting fresh: {}", e);
                Self::new()
            }
        }
    }

    /// Check if cache file exists
    pub fn exists(path: &Path) -> bool {
        path.exists()
    }

    /// Get the default cache path inside an index state directory
    pub fn default_path(state_dir: &Path) -> PathBuf {
        state_dir.join("index.bin")
    }

    /// Add or update a file in the cache
    pub fn update_file(&mut self, path: &Path, content_hash: u64, symbols: Vec<Symbol>) {
        let key = path_to_key(path);
        self.files.insert(
            key,
            FileCache {
                content_hash,
                symbols,
            },
        );
    }

    /// Remove a file from the cache
    pub fn remove_file(&mut self, path: &Path) {
        let key = path_to_key(path);
        self.files.remove(&key);
    }

    /// Get cached hash for a file
    pub fn get_hash(&self, path: &Path) -> Option<u64> {
        let key = path_to_key(path);
        self.files.get(&key).map(|f| f.content_hash)
    }

    /// Get cached symbols for a file
    pub fn get_symbols(&self, path: &Path) -> Option<&Vec<Symbol>> {
        let key = path_to_key(path);
        self.files.get(&key).map(|f| &f.symbols)
    }

    /// Convert to Arc<Symbol> for index compatibility
    pub fn symbols_as_arc(&self, path: &Path) -> Vec<Arc<Symbol>> {
        let key = path_to_key(path);
        self.files
            .get(&key)
            .map(|f| f.symbols.iter().cloned().map(Arc::new).collect())
            .unwrap_or_default()
    }

    /// Get all symbols from cache
    pub fn all_symbols(&self) -> Vec<Arc<Symbol>> {
        self.files
            .values()
            .flat_map(|f| f.symbols.iter().cloned().map(Arc::new))
            .collect()
    }

    /// Get file count
    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    /// Get symbol count
    pub fn symbol_count(&self) -> usize {
        self.files.values().map(|f| f.symbols.len()).sum()
    }
}

impl Default for IndexCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert Path to portable string key (forward slashes on all platforms)
fn path_to_key(path: &Path) -> String {
    // Use forward slashes for cross-platform compatibility
    path.to_string_lossy().replace('\\', "/")
}

/// Stable key for a workspace path, used to name its per-workspace index
/// state directory (survives restarts, unlike `DefaultHasher`).
pub fn workspace_key(path: &Path) -> String {
    let mut hasher = XxHash64::with_seed(0);
    hasher.write(path_to_key(path).as_bytes());
    format!("{:016x}", hasher.finish())
}

/// Fast content hash using xxHash64
pub fn hash_content(content: &str) -> u64 {
    let mut hasher = XxHash64::with_seed(0);
    hasher.write(content.as_bytes());
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::symbol::SymbolKind;
    use tempfile::tempdir;

    #[test]
    fn test_save_load_cache() {
        let dir = tempdir().unwrap();
        let cache_path = dir.path().join("test_cache.bin");

        let mut cache = IndexCache::new();

        // Add a test file
        let test_path = PathBuf::from("src/main.rs");
        let symbol = Symbol::new(
            "main".to_string(),
            SymbolKind::Function,
            test_path.clone(),
            0..10,
            0..100,
        );
        cache.update_file(&test_path, 12345, vec![symbol]);

        // Save
        cache.save(&cache_path).unwrap();

        // Load
        let loaded = IndexCache::load(&cache_path).unwrap();

        assert_eq!(loaded.file_count(), 1);
        assert_eq!(loaded.get_hash(&test_path), Some(12345));
        assert_eq!(loaded.get_symbols(&test_path).unwrap().len(), 1);
    }

    #[test]
    fn test_hash_content() {
        let hash1 = hash_content("fn main() {}");
        let hash2 = hash_content("fn main() {}");
        let hash3 = hash_content("fn main() { }");

        assert_eq!(hash1, hash2);
        assert_ne!(hash1, hash3);
    }

    #[test]
    fn test_path_to_key() {
        // Should normalize backslashes to forward slashes
        let key = path_to_key(Path::new("src/main.rs"));
        assert_eq!(key, "src/main.rs");
    }

    #[test]
    fn test_load_or_default() {
        // Non-existent file should return empty cache
        let cache = IndexCache::load_or_default(Path::new("/nonexistent/path.bin"));
        assert_eq!(cache.file_count(), 0);
    }
}
