use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use notagent_agent::types::BoxFuture;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::core::tasks::output::{
    NOTIFICATION_TAIL_BYTES, OutputRetention, OutputRetentionOptions, OutputSnapshot,
    SHELL_OUTPUT_CEILING_BYTES, empty_output_snapshot, output_ceiling_reason,
};
use crate::core::tasks::serial_queue::SerialQueue;
use crate::core::tasks::store::TaskStore;
use crate::core::tasks::types::{
    BackgroundTask, ForegroundRelease, TaskInfo, TaskInfoBase, TaskKind, TaskSettlement,
    TaskSettlementStatus, TaskSink, TaskSinkTarget, TaskStatus, is_terminal_task_status,
};

/// How long stopped work is given to settle before it is forced.
pub const STOP_GRACE_MS: u64 = 5_000;

const ID_ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";

const SESSION_CLOSED_REASON: &str = "Session closed";

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn generate_task_id(prefix: &str) -> String {
    let mut suffix = String::with_capacity(8);
    for _ in 0..8 {
        let byte = rand::random::<u8>();
        suffix.push(ID_ALPHABET[byte as usize % ID_ALPHABET.len()] as char);
    }
    format!("{prefix}-{suffix}")
}

#[derive(Clone)]
pub struct RegisterTaskOptions {
    /// False when a tool call is waiting on this task. Defaults to true.
    pub detached: bool,
    /// Deadline in milliseconds. Zero and `None` arm nothing.
    pub timeout_ms: Option<u64>,
    /// Deadline applied from the moment a foreground task is detached.
    pub detach_timeout_ms: Option<u64>,
    /// Whether reaching the deadline detaches rather than stops.
    pub auto_background_on_timeout: bool,
    /// The waiting turn's signal. Ignored for work registered detached.
    pub signal: Option<CancellationToken>,
}

impl Default for RegisterTaskOptions {
    /// `options.detached ?? true`.
    fn default() -> Self {
        RegisterTaskOptions {
            detached: true,
            timeout_ms: None,
            detach_timeout_ms: None,
            auto_background_on_timeout: false,
            signal: None,
        }
    }
}

/// Refusal of a registration, so the caller can report the numbers.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("Too many background tasks are already running: {running} of {max}.")]
pub struct TaskLimitError {
    pub running: usize,
    pub max: usize,
}

pub type MaxRunningTasksFn = Arc<dyn Fn() -> Option<usize> + Send + Sync>;
pub type TaskStartedFn = Arc<dyn Fn(TaskInfo) + Send + Sync>;
pub type TaskTerminatedFn = Arc<dyn Fn(TaskInfo, Option<String>) + Send + Sync>;

#[derive(Clone, Default)]
pub struct TaskManagerOptions {
    /// Cap on simultaneously running detached tasks. Unset means none.
    pub max_running_tasks: Option<MaxRunningTasksFn>,
    /// Fired when a task becomes visible as background work.
    pub on_started: Option<TaskStartedFn>,
    /// Fired once when a task settles, with a bounded tail of its output.
    pub on_terminated: Option<TaskTerminatedFn>,
}

/// A JS promise several callers await, as a one-way flag.
#[derive(Clone)]
struct Flag(Arc<watch::Sender<bool>>);

impl Flag {
    fn new() -> Self {
        Flag(Arc::new(watch::channel(false).0))
    }

    fn set(&self) {
        let _ = self.0.send(true);
    }

    async fn wait(&self) {
        let mut receiver = self.0.subscribe();
        let _ = receiver.wait_for(|value| *value).await;
    }
}

struct ManagedState {
    options: RegisterTaskOptions,
    status: TaskStatus,
    started_at: i64,
    ended_at: Option<i64>,
    stop_reason: Option<String>,
    notification_suppressed: Option<bool>,
    timed_out: bool,
    terminal_fired: bool,
    detached: bool,
    /// Cancels the armed deadline, standing in for `clearTimeout`.
    deadline: Option<CancellationToken>,
    /// Stops watching the turn's signal, standing in for `removeEventListener`.
    detach_signal_cleanup: Option<CancellationToken>,
}

