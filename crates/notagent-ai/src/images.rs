use std::sync::{Arc, Once};

use crate::api::openrouter_images::OpenRouterImages;
use crate::images_api_registry::{
    ImagesApiProvider, generate_images_checked, get_images_api_provider,
    register_images_api_provider,
};
use crate::types::{AssistantImages, ImagesContext, ImagesModel, ImagesOptions};

pub fn register_built_in_images_api_providers() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        register_images_api_provider(ImagesApiProvider {
            api: "openrouter-images".to_string(),
            generate_images: Arc::new(OpenRouterImages),
        });
    });
}

/// `generateImages(model, context, options)`
pub async fn generate_images(
    model: &ImagesModel,
    context: &ImagesContext,
    options: Option<ImagesOptions>,
) -> Result<AssistantImages, String> {
    register_built_in_images_api_providers();
    let Some(provider) = get_images_api_provider(&model.api) else {
        return Err(format!("No API provider registered for api: {}", model.api));
    };
    generate_images_checked(&provider, model, context, options).await
}

/// The catalog of built-in image models, keyed by provider id.
pub fn image_models() -> &'static serde_json::Map<String, serde_json::Value> {
    static CELL: std::sync::OnceLock<serde_json::Map<String, serde_json::Value>> =
        std::sync::OnceLock::new();
    CELL.get_or_init(|| {
        serde_json::from_str::<serde_json::Value>(include_str!("../data/image-models.json"))
            .expect("image-models.json parses")
            .as_object()
            .cloned()
            .expect("image-models.json is an object")
    })
}

/// The built-in image models of one provider.
pub fn built_in_image_models(provider: &str) -> Vec<ImagesModel> {
    image_models()
        .get(provider)
        .and_then(serde_json::Value::as_object)
        .map(|models| {
            models
                .values()
                .filter_map(|model| serde_json::from_value(model.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}
