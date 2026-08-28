use std::sync::Arc;

use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use crate::auth::oauth::device_code::{
    DeviceCodePollOptions, DeviceCodePollResult, poll_device_code_flow,
};
use crate::auth::oauth::http::{
    OAuthHttpResponse, positive_number, post_form, request_failure, required_string,
    validate_verification_uri,
};
use crate::auth::types::{
    AuthError, AuthEvent, BoxFuture, ModelAuth, OAuthAuth, OAuthCredential, ProviderAuthInteraction,
};

const XAI_CLIENT_ID: &str = "b1a00492-073a-47ea-816f-4c329264a828";
const XAI_SCOPE: &str = "openid profile email offline_access grok-cli:access api:access";
const XAI_DEVICE_CODE_URL: &str = "https://auth.x.ai/oauth2/device/code";
const XAI_TOKEN_URL: &str = "https://auth.x.ai/oauth2/token";
/// Refresh slightly before the reported expiry.
const REFRESH_SKEW_MS: i64 = 5 * 60 * 1000;
const DEFAULT_TOKEN_LIFETIME_SECONDS: f64 = 3600.0;

/// `XaiDeviceCode`
#[derive(Debug, Clone)]
pub struct XaiDeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: Option<String>,
    pub interval_seconds: Option<f64>,
    pub expires_in_seconds: f64,
}

/// `parseDeviceCode(body)`
pub fn parse_device_code(body: &Map<String, Value>) -> Result<XaiDeviceCode, AuthError> {
    // RFC 8628 allows interval 0; non-positive or malformed values fall back to the
    // poller default instead of failing.
    let interval_seconds = body
        .get("interval")
        .and_then(Value::as_f64)
        .filter(|interval| interval.is_finite() && *interval > 0.0);
    let verification_uri_complete = match body
        .get("verification_uri_complete")
        .and_then(Value::as_str)
    {
        Some(value) if !value.is_empty() => Some(validate_verification_uri(value, "xAI")?),
        _ => None,
    };
    Ok(XaiDeviceCode {
        device_code: required_string(body, "device_code", "xAI")?,
        user_code: required_string(body, "user_code", "xAI")?,
        verification_uri: validate_verification_uri(
            &required_string(body, "verification_uri", "xAI")?,
            "xAI",
        )?,
        verification_uri_complete,
        interval_seconds,
        expires_in_seconds: positive_number(body, "expires_in", "xAI")?,
    })
}

/// `credentialsFromTokenResponse(body, previousRefreshToken?)`
pub fn credentials_from_token_response(
    body: &Map<String, Value>,
    previous_refresh_token: Option<&str>,
) -> Result<OAuthCredential, AuthError> {
    let access = required_string(body, "access_token", "xAI")?;
    // xAI may omit refresh_token when the token is not rotated.
    let refresh = match (body.get("refresh_token"), previous_refresh_token) {
        (None, Some(previous)) => previous.to_string(),
        _ => required_string(body, "refresh_token", "xAI")?,
    };
    let expires_in_seconds = match body.get("expires_in") {
        None => DEFAULT_TOKEN_LIFETIME_SECONDS,
        Some(_) => positive_number(body, "expires_in", "xAI")?,
    };
    Ok(OAuthCredential {
        access,
        refresh,
        expires: crate::auth::resolve::now_ms() + (expires_in_seconds * 1000.0) as i64
            - REFRESH_SKEW_MS,
        extra: Default::default(),
    })
}

async fn request_device_code(signal: &CancellationToken) -> Result<XaiDeviceCode, AuthError> {
    let response = post_form(
        XAI_DEVICE_CODE_URL,
        &[
            ("client_id", XAI_CLIENT_ID.to_string()),
            ("scope", XAI_SCOPE.to_string()),
            ("referrer", "notagent".to_string()),
        ],
        signal,
        &[],
    )
    .await?;
    if !response.ok {
        return Err(request_failure("xAI", "device authorization", &response));
    }
    parse_device_code(&response.body)
}