struct ManagedTask {
    task_id: String,
    task: Arc<dyn BackgroundTask>,
    state: Mutex<ManagedState>,
    output: OutputRetention,
    controller: CancellationToken,
    /// Set once `start` has returned and any settle it forced has run.
    lifecycle: Flag,
    /// Set once the task reached a terminal status.
    settled: Flag,
    release: watch::Sender<Option<ForegroundRelease>>,
    record_queue: SerialQueue,
    store: Arc<TaskStore>,
    on_started: Option<TaskStartedFn>,
    on_terminated: Option<TaskTerminatedFn>,
}

impl ManagedTask {
    fn is_detached(&self) -> bool {
        self.state.lock().expect("poisoned").detached
    }

    fn status(&self) -> TaskStatus {
        self.state.lock().expect("poisoned").status
    }

    fn is_terminal(&self) -> bool {
        is_terminal_task_status(self.status())
    }

    fn to_info(&self) -> TaskInfo {
        let state = self.state.lock().expect("poisoned");
        let base = TaskInfoBase {
            task_id: self.task_id.clone(),
            description: self.task.description(),
            status: state.status,
            detached: Some(state.detached),
            started_at: state.started_at,
            ended_at: state.ended_at,
            stop_reason: state.stop_reason.clone(),
            notification_suppressed: state.notification_suppressed,
            timeout_ms: state.options.timeout_ms,
        };
        drop(state);
        self.task.to_info(base)
    }

    fn enqueue_persist(&self) {
        let info = self.to_info();
        let store = Arc::clone(&self.store);
        self.record_queue.enqueue(async move {
            let _ = store.write_record(&info).await;
        });
    }

    async fn persist(&self) {
        self.enqueue_persist();
        self.record_queue.drained().await;
    }

    fn clear_deadline(&self) {
        let token = self.state.lock().expect("poisoned").deadline.take();
        if let Some(token) = token {
            token.cancel();
        }
    }

    fn arm_deadline(self: &Arc<Self>, timeout_ms: Option<u64>) {
        let Some(timeout_ms) = timeout_ms.filter(|value| *value > 0) else {
            return;
        };
        let token = CancellationToken::new();
        self.state.lock().expect("poisoned").deadline = Some(token.clone());
        let entry = Arc::clone(self);
        tokio::spawn(async move {
            tokio::select! {
                _ = token.cancelled() => return,
                _ = tokio::time::sleep(Duration::from_millis(timeout_ms)) => {}
            }
            {
                let mut state = entry.state.lock().expect("poisoned");
                state.deadline = None;
                if state.options.auto_background_on_timeout && !state.detached {
                    drop(state);
                    entry.detach(true);
                    return;
                }
            }
            entry.terminate(TaskSettlementStatus::TimedOut, None).await;
        });
    }

    /// Moves a foreground task to the background, releasing its tool call.
    fn detach(self: &Arc<Self>, via_timeout: bool) -> Option<TaskInfo> {
        let detach_timeout_ms;
        {
            let mut state = self.state.lock().expect("poisoned");
            if is_terminal_task_status(state.status) {
                drop(state);
                return Some(self.to_info());
            }
            if state.detached {
                drop(state);
                return Some(self.to_info());
            }
            state.detached = true;
            if let Some(cleanup) = state.detach_signal_cleanup.take() {
                cleanup.cancel();
            }
            // A command started with a short foreground deadline gets the
            // background one from here, counted afresh: the point of detaching
            // is that the old deadline was the wrong bound for it.
            detach_timeout_ms = state.options.detach_timeout_ms;
            if let Some(timeout_ms) = detach_timeout_ms {
                state.options.timeout_ms = Some(timeout_ms);
            }
        }
        if detach_timeout_ms.is_some() {
            self.clear_deadline();
            self.arm_deadline(detach_timeout_ms);
        }

        // Detaching has already succeeded; a failing hook must not undo it.
        self.task.on_detach();
        self.output.start_persisting();
        self.enqueue_persist();
        if let Some(on_started) = &self.on_started {
            on_started(self.to_info());
        }
        let _ = self.release.send(Some(if via_timeout {
            ForegroundRelease::TimeoutDetached
        } else {
            ForegroundRelease::Detached
        }));
        Some(self.to_info())
    }

