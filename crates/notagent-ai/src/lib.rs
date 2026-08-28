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

/// Installs the process-wide rustls default. Both `ring` (via reqwest) and
/// `aws-lc-rs` (via the AWS SDK) are linked into the binaries, so rustls
/// cannot pick a provider on its own and panics on the first TLS handshake.
/// Every binary must call this before any network use; calling it twice is
/// harmless (the second install is ignored).
pub fn install_default_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

// ---------------------------------------------------------------------------
//
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
pub use api::pi_messages::PiMessagesOptions;
pub use auth::context::{default_provider_auth_context, expand_home};
pub use auth::credential_store::InMemoryCredentialStore;
pub use auth::helpers::EnvApiKeyAuth;
pub use auth::types::*;
// `export type { OAuthAuthInfo, OAuthDeviceCodeInfo, OAuthLoginCallbacks, OAuthPrompt,
//   OAuthSelectOption, OAuthSelectPrompt }`
pub use compat::extension_oauth_types::{
    OAuthAuthInfo, OAuthDeviceCodeInfo, OAuthLoginCallbacks, OAuthPrompt, OAuthSelectOption,
    OAuthSelectPrompt,
};
pub use images_models::{
    BuiltImagesProvider, CreateImagesProviderOptions, ImagesModels, ImagesProvider,
    create_images_models, create_images_provider,
};
pub use models::{
    CreateModelsOptions, CreateProviderOptions, Models, ModelsPublication, ModelsRefreshOptions,
    ModelsRefreshResult, Provider, ProviderApis, RefreshModelsContext, calculate_cost,
    clamp_thinking_level, create_models, create_provider, get_supported_thinking_levels, has_api,
    models_are_equal,
};
pub use models_store::{InMemoryModelsStore, ModelsStore, ModelsStoreEntry};
pub use providers::faux::{
    FauxCore, FauxModelDefinition, FauxProviderHandle, FauxProviderOptions, FauxProviderState,
    FauxResponseFactory, FauxResponseStep, create_faux_core, faux_assistant_message, faux_provider,
    faux_text, faux_thinking, faux_tool_call,
};
pub use session_resources::{
    SessionResourceCleanup, SessionResourceCleanupHandle, cleanup_session_resources,
    register_session_resource_cleanup,
};
pub use types::*;
pub use utils::diagnostics::{AssistantMessageDiagnostic, DiagnosticErrorInfo};
pub use utils::event_stream::{
    AssistantMessageEventStream, EventStream, create_assistant_message_event_stream,
};
pub use utils::json_parse::parse_streaming_json;
pub use utils::overflow::{
    NON_OVERFLOW_PATTERNS, OVERFLOW_PATTERNS, get_overflow_patterns, is_context_overflow,
    is_recoverable_length,
};
pub use utils::retry::{
    OnRetryAttemptStart, OnRetryFinished, OnRetryScheduled, RetryCallbacks, RetryPolicy,
    is_retryable_assistant_error,
};
// `export { contentText }`
pub use utils::text::content_text;
pub use utils::typebox_helpers::string_enum;
// `export { uuidv7 }`
pub use utils::uuid::uuidv7;
pub use utils::validation::{
    SchemaOrigin, ToolValidationError, ValidationError, coerce_with_json_schema,
    normalize_optional_nulls, validate_tool_arguments, validate_tool_arguments_with,
    validate_tool_call,
};
