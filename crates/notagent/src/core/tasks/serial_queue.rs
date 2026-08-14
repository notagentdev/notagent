//! Serialised background work with an awaitable drain point.
//!
//! New file (deviation class 1). Two places in `core/tasks/` chain writes onto a
//! promise instead of awaiting them — `OutputRetention.writeQueue` in
//! `output.ts` and `ManagedTask.recordQueue` in `manager.ts` — so that an
//! append stays cheap while a later reader can still wait for everything issued
//! so far to have reached the disk. A JS promise chain gives both properties for
//! free; in Rust the same two properties are a worker task fed by a channel plus
//! a ticket the caller can wait on.

use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::{mpsc, watch};

type Job = (u64, Pin<Box<dyn Future<Output = ()> + Send>>);

pub struct SerialQueue {
    sender: Mutex<Option<mpsc::UnboundedSender<Job>>>,
    issued: AtomicU64,
    completed: watch::Receiver<u64>,
}

impl SerialQueue {
    /// Starts the worker. Must be called from inside a tokio runtime.
    pub fn new() -> Self {
        let (sender, mut receiver) = mpsc::unbounded_channel::<Job>();
        let (completed_sender, completed) = watch::channel(0_u64);
        tokio::spawn(async move {
            while let Some((ticket, job)) = receiver.recv().await {
                job.await;
                let _ = completed_sender.send(ticket);
            }
        });
        SerialQueue {
            sender: Mutex::new(Some(sender)),
            issued: AtomicU64::new(0),
            completed,
        }
    }

    /// Queues `job` behind everything queued before it.
    pub fn enqueue<F>(&self, job: F) -> u64
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let ticket = self.issued.fetch_add(1, Ordering::SeqCst) + 1;
        let sender = self.sender.lock().expect("poisoned");
        if let Some(sender) = sender.as_ref() {
            let _ = sender.send((ticket, Box::pin(job)));
        }
        ticket
    }

    /// Settles once every job issued at the time of the call has run.
    pub async fn drained(&self) {
        let target = self.issued.load(Ordering::SeqCst);
        if target == 0 {
            return;
        }
        let mut completed = self.completed.clone();
        let _ = completed.wait_for(|value| *value >= target).await;
    }
}

impl Default for SerialQueue {
    fn default() -> Self {
        SerialQueue::new()
    }
}

impl Drop for SerialQueue {
    fn drop(&mut self) {
        // Closing the channel lets the worker finish what it has and exit.
        *self.sender.lock().expect("poisoned") = None;
    }
}
