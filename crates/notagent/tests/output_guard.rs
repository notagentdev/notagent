//! `core/output-guard.ts` — the ordered, non-blocking protocol writes and the
//! stdout takeover.
//!
//! No TypeScript suite covers this file either; it is exercised through the RPC
//! mode there. The queue is asserted here by pointing file descriptor 1 at a
//! file for the duration of one case, which is the only way to read back what
//! the writer task actually put on standard output.

#![cfg(unix)]

use notagent::core::output_guard::{
    console_log, flush_raw_stdout, is_stdout_taken_over, restore_stdout, stdout_write,
    take_over_stdout, write_raw_stdout,
};

/// Redirects file descriptor 1 into `path` and gives back the saved descriptor.
fn redirect_stdout(path: &std::path::Path) -> i32 {
    use std::os::fd::AsRawFd;
    let file = std::fs::File::create(path).expect("creates");
    // SAFETY: both descriptors are open for the duration of the calls.
    unsafe {
        let saved = libc::dup(libc::STDOUT_FILENO);
        libc::dup2(file.as_raw_fd(), libc::STDOUT_FILENO);
        saved
    }
}

fn restore_fd(saved: i32) {
    // SAFETY: `saved` came from `dup` above and is still open.
    unsafe {
        libc::dup2(saved, libc::STDOUT_FILENO);
        libc::close(saved);
    }
}

#[tokio::test]
async fn the_queue_keeps_its_order_and_the_takeover_moves_output_to_stderr() {
    let dir = tempfile::tempdir().expect("temp dir");
    let out = dir.path().join("stdout.txt");
    let saved = redirect_stdout(&out);

    // Queued writes reach standard output in the order they were queued.
    write_raw_stdout("{\"one\":1}\n");
    write_raw_stdout("{\"two\":2}\n");
    write_raw_stdout("");
    write_raw_stdout("{\"three\":3}\n");
    flush_raw_stdout().await;

    // While stdout belongs to the protocol, app output goes to stderr.
    assert!(!is_stdout_taken_over());
    take_over_stdout();
    assert!(is_stdout_taken_over());
    console_log("a line the protocol must not see");
    stdout_write("and a fragment");
    restore_stdout();
    assert!(!is_stdout_taken_over());
    // `println!` of `console_log` goes through the test harness' capture rather
    // than file descriptor 1, so the restored path is asserted on the write
    // that does reach it.
    stdout_write("visible again\n");
    flush_raw_stdout().await;

    restore_fd(saved);
    let written = std::fs::read_to_string(&out).expect("reads");
    assert_eq!(
        written, "{\"one\":1}\n{\"two\":2}\n{\"three\":3}\nvisible again\n",
        "the empty write is dropped, the order holds, and nothing from the takeover leaked"
    );
}
