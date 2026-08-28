use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD_NO_PAD, URL_SAFE_NO_PAD};
use serde_json::{Map, Value};
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

const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const AUTH_BASE_URL: &str = "https://auth.openai.com";
const REDIRECT_URI: &str = "http://localhost:1455/auth/callback";
const CALLBACK_PORT: u16 = 1455;
const CALLBACK_PATH: &str = "/auth/callback";
const DEVICE_CODE_TIMEOUT_SECONDS: f64 = 15.0 * 60.0;
const BROWSER_LOGIN_METHOD: &str = "browser";
const DEVICE_CODE_LOGIN_METHOD: &str = "device-code";
const SCOPE: &str = "openid profile email offline_access";
const JWT_CLAIM_PATH: &str = "https://api.openai.com/auth";

fn authorize_url() -> String {
    format!("{AUTH_BASE_URL}/oauth/authorize")
}

fn token_url() -> String {
    format!("{AUTH_BASE_URL}/oauth/token")
}

fn device_user_code_url() -> String {
    format!("{AUTH_BASE_URL}/api/accounts/deviceauth/usercode")
}

fn device_token_url() -> String {
    format!("{AUTH_BASE_URL}/api/accounts/deviceauth/token")
}

fn device_verification_uri() -> String {
    format!("{AUTH_BASE_URL}/codex/device")
}

fn device_redirect_uri() -> String {
    format!("{AUTH_BASE_URL}/deviceauth/callback")
}

