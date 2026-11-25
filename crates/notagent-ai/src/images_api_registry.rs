use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

use crate::types::{
    AssistantImages, ImagesApi, ImagesContext, ImagesModel, ImagesOptions, ProviderImages,
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

pub fn register_images_api_provider(provider: ImagesApiProvider) {
    registry()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(provider.api.clone(), Arc::new(provider));
}

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
