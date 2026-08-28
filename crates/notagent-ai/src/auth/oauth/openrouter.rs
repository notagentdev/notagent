use std::sync::Arc;

use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use crate::auth::oauth::callback_server::{CallbackServerOptions, start_callback_server};
use crate::auth::oauth::pkce::generate_pkce;
use crate::auth::types::{
    AuthError, AuthEvent, AuthPrompt, AuthPromptKind, BoxFuture, ModelAuth, OAuthAuth,
    OAuthCredential, ProviderAuthInteraction,
};
use crate::utils::provider_env::get_provider_env_value;

const AUTHORIZE_URL: &str = "https://openrouter.ai/auth";
const TOKEN_URL: &str = "https://openrouter.ai/api/v1/auth/keys";
const TOKEN_EXCHANGE_TIMEOUT_MS: u64 = 30_000;
const PERMANENT_EXPIRY: i64 = 9_007_199_254_740_991;

fn callback_host() -> String {
    get_provider_env_value("NOTAGENT_OAUTH_CALLBACK_HOST", None)
        .unwrap_or_else(|| "127.0.0.1".to_string())
}

/// `parseAuthorizationInput(input)` — redirect URL, query string or bare code.
pub fn parse_authorization_input(input: &str) -> Option<String> {
    let value = input.trim();
    if value.is_empty() {
        return None;
    }
    if value.starts_with("http") {
        let query = value.split_once('?').map(|(_, query)| query).unwrap_or("");
        return query_value(query, "code");
    }
    if value.contains("code=") {
        return query_value(value, "code");
    }
    Some(value.to_string())
}

fn query_value(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        (name == key).then(|| value.to_string())
    })
}

/// `errorDetail(body)`
fn error_detail(body: &Map<String, Value>) -> Option<String> {
    for field in ["error_description", "message"] {
        if let Some(value) = body.get(field).and_then(Value::as_str) {
            return Some(value.to_string());
        }
    }
    match body.get("error") {
        Some(Value::String(value)) => Some(value.clone()),
        Some(Value::Object(error)) => error
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_string),
        _ => None,
    }
}

/// `exchangeAuthorizationCode(code, verifier, signal)`
pub async fn exchange_authorization_code(
    code: &str,
    verifier: &str,
    signal: &CancellationToken,
) -> Result<OAuthCredential, AuthError> {
    if signal.is_cancelled() {
        return Err(AuthError("Login cancelled".to_string()));
    }

    let request = reqwest::Client::new()
        .post(TOKEN_URL)
        .header("accept", "application/json")
        .header("content-type", "application/json")
        .timeout(std::time::Duration::from_millis(TOKEN_EXCHANGE_TIMEOUT_MS))
        .json(&serde_json::json!({
            "code": code,
            "code_verifier": verifier,
            "code_challenge_method": "S256"
        }))
        .send();

    let response = tokio::select! {
        response = request => response,
        _ = signal.cancelled() => return Err(AuthError("Login cancelled".to_string())),
    };
    let response = response.map_err(|error| {
        if signal.is_cancelled() {
            AuthError("Login cancelled".to_string())
        } else if error.is_timeout() {
            AuthError("OpenRouter OAuth token exchange timed out".to_string())
        } else {
            AuthError(error.to_string())
        }
    })?;

    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|error| AuthError(error.to_string()))?;
    let body = match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(object)) => object,
        _ => {
            if status.is_success() {
                return Err(AuthError(
                    "OpenRouter OAuth returned invalid JSON".to_string(),
                ));
            }
            Map::new()
        }
    };

    if !status.is_success() {
        let detail = error_detail(&body)
            .map(|detail| format!(": {detail}"))
            .unwrap_or_default();
        return Err(AuthError(format!(
            "OpenRouter OAuth key exchange failed (HTTP {}){detail}",
            status.as_u16()
        )));
    }

    match body
        .get("key")
        .and_then(Value::as_str)
        .filter(|key| !key.is_empty())
    {
        Some(key) => Ok(OAuthCredential {
            access: key.to_string(),
            refresh: String::new(),
            expires: PERMANENT_EXPIRY,
            extra: Default::default(),
        }),
        None => Err(AuthError(
            "OpenRouter OAuth response carries no \"key\"".to_string(),
        )),
    }
}

