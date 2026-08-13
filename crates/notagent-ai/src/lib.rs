//! Multi-Provider-LLM-Layer (Streaming, Auth, Modellkatalog).
//!
//! 1:1-Port von `packages/ai` (siehe `crates/notagent-ai/PARITY.md`).

pub mod api;
pub mod auth;
pub mod env_api_keys;
pub mod model_catalog;
pub mod models;
pub mod models_store;
pub mod providers;
pub mod session_resources;
pub mod types;
pub mod utils;

pub use types::*;
pub use utils::diagnostics::{AssistantMessageDiagnostic, DiagnosticErrorInfo};
pub use utils::event_stream::{
    AssistantMessageEventStream, EventStream, create_assistant_message_event_stream,
};
pub use utils::uuid::uuidv7;
