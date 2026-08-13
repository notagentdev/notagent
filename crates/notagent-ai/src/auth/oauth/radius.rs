//! Radius gateway OAuth.
//!
//! 1:1 port of `packages/ai/src/auth/oauth/radius.ts` (403 LOC). The OAuth client APIs
//! live on the configured gateway; only the interactive authorization endpoint is
//! discovered. Both a browser and a device-code flow are offered.

use std::sync::Arc;

use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::auth::oauth::callback_server::{CallbackServerOptions, start_callback_server};
use crate::auth::oauth::device_code::{
    DeviceCodePollOptions, DeviceCodePollResult, poll_device_code_flow,
};
use crate::auth::oauth::http::post_form;
use crate::auth::oauth::pkce::generate_pkce;
use crate::auth::types::{
    AuthError, AuthEvent, AuthPrompt, AuthPromptKind, AuthPromptOption, BoxFuture, ModelAuth,
    OAuthAuth, OAuthCredential, ProviderAuthInteraction,
};

const CALLBACK_HOST: &str = "127.0.0.1";
const CALLBACK_PORT: u16 = 1456;
const CALLBACK_PATH: &str = "/oauth/callback";
const TOKEN_EXPIRY_SKEW_MS: i64 = 60_000;
const LOGIN_METHOD_BROWSER: &str = "browser";
const LOGIN_METHOD_DEVICE_CODE: &str = "device-code";
const OAUTH_CLIENT_ID: &str = "notagent-gateway";
const OAUTH_SCOPE: &str = "gateway offline_access";
const OAUTH_DEVICE_CODE_GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:device_code";

fn redirect_uri() -> String {
    format!("http://{CALLBACK_HOST}:{CALLBACK_PORT}{CALLBACK_PATH}")
}

/// `normalizeRadiusGatewayUrl(gateway)` — of `providers/radius-config.ts`.
pub fn normalize_radius_gateway_url(gateway: &str) -> String {
    let trimmed = gateway.trim().trim_end_matches('/');
    if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    }
}

/// `loadRadiusOAuthDiscovery(gateway, signal)` — only the authorization endpoint.
pub async fn load_authorization_endpoint(
    gateway: &str,
    signal: &CancellationToken,
) -> Result<String, AuthError> {
    let response = tokio::select! {
        response = reqwest::Client::new().get(format!("{gateway}/v1/oauth")).header("accept", "application/json").send() => {
            response.map_err(|error| AuthError(error.to_string()))?
        }
        _ = signal.cancelled() => return Err(AuthError("Login cancelled".to_string())),
    };
    if !response.status().is_success() {
        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        return Err(AuthError(format!(
            "Could not load Radius OAuth config from {gateway}: {status} {text}"
        )));
    }
    let body: Value = response
        .json()
        .await
        .map_err(|error| AuthError(error.to_string()))?;
    match body.get("authorizationEndpoint").and_then(Value::as_str) {
        Some(endpoint) => Ok(endpoint.to_string()),
        None => Err(AuthError(format!(
            "Invalid Radius OAuth config from {gateway}"
        ))),
    }
}

/// `requestOAuthToken(gateway, body, signal)`
pub async fn request_oauth_token(
    gateway: &str,
    fields: &[(&str, String)],
    signal: &CancellationToken,
) -> Result<OAuthCredential, AuthError> {
    let response = post_form(&format!("{gateway}/v1/oauth/token"), fields, signal, &[]).await?;
    if !response.ok {
        return Err(AuthError(format!(
            "Radius OAuth token request failed ({})",
            response.status
        )));
    }
    let access = response
        .body
        .get("access_token")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let refresh = response
        .body
        .get("refresh_token")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let expires_in = response
        .body
        .get("expires_in")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let mut extra = serde_json::Map::new();
    if let Some(scope) = response.body.get("scope") {
        extra.insert("scope".to_string(), scope.clone());
    }
    Ok(OAuthCredential {
        access: access.to_string(),
        refresh: refresh.to_string(),
        expires: crate::auth::resolve::now_ms() + (expires_in * 1000.0) as i64
            - TOKEN_EXPIRY_SKEW_MS,
        extra,
    })
}

