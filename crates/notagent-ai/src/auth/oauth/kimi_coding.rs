//! Kimi Code (subscription) OAuth flow.
//!
//! 1:1 port of `packages/ai/src/auth/oauth/kimi-coding.ts` (310 LOC): RFC 8628 device
//! authorization against auth.kimi.com with JSON responses. The access token
//! authenticates as `Authorization: Bearer`, not as an api key.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use crate::auth::oauth::device_code::{
    DeviceCodePollOptions, DeviceCodePollResult, poll_device_code_flow,
};
use crate::auth::oauth::http::post_form;
use crate::auth::types::{
    AuthError, AuthEvent, BoxFuture, ModelAuth, OAuthAuth, OAuthCredential, ProviderAuthInteraction,
};
use crate::utils::provider_env::get_provider_env_value;

const CLIENT_ID: &str = "17e5f671-d194-4dfb-9706-5516cb48c098";
const DEFAULT_OAUTH_HOST: &str = "https://auth.kimi.com";
const DEVICE_CODE_TIMEOUT_SECONDS: f64 = 15.0 * 60.0;
const DEFAULT_POLL_INTERVAL_SECONDS: f64 = 5.0;
const REFRESH_MAX_RETRIES: u32 = 3;

/// `getOauthHost()`
pub fn oauth_host() -> String {
    let override_host = get_provider_env_value("KIMI_CODE_OAUTH_HOST", None)
        .or_else(|| get_provider_env_value("KIMI_OAUTH_HOST", None));
    let host = override_host.unwrap_or_else(|| DEFAULT_OAUTH_HOST.to_string());
    host.trim_end_matches('/').to_string()
}

/// `trustedHttpUrl(value)` — http and https are both accepted here.
fn trusted_http_url(value: &str) -> bool {
    value.starts_with("https://") || value.starts_with("http://")
}

/// `DeviceAuthorization`
#[derive(Debug, Clone)]
pub struct KimiDeviceAuthorization {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: String,
    pub interval_seconds: f64,
    pub expires_in_seconds: f64,
}

/// Parses the device-authorization response; every field is required.
pub fn parse_device_authorization(
    json: &Map<String, Value>,
) -> Result<KimiDeviceAuthorization, AuthError> {
    let field = |name: &str| json.get(name).and_then(Value::as_str).map(str::to_string);
    let (device_code, user_code, verification_uri, verification_uri_complete) = (
        field("device_code"),
        field("user_code"),
        field("verification_uri"),
        field("verification_uri_complete"),
    );
    let invalid = || {
        AuthError(format!(
            "Invalid Kimi Code device authorization response: {}",
            Value::Object(json.clone())
        ))
    };
    let (
        Some(device_code),
        Some(user_code),
        Some(verification_uri),
        Some(verification_uri_complete),
    ) = (
        device_code,
        user_code,
        verification_uri,
        verification_uri_complete,
    )
    else {
        return Err(invalid());
    };
    if !trusted_http_url(&verification_uri) || !trusted_http_url(&verification_uri_complete) {
        return Err(invalid());
    }

    Ok(KimiDeviceAuthorization {
        device_code,
        user_code,
        verification_uri,
        verification_uri_complete,
        interval_seconds: json
            .get("interval")
            .and_then(Value::as_f64)
            .filter(|interval| interval.is_finite() && *interval > 0.0)
            .unwrap_or(DEFAULT_POLL_INTERVAL_SECONDS),
        expires_in_seconds: json
            .get("expires_in")
            .and_then(Value::as_f64)
            .filter(|expires| expires.is_finite() && *expires > 0.0)
            .unwrap_or(DEVICE_CODE_TIMEOUT_SECONDS),
    })
}

/// `parseTokenResponse(json, operation)`
pub fn parse_token_response(
    json: &Map<String, Value>,
    operation: &str,
) -> Result<OAuthCredential, AuthError> {
    let access = json
        .get("access_token")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty());
    let refresh = json
        .get("refresh_token")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty());
    let expires_in = json
        .get("expires_in")
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && *value > 0.0);
    let (Some(access), Some(refresh), Some(expires_in)) = (access, refresh, expires_in) else {
        return Err(AuthError(format!(
            "Kimi Code token {operation} response missing fields: {}",
            Value::Object(json.clone())
        )));
    };
    Ok(OAuthCredential {
        access: access.to_string(),
        refresh: refresh.to_string(),
        expires: crate::auth::resolve::now_ms() + (expires_in * 1000.0) as i64,
        extra: Default::default(),
    })
}

/// Classifies one token-poll response; shared with the tests.
pub fn classify_poll_response(
    status: u16,
    json: &Map<String, Value>,
) -> DeviceCodePollResult<OAuthCredential> {
    if status >= 500 {
        return DeviceCodePollResult::Failed {
            message: format!("Kimi Code device token request failed with status {status}"),
        };
    }
    if (200..300).contains(&status) && json.get("access_token").and_then(Value::as_str).is_some() {
        return match parse_token_response(json, "poll") {
            Ok(value) => DeviceCodePollResult::Complete { value },
            Err(error) => DeviceCodePollResult::Failed {
                message: error.to_string(),
            },
        };
    }

    let description = json
        .get("error_description")
        .and_then(Value::as_str)
        .map(|description| format!(": {description}"))
        .unwrap_or_default();
    match json.get("error").and_then(Value::as_str) {
        Some("authorization_pending") => DeviceCodePollResult::Pending,
        Some("slow_down") => DeviceCodePollResult::SlowDown {
            interval_seconds: json
                .get("interval")
                .and_then(Value::as_f64)
                .filter(|interval| *interval > 0.0),
        },
        Some("expired_token") => DeviceCodePollResult::Failed {
            message: "Kimi Code device authorization expired. Please restart login.".to_string(),
        },
        Some("access_denied") => DeviceCodePollResult::Failed {
            message: "Kimi Code login was denied.".to_string(),
        },
        _ => DeviceCodePollResult::Failed {
            message: format!(
                "Kimi Code device token request failed with status {status}{description}"
            ),
        },
    }
}

