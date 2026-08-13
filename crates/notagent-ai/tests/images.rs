//! Port of `packages/ai/test/openrouter-images.test.ts` (140 LOC) and
//! `packages/ai/test/images-models.test.ts` (209 LOC).
//!
//! The TS suite mocks the `openai` SDK; the port injects a canned `fetch`, because the
//! adapter speaks the same REST call directly (substitution class 3).

use std::sync::{Arc, Mutex};

use notagent_ai::auth::types::{AuthContext, BoxFuture};
use notagent_ai::images::generate_images;
use notagent_ai::images_models::{
    CreateImagesProviderOptions, ImagesProvider, builtin_images_models, create_images_models,
    create_images_provider,
};
use notagent_ai::models::CreateModelsOptions;
use notagent_ai::types::{
    AssistantImages, ImageContent, ImagesContext, ImagesInputContent, ImagesModel, ImagesOptions,
    ImagesOutputContent, ImagesStopReason, Modality, ModelCost, ProviderImages,
    ProviderRequestOptions, TextContent,
};
use notagent_ai::utils::fetch::{FetchBody, FetchError, FetchFn, FetchRequest, FetchResponse};
use serde_json::{Value, json};

fn image_model(id: &str, output: &[Modality]) -> ImagesModel {
    ImagesModel {
        id: id.to_string(),
        name: id.to_string(),
        api: "openrouter-images".to_string(),
        provider: "openrouter".to_string(),
        base_url: "https://openrouter.ai/api/v1".to_string(),
        thinking_level_map: None,
        input: vec![Modality::Text, Modality::Image],
        output: output.to_vec(),
        cost: ModelCost {
            input: 0.015,
            output: 0.03,
            cache_read: 0.0,
            cache_write: 0.0,
            ..ModelCost::default()
        },
        sampling_params: None,
        headers: None,
    }
}

fn context() -> ImagesContext {
    ImagesContext {
        input: vec![ImagesInputContent::Text(TextContent {
            text: "Generate a dog".to_string(),
            ..TextContent::default()
        })],
    }
}

struct CannedFetch {
    body: String,
    seen: Arc<Mutex<Option<FetchRequest>>>,
}