/// `openRouterOAuth`
pub struct OpenRouterOAuth;

impl OAuthAuth for OpenRouterOAuth {
    fn name(&self) -> &str {
        "OpenRouter OAuth"
    }

    fn login_label(&self) -> Option<&str> {
        Some("Sign in with OpenRouter")
    }

    fn login<'a>(
        &'a self,
        interaction: &'a ProviderAuthInteraction<'a>,
    ) -> BoxFuture<'a, Result<OAuthCredential, AuthError>> {
        Box::pin(async move {
            let pkce = generate_pkce();
            let callback_path = format!("/oauth/callback/{}", crate::utils::uuid::uuidv7());
            let host = callback_host();
            let listener = tokio::net::TcpListener::bind((host.as_str(), 0))
                .await
                .map_err(|error| AuthError(error.to_string()))?;
            let port = listener
                .local_addr()
                .map_err(|error| AuthError(error.to_string()))?
                .port();
            drop(listener);

            let callback_url = format!("http://{host}:{port}{callback_path}");
            let server = start_callback_server(CallbackServerOptions {
                host: host.clone(),
                port,
                path: callback_path.clone(),
                redirect_uri: callback_url.clone(),
                expected_state: pkce.verifier.clone(),
                success_message: "Signed in to OpenRouter. You may now close this page."
                    .to_string(),
                error_message: "OpenRouter sign-in did not complete.".to_string(),
            })
            .await
            .map_err(|error| AuthError(error.to_string()))?;
            let cancel = server.cancel_token();

            let authorize_url = format!(
                "{AUTHORIZE_URL}?callback_url={}&code_challenge={}&code_challenge_method=S256",
                super::anthropic::urlencode(&callback_url),
                super::anthropic::urlencode(&pkce.challenge)
            );
            interaction.notify(AuthEvent::Progress {
                message: format!("Listening for OpenRouter OAuth callback on {callback_url}"),
            });
            interaction.notify(AuthEvent::AuthUrl {
                url: authorize_url,
                instructions: Some(
                    "Complete sign-in in your browser. If the browser is on another machine, paste the final redirect URL here."
                        .to_string(),
                ),
            });

            let manual = interaction.prompt(AuthPrompt {
                signal: Some(cancel.clone()),
                kind: AuthPromptKind::ManualCode {
                    message: "Complete sign-in in your browser, or paste the authorization code / redirect URL here:"
                        .to_string(),
                    placeholder: Some(callback_url.clone()),
                },
            });

            // OpenRouter's callback carries the code but no state, so the exchange
            // happens here rather than inside the server.
            let code = tokio::select! {
                result = server.wait_for_code() => result.map(|result| result.code),
                manual_input = manual => {
                    cancel.cancel();
                    parse_authorization_input(&manual_input?)
                }
            };

            let Some(code) = code else {
                return Err(AuthError("Missing authorization code".to_string()));
            };
            interaction.notify(AuthEvent::Progress {
                message: "Exchanging authorization code for an API key...".to_string(),
            });
            exchange_authorization_code(&code, &pkce.verifier, &interaction.signal).await
        })
    }

    fn refresh<'a>(
        &'a self,
        credential: OAuthCredential,
        _signal: CancellationToken,
    ) -> BoxFuture<'a, Result<OAuthCredential, AuthError>> {
        Box::pin(async move { Ok(credential) })
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

pub fn open_router_oauth() -> Arc<dyn OAuthAuth> {
    Arc::new(OpenRouterOAuth)
}
