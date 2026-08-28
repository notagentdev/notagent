use std::pin::Pin;
use std::sync::{Arc, Mutex};

use notagent_ai::api::simple_options::build_base_options;
use notagent_ai::auth::types::{
    ApiKeyAuth, ApiKeyAuthInput, AuthError, AuthResult, BoxFuture, ModelAuth, ProviderAuth,
};
use notagent_ai::images::generate_images;
use notagent_ai::images_api_registry::{ImagesApiProvider, register_images_api_provider};
use notagent_ai::images_models::{
    CreateImagesProviderOptions, create_images_models, create_images_provider,
};
use notagent_ai::models::{
    CreateProviderOptions, Provider, ProviderApis, create_models, create_provider,
};
use notagent_ai::types::*;
use notagent_ai::utils::event_stream::{
    AssistantMessageEventStream, create_assistant_message_event_stream,
};
use notagent_telemetry::{TelemetryContext, noop_telemetry_context};

struct EmptyApiKeyAuth;

impl ApiKeyAuth for EmptyApiKeyAuth {
    fn name(&self) -> &str {
        "Test"
    }

    fn resolve<'a>(
        &'a self,
        _input: ApiKeyAuthInput<'a>,
    ) -> BoxFuture<'a, Result<Option<AuthResult>, AuthError>> {
        Box::pin(async {
            Ok(Some(AuthResult {
                auth: ModelAuth::default(),
                env: None,
                source: None,
            }))
        })
    }
}

fn model() -> Model {
    Model {
        id: "model".to_string(),
        name: "Model".to_string(),
        api: "telemetry-test".to_string(),
        provider: "telemetry-provider".to_string(),
        base_url: "https://example.test".to_string(),
        reasoning: false,
        thinking_level_map: None,
        input: vec![Modality::Text],
        cost: ModelCost::default(),
        context_window: 1000,
        max_tokens: 100,
        sampling_params: None,
        headers: None,
        compat: None,
    }
}

fn image_model() -> ImagesModel {
    ImagesModel {
        id: "image-model".to_string(),
        name: "Image Model".to_string(),
        api: "telemetry-test-images".to_string(),
        provider: "telemetry-image-provider".to_string(),
        base_url: "https://example.test".to_string(),
        thinking_level_map: None,
        input: vec![Modality::Text],
        output: vec![Modality::Image],
        cost: ModelCost::default(),
        sampling_params: None,
        headers: None,
    }
}

fn images_context() -> ImagesContext {
    ImagesContext {
        input: vec![ImagesInputContent::Text(TextContent {
            text: "circle".to_string(),
            ..TextContent::default()
        })],
    }
}

