use std::sync::{Arc, Mutex};

use notagent_agent::types::BoxFuture;
use tokio::sync::oneshot;

use crate::core::tasks::types::{
    BackgroundTask, SubagentTaskInfo, TaskInfo, TaskInfoBase, TaskKind, TaskSettlement,
    TaskSettlementStatus, TaskSink,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentRunResult {
    /// The answer, or an explanation of why there is none.
    pub text: String,
    pub failed: bool,
}

/// Reports the child's running token total, for the panel that shows it.
pub type SubagentTokensFn = Arc<dyn Fn() -> u64 + Send + Sync>;
/// Stops the run. Separate from the task signal so the run keeps its own.
pub type SubagentCancelFn = Arc<dyn Fn() + Send + Sync>;

pub struct SubagentTaskOptions {
    pub description: String,
    pub tokens: Option<SubagentTokensFn>,
    /// The subagent's own id — what continues it later.
    pub session_id: String,
    /// The type it runs as: `read-only` or `worker`.
    pub agent: String,
    /// The star name the user sees it under (`core/delegation/aliases.rs`).
    pub alias: String,
    /// The run, already started, which the task observes rather than owns.
    /// Deviation (class 1): a JS promise several places await becomes a
    /// `oneshot` receiver here, since only the task itself awaits the answer.
    pub run: oneshot::Receiver<SubagentRunResult>,
    pub cancel: SubagentCancelFn,
}

pub struct SubagentTask {
    description: String,
    session_id: String,
    agent: String,
    alias: String,
    run: Mutex<Option<oneshot::Receiver<SubagentRunResult>>>,
    cancel: SubagentCancelFn,
    tokens: SubagentTokensFn,
}

impl SubagentTask {
    pub fn new(options: SubagentTaskOptions) -> Self {
        SubagentTask {
            description: options.description,
            session_id: options.session_id,
            agent: options.agent,
            alias: options.alias,
            run: Mutex::new(Some(options.run)),
            cancel: options.cancel,
            tokens: options.tokens.unwrap_or_else(|| Arc::new(|| 0)),
        }
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn agent(&self) -> &str {
        &self.agent
    }

    pub fn alias(&self) -> &str {
        &self.alias
    }
}

impl BackgroundTask for SubagentTask {
    fn kind(&self) -> TaskKind {
        TaskKind::Subagent
    }

    fn id_prefix(&self) -> &str {
        "agent"
    }

    fn description(&self) -> String {
        self.description.clone()
    }

    fn start<'a>(&'a self, sink: TaskSink) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let stop_signal = sink.signal.clone();
            let cancel = Arc::clone(&self.cancel);
            if stop_signal.is_cancelled() {
                cancel();
            } else {
                let watcher_signal = stop_signal.clone();
                let watcher_cancel = Arc::clone(&cancel);
                // Dropped with the guard below, which is what removing the
                let stop_watcher = tokio::spawn(async move {
                    watcher_signal.cancelled().await;
                    watcher_cancel();
                });
                let result = self.await_run(&sink).await;
                stop_watcher.abort();
                return result;
            }
            self.await_run(&sink).await
        })
    }

    fn to_info(&self, base: TaskInfoBase) -> TaskInfo {
        TaskInfo::Subagent(SubagentTaskInfo {
            base,
            tokens: (self.tokens)(),
            session_id: self.session_id.clone(),
            agent: self.agent.clone(),
            alias: self.alias.clone(),
        })
    }
}

impl SubagentTask {
    async fn await_run(&self, sink: &TaskSink) -> Result<(), String> {
        let receiver = self
            .run
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        // A task is started once; a second call has no run left to observe,
        // which is the same nothing a dropped sender leaves behind.
        let result = match receiver {
            Some(receiver) => receiver.await.map_err(|error| error.to_string()),
            None => Err("the delegated run produced no result".to_owned()),
        };
        match result {
            Ok(result) => {
                // The answer is the task's output, which is what makes it
                // readable through the ordinary snapshot path and what the
                // completion carries.
                sink.append_output(&result.text);
                if sink.signal.is_cancelled() {
                    sink.settle(TaskSettlement::new(TaskSettlementStatus::Killed))
                        .await;
                    return Ok(());
                }
                // A subagent that came back without an answer is a failure the
                // parent has to read, not an exception: the text explains what
                // happened.
                if result.failed {
                    sink.settle(TaskSettlement::with_reason(
                        TaskSettlementStatus::Failed,
                        result.text,
                    ))
                    .await;
                } else {
                    sink.settle(TaskSettlement::new(TaskSettlementStatus::Completed))
                        .await;
                }
                Ok(())
            }
            Err(message) => {
                // The sender was dropped without an answer, which is what a
                // rejected run promise is here.
                if sink.signal.is_cancelled() {
                    sink.settle(TaskSettlement::new(TaskSettlementStatus::Killed))
                        .await;
                    return Ok(());
                }
                sink.settle(TaskSettlement::with_reason(
                    TaskSettlementStatus::Failed,
                    message,
                ))
                .await;
                Ok(())
            }
        }
    }
}