    /// Signal, wait, force. The middle step is the whole point: work told to
    /// stop usually stops, and forcing immediately would deny it the chance.
    async fn terminate(
        self: &Arc<Self>,
        final_status: TaskSettlementStatus,
        stop_reason: Option<String>,
    ) -> Option<TaskInfo> {
        let already_terminal = {
            let mut state = self.state.lock().expect("poisoned");
            if is_terminal_task_status(state.status) {
                true
            } else {
                if final_status == TaskSettlementStatus::TimedOut {
                    state.timed_out = true;
                }
                state.stop_reason = stop_reason.clone();
                false
            }
        };
        if already_terminal {
            self.record_queue.drained().await;
            return Some(self.to_info());
        }
        self.clear_deadline();
        self.controller.cancel();

        let settled_in_time =
            tokio::time::timeout(Duration::from_millis(STOP_GRACE_MS), self.lifecycle.wait())
                .await
                .is_ok();

        if !self.is_terminal() && !settled_in_time {
            // Forcing is best effort; the task is settled below either way.
            if let Some(force_stop) = self.task.force_stop() {
                force_stop.await;
            }
        }
        if !self.is_terminal() {
            self.settle(TaskSettlement {
                status: final_status,
                stop_reason,
            })
            .await;
        }
        self.record_queue.drained().await;
        Some(self.to_info())
    }

    /// A task killed after its deadline fired reports the deadline, not the kill.
    fn coerce_timeout(&self, settlement: TaskSettlement) -> TaskSettlement {
        let timed_out = self.state.lock().expect("poisoned").timed_out;
        if timed_out && settlement.status == TaskSettlementStatus::Killed {
            return TaskSettlement {
                status: TaskSettlementStatus::TimedOut,
                ..settlement
            };
        }
        settlement
    }

    async fn settle(&self, settlement: TaskSettlement) -> bool {
        let Some(persisted) = ({
            let mut state = self.state.lock().expect("poisoned");
            if is_terminal_task_status(state.status) {
                None
            } else {
                state.status = settlement.status.into();
                state.ended_at = Some(now_ms());
                state.stop_reason = match settlement.stop_reason.clone() {
                    Some(reason) => Some(reason),
                    None => {
                        if settlement.status == TaskSettlementStatus::Killed {
                            state.stop_reason.clone()
                        } else {
                            None
                        }
                    }
                };
                if let Some(cleanup) = state.detach_signal_cleanup.take() {
                    cleanup.cancel();
                }
                if let Some(deadline) = state.deadline.take() {
                    deadline.cancel();
                }
                Some(self.output.persisted())
            }
        }) else {
            return false;
        };

        if persisted {
            self.output.drained().await;
            self.persist().await;
        } else {
            // Nothing reached disk, so nothing should: a foreground command
            // whose output went straight back to the model leaves no orphan log
            // behind.
            self.output.discard_pending();
        }

        self.fire_terminal();
        let _ = self.release.send(Some(ForegroundRelease::Terminal));
        self.settled.set();
        true
    }

    fn fire_terminal(&self) {
        {
            let mut state = self.state.lock().expect("poisoned");
            if state.terminal_fired {
                return;
            }
            // Foreground work reports through its own tool call. Announcing it
            // as well would tell the model twice about one thing it already has
            // the result of.
            if !state.detached {
                return;
            }
            state.terminal_fired = true;
        }
        let Some(on_terminated) = &self.on_terminated else {
            return;
        };
        let tail = self.output.tail(NOTIFICATION_TAIL_BYTES);
        on_terminated(
            self.to_info(),
            if tail.is_empty() { None } else { Some(tail) },
        );
    }

    fn install_foreground_signal(self: &Arc<Self>) {
        let signal = self.state.lock().expect("poisoned").options.signal.clone();
        let Some(signal) = signal else {
            return;
        };
        let cleanup = CancellationToken::new();
        self.state.lock().expect("poisoned").detach_signal_cleanup = Some(cleanup.clone());
        let entry = Arc::clone(self);
        tokio::spawn(async move {
            tokio::select! {
                _ = cleanup.cancelled() => return,
                _ = signal.cancelled() => {}
            }
            // A detached task has outlived the turn that started it on purpose;
            // that turn ending is not a reason to stop it.
            if entry.is_detached() {
                return;
            }
            entry
                .terminate(
                    TaskSettlementStatus::Killed,
                    Some("Interrupted by user".to_owned()),
                )
                .await;
        });
    }
}

impl TaskSinkTarget for ManagedTask {
    fn append_output(&self, chunk: &str) {
        self.output.append(chunk);
    }

    fn settle<'a>(&'a self, settlement: TaskSettlement) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            let settlement = self.coerce_timeout(settlement);
            ManagedTask::settle(self, settlement).await
        })
    }
}