/// Classifies one token-poll response; shared with the tests.
pub fn classify_poll_response(
    response: &OAuthHttpResponse,
) -> DeviceCodePollResult<OAuthCredential> {
    if response.ok {
        return match credentials_from_token_response(&response.body, None) {
            Ok(value) => DeviceCodePollResult::Complete { value },
            Err(error) => DeviceCodePollResult::Failed {
                message: error.to_string(),
            },
        };
    }
    match response.body.get("error").and_then(Value::as_str) {
        Some("authorization_pending") => DeviceCodePollResult::Pending,
        Some("slow_down") => DeviceCodePollResult::SlowDown {
            interval_seconds: response.body.get("interval").and_then(Value::as_f64),
        },
        Some("access_denied") | Some("authorization_denied") => DeviceCodePollResult::Failed {
            message: "xAI device authorization was denied".to_string(),
        },
        Some("expired_token") => DeviceCodePollResult::Failed {
            message: "xAI device code expired".to_string(),
        },
        _ => DeviceCodePollResult::Failed {
            message: request_failure("xAI", "device token polling", response).to_string(),
        },
    }
}

async fn poll_for_tokens(
    device: &XaiDeviceCode,
    signal: &CancellationToken,
) -> Result<OAuthCredential, AuthError> {
    poll_device_code_flow(
        DeviceCodePollOptions {
            interval_seconds: device.interval_seconds,
            expires_in_seconds: Some(device.expires_in_seconds),
            wait_before_first_poll: true,
            signal: signal.clone(),
        },
        || async {
            let response = post_form(
                XAI_TOKEN_URL,
                &[
                    (
                        "grant_type",
                        "urn:ietf:params:oauth:grant-type:device_code".to_string(),
                    ),
                    ("client_id", XAI_CLIENT_ID.to_string()),
                    ("device_code", device.device_code.clone()),
                ],
                signal,
                &[],
            )
            .await;
            match response {
                Ok(response) => classify_poll_response(&response),
                Err(error) => DeviceCodePollResult::Failed {
                    message: error.to_string(),
                },
            }
        },
    )
    .await
    .map_err(|error| AuthError(error.to_string()))
}

/// `xaiOAuth`
pub struct XaiOAuth;

impl OAuthAuth for XaiOAuth {
    fn name(&self) -> &str {
        "xAI (Grok/X subscription)"
    }

    fn is_subscription(&self) -> bool {
        true
    }

    fn login_label(&self) -> Option<&str> {
        Some("Sign in with SuperGrok or X Premium")
    }

    fn login<'a>(
        &'a self,
        interaction: &'a ProviderAuthInteraction<'a>,
    ) -> BoxFuture<'a, Result<OAuthCredential, AuthError>> {
        Box::pin(async move {
            let device = request_device_code(&interaction.signal).await?;
            interaction.notify(AuthEvent::DeviceCode {
                user_code: device.user_code.clone(),
                verification_uri: device
                    .verification_uri_complete
                    .clone()
                    .unwrap_or_else(|| device.verification_uri.clone()),
                interval_seconds: device.interval_seconds.map(|seconds| seconds as u64),
                expires_in_seconds: Some(device.expires_in_seconds as u64),
            });
            poll_for_tokens(&device, &interaction.signal).await
        })
    }

    fn refresh<'a>(
        &'a self,
        credential: OAuthCredential,
        signal: CancellationToken,
    ) -> BoxFuture<'a, Result<OAuthCredential, AuthError>> {
        Box::pin(async move {
            let response = post_form(
                XAI_TOKEN_URL,
                &[
                    ("grant_type", "refresh_token".to_string()),
                    ("client_id", XAI_CLIENT_ID.to_string()),
                    ("refresh_token", credential.refresh.clone()),
                ],
                &signal,
                &[],
            )
            .await?;
            if !response.ok {
                return Err(request_failure("xAI", "token refresh", &response));
            }
            credentials_from_token_response(&response.body, Some(&credential.refresh))
        })
    }

    fn to_auth<'a>(
        &'a self,
        credential: OAuthCredential,
    ) -> BoxFuture<'a, Result<ModelAuth, AuthError>> {
        Box::pin(async move {
            Ok(ModelAuth {
                api_key: Some(credential.access),
                ..Default::default()
            })
        })
    }
}

pub fn xai_oauth() -> Arc<dyn OAuthAuth> {
    Arc::new(XaiOAuth)
}
