//! Registry of image API providers.
//!
//! 1:1 port of `packages/ai/src/images-api-registry.ts` (53 LOC).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

use crate::types::{
    AssistantImages, ImagesApi, ImagesContext, ImagesModel, ImagesOptions, ImagesStopReason,
    ProviderImages,
};

/// `ImagesApiProvider { api, generateImages }`
pub struct ImagesApiProvider {
    pub api: ImagesApi,
    pub generate_images: Arc<dyn ProviderImages>,
}

fn registry() -> &'static Mutex<BTreeMap<String, Arc<ImagesApiProvider>>> {
    static CELL: OnceLock<Mutex<BTreeMap<String, Arc<ImagesApiProvider>>>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// `registerImagesApiProvider(provider, sourceId?)`
pub fn register_images_api_provider(provider: ImagesApiProvider) {
    registry()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(provider.api.clone(), Arc::new(provider));
}

/// `getImagesApiProvider(api)`
pub fn get_images_api_provider(api: &str) -> Option<Arc<ImagesApiProvider>> {
    registry()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(api)
        .cloned()
}

/// `wrapGenerateImages(api, generateImages)` — the api mismatch check of the registry.
pub async fn generate_images_checked(
    provider: &ImagesApiProvider,
    model: &ImagesModel,
    context: &ImagesContext,
    options: Option<ImagesOptions>,
) -> Result<AssistantImages, String> {
    if model.api != provider.api {
        return Err(format!(
            "Mismatched api: {} expected {}",
            model.api, provider.api
        ));
    }
    Ok(provider
        .generate_images
        .generate_images(model, context, options)
        .await)
}

/// An error result in the shape the image APIs return.
pub fn images_error(
    model: &ImagesModel,
    message: impl Into<String>,
    timestamp: i64,
) -> AssistantImages {
    AssistantImages {
        api: model.api.clone(),
        provider: model.provider.clone(),
        model: model.id.clone(),
        output: Vec::new(),
        response_id: None,
        usage: None,
        stop_reason: ImagesStopReason::Error,
        error_message: Some(message.into()),
        timestamp,
    }
}