struct ManagerInner {
    /// Deviation (class 1): a `Vec` rather than a hash map, because `list()`
    /// walks the tasks in registration order as the JS `Map` does and the
    /// number of live tasks is bounded by `maxRunningTasks`.
    tasks: Mutex<Vec<Arc<ManagedTask>>>,
    /// Records restored from disk, with no live work behind them.
    restored: Mutex<Vec<TaskInfo>>,
    store: Arc<TaskStore>,
    max_running_tasks: Option<MaxRunningTasksFn>,
    on_started: Option<TaskStartedFn>,
    on_terminated: Option<TaskTerminatedFn>,
}

#[derive(Clone)]
pub struct TaskManager {
    inner: Arc<ManagerInner>,
}

impl TaskManager {
    pub fn new(store: Arc<TaskStore>, options: TaskManagerOptions) -> Self {
        TaskManager {
            inner: Arc::new(ManagerInner {
                tasks: Mutex::new(Vec::new()),
                restored: Mutex::new(Vec::new()),
                store,
                max_running_tasks: options.max_running_tasks,
                on_started: options.on_started,
                on_terminated: options.on_terminated,
            }),
        }
    }

    pub fn store(&self) -> &Arc<TaskStore> {
        &self.inner.store
    }

    // ── registration ───────────────────────────────────────────────────

    pub fn register(
        &self,
        task: Arc<dyn BackgroundTask>,
        options: RegisterTaskOptions,
    ) -> Result<String, TaskLimitError> {
        let detached = options.detached;
        self.assert_can_register(detached)?;

        let task_id = generate_task_id(task.id_prefix());
        let store = Arc::clone(&self.inner.store);
        let kind = task.kind();
        let timeout_ms = options.timeout_ms;
        let entry = Arc::new_cyclic(|weak: &Weak<ManagedTask>| {
            let log_task_id = task_id.clone();
            let log_store = Arc::clone(&store);
            let ceiling_weak = weak.clone();
            ManagedTask {
                task_id: task_id.clone(),
                task: Arc::clone(&task),
                state: Mutex::new(ManagedState {
                    options: RegisterTaskOptions {
                        detached,
                        timeout_ms: options.timeout_ms,
                        detach_timeout_ms: options.detach_timeout_ms,
                        auto_background_on_timeout: options.auto_background_on_timeout,
                        signal: if detached {
                            None
                        } else {
                            options.signal.clone()
                        },
                    },
                    status: TaskStatus::Running,
                    started_at: now_ms(),
                    ended_at: None,
                    stop_reason: None,
                    notification_suppressed: None,
                    timed_out: false,
                    terminal_fired: false,
                    detached,
                    deadline: None,
                    detach_signal_cleanup: None,
                }),
                output: OutputRetention::new(OutputRetentionOptions {
                    persist_from_start: detached,
                    write: Arc::new(move |chunk: String| {
                        let store = Arc::clone(&log_store);
                        let task_id = log_task_id.clone();
                        Box::pin(async move {
                            let _ = store.append_log(&task_id, &chunk).await;
                        })
                    }),
                    ceiling_bytes: match kind {
                        TaskKind::Shell => Some(SHELL_OUTPUT_CEILING_BYTES),
                        TaskKind::Subagent => None,
                    },
                    on_ceiling_exceeded: Some(Arc::new(move || {
                        let Some(entry) = ceiling_weak.upgrade() else {
                            return;
                        };
                        tokio::spawn(async move {
                            entry
                                .terminate(
                                    TaskSettlementStatus::Killed,
                                    Some(output_ceiling_reason()),
                                )
                                .await;
                        });
                    })),
                }),
                controller: CancellationToken::new(),
                lifecycle: Flag::new(),
                settled: Flag::new(),
                release: watch::channel(None).0,
                record_queue: SerialQueue::new(),
                store: Arc::clone(&store),
                on_started: self.inner.on_started.clone(),
                on_terminated: self.inner.on_terminated.clone(),
            }
        });

        {
            let mut tasks = self.inner.tasks.lock().expect("poisoned");
            tasks.push(Arc::clone(&entry));
        }
        self.inner
            .restored
            .lock()
            .expect("poisoned")
            .retain(|info| info.task_id() != task_id);

        entry.arm_deadline(timeout_ms);

        let sink = TaskSink::new(
            entry.controller.clone(),
            Arc::clone(&entry) as Arc<dyn TaskSinkTarget>,
        );
        let lifecycle_entry = Arc::clone(&entry);
        tokio::spawn(async move {
            let result = lifecycle_entry.task.start(sink).await;
            if let Err(message) = result {
                // Work that threw instead of settling still has to reach a
                // terminal state, or everything waiting on it waits forever.
                let (timed_out, aborted) = {
                    let state = lifecycle_entry.state.lock().expect("poisoned");
                    (state.timed_out, lifecycle_entry.controller.is_cancelled())
                };
                let status = if timed_out {
                    TaskSettlementStatus::TimedOut
                } else if aborted {
                    TaskSettlementStatus::Killed
                } else {
                    TaskSettlementStatus::Failed
                };
                let stop_reason = if status == TaskSettlementStatus::Failed {
                    Some(message)
                } else {
                    None
                };
                ManagedTask::settle(
                    &lifecycle_entry,
                    TaskSettlement {
                        status,
                        stop_reason,
                    },
                )
                .await;
            }
            lifecycle_entry.lifecycle.set();
        });

        entry.install_foreground_signal();

        if entry.is_detached() {
            entry.enqueue_persist();
            if let Some(on_started) = &self.inner.on_started {
                on_started(entry.to_info());
            }
        }
        Ok(task_id)
    }

