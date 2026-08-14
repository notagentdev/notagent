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

// ---------------------------------------------------------------------------
// Flache Oberfläche von `src/index.ts` (47 LOC)
//
// TypeScripts `export type { Static, TSchema }` und `export { Type }` entfallen:
// Schemas sind `serde_json::Value` (Master-Substitutionen, Klasse 3); die
// Hilfsfunktionen dazu stehen in `utils::typebox_helpers`.
// ---------------------------------------------------------------------------

// `export type { AnthropicEffort, AnthropicOptions, AnthropicThinkingDisplay }`
pub use api::anthropic_params::{AnthropicEffort, AnthropicOptions, AnthropicThinkingDisplay};
// `export type { AzureOpenAIResponsesOptions }`
pub use api::azure_openai_responses::AzureOpenAIResponsesOptions;
// `export type { BedrockOptions, BedrockThinkingDisplay }`
pub use api::bedrock_converse_stream::{BedrockOptions, BedrockThinkingDisplay};
// `export type { GoogleOptions }` / `{ GoogleThinkingLevel }` / `{ GoogleVertexOptions }`
pub use api::google_generative_ai::GoogleOptions;
pub use api::google_shared::GoogleThinkingLevel;
pub use api::google_vertex::GoogleVertexOptions;
// `export * from "./api/lazy.ts"`
pub use api::lazy::{create_setup_error_message, lazy_stream};
// `export type { MistralOptions }`
pub use api::mistral_conversations::MistralOptions;
// `export type { OpenAICodexResponsesOptions, OpenAICodexWebSocketDebugStats }`
pub use api::openai_codex_responses::{
    OpenAICodexResponsesOptions, OpenAICodexWebSocketDebugStats,
};
// `export type { OpenAICompletionsOptions }`
pub use api::openai_completions_params::OpenAICompletionsOptions;
// `export type { OpenAIResponsesOptions }`
pub use api::openai_responses::OpenAIResponsesOptions;
// `export type { PiMessagesOptions, ... }` — `PiMessagesEvent` und
// `PiMessagesRewriteImpact` sind Wire-Typen des vorserialisierten Streams und im Port
// `serde_json::Value` (Klasse 1, siehe PARITY.md), haben also keinen eigenen Namen.
pub use api::pi_messages::PiMessagesOptions;
// `export * from "./auth/context.ts"`
pub use auth::context::{default_provider_auth_context, expand_home};
// `export * from "./auth/credential-store.ts"`
pub use auth::credential_store::InMemoryCredentialStore;
// `export * from "./auth/helpers.ts"`
pub use auth::helpers::EnvApiKeyAuth;
// `export * from "./auth/types.ts"`
pub use auth::types::*;
// `export type { OAuthAuthInfo, OAuthDeviceCodeInfo, OAuthLoginCallbacks, OAuthPrompt,
//   OAuthSelectOption, OAuthSelectPrompt }`
pub use compat::extension_oauth_types::{
    OAuthAuthInfo, OAuthDeviceCodeInfo, OAuthLoginCallbacks, OAuthPrompt, OAuthSelectOption,
    OAuthSelectPrompt,
};
// `export * from "./images-models.ts"`
pub use images_models::{
    BuiltImagesProvider, CreateImagesProviderOptions, ImagesModels, ImagesProvider,
    create_images_models, create_images_provider,
};
// `export * from "./models.ts"`
pub use models::{
    CreateModelsOptions, CreateProviderOptions, Models, ModelsPublication, ModelsRefreshOptions,
    ModelsRefreshResult, Provider, ProviderApis, RefreshModelsContext, calculate_cost,
    clamp_thinking_level, create_models, create_provider, get_supported_thinking_levels, has_api,
    models_are_equal,
};
// `export * from "./models-store.ts"`
pub use models_store::{InMemoryModelsStore, ModelsStore, ModelsStoreEntry};
// `export * from "./providers/faux.ts"`
pub use providers::faux::{
    FauxCore, FauxModelDefinition, FauxProviderHandle, FauxProviderOptions, FauxProviderState,
    FauxResponseFactory, FauxResponseStep, create_faux_core, faux_assistant_message, faux_provider,
    faux_text, faux_thinking, faux_tool_call,
};
// `export * from "./session-resources.ts"`
pub use session_resources::{
    SessionResourceCleanup, SessionResourceCleanupHandle, cleanup_session_resources,
    register_session_resource_cleanup,
};
// `export * from "./types.ts"`
pub use types::*;
// `export * from "./utils/diagnostics.ts"`
pub use utils::diagnostics::{AssistantMessageDiagnostic, DiagnosticErrorInfo};
// `export * from "./utils/event-stream.ts"`
pub use utils::event_stream::{
    AssistantMessageEventStream, EventStream, create_assistant_message_event_stream,
};
// `export * from "./utils/json-parse.ts"`
pub use utils::json_parse::parse_streaming_json;
// `export * from "./utils/overflow.ts"`
pub use utils::overflow::{
    NON_OVERFLOW_PATTERNS, OVERFLOW_PATTERNS, get_overflow_patterns, is_context_overflow,
    is_recoverable_length,
};
// `export * from "./utils/retry.ts"`
pub use utils::retry::{
    OnRetryAttemptStart, OnRetryFinished, OnRetryScheduled, RetryCallbacks, RetryPolicy,
    is_retryable_assistant_error,
};
// `export { contentText }`
pub use utils::text::content_text;
// `export * from "./utils/typebox-helpers.ts"`
pub use utils::typebox_helpers::string_enum;
// `export { uuidv7 }`
pub use utils::uuid::uuidv7;
// `export * from "./utils/validation.ts"`
pub use utils::validation::{
    SchemaOrigin, ToolValidationError, ValidationError, coerce_with_json_schema,
    normalize_optional_nulls, validate_tool_arguments, validate_tool_arguments_with,
    validate_tool_call,
};
