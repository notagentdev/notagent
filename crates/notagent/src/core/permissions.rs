//! Port of `packages/coding-agent/src/core/permissions/`.
//!
//! Approval is decided by an ordered list of policies. In TypeScript the chain
//! is attached through a hidden inline extension (`permissions/extension.ts`);
//! the Rust port has no extension system, so [`gate::PermissionGate`] is the
//! native pre-tool gate the agent loop calls directly. Everything the chain
//! itself decides is unchanged — same policies, same order, same wording.

pub mod chain;
pub mod coordinator;
pub mod gate;
pub mod hook;
pub mod policies;
pub mod policy;
pub mod request;
/// Who is asking, when it is not the main agent.
pub mod requester;
pub mod user_rules;