    fn assert_can_register(&self, detached: bool) -> Result<(), TaskLimitError> {
        // Only detached work is capped. A foreground command is bounded by the
        // turn waiting on it, and refusing one would break an ordinary tool call.
        if !detached {
            return Ok(());
        }
        let Some(max_running_tasks) = &self.inner.max_running_tasks else {
            return Ok(());
        };
        let Some(max) = max_running_tasks() else {
            return Ok(());
        };
        let running = self.running_detached_count();
        if running < max {
            return Ok(());
        }
        Err(TaskLimitError { running, max })
    }

    fn running_detached_count(&self) -> usize {
        self.inner
            .tasks
            .lock()
            .expect("poisoned")
            .iter()
            .filter(|entry| {
                let state = entry.state.lock().expect("poisoned");
                !is_terminal_task_status(state.status) && state.detached
            })
            .count()
    }

    fn entry(&self, task_id: &str) -> Option<Arc<ManagedTask>> {
        self.inner
            .tasks
            .lock()
            .expect("poisoned")
            .iter()
            .find(|entry| entry.task_id == task_id)
            .cloned()
    }

    fn restored_record(&self, task_id: &str) -> Option<TaskInfo> {
        self.inner
            .restored
            .lock()
            .expect("poisoned")
            .iter()
            .find(|info| info.task_id() == task_id)
            .cloned()
    }

    // ── reading ────────────────────────────────────────────────────────

    pub fn get(&self, task_id: &str) -> Option<TaskInfo> {
        match self.entry(task_id) {
            Some(entry) => Some(entry.to_info()),
            None => self.restored_record(task_id),
        }
    }

    /// Lists tasks. Finished foreground work is left out even when everything is
    /// asked for: it was already reported to whoever was waiting on it, and
    /// listing it again would fill the panel with every command ever run.
    pub fn list(&self, active_only: bool, limit: Option<usize>) -> Vec<TaskInfo> {
        let wanted = |info: &TaskInfo| -> bool {
            if !info.is_terminal() {
                return true;
            }
            if active_only {
                return false;
            }
            info.is_detached()
        };
        let mut result: Vec<TaskInfo> = Vec::new();
        let entries: Vec<Arc<ManagedTask>> = self
            .inner
            .tasks
            .lock()
            .expect("poisoned")
            .iter()
            .cloned()
            .collect();
        for entry in entries {
            let info = entry.to_info();
            if !wanted(&info) {
                continue;
            }
            result.push(info);
            if limit.is_some_and(|limit| result.len() >= limit) {
                return result;
            }
        }
        if !active_only {
            let restored: Vec<TaskInfo> = self.inner.restored.lock().expect("poisoned").clone();
            for info in restored {
                if !wanted(&info) {
                    continue;
                }
                result.push(info);
                if limit.is_some_and(|limit| result.len() >= limit) {
                    return result;
                }
            }
        }
        result
    }

