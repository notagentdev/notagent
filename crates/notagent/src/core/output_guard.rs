use std::io::IsTerminal;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::sync::{Notify, mpsc};

const RAW_STDOUT_RETRY_DELAY_MS: u64 = 10;

static TAKEN_OVER: AtomicBool = AtomicBool::new(false);

struct RawStdoutWriter {
    sender: mpsc::UnboundedSender<String>,
    pending: Arc<AtomicUsize>,
    drained: Arc<Notify>,
}

fn writer() -> &'static RawStdoutWriter {
    static WRITER: OnceLock<RawStdoutWriter> = OnceLock::new();
    WRITER.get_or_init(|| {
        let (sender, mut receiver) = mpsc::unbounded_channel::<String>();
        let pending = Arc::new(AtomicUsize::new(0));
        let drained = Arc::new(Notify::new());
        let task_pending = Arc::clone(&pending);
        let task_drained = Arc::clone(&drained);
        tokio::spawn(async move {
            while let Some(chunk) = receiver.recv().await {
                write_raw_stdout_chunk(&chunk).await;
                if task_pending.fetch_sub(1, Ordering::SeqCst) == 1 {
                    task_drained.notify_waiters();
                }
            }
        });
        RawStdoutWriter {
            sender,
            pending,
            drained,
        }
    })
}

/// Writes one chunk to the real standard output, waiting out a full pipe.
/// A write that fails for any other reason takes the process down, as it does
/// half-written one is worse than none.
async fn write_raw_stdout_chunk(text: &str) {
    let mut stdout = tokio::io::stdout();
    loop {
        let result = async {
            stdout.write_all(text.as_bytes()).await?;
            stdout.flush().await
        }
        .await;
        match result {
            Ok(()) => return,
            Err(error) => {
                // EAGAIN and EWOULDBLOCK are the same number on this target.
                let retryable = matches!(
                    error.raw_os_error(),
                    Some(libc::ENOBUFS) | Some(libc::EAGAIN)
                ) || error.kind() == std::io::ErrorKind::WouldBlock;
                if !retryable {
                    std::process::exit(1);
                }
                tokio::time::sleep(Duration::from_millis(RAW_STDOUT_RETRY_DELAY_MS)).await;
            }
        }
    }
}

/// Routes app output to standard error, keeping standard out for the protocol.
pub fn take_over_stdout() {
    TAKEN_OVER.store(true, Ordering::SeqCst);
}

pub fn restore_stdout() {
    TAKEN_OVER.store(false, Ordering::SeqCst);
}

pub fn is_stdout_taken_over() -> bool {
    TAKEN_OVER.load(Ordering::SeqCst)
}

/// Queues one protocol write. Returns immediately; order is preserved.
pub fn write_raw_stdout(text: &str) {
    if text.is_empty() {
        return;
    }
    let writer = writer();
    writer.pending.fetch_add(1, Ordering::SeqCst);
    if writer.sender.send(text.to_owned()).is_err() {
        writer.pending.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Resolves once every queued write has reached standard output.
pub async fn wait_for_raw_stdout_backpressure() {
    let writer = writer();
    loop {
        // Enabled before the check: `notified()` registers the waiter on first
        // poll, so a drain that happens in between would notify nobody.
        let notified = writer.drained.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if writer.pending.load(Ordering::SeqCst) == 0 {
            return;
        }
        notified.await;
    }
}

pub async fn flush_raw_stdout() {
    wait_for_raw_stdout_backpressure().await;
    let _ = tokio::io::stdout().flush().await;
}

/// `console.log` — standard output, unless it has been taken over.
pub fn console_log(line: &str) {
    if is_stdout_taken_over() {
        eprintln!("{line}");
    } else {
        println!("{line}");
    }
}

/// `process.stdout.write` — no newline appended.
pub fn stdout_write(text: &str) {
    use std::io::Write;
    if is_stdout_taken_over() {
        let mut stderr = std::io::stderr();
        let _ = stderr.write_all(text.as_bytes());
        let _ = stderr.flush();
    } else {
        let mut stdout = std::io::stdout();
        let _ = stdout.write_all(text.as_bytes());
        let _ = stdout.flush();
    }
}

/// `process.stdout.isTTY`.
pub fn stdout_is_tty() -> bool {
    std::io::stdout().is_terminal()
}

/// `process.stdin.isTTY`.
pub fn stdin_is_tty() -> bool {
    std::io::stdin().is_terminal()
}
