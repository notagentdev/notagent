use std::io::SeekFrom;
use std::path::{Path, PathBuf};

use tokio::io::{AsyncReadExt, AsyncSeekExt};

use crate::core::tasks::types::TaskInfo;

const RECORD_SUFFIX: &str = ".json";
const LOG_NAME: &str = "output.log";

/// `{prefix}-{8 chars}`. The prefix is open so a new kind needs no change here.
/// checked by hand rather than through the `regex` crate — it is three
/// character-class tests and runs on every path built from an id.
pub fn is_valid_task_id(task_id: &str) -> bool {
    let segments: Vec<&str> = task_id.split('-').collect();
    if segments.len() < 2 {
        return false;
    }
    let is_lower_alphanumeric = |value: &str| {
        !value.is_empty()
            && value
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    };
    let (last, leading) = segments.split_last().expect("checked above");
    if last.len() != 8 || !is_lower_alphanumeric(last) {
        return false;
    }
    leading.iter().all(|segment| is_lower_alphanumeric(segment))
}

fn assert_task_id(task_id: &str) -> Result<(), String> {
    if is_valid_task_id(task_id) {
        Ok(())
    } else {
        Err(format!("Invalid task id: \"{task_id}\""))
    }
}

pub struct TaskStore {
    dir: PathBuf,
}

impl TaskStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        TaskStore { dir: dir.into() }
    }

    pub fn directory(&self) -> &Path {
        &self.dir
    }

    /// same message as an error, since a path is a value here.
    pub fn log_path(&self, task_id: &str) -> Result<PathBuf, String> {
        assert_task_id(task_id)?;
        Ok(self.dir.join(task_id).join(LOG_NAME))
    }

    fn record_path(&self, task_id: &str) -> Result<PathBuf, String> {
        assert_task_id(task_id)?;
        Ok(self.dir.join(format!("{task_id}{RECORD_SUFFIX}")))
    }

    /// Writes a record so that a reader sees either the previous one or this
    /// one. A partially written record would describe a task that never existed.
    pub async fn write_record(&self, info: &TaskInfo) -> Result<(), String> {
        let path = self.record_path(info.task_id())?;
        create_dir_all_mode(&self.dir, 0o700)
            .await
            .map_err(|error| error.to_string())?;
        let temporary = path.with_extension(format!("json.{}.tmp", std::process::id()));
        let encoded = serde_json::to_string(info).map_err(|error| error.to_string())?;
        match write_file_mode(&temporary, encoded.as_bytes(), 0o600).await {
            Ok(()) => match tokio::fs::rename(&temporary, &path).await {
                Ok(()) => Ok(()),
                Err(error) => {
                    let _ = tokio::fs::remove_file(&temporary).await;
                    Err(error.to_string())
                }
            },
            Err(error) => {
                let _ = tokio::fs::remove_file(&temporary).await;
                Err(error.to_string())
            }
        }
    }

    pub async fn read_record(&self, task_id: &str) -> Option<TaskInfo> {
        let path = self.record_path(task_id).ok()?;
        let text = tokio::fs::read_to_string(path).await.ok()?;
        normalize_record(&text)
    }

    /// Every readable record in this session's directory.
    /// Anything unreadable is skipped rather than reported: a record that cannot
    /// be parsed carries nothing recoverable beyond its filename, and failing
    /// the whole listing over one such file would hide every task beside it.
    pub async fn list_records(&self) -> Vec<TaskInfo> {
        let Ok(mut entries) = tokio::fs::read_dir(&self.dir).await else {
            return Vec::new();
        };
        let mut names: Vec<String> = Vec::new();
        while let Ok(Some(entry)) = entries.next_entry().await {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
        names.sort();
        let mut records = Vec::new();
        for name in names {
            let Some(task_id) = name.strip_suffix(RECORD_SUFFIX) else {
                continue;
            };
            if !is_valid_task_id(task_id) {
                continue;
            }
            if let Some(record) = self.read_record(task_id).await {
                records.push(record);
            }
        }
        records
    }

    pub async fn append_log(&self, task_id: &str, chunk: &str) -> Result<(), String> {
        if chunk.is_empty() {
            return Ok(());
        }
        let path = self.log_path(task_id)?;
        create_dir_all_mode(&self.dir.join(task_id), 0o700)
            .await
            .map_err(|error| error.to_string())?;
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await
            .map_err(|error| error.to_string())?;
        use tokio::io::AsyncWriteExt;
        file.write_all(chunk.as_bytes())
            .await
            .map_err(|error| error.to_string())
    }

    pub fn log_exists(&self, task_id: &str) -> bool {
        match self.log_path(task_id) {
            Ok(path) => path.exists(),
            Err(_) => false,
        }
    }

    pub async fn log_size_bytes(&self, task_id: &str) -> u64 {
        let Ok(path) = self.log_path(task_id) else {
            return 0;
        };
        match tokio::fs::metadata(path).await {
            Ok(metadata) => metadata.len(),
            Err(_) => 0,
        }
    }

    /// Reads a byte window of the log.
    /// Byte-addressed rather than line-addressed because that is how it is
    /// stored, and because a tail of a log with one enormous line must still be
    /// bounded by what the caller asked for.
    pub async fn read_log_bytes(&self, task_id: &str, offset: u64, max_bytes: u64) -> String {
        if max_bytes == 0 {
            return String::new();
        }
        let Ok(path) = self.log_path(task_id) else {
            return String::new();
        };
        let Ok(mut file) = tokio::fs::File::open(&path).await else {
            return String::new();
        };
        let Ok(metadata) = file.metadata().await else {
            return String::new();
        };
        let size = metadata.len();
        if offset >= size {
            return String::new();
        }
        let length = max_bytes.min(size - offset);
        if file.seek(SeekFrom::Start(offset)).await.is_err() {
            return String::new();
        }
        let mut buffer = vec![0_u8; length as usize];
        let mut read = 0_usize;
        while read < buffer.len() {
            match file.read(&mut buffer[read..]).await {
                Ok(0) => break,
                Ok(count) => read += count,
                Err(_) => return String::new(),
            }
        }
        String::from_utf8_lossy(&buffer[..read]).into_owned()
    }
}

/// A record read back from disk.
/// `detached` is filled in because a record written before the field existed
/// describes a task nothing can be waiting on any more — the process that held
/// the tool call is gone by definition.
/// round trip; serde drops them. Nothing but this app writes these records.
fn normalize_record(text: &str) -> Option<TaskInfo> {
    let mut info: TaskInfo = serde_json::from_str(text).ok()?;
    let base = info.base_mut();
    base.detached = Some(base.detached != Some(false));
    Some(info)
}

#[cfg(unix)]
async fn create_dir_all_mode(path: &Path, mode: u32) -> std::io::Result<()> {
    let mut builder = tokio::fs::DirBuilder::new();
    builder.recursive(true).mode(mode);
    builder.create(path).await
}

#[cfg(not(unix))]
async fn create_dir_all_mode(path: &Path, _mode: u32) -> std::io::Result<()> {
    tokio::fs::create_dir_all(path).await
}

#[cfg(unix)]
async fn write_file_mode(path: &Path, contents: &[u8], mode: u32) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt;
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(mode)
        .open(path)
        .await?;
    file.write_all(contents).await
}

#[cfg(not(unix))]
async fn write_file_mode(path: &Path, contents: &[u8], _mode: u32) -> std::io::Result<()> {
    tokio::fs::write(path, contents).await
}