impl FetchFn for CannedFetch {
    fn fetch(
        &self,
        request: FetchRequest,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<FetchResponse, FetchError>> + Send>,
    > {
        *self.seen.lock().expect("poisoned") = Some(request);
        let body = self.body.clone();
        Box::pin(async move {
            Ok(FetchResponse {
                status: 200,
                status_text: String::new(),
                headers: vec![("content-type".to_string(), "application/json".to_string())],
                body: FetchBody::Bytes(body.into_bytes()),
            })
        })
    }
}

fn canned_response() -> String {
    json!({
        "id": "img-1",
        "usage": {
            "prompt_tokens": 12,
            "completion_tokens": 34,
            "prompt_tokens_details": { "cached_tokens": 0 },
        },
        "choices": [{
            "message": {
                "content": "Here is your image.",
                "images": [{ "image_url": "data:image/png;base64,ZmFrZS1wbmc=" }],
            },
        }],
    })
    .to_string()
}

#[tokio::test]
async fn returns_text_plus_images_in_the_final_output() {
    let seen = Arc::new(Mutex::new(None));
    let mut model = image_model(
        "google/gemini-3.1-flash-image-preview",
        &[Modality::Text, Modality::Image],
    );
    model.headers = Some(
        [(
            "HTTP-Referer".to_string(),
            "https://example.com".to_string(),
        )]
        .into_iter()
        .collect(),
    );

    let output = generate_images(
        &model,
        &context(),
        Some(ImagesOptions {
            base: ProviderRequestOptions {
                api_key: Some("test".to_string()),
                fetch: Some(Arc::new(CannedFetch {
                    body: canned_response(),
                    seen: seen.clone(),
                })),
                ..ProviderRequestOptions::default()
            },
            metadata: None,
        }),
    )
    .await
    .expect("registered api");

    assert_eq!(output.stop_reason, ImagesStopReason::Stop);
    assert_eq!(output.response_id.as_deref(), Some("img-1"));
    assert_eq!(
        output.output[0],
        ImagesOutputContent::Text(TextContent {
            text: "Here is your image.".to_string(),
            ..TextContent::default()
        })
    );
    assert_eq!(
        output.output[1],
        ImagesOutputContent::Image(ImageContent {
            data: "ZmFrZS1wbmc=".to_string(),
            mime_type: "image/png".to_string(),
        })
    );

    let request = seen.lock().expect("poisoned").clone().expect("request");
    let body: Value =
        serde_json::from_slice(request.body.as_deref().expect("body")).expect("json body");
    assert_eq!(body["stream"], json!(false));
    assert_eq!(body["modalities"], json!(["image", "text"]));
    assert_eq!(
        body["messages"][0]["content"][0],
        json!({ "type": "text", "text": "Generate a dog" })
    );
    // The model headers reach the request, as the SDK's `defaultHeaders` would.
    assert!(
        request
            .headers
            .iter()
            .any(|(name, value)| name == "HTTP-Referer" && value == "https://example.com")
    );
}

#[tokio::test]
async fn image_only_models_request_the_image_modality_only() {
    let seen = Arc::new(Mutex::new(None));
    let model = image_model("black-forest-labs/flux.2-pro", &[Modality::Image]);

    let output = generate_images(
        &model,
        &context(),
        Some(ImagesOptions {
            base: ProviderRequestOptions {
                api_key: Some("test".to_string()),
                fetch: Some(Arc::new(CannedFetch {
                    body: canned_response(),
                    seen: seen.clone(),
                })),
                ..ProviderRequestOptions::default()
            },
            metadata: None,
        }),
    )
    .await
    .expect("registered api");

    assert!(
        output
            .output
            .iter()
            .any(|item| matches!(item, ImagesOutputContent::Image(_)))
    );
    let request = seen.lock().expect("poisoned").clone().expect("request");
    let body: Value =
        serde_json::from_slice(request.body.as_deref().expect("body")).expect("json body");
    assert_eq!(body["modalities"], json!(["image"]));
}

#[tokio::test]
async fn passes_through_the_abort_signal_and_returns_an_aborted_result() {
    let model = image_model("black-forest-labs/flux.2-pro", &[Modality::Image]);
    let signal = tokio_util::sync::CancellationToken::new();
    signal.cancel();

    struct AbortingFetch;
    impl FetchFn for AbortingFetch {
        fn fetch(
            &self,
            _request: FetchRequest,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<FetchResponse, FetchError>> + Send>,
        > {
            Box::pin(async move {
                Err(FetchError {
                    message: "Request aborted".to_string(),
                })
            })
        }
    }

    let output = generate_images(
        &model,
        &context(),
        Some(ImagesOptions {
            base: ProviderRequestOptions {
                api_key: Some("test".to_string()),
                signal: Some(signal),
                fetch: Some(Arc::new(AbortingFetch)),
                ..ProviderRequestOptions::default()
            },
            metadata: None,
        }),
    )
    .await
    .expect("registered api");

    assert_eq!(output.stop_reason, ImagesStopReason::Aborted);
    assert_eq!(output.error_message.as_deref(), Some("Request aborted"));
}

#[tokio::test]
async fn reports_a_missing_api_key() {
    let model = image_model("black-forest-labs/flux.2-pro", &[Modality::Image]);
    let output = generate_images(&model, &context(), None)
        .await
        .expect("registered api");
    assert_eq!(output.stop_reason, ImagesStopReason::Error);
    assert_eq!(
        output.error_message.as_deref(),
        Some("No API key for provider: openrouter")
    );
}

#[tokio::test]
async fn rejects_an_unregistered_api() {
    let mut model = image_model("x", &[Modality::Image]);
    model.api = "unknown-images".to_string();
    let error = generate_images(&model, &context(), None)
        .await
        .expect_err("unregistered");
    assert_eq!(error, "No API provider registered for api: unknown-images");
}

// === ImagesModels ===

/// The `calls` array of the TS `testProvider`.
type GenerateCalls = Arc<Mutex<Vec<(ImagesModel, Option<ImagesOptions>)>>>;

struct RecordingImagesApi {
    calls: GenerateCalls,
}

impl ProviderImages for RecordingImagesApi {
    fn generate_images(
        &self,
        model: &ImagesModel,
        _context: &ImagesContext,
        options: Option<ImagesOptions>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = AssistantImages> + Send>> {
        self.calls
            .lock()
            .expect("poisoned")
            .push((model.clone(), options));
        let model = model.clone();
        Box::pin(async move {
            AssistantImages {
                api: model.api.clone(),
                provider: model.provider.clone(),
                model: model.id.clone(),
                output: vec![ImagesOutputContent::Image(ImageContent {
                    data: "aGk=".to_string(),
                    mime_type: "image/png".to_string(),
                })],
                response_id: None,
                usage: None,
                stop_reason: ImagesStopReason::Stop,
                error_message: None,
                timestamp: 0,
            }
        })
    }
}

fn test_image_model(provider: &str, id: &str) -> ImagesModel {
    ImagesModel {
        id: id.to_string(),
        name: id.to_string(),
        api: "test-images".to_string(),
        provider: provider.to_string(),
        base_url: "https://example.test/v1".to_string(),
        thinking_level_map: None,
        input: vec![Modality::Text],
        output: vec![Modality::Image],
        cost: ModelCost::default(),
        sampling_params: None,
        headers: None,
    }
}

struct FakeAuthContext {
    env: std::collections::BTreeMap<String, String>,
}

impl AuthContext for FakeAuthContext {
    fn env(&self, name: &str) -> BoxFuture<'_, Option<String>> {
        let value = self.env.get(name).cloned();
        Box::pin(async move { value })
    }

    fn file_exists(&self, _path: &str) -> BoxFuture<'_, bool> {
        Box::pin(async move { false })
    }
}

fn test_provider(id: &str, env_var: Option<&str>, calls: GenerateCalls) -> Arc<dyn ImagesProvider> {
    create_images_provider(CreateImagesProviderOptions {
        id: id.to_string(),
        name: None,
        auth: notagent_ai::auth::types::ProviderAuth {
            api_key: Some(Arc::new(notagent_ai::auth::helpers::EnvApiKeyAuth::new(
                "Test key",
                env_var.into_iter().collect::<Vec<_>>(),
            ))),
            oauth: None,
        },
        models: vec![test_image_model(id, "model-a")],
        refresh_models: None,
        api: Arc::new(RecordingImagesApi { calls }),
    })
}

#[tokio::test]
async fn registers_providers_and_reads_models_synchronously() {
    let models = create_images_models(None);
    let calls = Arc::new(Mutex::new(Vec::new()));
    models.set_provider(test_provider("alpha", None, calls.clone()));
    models.set_provider(test_provider("beta", None, calls));

    assert_eq!(
        models
            .get_providers()
            .iter()
            .map(|provider| provider.id().to_string())
            .collect::<Vec<_>>(),
        ["alpha", "beta"]
    );
    assert_eq!(models.get_models(None).len(), 2);
    assert_eq!(models.get_models(Some("alpha")).len(), 1);
    assert!(models.get_model("alpha", "model-a").is_some());
    assert!(models.get_model("alpha", "missing").is_none());

    models.delete_provider("alpha");
    assert_eq!(models.get_providers().len(), 1);
    models.clear_providers();
    assert!(models.get_providers().is_empty());
}

#[tokio::test]
async fn returns_an_error_result_for_unknown_providers() {
    let models = create_images_models(None);
    let output = models
        .generate_images(&test_image_model("missing", "model-a"), &context(), None)
        .await;
    assert_eq!(output.stop_reason, ImagesStopReason::Error);
    assert_eq!(
        output.error_message.as_deref(),
        Some("Unknown provider: missing")
    );
}

#[tokio::test]
async fn resolves_auth_and_merges_it_into_requests() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let models = create_images_models(Some(CreateModelsOptions {
        auth_context: Some(Arc::new(FakeAuthContext {
            env: [("TEST_KEY".to_string(), "env-key".to_string())]
                .into_iter()
                .collect(),
        })),
        ..CreateModelsOptions::default()
    }));
    models.set_provider(test_provider("alpha", Some("TEST_KEY"), calls.clone()));

