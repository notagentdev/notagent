//! `ProviderStreams` implementations of the ported API modules.
//!
//! Port of the nine `packages/ai/src/api/*.lazy.ts` wrappers (`anthropicMessagesApi()`
//! and friends). Deviation class 4: the wrappers exist so a bundler can split the
//! dynamically imported module out of browser builds — Rust links statically, so each
//! wrapper collapses to a unit struct that forwards into its module.
//!
//! Deviation class 1: `ProviderStreams::stream` receives the generic [`StreamOptions`];
//! the provider-specific extras of each adapter (thinking, tool choice, …) have no
//! counterpart there and stay unset, exactly as they would for a TS caller that passes a
//! plain `StreamOptions` object.

use crate::api::{
    anthropic_messages, azure_openai_responses, bedrock_converse_stream, google_generative_ai,
    google_vertex, mistral_conversations, openai_codex_responses, openai_completions,
    openai_responses, pi_messages,
};
use crate::types::{Context, Model, ProviderStreams, SimpleStreamOptions, StreamOptions};
use crate::utils::event_stream::AssistantMessageEventStream;

/// `anthropicMessagesApi()`
pub struct AnthropicMessagesApi;

impl ProviderStreams for AnthropicMessagesApi {
    fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: Option<StreamOptions>,
    ) -> AssistantMessageEventStream {
        let options = options.unwrap_or_default();
        anthropic_messages::stream(
            model.clone(),
            context.clone(),
            options.base.clone(),
            crate::api::anthropic_params::AnthropicOptions {
                max_tokens: options.max_tokens,
                temperature: options.temperature,
                cache_retention: options.cache_retention,
                metadata: options.metadata.clone(),
                env: options.base.env.clone(),
                ..Default::default()
            },
        )
    }

    fn stream_simple(
        &self,
        model: &Model,
        context: &Context,
        options: Option<SimpleStreamOptions>,
    ) -> AssistantMessageEventStream {
        anthropic_messages::stream_simple(model.clone(), context.clone(), options)
    }
}

/// `openAICompletionsApi()`
pub struct OpenAICompletionsApi;

impl ProviderStreams for OpenAICompletionsApi {
    fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: Option<StreamOptions>,
    ) -> AssistantMessageEventStream {
        let options = options.unwrap_or_default();
        openai_completions::stream(
            model.clone(),
            context.clone(),
            options.base.clone(),
            crate::api::openai_completions_params::OpenAICompletionsOptions {
                max_tokens: options.max_tokens,
                temperature: options.temperature,
                sampling_params: options.sampling_params.clone(),
                cache_retention: options.cache_retention,
                session_id: options.session_id.clone(),
                env: options.base.env.clone(),
                ..Default::default()
            },
        )
    }

    fn stream_simple(
        &self,
        model: &Model,
        context: &Context,
        options: Option<SimpleStreamOptions>,
    ) -> AssistantMessageEventStream {
        openai_completions::stream_simple(model.clone(), context.clone(), options)
    }
}

/// `openAIResponsesApi()`
pub struct OpenAIResponsesApi;

impl ProviderStreams for OpenAIResponsesApi {
    fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: Option<StreamOptions>,
    ) -> AssistantMessageEventStream {
        let options = options.unwrap_or_default();
        openai_responses::stream(
            model.clone(),
            context.clone(),
            options.base.clone(),
            openai_responses::OpenAIResponsesOptions {
                max_tokens: options.max_tokens,
                temperature: options.temperature,
                sampling_params: options.sampling_params.clone(),
                cache_retention: options.cache_retention,
                session_id: options.session_id.clone(),
                env: options.base.env.clone(),
                ..Default::default()
            },
        )
    }

    fn stream_simple(
        &self,
        model: &Model,
        context: &Context,
        options: Option<SimpleStreamOptions>,
    ) -> AssistantMessageEventStream {
        openai_responses::stream_simple(model.clone(), context.clone(), options)
    }
}

/// `azureOpenAIResponsesApi()`
pub struct AzureOpenAIResponsesApi;

impl ProviderStreams for AzureOpenAIResponsesApi {
    fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: Option<StreamOptions>,
    ) -> AssistantMessageEventStream {
        let options = options.unwrap_or_default();
        azure_openai_responses::stream(
            model.clone(),
            context.clone(),
            options.base.clone(),
            azure_openai_responses::AzureOpenAIResponsesOptions {
                max_tokens: options.max_tokens,
                temperature: options.temperature,
                sampling_params: options.sampling_params.clone(),
                session_id: options.session_id.clone(),
                env: options.base.env.clone(),
                ..Default::default()
            },
        )
    }

    fn stream_simple(
        &self,
        model: &Model,
        context: &Context,
        options: Option<SimpleStreamOptions>,
    ) -> AssistantMessageEventStream {
        let model_for_error = model.clone();
        azure_openai_responses::stream_simple(model.clone(), context.clone(), options)
            .unwrap_or_else(|error| error_stream(&model_for_error, &error.to_string()))
    }
}

/// `openAICodexResponsesApi()`
pub struct OpenAICodexResponsesApi;

impl ProviderStreams for OpenAICodexResponsesApi {
    fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: Option<StreamOptions>,
    ) -> AssistantMessageEventStream {
        let options = options.unwrap_or_default();
        openai_codex_responses::stream(
            model.clone(),
            context.clone(),
            options.base.clone(),
            openai_codex_responses::OpenAICodexResponsesOptions {
                temperature: options.temperature,
                cache_retention: options.cache_retention,
                session_id: options.session_id.clone(),
                transport: options.transport,
                websocket_connect_timeout_ms: options.websocket_connect_timeout_ms,
                env: options.base.env.clone(),
                ..Default::default()
            },
        )
    }

    fn stream_simple(
        &self,
        model: &Model,
        context: &Context,
        options: Option<SimpleStreamOptions>,
    ) -> AssistantMessageEventStream {
        let model_for_error = model.clone();
        openai_codex_responses::stream_simple(model.clone(), context.clone(), options)
            .unwrap_or_else(|error| error_stream(&model_for_error, &error.to_string()))
    }
}