/// `createState()` — 16 random bytes as hex.
fn create_state() -> String {
    use rand::RngExt;
    let mut bytes = [0u8; 16];
    rand::rng().fill(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn decode_jwt(token: &str) -> Option<Map<String, Value>> {
    let payload = token.split('.').nth(1)?;
    if token.split('.').count() != 3 {
        return None;
    }
    // JWT payloads are base64url; `atob` also accepts unpadded standard alphabet.
    let decoded = URL_SAFE_NO_PAD
        .decode(payload)
        .or_else(|_| STANDARD_NO_PAD.decode(payload))
        .ok()?;
    match serde_json::from_slice::<Value>(&decoded).ok()? {
        Value::Object(object) => Some(object),
        _ => None,
    }
}

/// `getAccountId(accessToken)` — `https://api.openai.com/auth.chatgpt_account_id`.
pub fn account_id(access_token: &str) -> Option<String> {
    let payload = decode_jwt(access_token)?;
    payload
        .get(JWT_CLAIM_PATH)?
        .get("chatgpt_account_id")?
        .as_str()
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

/// `readTokenResponse(response, operation)`
pub fn read_token_response(
    body: &Map<String, Value>,
    operation: &str,
) -> Result<(String, String, i64), AuthError> {
    let access = body
        .get("access_token")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty());
    let refresh = body
        .get("refresh_token")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty());
    let expires_in = body.get("expires_in").and_then(Value::as_f64);
    let (Some(access), Some(refresh), Some(expires_in)) = (access, refresh, expires_in) else {
        return Err(AuthError(format!(
            "OpenAI Codex token {operation} response missing fields: {}",
            Value::Object(body.clone())
        )));
    };
    Ok((
        access.to_string(),
        refresh.to_string(),
        crate::auth::resolve::now_ms() + (expires_in * 1000.0) as i64,
    ))
}

/// `credentialsFromToken(token)` — the account id is mandatory.
pub fn credentials_from_token(
    access: String,
    refresh: String,
    expires: i64,
) -> Result<OAuthCredential, AuthError> {
    let Some(account_id) = account_id(&access) else {
        return Err(AuthError(
            "Failed to extract accountId from token".to_string(),
        ));
    };
    let mut extra = Map::new();
    extra.insert("accountId".to_string(), Value::String(account_id));
    Ok(OAuthCredential {
        access,
        refresh,
        expires,
        extra,
    })
}

/// `parseAuthorizationInput(input)`
pub fn parse_authorization_input(input: &str) -> (Option<String>, Option<String>) {
    super::anthropic::parse_authorization_input(input)
}

async fn exchange_authorization_code(
    code: &str,
    verifier: &str,
    redirect_uri: &str,
    signal: &CancellationToken,
) -> Result<OAuthCredential, AuthError> {
    let response = post_form(
        &token_url(),
        &[
            ("grant_type", "authorization_code".to_string()),
            ("client_id", CLIENT_ID.to_string()),
            ("code", code.to_string()),
            ("code_verifier", verifier.to_string()),
            ("redirect_uri", redirect_uri.to_string()),
        ],
        signal,
        &[],
    )
    .await?;
    if !response.ok {
        return Err(AuthError(format!(
            "OpenAI Codex token exchange failed ({})",
            response.status
        )));
    }
    let (access, refresh, expires) = read_token_response(&response.body, "exchange")?;
    credentials_from_token(access, refresh, expires)
}

async fn refresh_access_token(
    refresh_token: &str,
    signal: &CancellationToken,
) -> Result<OAuthCredential, AuthError> {
    let response = post_form(
        &token_url(),
        &[
            ("grant_type", "refresh_token".to_string()),
            ("client_id", CLIENT_ID.to_string()),
            ("refresh_token", refresh_token.to_string()),
            ("scope", SCOPE.to_string()),
        ],
        signal,
        &[],
    )
    .await?;
    if !response.ok {
        return Err(AuthError(format!(
            "OpenAI Codex token refresh failed ({})",
            response.status
        )));
    }
    let (access, refresh, expires) = read_token_response(&response.body, "refresh")?;
    credentials_from_token(access, refresh, expires)
}

/// The device-authorization response of the headless flow.
#[derive(Debug, Clone)]
pub struct CodexDeviceAuth {
    pub device_code: String,
    pub user_code: String,
    pub interval_seconds: Option<f64>,
}

async fn start_device_auth(signal: &CancellationToken) -> Result<CodexDeviceAuth, AuthError> {
    let response = reqwest::Client::new()
        .post(device_user_code_url())
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .json(&serde_json::json!({ "client_id": CLIENT_ID }))
        .send()
        .await
        .map_err(|error| {
            if signal.is_cancelled() {
                AuthError("Login cancelled".to_string())
            } else {
                AuthError(error.to_string())
            }
        })?;
    if !response.status().is_success() {
        return Err(AuthError(format!(
            "OpenAI Codex device authorization failed ({})",
            response.status().as_u16()
        )));
    }
    let body: Value = response
        .json()
        .await
        .map_err(|error| AuthError(error.to_string()))?;
    let device_code = body.get("device_code").and_then(Value::as_str);
    let user_code = body.get("user_code").and_then(Value::as_str);
    let (Some(device_code), Some(user_code)) = (device_code, user_code) else {
        return Err(AuthError(
            "Invalid OpenAI Codex device authorization response".to_string(),
        ));
    };
    Ok(CodexDeviceAuth {
        device_code: device_code.to_string(),
        user_code: user_code.to_string(),
        interval_seconds: body.get("interval").and_then(Value::as_f64),
    })
}

/// `openaiCodexOAuth`
pub struct OpenAICodexOAuth;

impl OAuthAuth for OpenAICodexOAuth {
    fn name(&self) -> &str {
        "OpenAI (ChatGPT Plus/Pro)"
    }

    fn is_subscription(&self) -> bool {
        true
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
                        message: "Select OpenAI Codex login method:".to_string(),
                        options: vec![
                            AuthPromptOption {
                                id: BROWSER_LOGIN_METHOD.to_string(),
                                label: "Browser login (default)".to_string(),
                                description: None,
                            },
                            AuthPromptOption {
                                id: DEVICE_CODE_LOGIN_METHOD.to_string(),
                                label: "Device code login (headless)".to_string(),
                                description: None,
                            },
                        ],
                    },
                })
                .await?;

            if method == DEVICE_CODE_LOGIN_METHOD {
                return login_device_code(interaction).await;
            }
            if method != BROWSER_LOGIN_METHOD {
                return Err(AuthError(format!(
                    "Unknown OpenAI Codex login method: {method}"
                )));
            }
            login_browser(interaction).await
        })
    }

    fn refresh<'a>(
        &'a self,
        credential: OAuthCredential,
        signal: CancellationToken,
    ) -> BoxFuture<'a, Result<OAuthCredential, AuthError>> {
        Box::pin(async move { refresh_access_token(&credential.refresh, &signal).await })
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

