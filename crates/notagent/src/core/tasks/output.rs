use std::sync::{Arc, Mutex};

use notagent_agent::types::BoxFuture;

use crate::core::tasks::serial_queue::SerialQueue;

/// Bytes of tail kept in memory. Sized for the panel, not for the model.
pub const OUTPUT_RING_BYTES: usize = 1024 * 1024;

/// Bytes a shell task may produce before it is terminated.
/// Only shell tasks are capped. A subagent appends one bounded answer and must
/// always keep it; a command can emit without bound, and has.
pub const SHELL_OUTPUT_CEILING_BYTES: usize = 16 * 1024 * 1024;

/// Bytes of tail carried in a completion note when no log file exists.
pub const NOTIFICATION_TAIL_BYTES: usize = 4 * 1024;

pub fn output_ceiling_reason() -> String {
    let mib = SHELL_OUTPUT_CEILING_BYTES / (1024 * 1024);
    format!(
        "Output limit exceeded: the command produced more than {mib} MiB and was stopped. \
Redirect large output to a file and inspect it in slices instead."
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct OutputSnapshot {
    /// Path to the complete log, when one exists.
    pub output_path: Option<String>,
    /// Everything the task has produced, including what the tail dropped.
    pub total_bytes: u64,
    /// Bytes in `preview`.
    pub preview_bytes: u64,
    /// Whether `preview` omits the start of the stream.
    pub truncated: bool,
    /// Whether the complete stream is on disk.
    pub full_output_available: bool,
    pub preview: String,
}

pub fn empty_output_snapshot() -> OutputSnapshot {
    OutputSnapshot::default()
}

/// Appends to the complete log. Called in order, never concurrently.
pub type OutputWriteFn = Arc<dyn Fn(String) -> BoxFuture<'static, ()> + Send + Sync>;

pub struct OutputRetentionOptions {
    /// Whether disk writes start immediately. False for foreground tasks.
    pub persist_from_start: bool,
    pub write: OutputWriteFn,
    /// Called once when a shell task crosses the ceiling.
    pub on_ceiling_exceeded: Option<Arc<dyn Fn() + Send + Sync>>,
    /// Ceiling in bytes, or `None` for the kinds that are not capped.
    pub ceiling_bytes: Option<usize>,
}

#[derive(Default)]
struct RetentionState {
    chunks: Vec<String>,
    ring_bytes: usize,
    total: u64,
    pending: Vec<String>,
    pending_bytes: usize,
    persisting: bool,
    ceiling_tripped: bool,
}

/// Per-task output retention.
/// Appends stay cheap: the ring is maintained with a running byte total rather
/// than by re-measuring, and disk writes are queued rather than awaited.
pub struct OutputRetention {
    state: Mutex<RetentionState>,
    queue: SerialQueue,
    write: OutputWriteFn,
    on_ceiling_exceeded: Option<Arc<dyn Fn() + Send + Sync>>,
    ceiling_bytes: Option<usize>,
}

impl OutputRetention {
    pub fn new(options: OutputRetentionOptions) -> Self {
        OutputRetention {
            state: Mutex::new(RetentionState {
                persisting: options.persist_from_start,
                ..RetentionState::default()
            }),
            queue: SerialQueue::new(),
            write: options.write,
            on_ceiling_exceeded: options.on_ceiling_exceeded,
            ceiling_bytes: options.ceiling_bytes,
        }
    }

    /// Whether anything has been written to the complete log.
    pub fn persisted(&self) -> bool {
        self.state.lock().expect("poisoned").persisting
    }

    pub fn total_bytes(&self) -> u64 {
        self.state.lock().expect("poisoned").total
    }

    /// Settles once every append issued so far has reached the log.
    pub async fn drained(&self) {
        self.queue.drained().await;
    }

    pub fn append(&self, chunk: &str) {
        if chunk.is_empty() {
            return;
        }
        let bytes = chunk.len();
        let mut trip_ceiling = false;
        let mut write_now: Option<String> = None;
        {
            let mut state = self.state.lock().expect("poisoned");
            state.total += bytes as u64;
            append_to_ring(&mut state, chunk, bytes);

            if !state.ceiling_tripped
                && let Some(ceiling) = self.ceiling_bytes
                && state.total > ceiling as u64
            {
                state.ceiling_tripped = true;
                trip_ceiling = true;
            }
            // Past the ceiling the task is being stopped. Keep the bounded tail
            // so the note can still show what happened, but stop feeding the
            // unbounded write chain.
            if !state.ceiling_tripped {
                if !state.persisting {
                    state.pending.push(chunk.to_owned());
                    state.pending_bytes += bytes;
                    // A foreground command that never detaches must not buffer
                    // without bound either, so outgrowing the tail is itself a
                    // reason to spill.
                    if state.pending_bytes > OUTPUT_RING_BYTES {
                        write_now = start_persisting(&mut state);
                    }
                } else {
                    write_now = Some(chunk.to_owned());
                }
            }
        }
        if let Some(chunk) = write_now {
            self.enqueue_write(chunk);
        }
        if trip_ceiling && let Some(on_ceiling_exceeded) = &self.on_ceiling_exceeded {
            on_ceiling_exceeded();
        }
    }

    /// Begins writing the complete log, flushing what was buffered first so the
    /// file holds the whole stream in order. Idempotent.
    pub fn start_persisting(&self) {
        let flush = {
            let mut state = self.state.lock().expect("poisoned");
            start_persisting(&mut state)
        };
        if let Some(chunk) = flush {
            self.enqueue_write(chunk);
        }
    }

    /// Drops what was buffered for a task that ended without ever persisting.
    pub fn discard_pending(&self) {
        let mut state = self.state.lock().expect("poisoned");
        state.pending.clear();
        state.pending_bytes = 0;
    }

    /// The in-memory tail, capped to `max_bytes` from the end.
    pub fn tail(&self, max_bytes: usize) -> String {
        let state = self.state.lock().expect("poisoned");
        if state.chunks.is_empty() {
            return String::new();
        }
        let retained = state.chunks.concat();
        let offset = retained.len().saturating_sub(max_bytes);
        String::from_utf8_lossy(&retained.as_bytes()[offset..]).into_owned()
    }

    /// A snapshot built from memory alone, for when no log file exists.
    pub fn snapshot(&self, max_preview_bytes: usize) -> OutputSnapshot {
        let state = self.state.lock().expect("poisoned");
        let available = state.chunks.concat();
        let preview_bytes = max_preview_bytes
            .min(available.len())
            .min(state.total.min(usize::MAX as u64) as usize);
        let offset = available.len() - preview_bytes;
        OutputSnapshot {
            output_path: None,
            total_bytes: state.total,
            preview_bytes: preview_bytes as u64,
            truncated: state.total > preview_bytes as u64,
            full_output_available: false,
            preview: String::from_utf8_lossy(&available.as_bytes()[offset..]).into_owned(),
        }
    }

    fn enqueue_write(&self, chunk: String) {
        // Failures are swallowed on purpose: a log that cannot be written must
        // not take down the task it belongs to, and the reader is told
        // separately whether a complete log exists.
        let write = Arc::clone(&self.write);
        self.queue.enqueue(async move { write(chunk).await });
    }
}

/// Returns the buffered text that has to be flushed, if any.
fn start_persisting(state: &mut RetentionState) -> Option<String> {
    if state.persisting {
        return None;
    }
    state.persisting = true;
    let flush = if state.pending.is_empty() {
        None
    } else {
        Some(state.pending.concat())
    };
    state.pending.clear();
    state.pending_bytes = 0;
    flush
}

fn append_to_ring(state: &mut RetentionState, chunk: &str, bytes: usize) {
    // A single chunk larger than the whole ring replaces it; keeping its tail
    // is the only thing that fits, and keeping nothing would be worse.
    if bytes >= OUTPUT_RING_BYTES {
        let retained =
            String::from_utf8_lossy(&chunk.as_bytes()[bytes - OUTPUT_RING_BYTES..]).into_owned();
        state.ring_bytes = retained.len();
        state.chunks.clear();
        state.chunks.push(retained);
        return;
    }
    state.chunks.push(chunk.to_owned());
    state.ring_bytes += bytes;
    while state.ring_bytes > OUTPUT_RING_BYTES && state.chunks.len() > 1 {
        let removed = state.chunks.remove(0);
        state.ring_bytes -= removed.len();
    }
}
