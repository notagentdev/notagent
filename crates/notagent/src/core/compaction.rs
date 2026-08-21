//! Port of `packages/coding-agent/src/core/compaction/`.
//!
//! Compaction and branch summarization: the two places where the session hands
//! part of its own history to a model and keeps the answer instead.

pub mod branch_summarization;
// The TypeScript directory has a `compaction.ts` inside `compaction/`; keeping
// the same file names keeps the ledger a one-to-one mapping.
#[allow(clippy::module_inception)]
pub mod compaction;
pub mod retention;
pub mod utils;

pub use branch_summarization::*;
pub use compaction::*;
pub use retention::*;
pub use utils::*;
