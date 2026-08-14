//! Port of `packages/coding-agent/src/core/tasks/`.
//!
//! Every piece of background work — a detached shell command, a delegated
//! subagent — has its lifecycle here, and both run as real tokio tasks.

pub mod manager;
pub mod notification;
pub mod output;
pub mod serial_queue;
pub mod shell_task;
pub mod store;
pub mod subagent_task;
pub mod types;
