//! Provider protocol implementations.
//!
//! 1:1 port of `packages/ai/src/api/`.

pub mod anthropic_messages;
pub mod constrained_sampling;
pub mod github_copilot_headers;
pub mod lazy;
pub mod simple_options;
pub mod sse;
pub mod transform_messages;