    /// The output of a task, preferring the complete log when one exists.
    /// The distinction is reported rather than hidden: a reader that receives a
    /// memory tail needs to know no fuller copy exists, because that changes
    /// what it can do about a truncation.
    pub async fn output_snapshot(&self, task_id: &str, max_preview_bytes: u64) -> OutputSnapshot {
        if self.get(task_id).is_none() {
            return empty_output_snapshot();
        }
        let entry = self.entry(task_id);
        if let Some(entry) = &entry {
            entry.output.drained().await;
        }

        if self.inner.store.log_exists(task_id) {
            let total_bytes = self.inner.store.log_size_bytes(task_id).await;
            let offset = total_bytes.saturating_sub(max_preview_bytes);
            return OutputSnapshot {
                output_path: self
                    .inner
                    .store
                    .log_path(task_id)
                    .ok()
                    .map(|path| path.to_string_lossy().into_owned()),
                total_bytes,
                preview_bytes: total_bytes - offset,
                truncated: offset > 0,
                full_output_available: true,
                preview: self
                    .inner
                    .store
                    .read_log_bytes(task_id, offset, total_bytes - offset)
                    .await,
            };
        }
        match entry {
            Some(entry) => entry
                .output
                .snapshot(usize::try_from(max_preview_bytes).unwrap_or(usize::MAX)),
            None => empty_output_snapshot(),
        }
    }

    pub async fn read_output(&self, task_id: &str, tail: Option<usize>) -> String {
        let output = self.output_snapshot(task_id, u64::MAX).await.preview;
        match tail {
            None => output,
            Some(tail) => {
                let offset = output.len().saturating_sub(tail);
                String::from_utf8_lossy(&output.as_bytes()[offset..]).into_owned()
            }
        }
    }

    // ── waiting ────────────────────────────────────────────────────────

    /// Waits for a foreground task to release its tool call, either by settling
    /// or by being moved to the background.
    pub async fn wait_for_foreground_release(&self, task_id: &str) -> Option<ForegroundRelease> {
        let entry = self.entry(task_id)?;
        if entry.is_terminal() {
            entry.record_queue.drained().await;
            return Some(ForegroundRelease::Terminal);
        }
        if entry.is_detached() {
            return Some(ForegroundRelease::Detached);
        }
        let mut release = entry.release.subscribe();
        let reason = tokio::select! {
            result = release.wait_for(|value| value.is_some()) => {
                result.ok().and_then(|value| *value).unwrap_or(ForegroundRelease::Terminal)
            }
            _ = entry.lifecycle.wait() => ForegroundRelease::Terminal,
        };
        if reason == ForegroundRelease::Terminal {
            entry.record_queue.drained().await;
        }
        Some(reason)
    }

