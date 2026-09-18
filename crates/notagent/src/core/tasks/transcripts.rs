use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

use notagent_agent::types::AgentMessage;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::core::delegation::agent_type::SubagentType;

const CACHE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct KeptTranscript {
    pub agent: SubagentType,
    pub alias: String,
    pub messages: Vec<AgentMessage>,
}

#[derive(Default)]
struct Cache {
    entries: VecDeque<(String, Box<[u8]>)>,
    bytes: usize,
    failed: HashMap<String, String>,
}

/// Disk is authoritative; the cache only retains a bounded amount of encoded history.
#[derive(Clone)]
pub struct TaskTranscriptStore {
    directory: PathBuf,
    cache: Arc<Mutex<Cache>>,
    budget: usize,
}

impl Default for TaskTranscriptStore {
    fn default() -> Self {
        Self::new(
            std::env::temp_dir().join(format!("notagent-subagents-{}", notagent_ai::uuidv7())),
        )
    }
}

impl TaskTranscriptStore {
    pub fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            cache: Arc::new(Mutex::new(Cache::default())),
            budget: CACHE_BYTES,
        }
    }

    fn path(&self, id: &str) -> Result<PathBuf, String> {
        if id.is_empty()
            || id.len() > 128
            || !id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
        {
            return Err("Invalid subagent session id".to_owned());
        }
        Ok(self.directory.join(format!("{id}.json")))
    }

    fn cache(&self, cache: &mut Cache, id: &str, bytes: Vec<u8>) {
        if let Some(index) = cache.entries.iter().position(|(key, _)| key == id)
            && let Some((_, previous)) = cache.entries.remove(index)
        {
            cache.bytes -= previous.len();
        }
        if bytes.len() > self.budget {
            return;
        }
        while cache.bytes + bytes.len() > self.budget {
            let Some((_, oldest)) = cache.entries.pop_front() else {
                break;
            };
            cache.bytes -= oldest.len();
        }
        cache.bytes += bytes.len();
        cache
            .entries
            .push_back((id.to_owned(), bytes.into_boxed_slice()));
    }

    pub(crate) async fn save(&self, id: &str, transcript: &KeptTranscript) -> Result<(), String> {
        let path = self.path(id)?;
        let bytes = serde_json::to_vec(transcript).map_err(|e| e.to_string())?;
        // Serialize publication and reads so an older disk read cannot replace a newer cache entry.
        let mut cache = self.cache.lock().await;
        if let Err(error) = super::store::write_private_atomic(&path, &bytes).await {
            cache.failed.insert(id.to_owned(), error.clone());
            return Err(error);
        }
        cache.failed.remove(id);
        self.cache(&mut cache, id, bytes);
        Ok(())
    }

    pub(crate) async fn load(&self, id: &str) -> Result<Option<KeptTranscript>, String> {
        let path = self.path(id)?;
        let mut cache = self.cache.lock().await;
        if let Some(error) = cache.failed.get(id) {
            return Err(format!(
                "The latest subagent history was not saved: {error}"
            ));
        }
        if let Some(index) = cache.entries.iter().position(|(key, _)| key == id)
            && let Some(entry) = cache.entries.remove(index)
        {
            let result = serde_json::from_slice(&entry.1)
                .map_err(|e| format!("Cannot read subagent history: {e}"));
            cache.entries.push_back(entry);
            return result.map(Some);
        }
        let bytes = match tokio::fs::read(path).await {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("Cannot read subagent history: {error}")),
        };
        let transcript = serde_json::from_slice(&bytes)
            .map_err(|e| format!("Cannot read subagent history: {e}"))?;
        self.cache(&mut cache, id, bytes);
        Ok(Some(transcript))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn history(alias: &str) -> KeptTranscript {
        KeptTranscript {
            agent: SubagentType::Worker,
            alias: alias.to_owned(),
            messages: vec![],
        }
    }

    #[tokio::test]
    async fn evicted_and_oversized_histories_remain_continuable_from_disk() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = TaskTranscriptStore::new(directory.path().join("children"));
        let small = history("first");
        store.budget = serde_json::to_vec(&small).unwrap().len() * 2;
        store.save("one", &small).await.unwrap();
        store.save("two", &history("other")).await.unwrap();
        assert_eq!(store.load("one").await.unwrap().unwrap().alias, "first");
        store.save("three", &history("third")).await.unwrap();
        {
            let cache = store.cache.lock().await;
            assert!(
                cache.bytes <= store.budget,
                "cached histories must obey the byte budget"
            );
            assert!(
                !cache.entries.iter().any(|(id, _)| id == "two"),
                "the least recently read entry must leave RAM"
            );
        }
        assert_eq!(store.load("two").await.unwrap().unwrap().alias, "other");
        let large = history(&"large".repeat(1024));
        store.save("large", &large).await.unwrap();
        assert_eq!(
            store.load("large").await.unwrap().unwrap().alias,
            large.alias
        );
        assert!(
            !store
                .cache
                .lock()
                .await
                .entries
                .iter()
                .any(|(id, _)| id == "large")
        );
        let reopened = TaskTranscriptStore::new(directory.path().join("children"));
        assert_eq!(reopened.load("one").await.unwrap().unwrap().alias, "first");
        assert_eq!(
            reopened.load("large").await.unwrap().unwrap().alias,
            large.alias
        );
    }

    #[tokio::test]
    async fn broken_or_missing_histories_are_reported_without_inventing_context() {
        let directory = tempfile::tempdir().unwrap();
        let store = TaskTranscriptStore::new(directory.path().to_owned());
        assert!(store.load("missing").await.unwrap().is_none());
        tokio::fs::write(directory.path().join("broken.json"), b"{")
            .await
            .unwrap();
        assert!(store.load("broken").await.is_err());
        assert!(store.load("../outside").await.is_err());
        assert!(store.save("../outside", &history("bad")).await.is_err());
    }

    #[tokio::test]
    async fn a_failed_save_does_not_resume_an_older_cached_turn() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("children");
        let store = TaskTranscriptStore::new(path.clone());
        store.save("one", &history("old")).await.unwrap();
        tokio::fs::rename(&path, directory.path().join("backup"))
            .await
            .unwrap();
        tokio::fs::write(&path, b"blocked").await.unwrap();
        assert!(store.save("one", &history("new")).await.is_err());
        assert!(
            store.load("one").await.is_err(),
            "a failed update must not silently load stale history"
        );
        tokio::fs::remove_file(&path).await.unwrap();
        store.save("one", &history("new")).await.unwrap();
        assert_eq!(store.load("one").await.unwrap().unwrap().alias, "new");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn histories_are_private_to_the_current_user() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let store = TaskTranscriptStore::new(directory.path().join("children"));
        store.save("one", &history("private")).await.unwrap();
        assert_eq!(
            std::fs::metadata(&store.directory)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(store.path("one").unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}