    let output = models
        .generate_images(&test_image_model("alpha", "model-a"), &context(), None)
        .await;
    assert_eq!(output.stop_reason, ImagesStopReason::Stop);
    assert_eq!(
        calls.lock().expect("poisoned")[0]
            .1
            .as_ref()
            .and_then(|options| options.base.api_key.clone()),
        Some("env-key".to_string()),
        "the resolved key reaches the request"
    );

    // An explicit key wins over the resolved one.
    let output = models
        .generate_images(
            &test_image_model("alpha", "model-a"),
            &context(),
            Some(ImagesOptions {
                base: ProviderRequestOptions {
                    api_key: Some("explicit".to_string()),
                    ..ProviderRequestOptions::default()
                },
                metadata: None,
            }),
        )
        .await;
    assert_eq!(output.stop_reason, ImagesStopReason::Stop);
    assert_eq!(
        calls.lock().expect("poisoned")[1]
            .1
            .as_ref()
            .and_then(|options| options.base.api_key.clone()),
        Some("explicit".to_string())
    );
}

#[tokio::test]
async fn builtin_images_models_registers_the_openrouter_provider_with_its_catalog() {
    let models = builtin_images_models(Some(CreateModelsOptions {
        auth_context: Some(Arc::new(FakeAuthContext {
            env: [("OPENROUTER_API_KEY".to_string(), "or-key".to_string())]
                .into_iter()
                .collect(),
        })),
        ..CreateModelsOptions::default()
    }));

    assert_eq!(
        models
            .get_providers()
            .iter()
            .map(|provider| provider.id().to_string())
            .collect::<Vec<_>>(),
        ["openrouter"]
    );

    let list = models.get_models(Some("openrouter"));
    assert!(!list.is_empty());
    assert!(list.iter().all(|model| model.api == "openrouter-images"));

    let auth = models
        .get_auth("openrouter", None)
        .await
        .expect("auth resolves")
        .expect("configured");
    assert_eq!(auth.auth.api_key.as_deref(), Some("or-key"));
}
