use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;

use crate::core::tools::bash::{BashExecError, BashExecOptions, BashOperations};
use crate::core::tools::truncate::{DEFAULT_MAX_BYTES, TruncationOptions, truncate_tail};
use crate::utils::ansi::strip_ansi;
use crate::utils::shell::sanitize_binary_output;

/// Callback for streaming output chunks (already sanitized).
pub type BashChunkSink = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Clone, Default)]
pub struct BashExecutorOptions {
    pub on_chunk: Option<BashChunkSink>,
    pub signal: Option<CancellationToken>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BashResult {
    /// Combined stdout and stderr (sanitized, possibly truncated).
    pub output: String,
    /// Process exit code; `None` if killed or cancelled.
    pub exit_code: Option<i32>,
    /// Whether the command was cancelled via the signal.
    pub cancelled: bool,
    pub truncated: bool,
    /// Path to the temp file holding the full output, once it exceeded the
    /// truncation threshold.
    pub full_output_path: Option<String>,
}

/// The rolling buffer plus the temp file behind it.
/// Deviation (class 1): the buffer is bounded by characters rather than by JS
/// string length (UTF-16 code units); the two differ only for astral
/// characters, and the bound is a memory guard rather than an observable.
struct Collector {
    chunks: Vec<String>,
    output_chars: usize,
    max_output_chars: usize,
    total_bytes: usize,
    temp_file_path: Option<PathBuf>,
    temp_file: Option<std::fs::File>,
    /// `TextDecoder` state: bytes that end mid-character wait for the next chunk.
    pending_bytes: Vec<u8>,
}

impl Collector {
    fn new() -> Self {
        Self {
            chunks: Vec::new(),
            output_chars: 0,
            max_output_chars: DEFAULT_MAX_BYTES * 2,
            total_bytes: 0,
            temp_file_path: None,
            temp_file: None,
            pending_bytes: Vec::new(),
        }
    }

    fn ensure_temp_file(&mut self) {
        if self.temp_file_path.is_some() {
            return;
        }
        let id: String = (0..8)
            .map(|_| format!("{:02x}", rand::random::<u8>()))
            .collect();
        let path = std::env::temp_dir().join(format!("notagent-bash-{id}.log"));
        let file = std::fs::File::create(&path).ok();
        self.temp_file_path = Some(path);
        self.temp_file = file;
        if let Some(file) = &mut self.temp_file {
            for chunk in &self.chunks {
                let _ = file.write_all(chunk.as_bytes());
            }
        }
    }

    fn decode_streaming(&mut self, data: &[u8]) -> String {
        self.pending_bytes.extend_from_slice(data);
        let bytes = std::mem::take(&mut self.pending_bytes);
        match std::str::from_utf8(&bytes) {
            Ok(text) => text.to_owned(),
            Err(error) => {
                let valid_up_to = error.valid_up_to();
                let mut decoded = String::from_utf8_lossy(&bytes[..valid_up_to]).into_owned();
                match error.error_len() {
                    None => {
                        self.pending_bytes.extend_from_slice(&bytes[valid_up_to..]);
                        decoded
                    }
                    Some(length) => {
                        decoded.push('\u{FFFD}');
                        let rest = bytes[valid_up_to + length..].to_vec();
                        decoded.push_str(&self.decode_streaming(&rest));
                        decoded
                    }
                }
            }
        }
    }

    fn append(&mut self, data: &[u8]) -> String {
        self.total_bytes += data.len();

        // Sanitize: strip ANSI, replace binary garbage, normalize newlines.
        let decoded = self.decode_streaming(data);
        let text = sanitize_binary_output(&strip_ansi(&decoded)).replace('\r', "");

        // Start writing to the temp file once the threshold is exceeded.
        if self.total_bytes > DEFAULT_MAX_BYTES {
            self.ensure_temp_file();
        }
        if let Some(file) = &mut self.temp_file {
            let _ = file.write_all(text.as_bytes());
        }

        // Keep the rolling buffer.
        self.output_chars += text.chars().count();
        self.chunks.push(text.clone());
        while self.output_chars > self.max_output_chars && self.chunks.len() > 1 {
            let removed = self.chunks.remove(0);
            self.output_chars -= removed.chars().count();
        }

        text
    }

    fn finish(&mut self, cancelled: bool) -> BashResult {
        let full_output = self.chunks.concat();
        let truncation = truncate_tail(&full_output, TruncationOptions::default());
        if truncation.truncated {
            self.ensure_temp_file();
        }
        if let Some(mut file) = self.temp_file.take() {
            let _ = file.flush();
        }
        BashResult {
            output: if truncation.truncated {
                truncation.content.clone()
            } else {
                full_output
            },
            exit_code: None,
            cancelled,
            truncated: truncation.truncated,
            full_output_path: self
                .temp_file_path
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
        }
    }
}

/// Execute a bash command using custom [`BashOperations`].
/// Used for remote execution (SSH, containers, …) as well as for the local
/// backend.
pub async fn execute_bash_with_operations(
    command: &str,
    cwd: &str,
    operations: &dyn BashOperations,
    options: Option<BashExecutorOptions>,
) -> Result<BashResult, BashExecError> {
    let options = options.unwrap_or_default();
    let collector = Arc::new(Mutex::new(Collector::new()));

    let sink_collector = Arc::clone(&collector);
    let on_chunk = options.on_chunk.clone();
    let result = operations
        .exec(
            command,
            cwd,
            BashExecOptions {
                on_data: Some(Arc::new(move |data: &[u8]| {
                    let text = sink_collector
                        .lock()
                        .expect("bash executor collector mutex")
                        .append(data);
                    if let Some(on_chunk) = &on_chunk {
                        on_chunk(&text);
                    }
                })),
                signal: options.signal.clone(),
                timeout: None,
                env: None,
                on_spawn: None,
            },
        )
        .await;

    let cancelled = matches!(&options.signal, Some(signal) if signal.is_cancelled());
    let mut collector = collector.lock().expect("bash executor collector mutex");
    match result {
        Ok(result) => {
            let mut finished = collector.finish(cancelled);
            finished.exit_code = if cancelled { None } else { result.exit_code };
            Ok(finished)
        }
        // An abort is an outcome rather than a failure: the output collected so
        // far is still what the caller asked for.
        Err(error) => {
            if cancelled {
                Ok(collector.finish(true))
            } else {
                if let Some(mut file) = collector.temp_file.take() {
                    let _ = file.flush();
                }
                Err(error)
            }
        }
    }
}