/// `loginOpenAICodex(interaction)` — PKCE browser flow with a manual fallback.
async fn login_browser(
    interaction: &ProviderAuthInteraction<'_>,
) -> Result<OAuthCredential, AuthError> {
    let pkce = generate_pkce();
    let state = create_state();
    let server = start_callback_server(CallbackServerOptions {
        host: "127.0.0.1".to_string(),
        port: CALLBACK_PORT,
        path: CALLBACK_PATH.to_string(),
        redirect_uri: REDIRECT_URI.to_string(),
        expected_state: state.clone(),
        success_message: "OpenAI authentication completed. You can close this window.".to_string(),
        error_message: "OpenAI authentication did not complete.".to_string(),
    })
    .await
    .map_err(|error| AuthError(error.to_string()))?;
    let cancel = server.cancel_token();

    let url = format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&scope={}&code_challenge={}&code_challenge_method=S256&state={}&id_token_add_organizations=true",
        authorize_url(),
        super::anthropic::urlencode(CLIENT_ID),
        super::anthropic::urlencode(REDIRECT_URI),
        super::anthropic::urlencode(SCOPE),
        super::anthropic::urlencode(&pkce.challenge),
        super::anthropic::urlencode(&state)
    );
    interaction.notify(AuthEvent::AuthUrl {
        url,
        instructions: Some(
            "Complete login in your browser. If the browser is on another machine, paste the final redirect URL here."
                .to_string(),
        ),
    });

    let manual = interaction.prompt(AuthPrompt {
        signal: Some(cancel.clone()),
        kind: AuthPromptKind::ManualCode {
            message: "Complete login in your browser, or paste the authorization code / redirect URL here:"
                .to_string(),
            placeholder: Some(REDIRECT_URI.to_string()),
        },
    });

    let code = tokio::select! {
        result = server.wait_for_code() => result.map(|result| result.code),
        manual_input = manual => {
            cancel.cancel();
            let (code, manual_state) = parse_authorization_input(&manual_input?);
            if let Some(manual_state) = manual_state
                && manual_state != state
            {
                return Err(AuthError("State mismatch".to_string()));
            }
            code
        }
    };

    let Some(code) = code else {
        return Err(AuthError("Missing authorization code".to_string()));
    };
    exchange_authorization_code(&code, &pkce.verifier, REDIRECT_URI, &interaction.signal).await
}

/// `loginOpenAICodexDeviceCode(interaction)`
async fn login_device_code(
    interaction: &ProviderAuthInteraction<'_>,
) -> Result<OAuthCredential, AuthError> {
    let device = start_device_auth(&interaction.signal).await?;
    interaction.notify(AuthEvent::DeviceCode {
        user_code: device.user_code.clone(),
        verification_uri: device_verification_uri(),
        interval_seconds: device.interval_seconds.map(|interval| interval as u64),
        expires_in_seconds: Some(DEVICE_CODE_TIMEOUT_SECONDS as u64),
    });

    let pkce = generate_pkce();
    let code = poll_device_code_flow(
        DeviceCodePollOptions {
            interval_seconds: device.interval_seconds,
            expires_in_seconds: Some(DEVICE_CODE_TIMEOUT_SECONDS),
            wait_before_first_poll: true,
            signal: interaction.signal.clone(),
        },
        || async {
            let response = reqwest::Client::new()
                .post(device_token_url())
                .header("Content-Type", "application/json")
                .header("Accept", "application/json")
                .json(&serde_json::json!({
                    "client_id": CLIENT_ID,
                    "device_code": device.device_code,
                    "code_challenge": pkce.challenge,
                    "code_challenge_method": "S256",
                }))
                .send()
                .await;
            let Ok(response) = response else {
                return DeviceCodePollResult::Failed {
                    message: "OpenAI Codex device polling failed".to_string(),
                };
            };
            let status = response.status();
            let Ok(body) = response.json::<Value>().await else {
                return DeviceCodePollResult::Failed {
                    message: format!(
                        "OpenAI Codex device polling returned invalid JSON ({})",
                        status.as_u16()
                    ),
                };
            };
            if let Some(code) = body.get("authorization_code").and_then(Value::as_str) {
                return DeviceCodePollResult::Complete {
                    value: code.to_string(),
                };
            }
            match body.get("error").and_then(Value::as_str) {
                Some("authorization_pending") | None => DeviceCodePollResult::Pending,
                Some("slow_down") => DeviceCodePollResult::SlowDown {
                    interval_seconds: body.get("interval").and_then(Value::as_f64),
                },
                Some(error) => DeviceCodePollResult::Failed {
                    message: format!("OpenAI Codex device authorization failed: {error}"),
                },
            }
        },
    )
    .await
    .map_err(|error| AuthError(error.to_string()))?;

    exchange_authorization_code(
        &code,
        &pkce.verifier,
        &device_redirect_uri(),
        &interaction.signal,
    )
    .await
}

pub fn openai_codex_oauth() -> Arc<dyn OAuthAuth> {
    Arc::new(OpenAICodexOAuth)
}
