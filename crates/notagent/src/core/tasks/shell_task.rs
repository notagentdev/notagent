//! Port of `packages/coding-agent/src/core/tasks/shell-task.ts`.
//!
//! A shell command as a piece of managed work.
//!
//! The command runs through the same operations the shell tool has always used,
//! so a caller that redirects execution somewhere else — an SSH backend, a
//! sandbox — keeps working when the command is backgrounded. What this adds is
//! only the lifecycle: reporting output to the manager, settling once with a
//! status the exit code decides, and offering a forced stop for a process that
//! ignored the polite one.
//!
//! The foreground callback exists because a running command still has a reader:
//! the tool call that started it streams output into the transcript. It stops
//! being called the moment the task detaches.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use notagent_agent::types::BoxFuture;

use crate::core::tasks::types::{
    BackgroundTask, ShellTaskInfo, TaskInfo, TaskInfoBase, TaskKind, TaskSettlement,
    TaskSettlementStatus, TaskSink,
};
use crate::core::tools::bash::{BashExecOptions, BashOperations};
use crate::utils::shell::kill_process_tree;

/// Receives output while a tool call is still watching.
pub type ShellTaskOutputSink = Arc<dyn Fn(&str) + Send + Sync>;

/// `ShellTaskOptions` — what the shell tool hands the manager.
#[derive(Clone)]
pub struct ShellTaskSpec {
    pub operations: Arc<dyn BashOperations>,
    pub command: String,
    pub cwd: String,
    pub env: Option<BTreeMap<String, String>>,
    pub description: String,
    /// Receives output while a tool call is still watching.
    pub on_output: Option<ShellTaskOutputSink>,
}

pub struct ShellTask {
    spec: ShellTaskSpec,
    /// Shared rather than owned so the `on_spawn` callback — which `exec` takes
    /// as an `Arc<dyn Fn>` and may keep for the length of the call — can write
    /// the pid without borrowing `self`.
    state: Arc<Mutex<ShellTaskState>>,
}

#[derive(Default)]
struct ShellTaskState {
    pid: u32,
    exit_code: Option<i32>,
}

impl ShellTask {
    pub fn new(spec: ShellTaskSpec) -> Self {
        ShellTask {
            spec,
            state: Arc::new(Mutex::new(ShellTaskState::default())),
        }
    }

    pub fn command(&self) -> &str {
        &self.spec.command
    }
}

impl BackgroundTask for ShellTask {
    fn kind(&self) -> TaskKind {
        TaskKind::Shell
    }

    fn id_prefix(&self) -> &str {
        "bash"
    }

    fn description(&self) -> String {
        self.spec.description.clone()
    }

    fn start<'a>(&'a self, sink: TaskSink) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let data_sink = sink.clone();
            let on_output = self.spec.on_output.clone();
            let result = self
                .spec
                .operations
                .exec(
                    &self.spec.command,
                    &self.spec.cwd,
                    BashExecOptions {
                        on_data: Some(Arc::new(move |data: &[u8]| {
                            let text = String::from_utf8_lossy(data);
                            if text.is_empty() {
                                return;
                            }
                            data_sink.append_output(&text);
                            // Once stopping has begun the tool call is no longer
                            // showing this; forwarding would grow a buffer
                            // nobody reads.
                            if data_sink.signal.is_cancelled() {
                                return;
                            }
                            if let Some(on_output) = &on_output {
                                on_output(&text);
                            }
                        })),
                        signal: Some(sink.signal.clone()),
                        timeout: None,
                        env: self.spec.env.clone(),
                        on_spawn: Some({
                            let state = Arc::clone(&self.state);
                            Arc::new(move |pid: u32| {
                                state.lock().expect("poisoned").pid = pid;
                            })
                        }),
                    },
                )
                .await;

            match result {
                Ok(result) => {
                    self.state.lock().expect("poisoned").exit_code = result.exit_code;
                    let status = if sink.signal.is_cancelled() {
                        TaskSettlementStatus::Killed
                    } else if result.exit_code == Some(0) {
                        TaskSettlementStatus::Completed
                    } else {
                        TaskSettlementStatus::Failed
                    };
                    sink.settle(TaskSettlement::new(status)).await;
                }
                Err(error) => {
                    if sink.signal.is_cancelled() {
                        sink.settle(TaskSettlement::new(TaskSettlementStatus::Killed))
                            .await;
                    } else {
                        sink.settle(TaskSettlement::with_reason(
                            TaskSettlementStatus::Failed,
                            error.to_string(),
                        ))
                        .await;
                    }
                }
            }
            Ok(())
        })
    }

    /// Kills the whole process group rather than the shell we spawned.
    ///
    /// A dev server or a watcher starts children of its own, and killing only
    /// the shell leaves them holding the port that made the command worth
    /// stopping.
    fn force_stop<'a>(&'a self) -> Option<BoxFuture<'a, ()>> {
        Some(Box::pin(async move {
            let pid = self.state.lock().expect("poisoned").pid;
            if pid > 0 {
                kill_process_tree(pid);
            }
        }))
    }

    fn to_info(&self, base: TaskInfoBase) -> TaskInfo {
        let state = self.state.lock().expect("poisoned");
        TaskInfo::Shell(ShellTaskInfo {
            base,
            command: self.spec.command.clone(),
            pid: state.pid,
            exit_code: state.exit_code,
        })
    }
}
