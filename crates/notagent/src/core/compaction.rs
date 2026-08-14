//! Port of `packages/coding-agent/src/core/compaction/`.
//!
//! Compaction and branch summarization: the two places where the session hands
//! part of its own history to a model and keeps the answer instead.

pub mod branch_summarization;
pub mod compaction;
pub mod utils;

pub use branch_summarization::*;
pub use compaction::*;
pub use utils::*;