/// `googleGenerativeAIApi()`
pub struct GoogleGenerativeAIApi;

impl ProviderStreams for GoogleGenerativeAIApi {
    fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: Option<StreamOptions>,
    ) -> AssistantMessageEventStream {
        let options = options.unwrap_or_default();
        google_generative_ai::stream(
            model.clone(),
            context.clone(),
            options.base.clone(),
            google_generative_ai::GoogleOptions {
                max_tokens: options.max_tokens,
                temperature: options.temperature,
                ..Default::default()
            },
        )
    }

    fn stream_simple(
        &self,
        model: &Model,
        context: &Context,
        options: Option<SimpleStreamOptions>,
    ) -> AssistantMessageEventStream {
        let model_for_error = model.clone();
        google_generative_ai::stream_simple(model.clone(), context.clone(), options)
            .unwrap_or_else(|error| error_stream(&model_for_error, &error.to_string()))
    }
}

/// `googleVertexApi()`
pub struct GoogleVertexApi;

impl ProviderStreams for GoogleVertexApi {
    fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: Option<StreamOptions>,
    ) -> AssistantMessageEventStream {
        let options = options.unwrap_or_default();
        google_vertex::stream(
            model.clone(),
            context.clone(),
            options.base.clone(),
            google_vertex::GoogleVertexOptions {
                max_tokens: options.max_tokens,
                temperature: options.temperature,
                ..Default::default()
            },
            None,
        )
    }

    fn stream_simple(
        &self,
        model: &Model,
        context: &Context,
        options: Option<SimpleStreamOptions>,
    ) -> AssistantMessageEventStream {
        google_vertex::stream_simple(model.clone(), context.clone(), options, None)
    }
}

/// `mistralConversationsApi()`
pub struct MistralConversationsApi;

impl ProviderStreams for MistralConversationsApi {
    fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: Option<StreamOptions>,
    ) -> AssistantMessageEventStream {
        let options = options.unwrap_or_default();
        mistral_conversations::stream(
            model.clone(),
            context.clone(),
            options.base.clone(),
            mistral_conversations::MistralOptions {
                max_tokens: options.max_tokens,
                temperature: options.temperature,
                cache_retention: options.cache_retention,
                session_id: options.session_id.clone(),
                headers: options.base.headers.clone(),
                ..Default::default()
            },
        )
    }

    fn stream_simple(
        &self,
        model: &Model,
        context: &Context,
        options: Option<SimpleStreamOptions>,
    ) -> AssistantMessageEventStream {
        let model_for_error = model.clone();
        mistral_conversations::stream_simple(model.clone(), context.clone(), options)
            .unwrap_or_else(|error| error_stream(&model_for_error, &error.message))
    }
}

/// `piMessagesApi()`
pub struct PiMessagesApi;

impl ProviderStreams for PiMessagesApi {
    fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: Option<StreamOptions>,
    ) -> AssistantMessageEventStream {
        let options = options.unwrap_or_default();
        pi_messages::stream(
            model.clone(),
            context.clone(),
            options.base.clone(),
            pi_messages::PiMessagesOptions {
                max_tokens: options.max_tokens,
                temperature: options.temperature,
                cache_retention: options.cache_retention,
                session_id: options.session_id.clone(),
                headers: options.base.headers.clone(),
                env: options.base.env.clone(),
                ..Default::default()
            },
        )
    }

    fn stream_simple(
        &self,
        model: &Model,
        context: &Context,
        options: Option<SimpleStreamOptions>,
    ) -> AssistantMessageEventStream {
        pi_messages::stream_simple(model.clone(), context.clone(), options)
    }
}

/// A stream that only carries the terminal error event, as `lazyStream` does for a
/// failed setup.
fn error_stream(model: &Model, message: &str) -> AssistantMessageEventStream {
    let stream = crate::utils::event_stream::create_assistant_message_event_stream();
    let error = crate::api::lazy::create_setup_error_message(
        model,
        &message,
        crate::auth::resolve::now_ms(),
    );
    stream.push(crate::types::AssistantMessageEvent::Error {
        reason: crate::types::ErrorReason::Error,
        error: error.clone(),
    });
    stream.end(Some(error));
    stream
}

/// `bedrockConverseStreamApi()`
pub struct BedrockConverseStreamApi;

impl ProviderStreams for BedrockConverseStreamApi {
    fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: Option<StreamOptions>,
    ) -> AssistantMessageEventStream {
        let options = options.unwrap_or_default();
        bedrock_converse_stream::stream(
            model.clone(),
            context.clone(),
            options.base.clone(),
            bedrock_converse_stream::BedrockOptions {
                max_tokens: options.max_tokens,
                temperature: options.temperature,
                cache_retention: options.cache_retention,
                api_key: options.base.api_key.clone(),
                env: options.base.env.clone(),
                ..Default::default()
            },
        )
    }

    fn stream_simple(
        &self,
        model: &Model,
        context: &Context,
        options: Option<SimpleStreamOptions>,
    ) -> AssistantMessageEventStream {
        bedrock_converse_stream::stream_simple(model.clone(), context.clone(), options)
    }
}
