//! llama.cpp backend of the built-in `/llama` extension.
//!
//! Port of `packages/coding-agent/src/extensions/llama/` — the provider, the
//! management client of a llama.cpp server in router mode and the Hugging Face
//! search behind the model download. `ui.ts` and the command wiring stay with
//! the app workstream (interface request O-8).

pub mod client;
pub mod huggingface;
pub mod provider;