async fn start_device_authorization(
    oauth_host: &str,
    signal: &CancellationToken,
) -> Result<KimiDeviceAuthorization, AuthError> {
    let response = post_form(
        &format!("{oauth_host}/api/oauth/device_authorization"),
        &[("client_id", CLIENT_ID.to_string())],
        signal,
        &[],
    )
    .await?;
    if !response.ok {
        return Err(AuthError(format!(
            "Kimi Code device authorization failed with status {}",
            response.status
        )));
    }
    parse_device_authorization(&response.body)
}

/// `refreshToken(oauthHost, refreshToken, signal)` — retries transient failures.
pub async fn refresh_token(
    oauth_host: &str,
    refresh_token: &str,
    signal: &CancellationToken,
) -> Result<OAuthCredential, AuthError> {
    let mut last_error: Option<AuthError> = None;
    for attempt in 1..=REFRESH_MAX_RETRIES {
        let response = post_form(
            &format!("{oauth_host}/api/oauth/token"),
            &[
                ("client_id", CLIENT_ID.to_string()),
                ("grant_type", "refresh_token".to_string()),
                ("refresh_token", refresh_token.to_string()),
            ],
            signal,
            &[],
        )
        .await?;

        if response.ok {
            return parse_token_response(&response.body, "refresh");
        }

        // Unauthorized: the stored credential is dead and Models prompts a re-login.
        let invalid_grant =
            response.body.get("error").and_then(Value::as_str) == Some("invalid_grant");
        if response.status == 401 || response.status == 403 || invalid_grant {
            let description = response
                .body
                .get("error_description")
                .and_then(Value::as_str)
                .map(|description| format!(": {description}"))
                .unwrap_or_default();
            return Err(AuthError(format!(
                "Kimi Code token refresh unauthorized (status {}){description}",
                response.status
            )));
        }

        let retryable = response.status == 408 || response.status == 429 || response.status >= 500;
        if retryable && attempt < REFRESH_MAX_RETRIES {
            last_error = Some(AuthError(format!(
                "Kimi Code token refresh failed with status {}",
                response.status
            )));
            continue;
        }

        return Err(AuthError(format!(
            "Kimi Code token refresh failed with status {}: {}",
            response.status,
            Value::Object(response.body.clone())
        )));
    }

    Err(last_error.unwrap_or_else(|| AuthError("Kimi Code token refresh failed".to_string())))
}

/// `kimiCodingOAuth`
pub struct KimiCodingOAuth;

impl OAuthAuth for KimiCodingOAuth {
    fn name(&self) -> &str {
        "Kimi Code (subscription)"
    }

    fn is_subscription(&self) -> bool {
        true
    }

    fn login_label(&self) -> Option<&str> {
        Some("Sign in with Kimi Code")
    }

    fn login<'a>(
        &'a self,
        interaction: &'a ProviderAuthInteraction<'a>,
    ) -> BoxFuture<'a, Result<OAuthCredential, AuthError>> {
        Box::pin(async move {
            let host = oauth_host();
            let device = start_device_authorization(&host, &interaction.signal).await?;
            interaction.notify(AuthEvent::DeviceCode {
                user_code: device.user_code.clone(),
                verification_uri: device.verification_uri_complete.clone(),
                interval_seconds: Some(device.interval_seconds as u64),
                expires_in_seconds: Some(device.expires_in_seconds as u64),
            });

            poll_device_code_flow(
                DeviceCodePollOptions {
                    interval_seconds: Some(device.interval_seconds),
                    expires_in_seconds: Some(device.expires_in_seconds),
                    wait_before_first_poll: false,
                    signal: interaction.signal.clone(),
                },
                || async {
                    let response = post_form(
                        &format!("{host}/api/oauth/token"),
                        &[
                            ("client_id", CLIENT_ID.to_string()),
                            ("device_code", device.device_code.clone()),
                            (
                                "grant_type",
                                "urn:ietf:params:oauth:grant-type:device_code".to_string(),
                            ),
                        ],
                        &interaction.signal,
                        &[],
                    )
                    .await;
                    match response {
                        Ok(response) => classify_poll_response(response.status, &response.body),
                        Err(error) => DeviceCodePollResult::Failed {
                            message: error.to_string(),
                        },
                    }
                },
            )
            .await
            .map_err(|error| AuthError(error.to_string()))
        })
    }

    fn refresh<'a>(
        &'a self,
        credential: OAuthCredential,
        signal: CancellationToken,
    ) -> BoxFuture<'a, Result<OAuthCredential, AuthError>> {
        Box::pin(async move { refresh_token(&oauth_host(), &credential.refresh, &signal).await })
    }

    fn to_auth<'a>(
        &'a self,
        credential: OAuthCredential,
    ) -> BoxFuture<'a, Result<ModelAuth, AuthError>> {
        Box::pin(async move {
            // Kimi authenticates through the Authorization header, not an api key.
            let mut headers = BTreeMap::new();
            headers.insert(
                "Authorization".to_string(),
                Some(format!("Bearer {}", credential.access)),
            );
            Ok(ModelAuth {
                headers: Some(headers),
                ..Default::default()
            })
        })
    }
}

/// The shared instance, matching the TS export.
pub fn kimi_coding_oauth() -> Arc<dyn OAuthAuth> {
    Arc::new(KimiCodingOAuth)
}
