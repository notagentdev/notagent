//! Multi-Provider-LLM-Layer (Streaming, Auth, Modellkatalog).
//!
//! 1:1-Port von `packages/ai` (siehe `crates/notagent-ai/PARITY.md`).

pub mod api;
pub mod auth;
pub mod compat;
pub mod env_api_keys;
pub mod images;
pub mod images_api_registry;
pub mod images_models;
pub mod model_catalog;
pub mod models;
pub mod models_store;
pub mod providers;
pub mod session_resources;
pub mod types;
pub mod utils;

// `index.ts` re-exports these modules flat; the Rust surface mirrors that for the
// symbols consumers name directly.
pub use auth::helpers::EnvApiKeyAuth;
pub use auth::types::*;
pub use models::{
    CreateModelsOptions, CreateProviderOptions, Models, ModelsPublication, ModelsRefreshOptions,
    ModelsRefreshResult, Provider, ProviderApis, RefreshModelsContext, create_models,
    create_provider,
};
pub use models_store::{InMemoryModelsStore, ModelsStore, ModelsStoreEntry};
pub use types::*;
pub use utils::diagnostics::{AssistantMessageDiagnostic, DiagnosticErrorInfo};
pub use utils::event_stream::{
    AssistantMessageEventStream, EventStream, create_assistant_message_event_stream,
};
pub use utils::json_parse::parse_streaming_json;
pub use utils::text::content_text;
pub use utils::typebox_helpers::string_enum;
pub use utils::uuid::uuidv7;