    /// Waits for a task to settle, or for the deadline to pass.
    pub async fn wait(
        &self,
        task_id: &str,
        timeout_ms: u64,
        signal: Option<CancellationToken>,
    ) -> Option<TaskInfo> {
        let Some(entry) = self.entry(task_id) else {
            return self.restored_record(task_id);
        };
        if entry.is_terminal() {
            entry.record_queue.drained().await;
            return Some(entry.to_info());
        }
        if timeout_ms == 0 {
            return Some(entry.to_info());
        }
        let abort = async {
            match &signal {
                Some(signal) => signal.cancelled().await,
                None => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            _ = entry.settled.wait() => {}
            _ = tokio::time::sleep(Duration::from_millis(timeout_ms)) => {}
            _ = abort => {}
        }
        if entry.is_terminal() {
            entry.record_queue.drained().await;
        }
        Some(entry.to_info())
    }

    // ── state changes ──────────────────────────────────────────────────

    /// Moves a foreground task to the background, releasing its tool call.
    pub fn detach(&self, task_id: &str) -> Option<TaskInfo> {
        match self.entry(task_id) {
            Some(entry) => entry.detach(false),
            None => self.restored_record(task_id),
        }
    }

    pub async fn stop(&self, task_id: &str, reason: Option<&str>) -> Option<TaskInfo> {
        let Some(entry) = self.entry(task_id) else {
            return self.restored_record(task_id);
        };
        let normalized = reason
            .map(str::trim)
            .filter(|reason| !reason.is_empty())
            .map(str::to_owned);
        entry
            .terminate(TaskSettlementStatus::Killed, normalized)
            .await
    }

    pub async fn stop_all(&self, reason: Option<&str>) -> Vec<TaskInfo> {
        let task_ids: Vec<String> = self
            .inner
            .tasks
            .lock()
            .expect("poisoned")
            .iter()
            .map(|entry| entry.task_id.clone())
            .collect();
        let settled = futures::future::join_all(
            task_ids
                .iter()
                .map(|task_id| self.stop(task_id, reason))
                .collect::<Vec<_>>(),
        )
        .await;
        settled.into_iter().flatten().collect()
    }

    /// Stops everything at session end, suppressing the completion notes first:
    /// nobody remains to read them, and an announcement into a closing session
    /// would be delivered to the next one.
    pub async fn shutdown(&self, reason: Option<&str>) -> Vec<TaskInfo> {
        let reason = reason.unwrap_or(SESSION_CLOSED_REASON);
        for info in self.list(true, None) {
            if info.is_detached() {
                self.suppress_notification(info.task_id()).await;
            }
        }
        self.stop_all(Some(reason)).await
    }

    /// Stops a task from announcing itself when it settles.
    /// Used when the result is being handed back directly — stopping a task from
    /// a tool call, or ending the session — so the model is never told twice.
    pub async fn suppress_notification(&self, task_id: &str) {
        let Some(entry) = self.entry(task_id) else {
            return;
        };
        {
            let mut state = entry.state.lock().expect("poisoned");
            if state.notification_suppressed == Some(true) {
                return;
            }
            state.notification_suppressed = Some(true);
        }
        entry.persist().await;
    }

    // ── restore ────────────────────────────────────────────────────────

    /// Loads persisted records and reclassifies whatever the previous process
    /// left running. Returns the tasks that were lost, so the caller can say so.
    pub async fn reconcile(&self) -> Vec<TaskInfo> {
        for record in self.inner.store.list_records().await {
            if self.entry(record.task_id()).is_some() {
                continue;
            }
            let mut restored = self.inner.restored.lock().expect("poisoned");
            if restored
                .iter()
                .any(|info| info.task_id() == record.task_id())
            {
                continue;
            }
            restored.push(record);
        }
        let snapshot: Vec<TaskInfo> = self.inner.restored.lock().expect("poisoned").clone();
        let mut lost = Vec::new();
        for info in snapshot {
            if info.is_terminal() {
                continue;
            }
            let mut updated = info.clone();
            {
                let base = updated.base_mut();
                base.status = TaskStatus::Lost;
                base.ended_at = base.ended_at.or_else(|| Some(now_ms()));
            }
            {
                let mut restored = self.inner.restored.lock().expect("poisoned");
                if let Some(slot) = restored
                    .iter_mut()
                    .find(|entry| entry.task_id() == updated.task_id())
                {
                    *slot = updated.clone();
                }
            }
            let _ = self.inner.store.write_record(&updated).await;
            lost.push(updated);
        }
        lost
    }

    /// Every restored record, including those that settled before the restart.
    pub fn restored_tasks(&self) -> Vec<TaskInfo> {
        self.inner.restored.lock().expect("poisoned").clone()
    }
}

/// The shell tool talks to the manager through this trait so that it can be
/// built and tested without one (`core/tools/bash.rs`).
impl crate::core::tools::bash::BashTaskManager for TaskManager {
    fn register_shell_task(
        &self,
        task: crate::core::tasks::shell_task::ShellTaskSpec,
        options: RegisterTaskOptions,
    ) -> Result<String, String> {
        let shell_task: Arc<dyn BackgroundTask> =
            Arc::new(crate::core::tasks::shell_task::ShellTask::new(task));
        self.register(shell_task, options)
            .map_err(|error| error.to_string())
    }

    fn wait_for_foreground_release<'a>(
        &'a self,
        task_id: &'a str,
    ) -> BoxFuture<'a, Option<ForegroundRelease>> {
        Box::pin(async move { TaskManager::wait_for_foreground_release(self, task_id).await })
    }

    fn get_task(&self, task_id: &str) -> Option<crate::core::tools::bash::ManagedTaskSnapshot> {
        let info = self.get(task_id)?;
        Some(crate::core::tools::bash::ManagedTaskSnapshot {
            status: info.status(),
            stop_reason: info.stop_reason().map(str::to_owned),
            exit_code: match &info {
                TaskInfo::Shell(shell) => shell.exit_code,
                TaskInfo::Subagent(_) => None,
            },
        })
    }
}