/// `RadiusOAuthOptions`
#[derive(Debug, Clone)]
pub struct RadiusOAuthOptions {
    pub name: String,
    pub gateway: String,
}

/// `createRadiusOAuth(options)`
pub struct RadiusOAuth {
    name: String,
    gateway: String,
}

impl OAuthAuth for RadiusOAuth {
    fn name(&self) -> &str {
        &self.name
    }

    fn login<'a>(
        &'a self,
        interaction: &'a ProviderAuthInteraction<'a>,
    ) -> BoxFuture<'a, Result<OAuthCredential, AuthError>> {
        Box::pin(async move {
            let method = interaction
                .prompt(AuthPrompt {
                    signal: None,
                    kind: AuthPromptKind::Select {
                        message: format!("Sign in to {}:", self.name),
                        options: vec![
                            AuthPromptOption {
                                id: LOGIN_METHOD_BROWSER.to_string(),
                                label: "Sign in with browser (recommended)".to_string(),
                                description: None,
                            },
                            AuthPromptOption {
                                id: LOGIN_METHOD_DEVICE_CODE.to_string(),
                                label:
                                    "Sign in with device code (when signing in from another device)"
                                        .to_string(),
                                description: None,
                            },
                        ],
                    },
                })
                .await?;

            if method == LOGIN_METHOD_DEVICE_CODE {
                return self.login_device_code(interaction).await;
            }
            if method != LOGIN_METHOD_BROWSER {
                return Err(AuthError(format!(
                    "Unknown {} sign-in method: {method}",
                    self.name
                )));
            }
            let endpoint = load_authorization_endpoint(&self.gateway, &interaction.signal).await?;
            self.login_browser(&endpoint, interaction).await
        })
    }

    fn refresh<'a>(
        &'a self,
        credential: OAuthCredential,
        signal: CancellationToken,
    ) -> BoxFuture<'a, Result<OAuthCredential, AuthError>> {
        Box::pin(async move {
            request_oauth_token(
                &self.gateway,
                &[
                    ("grant_type", "refresh_token".to_string()),
                    ("client_id", OAUTH_CLIENT_ID.to_string()),
                    ("refresh_token", credential.refresh.clone()),
                ],
                &signal,
            )
            .await
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

impl RadiusOAuth {
    async fn login_browser(
        &self,
        authorization_endpoint: &str,
        interaction: &ProviderAuthInteraction<'_>,
    ) -> Result<OAuthCredential, AuthError> {
        let pkce = generate_pkce();
        let state = pkce.verifier.clone();
        let server = start_callback_server(CallbackServerOptions {
            host: CALLBACK_HOST.to_string(),
            port: CALLBACK_PORT,
            path: CALLBACK_PATH.to_string(),
            redirect_uri: redirect_uri(),
            expected_state: state.clone(),
            success_message: format!(
                "{} authentication completed. You can close this window.",
                self.name
            ),
            error_message: format!("{} authentication did not complete.", self.name),
        })
        .await
        .map_err(|error| AuthError(error.to_string()))?;

        let url = format!(
            "{authorization_endpoint}?response_type=code&client_id={}&redirect_uri={}&scope={}&code_challenge={}&code_challenge_method=S256&handoff=url&state={}",
            super::anthropic::urlencode(OAUTH_CLIENT_ID),
            super::anthropic::urlencode(&redirect_uri()),
            super::anthropic::urlencode(OAUTH_SCOPE),
            super::anthropic::urlencode(&pkce.challenge),
            super::anthropic::urlencode(&state)
        );
        interaction.notify(AuthEvent::AuthUrl {
            url,
            instructions: None,
        });

        let Some(callback) = server.wait_for_code().await else {
            return Err(AuthError("Login cancelled".to_string()));
        };
        request_oauth_token(
            &self.gateway,
            &[
                ("grant_type", "authorization_code".to_string()),
                ("client_id", OAUTH_CLIENT_ID.to_string()),
                ("redirect_uri", redirect_uri()),
                ("code", callback.code),
                ("code_verifier", pkce.verifier),
            ],
            &interaction.signal,
        )
        .await
    }

    async fn login_device_code(
        &self,
        interaction: &ProviderAuthInteraction<'_>,
    ) -> Result<OAuthCredential, AuthError> {
        let response = post_form(
            &format!("{}/v1/oauth/device_authorization", self.gateway),
            &[
                ("client_id", OAUTH_CLIENT_ID.to_string()),
                ("scope", OAUTH_SCOPE.to_string()),
            ],
            &interaction.signal,
            &[],
        )
        .await?;
        if !response.ok {
            return Err(AuthError(format!(
                "Radius OAuth device authorization failed ({})",
                response.status
            )));
        }

        let device_code = response
            .body
            .get("device_code")
            .and_then(Value::as_str)
            .ok_or_else(|| AuthError("Invalid Radius device authorization response".to_string()))?
            .to_string();
        let user_code = response
            .body
            .get("user_code")
            .and_then(Value::as_str)
            .ok_or_else(|| AuthError("Invalid Radius device authorization response".to_string()))?
            .to_string();
        let verification_uri = response
            .body
            .get("verification_uri")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let interval = response.body.get("interval").and_then(Value::as_f64);
        let expires_in = response.body.get("expires_in").and_then(Value::as_f64);

        interaction.notify(AuthEvent::DeviceCode {
            user_code,
            verification_uri,
            interval_seconds: interval.map(|interval| interval as u64),
            expires_in_seconds: expires_in.map(|expires| expires as u64),
        });

        poll_device_code_flow(
            DeviceCodePollOptions {
                interval_seconds: interval,
                expires_in_seconds: expires_in,
                wait_before_first_poll: true,
                signal: interaction.signal.clone(),
            },
            || async {
                let response = post_form(
                    &format!("{}/v1/oauth/token", self.gateway),
                    &[
                        ("grant_type", OAUTH_DEVICE_CODE_GRANT_TYPE.to_string()),
                        ("client_id", OAUTH_CLIENT_ID.to_string()),
                        ("device_code", device_code.clone()),
                    ],
                    &interaction.signal,
                    &[],
                )
                .await;
                let Ok(response) = response else {
                    return DeviceCodePollResult::Failed {
                        message: "Radius OAuth device polling failed".to_string(),
                    };
                };
                if response.ok {
                    let access = response
                        .body
                        .get("access_token")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let refresh = response
                        .body
                        .get("refresh_token")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let expires_in = response
                        .body
                        .get("expires_in")
                        .and_then(Value::as_f64)
                        .unwrap_or(0.0);
                    return DeviceCodePollResult::Complete {
                        value: OAuthCredential {
                            access,
                            refresh,
                            expires: crate::auth::resolve::now_ms() + (expires_in * 1000.0) as i64
                                - TOKEN_EXPIRY_SKEW_MS,
                            extra: Default::default(),
                        },
                    };
                }
                match response.body.get("error").and_then(Value::as_str) {
                    Some("authorization_pending") | None => DeviceCodePollResult::Pending,
                    Some("slow_down") => DeviceCodePollResult::SlowDown {
                        interval_seconds: response.body.get("interval").and_then(Value::as_f64),
                    },
                    Some(error) => DeviceCodePollResult::Failed {
                        message: format!("Radius OAuth device authorization failed: {error}"),
                    },
                }
            },
        )
        .await
        .map_err(|error| AuthError(error.to_string()))
    }
}

/// `createRadiusOAuth(options)`
pub fn create_radius_oauth(options: RadiusOAuthOptions) -> Arc<dyn OAuthAuth> {
    Arc::new(RadiusOAuth {
        name: options.name,
        gateway: normalize_radius_gateway_url(&options.gateway),
    })
}
