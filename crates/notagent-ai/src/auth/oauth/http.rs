//! Shared HTTP helpers of the OAuth flows.
//!
//! The TS flows call `fetch` directly with form or JSON bodies; this module keeps the
//! response shape (`ok`, `status`, parsed body) they branch on.

use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use crate::auth::types::AuthError;

/// `OAuthHttpResponse`
#[derive(Debug, Clone)]
pub struct OAuthHttpResponse {
    pub ok: bool,
    pub status: u16,
    pub body: Map<String, Value>,
}

/// `postForm(url, fields, signal)`
pub async fn post_form(
    url: &str,
    fields: &[(&str, String)],
    signal: &CancellationToken,
    extra_headers: &[(&str, String)],
) -> Result<OAuthHttpResponse, AuthError> {
    let body = fields
        .iter()
        .map(|(key, value)| format!("{}={}", urlencode(key), urlencode(value)))
        .collect::<Vec<_>>()
        .join("&");

    let mut request = reqwest::Client::new()
        .post(url)
        .header("Accept", "application/json")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body);
    for (name, value) in extra_headers {
        request = request.header(*name, value);
    }

    let response = tokio::select! {
        response = request.send() => response,
        _ = signal.cancelled() => return Err(AuthError("Login cancelled".to_string())),
    };
    let response = response.map_err(|error| {
        if signal.is_cancelled() {
            AuthError("Login cancelled".to_string())
        } else {
            AuthError(error.to_string())
        }
    })?;

    let status = response.status().as_u16();
    let ok = response.status().is_success();
    let text = response
        .text()
        .await
        .map_err(|error| AuthError(error.to_string()))?;
    let body = match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(object)) => object,
        Ok(_) => Map::new(),
        Err(_) => {
            if signal.is_cancelled() {
                return Err(AuthError("Login cancelled".to_string()));
            }
            return Err(AuthError(format!(
                "OAuth returned invalid JSON (HTTP {status})"
            )));
        }
    };
    Ok(OAuthHttpResponse { ok, status, body })
}

/// `requiredString(body, field)`
pub fn required_string(
    body: &Map<String, Value>,
    field: &str,
    provider: &str,
) -> Result<String, AuthError> {
    match body.get(field).and_then(Value::as_str) {
        Some(value) if !value.is_empty() => Ok(value.to_string()),
        _ => Err(AuthError(format!(
            "Invalid {provider} OAuth response field: {field}"
        ))),
    }
}

/// `positiveNumber(body, field)`
pub fn positive_number(
    body: &Map<String, Value>,
    field: &str,
    provider: &str,
) -> Result<f64, AuthError> {
    match body.get(field).and_then(Value::as_f64) {
        Some(value) if value.is_finite() && value > 0.0 => Ok(value),
        _ => Err(AuthError(format!(
            "Invalid {provider} OAuth response field: {field}"
        ))),
    }
}

/// `requestFailure(action, response)`
pub fn request_failure(provider: &str, action: &str, response: &OAuthHttpResponse) -> AuthError {
    let error = response.body.get("error").and_then(Value::as_str);
    let description = response
        .body
        .get("error_description")
        .and_then(Value::as_str);
    let detail = [error, description]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(": ");
    let suffix = if detail.is_empty() {
        String::new()
    } else {
        format!(": {detail}")
    };
    AuthError(format!(
        "{provider} OAuth {action} failed (HTTP {}){suffix}",
        response.status
    ))
}

/// The verification URI is opened in a browser, so only https is accepted.
pub fn validate_verification_uri(raw: &str, provider: &str) -> Result<String, AuthError> {
    if !raw.starts_with("https://") {
        return Err(AuthError(format!(
            "Untrusted verification URI in {provider} OAuth response"
        )));
    }
    Ok(raw.to_string())
}

/// Percent-encoding for form and query values.
pub fn urlencode(value: &str) -> String {
    crate::auth::oauth::anthropic::urlencode(value)
}