fn completed_stream(request_model: &Model) -> AssistantMessageEventStream {
    let stream = create_assistant_message_event_stream();
    let message = AssistantMessage {
        content: Vec::new(),
        api: request_model.api.clone(),
        provider: request_model.provider.clone(),
        model: request_model.id.clone(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: StopReason::Stop,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: 0,
    };
    stream.push(AssistantMessageEvent::Done {
        reason: DoneReason::Stop,
        message: message.clone(),
    });
    stream.end(Some(message));
    stream
}

type Observed = Arc<Mutex<Vec<bool>>>;

/// Records for every dispatch whether the expected telemetry context arrived.
struct RecordingStreams {
    observed: Observed,
    expected: Arc<dyn TelemetryContext>,
}

impl RecordingStreams {
    fn record(&self, seen: Option<&Arc<dyn TelemetryContext>>) {
        let matches = seen.is_some_and(|seen| Arc::ptr_eq(seen, &self.expected));
        self.observed.lock().expect("poisoned").push(matches);
    }
}

impl ProviderStreams for RecordingStreams {
    fn stream(
        &self,
        model: &Model,
        _context: &Context,
        options: Option<StreamOptions>,
    ) -> AssistantMessageEventStream {
        self.record(
            options
                .as_ref()
                .and_then(|options| options.base.telemetry_context.as_ref()),
        );
        completed_stream(model)
    }

    fn stream_simple(
        &self,
        model: &Model,
        _context: &Context,
        options: Option<SimpleStreamOptions>,
    ) -> AssistantMessageEventStream {
        self.record(
            options
                .as_ref()
                .and_then(|options| options.base.base.telemetry_context.as_ref()),
        );
        completed_stream(model)
    }

    fn fetch_deferred(
        &self,
        model: &Model,
        _handle: &DeferredHandle,
        options: Option<DeferredFetchOptions>,
    ) -> Option<AssistantMessageEventStream> {
        self.record(
            options
                .as_ref()
                .and_then(|options| options.base.telemetry_context.as_ref()),
        );
        Some(completed_stream(model))
    }

    fn cancel_deferred(
        &self,
        _model: &Model,
        _handle: &DeferredHandle,
        options: Option<DeferredCancelOptions>,
    ) -> Option<Pin<Box<dyn Future<Output = ()> + Send>>> {
        self.record(
            options
                .as_ref()
                .and_then(|options| options.telemetry_context.as_ref()),
        );
        Some(Box::pin(async {}))
    }

    fn supports_fetch_deferred(&self) -> bool {
        true
    }

    fn supports_cancel_deferred(&self) -> bool {
        true
    }
}

/// The image counterpart of [`RecordingStreams`].
struct RecordingImages {
    observed: Observed,
    expected: Arc<dyn TelemetryContext>,
}

impl ProviderImages for RecordingImages {
    fn generate_images(
        &self,
        model: &ImagesModel,
        _context: &ImagesContext,
        options: Option<ImagesOptions>,
    ) -> Pin<Box<dyn Future<Output = AssistantImages> + Send>> {
        let seen = options
            .as_ref()
            .and_then(|options| options.base.telemetry_context.as_ref())
            .is_some_and(|seen| Arc::ptr_eq(seen, &self.expected));
        self.observed.lock().expect("poisoned").push(seen);
        let result = AssistantImages {
            api: model.api.clone(),
            provider: model.provider.clone(),
            model: model.id.clone(),
            response_id: None,
            output: Vec::new(),
            usage: None,
            stop_reason: ImagesStopReason::Stop,
            error_message: None,
            timestamp: 0,
        };
        Box::pin(async move { result })
    }
}

fn request_options(telemetry: &Arc<dyn TelemetryContext>) -> ProviderRequestOptions<Model> {
    ProviderRequestOptions {
        telemetry_context: Some(Arc::clone(telemetry)),
        ..ProviderRequestOptions::default()
    }
}

#[test]
fn is_inherited_by_every_request_option_surface_and_simple_stream_conversion() {
    let telemetry = noop_telemetry_context();
    let options = request_options(&telemetry);
    assert!(
        options
            .telemetry_context
            .as_ref()
            .is_some_and(|seen| Arc::ptr_eq(seen, &telemetry))
    );

    let simple = SimpleStreamOptions {
        base: StreamOptions {
            base: request_options(&telemetry),
            ..StreamOptions::default()
        },
        ..SimpleStreamOptions::default()
    };
    let built = build_base_options(&model(), &Context::default(), Some(&simple), None);
    assert!(
        built
            .base
            .telemetry_context
            .as_ref()
            .is_some_and(|seen| Arc::ptr_eq(seen, &telemetry))
    );
}

#[tokio::test]
async fn survives_provider_and_models_stream_and_deferred_dispatch() {
    let telemetry = noop_telemetry_context();
    let observed: Observed = Arc::new(Mutex::new(Vec::new()));
    let model = model();
    let handle = DeferredHandle {
        provider: model.provider.clone(),
        model_id: model.id.clone(),
        api: model.api.clone(),
        id: "response".to_string(),
        expires_at: None,
        poll_after_ms: None,
        data: None,
    };
    let provider = create_provider(CreateProviderOptions {
        id: model.provider.clone(),
        name: None,
        base_url: None,
        headers: None,
        auth: ProviderAuth {
            api_key: Some(Arc::new(EmptyApiKeyAuth)),
            oauth: None,
        },
        models: vec![model.clone()],
        fetch_models: None,
        filter_models: None,
        api: ProviderApis::Single(Arc::new(RecordingStreams {
            observed: Arc::clone(&observed),
            expected: Arc::clone(&telemetry),
        })),
    });

    let stream_options = StreamOptions {
        base: request_options(&telemetry),
        ..StreamOptions::default()
    };
    let simple_options = SimpleStreamOptions {
        base: stream_options.clone(),
        ..SimpleStreamOptions::default()
    };

    provider
        .stream(&model, &Context::default(), Some(stream_options.clone()))
        .result()
        .await;
    provider
        .stream_simple(&model, &Context::default(), Some(simple_options.clone()))
        .result()
        .await;
    provider
        .fetch_deferred(
            &model,
            &handle,
            Some(DeferredFetchOptions {
                base: request_options(&telemetry),
                wait: None,
            }),
        )
        .expect("fetch_deferred")
        .result()
        .await;
    provider
        .cancel_deferred(&model, &handle, Some(request_options(&telemetry)))
        .expect("cancel_deferred")
        .await
        .expect("cancel");

    let models = create_models(None);
    models.set_provider(provider);
    models
        .stream(model.clone(), Context::default(), Some(stream_options))
        .result()
        .await;
    models
        .stream_simple(model.clone(), Context::default(), Some(simple_options))
        .result()
        .await;
    models
        .fetch_deferred(
            model.clone(),
            handle.clone(),
            Some(DeferredFetchOptions {
                base: request_options(&telemetry),
                wait: None,
            }),
        )
        .await;
    models
        .cancel_deferred(model.clone(), handle, Some(request_options(&telemetry)))
        .await
        .expect("cancel");

    let observed = observed.lock().expect("poisoned").clone();
    assert_eq!(observed.len(), 8);
    assert!(observed.iter().all(|seen| *seen), "{observed:?}");
}

#[tokio::test]
async fn survives_direct_and_images_models_image_dispatch() {
    let telemetry = noop_telemetry_context();
    let observed: Observed = Arc::new(Mutex::new(Vec::new()));
    let image_model = image_model();

    register_images_api_provider(ImagesApiProvider {
        api: image_model.api.clone(),
        generate_images: Arc::new(RecordingImages {
            observed: Arc::clone(&observed),
            expected: Arc::clone(&telemetry),
        }),
    });
    let _ = generate_images(
        &image_model,
        &images_context(),
        Some(ImagesOptions {
            base: ProviderRequestOptions {
                telemetry_context: Some(Arc::clone(&telemetry)),
                ..ProviderRequestOptions::default()
            },
            metadata: None,
        }),
    )
    .await;

    let models = create_images_models(None);
    models.set_provider(create_images_provider(CreateImagesProviderOptions {
        id: image_model.provider.clone(),
        name: None,
        auth: ProviderAuth {
            api_key: Some(Arc::new(EmptyApiKeyAuth)),
            oauth: None,
        },
        models: vec![image_model.clone()],
        refresh_models: None,
        api: Arc::new(RecordingImages {
            observed: Arc::clone(&observed),
            expected: Arc::clone(&telemetry),
        }),
    }));
    models
        .generate_images(
            &image_model,
            &images_context(),
            Some(ImagesOptions {
                base: ProviderRequestOptions {
                    telemetry_context: Some(Arc::clone(&telemetry)),
                    ..ProviderRequestOptions::default()
                },
                metadata: None,
            }),
        )
        .await;

    assert_eq!(observed.lock().expect("poisoned").clone(), vec![true, true]);
}
