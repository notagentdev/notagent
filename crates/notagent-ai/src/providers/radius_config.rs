use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use crate::auth::types::OAuthCredential;
use crate::types::Model;

pub const DEFAULT_RADIUS_GATEWAY: &str = "https://radius.notagent.dev";

/// `RadiusGatewayConfig { baseUrl, models }` — the models keep their raw JSON, because
#[derive(Debug, Clone, PartialEq)]
pub struct RadiusGatewayConfig {
    pub base_url: String,
    pub models: Vec<Map<String, Value>>,
}

/// `isRadiusGatewayModel(value)`
fn is_radius_gateway_model(value: &Value) -> bool {
    let Some(model) = value.as_object() else {
        return false;
    };
    model.get("id").is_some_and(Value::is_string)
        && model.get("name").is_some_and(Value::is_string)
        && model.get("reasoning").is_some_and(Value::is_boolean)
        && model.get("input").is_some_and(Value::is_array)
        && model.get("cost").is_some_and(Value::is_object)
        && model.get("contextWindow").is_some_and(Value::is_number)
        && model.get("maxTokens").is_some_and(Value::is_number)
}

/// `sanitizeRadiusGatewayConfig(config)`
pub fn sanitize_radius_gateway_config(config: &Value) -> Option<RadiusGatewayConfig> {
    let config = config.as_object()?;
    let base_url = config.get("baseUrl")?.as_str()?.to_string();
    let models = config.get("models")?.as_array()?;
    Some(RadiusGatewayConfig {
        base_url,
        models: models
            .iter()
            .filter(|model| is_radius_gateway_model(model))
            .filter_map(|model| model.as_object().cloned())
            .collect(),
    })
}

/// `normalizeRadiusGatewayUrl(value)`
pub fn normalize_radius_gateway_url(value: &str) -> String {
    let has_scheme = value.len() >= 7
        && (value[..7].eq_ignore_ascii_case("http://")
            || (value.len() >= 8 && value[..8].eq_ignore_ascii_case("https://")));
    let with_scheme = if has_scheme {
        value.to_string()
    } else {
        format!("https://{value}")
    };
    with_scheme.trim_end_matches('/').to_string()
}

/// `getRadiusCredentialConfig(credential)`
pub fn get_radius_credential_config(
    credential: Option<&OAuthCredential>,
) -> Option<RadiusGatewayConfig> {
    sanitize_radius_gateway_config(credential?.extra.get("gatewayConfig")?)
}

/// `getRadiusModelsFromConfig(providerId, config)`
pub fn get_radius_models_from_config(
    provider_id: &str,
    config: &RadiusGatewayConfig,
) -> Vec<Model> {
    config
        .models
        .iter()
        .filter_map(|model| {
            let mut model = model.clone();
            model.insert("api".to_string(), Value::String("pi-messages".to_string()));
            model.insert(
                "provider".to_string(),
                Value::String(provider_id.to_string()),
            );
            model.insert(
                "baseUrl".to_string(),
                Value::String(config.base_url.clone()),
            );
            // A validated gateway model can still miss a field the `Model` type requires
            // producing an ill-formed model.
            serde_json::from_value(Value::Object(model)).ok()
        })
        .collect()
}

/// `getRadiusModels(providerId, credential)`
pub fn get_radius_models(provider_id: &str, credential: Option<&OAuthCredential>) -> Vec<Model> {
    match get_radius_credential_config(credential) {
        Some(config) => get_radius_models_from_config(provider_id, &config),
        None => Vec::new(),
    }
}

/// `truncateHttpBody(body)` — `String.length`/`slice` count UTF-16 code units.
fn truncate_http_body(body: &str) -> String {
    let trimmed = body.trim();
    let units: Vec<u16> = trimmed.encode_utf16().collect();
    if units.len() > 512 {
        format!("{}\u{2026}", String::from_utf16_lossy(&units[..512]))
    } else {
        trimmed.to_string()
    }
}

/// `loadRadiusGatewayConfig(gateway, apiKey?, signal?)`
pub async fn load_radius_gateway_config(
    gateway: &str,
    api_key: Option<&str>,
    signal: &CancellationToken,
) -> Result<RadiusGatewayConfig, String> {
    // `new URL("/v1/config", gateway)` resolves against the origin and drops any path
    // the gateway URL carries.
    let url = url::Url::parse(gateway)
        .and_then(|gateway| gateway.join("/v1/config"))
        .map_err(|error| error.to_string())?;
    let mut request = reqwest::Client::new()
        .get(url)
        .header("accept", "application/json");
    if let Some(api_key) = api_key.filter(|key| !key.is_empty()) {
        request = request.header("authorization", format!("Bearer {api_key}"));
    }
    let response = tokio::select! {
        response = request.send() => response.map_err(|error| error.to_string())?,
        _ = signal.cancelled() => return Err("The operation was aborted".to_string()),
    };
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(format!(
            "Could not load Radius config from {gateway}: {}: {}",
            status.as_u16(),
            truncate_http_body(&body)
        ));
    }
    let body: Value = response.json().await.map_err(|error| error.to_string())?;
    sanitize_radius_gateway_config(&body)
        .ok_or_else(|| format!("Invalid Radius config from {gateway}"))
}
