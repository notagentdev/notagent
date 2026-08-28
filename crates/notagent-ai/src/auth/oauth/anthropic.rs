use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;

use crate::auth::oauth::callback_server::{CallbackServerOptions, start_callback_server};
use crate::auth::oauth::pkce::generate_pkce;
use crate::auth::types::{
    AuthError, AuthEvent, AuthPrompt, AuthPromptKind, BoxFuture, ModelAuth, OAuthAuth,
    OAuthCredential, ProviderAuthInteraction,
};
use crate::utils::provider_env::get_provider_env_value;

fn client_id() -> String {
    let decoded = STANDARD
        .decode("OWQxYzI1MGEtZTYxYi00NGQ5LTg4ZWQtNTk0NGQxOTYyZjVl")
        .expect("the embedded client id is valid base64");
    String::from_utf8(decoded).expect("the embedded client id is UTF-8")
}

const AUTHORIZE_URL: &str = "https://claude.ai/oauth/authorize";
const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const CALLBACK_PORT: u16 = 53692;
const CALLBACK_PATH: &str = "/callback";
const SCOPES: &str = "org:create_api_key user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload";

fn callback_host() -> String {
    get_provider_env_value("NOTAGENT_OAUTH_CALLBACK_HOST", None)
        .unwrap_or_else(|| "127.0.0.1".to_string())
}

fn redirect_uri() -> String {
    format!("http://localhost:{CALLBACK_PORT}{CALLBACK_PATH}")
}

/// `parseAuthorizationInput(input)` — accepts a full redirect URL, `code#state`,
/// a query string or a bare code.
pub fn parse_authorization_input(input: &str) -> (Option<String>, Option<String>) {
    let value = input.trim();
    if value.is_empty() {
        return (None, None);
    }

    if let Some((_, query)) = value.split_once('?')
        && value.starts_with("http")
    {
        let params = parse_query(query);
        return (params.0, params.1);
    }
    if value.starts_with("http") {
        return (None, None);
    }
    if let Some((code, state)) = value.split_once('#') {
        return (Some(code.to_string()), Some(state.to_string()));
    }
    if value.contains("code=") {
        let params = parse_query(value);
        return (params.0, params.1);
    }
    (Some(value.to_string()), None)
}

fn parse_query(query: &str) -> (Option<String>, Option<String>) {
    let mut code = None;
    let mut state = None;
    for pair in query.split('&') {
        match pair.split_once('=') {
            Some(("code", value)) => code = Some(value.to_string()),
            Some(("state", value)) => state = Some(value.to_string()),
            _ => {}
        }
    }
    (code, state)
}

/// `postJson(url, body, signal)`
async fn post_json(url: &str, body: serde_json::Value) -> Result<String, AuthError> {
    let client = reqwest::Client::new();
    let response = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .timeout(std::time::Duration::from_secs(30))
        .json(&body)
        .send()
        .await
        .map_err(|error| AuthError(error.to_string()))?;
    let status = response.status().as_u16();
    let response_body = response
        .text()
        .await
        .map_err(|error| AuthError(error.to_string()))?;
    if !(200..300).contains(&status) {
        return Err(AuthError(format!(
            "HTTP request failed. status={status}; url={url}; body={response_body}"
        )));
    }
    Ok(response_body)
}

#[derive(serde::Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    expires_in: i64,
}

fn credential_from(token: TokenResponse) -> OAuthCredential {
    OAuthCredential {
        refresh: token.refresh_token,
        access: token.access_token,
        expires: crate::auth::resolve::now_ms() + token.expires_in * 1000 - 5 * 60 * 1000,
        extra: Default::default(),
    }
}

/// `exchangeAuthorizationCode(...)`
async fn exchange_authorization_code(
    code: &str,
    state: &str,
    verifier: &str,
    redirect_uri: &str,
) -> Result<OAuthCredential, AuthError> {
    let body = serde_json::json!({
        "grant_type": "authorization_code",
        "client_id": client_id(),
        "code": code,
        "state": state,
        "redirect_uri": redirect_uri,
        "code_verifier": verifier,
    });
    let response_body = post_json(TOKEN_URL, body).await.map_err(|error| {
        AuthError(format!(
            "Token exchange request failed. url={TOKEN_URL}; redirect_uri={redirect_uri}; response_type=authorization_code; details={error}"
        ))
    })?;
    let token: TokenResponse = serde_json::from_str(&response_body).map_err(|error| {
        AuthError(format!(
            "Token exchange returned invalid JSON. url={TOKEN_URL}; body={response_body}; details={error}"
        ))
    })?;
    Ok(credential_from(token))
}

/// `refreshAnthropicToken(refreshToken, signal)`
async fn refresh_anthropic_token(refresh_token: &str) -> Result<OAuthCredential, AuthError> {
    let body = serde_json::json!({
        "grant_type": "refresh_token",
        "client_id": client_id(),
        "refresh_token": refresh_token,
    });
    let response_body = post_json(TOKEN_URL, body).await.map_err(|error| {
        AuthError(format!(
            "Anthropic token refresh request failed. url={TOKEN_URL}; details={error}"
        ))
    })?;
    let token: TokenResponse = serde_json::from_str(&response_body).map_err(|error| {
        AuthError(format!(
            "Anthropic token refresh returned invalid JSON. url={TOKEN_URL}; body={response_body}; details={error}"
        ))
    })?;
    Ok(credential_from(token))
}

/// `anthropicOAuth`
pub struct AnthropicOAuth;

impl OAuthAuth for AnthropicOAuth {
    fn name(&self) -> &str {
        "Anthropic (Claude Pro/Max)"
    }

    fn is_subscription(&self) -> bool {
        true
    }

    fn login<'a>(
        &'a self,
        interaction: &'a ProviderAuthInteraction<'a>,
    ) -> BoxFuture<'a, Result<OAuthCredential, AuthError>> {
        Box::pin(async move {
            let pkce = generate_pkce();
            let redirect_uri = redirect_uri();
            let server = start_callback_server(CallbackServerOptions {
                host: callback_host(),
                port: CALLBACK_PORT,
                path: CALLBACK_PATH.to_string(),
                redirect_uri: redirect_uri.clone(),
                expected_state: pkce.verifier.clone(),
                success_message: "Anthropic authentication completed. You can close this window."
                    .to_string(),
                error_message: "Anthropic authentication did not complete.".to_string(),
            })
            .await
            .map_err(|error| AuthError(error.to_string()))?;

            let cancel = server.cancel_token();
            let abort_signal = interaction.signal.clone();
            let abort_cancel = cancel.clone();
            tokio::spawn(async move {
                abort_signal.cancelled().await;
                abort_cancel.cancel();
            });

            let auth_params = [
                ("code", "true".to_string()),
                ("client_id", client_id()),
                ("response_type", "code".to_string()),
                ("redirect_uri", redirect_uri.clone()),
                ("scope", SCOPES.to_string()),
                ("code_challenge", pkce.challenge.clone()),
                ("code_challenge_method", "S256".to_string()),
                ("state", pkce.verifier.clone()),
            ]
            .iter()
            .map(|(key, value)| format!("{key}={}", urlencode(value)))
            .collect::<Vec<_>>()
            .join("&");

            interaction.notify(AuthEvent::AuthUrl {
                url: format!("{AUTHORIZE_URL}?{auth_params}"),
                instructions: Some(
                    "Complete login in your browser. If the browser is on another machine, paste the final redirect URL here."
                        .to_string(),
                ),
            });

            // The manual prompt races the callback server; whichever answers first wins.
            let manual = interaction.prompt(AuthPrompt {
                signal: Some(cancel.clone()),
                kind: AuthPromptKind::ManualCode {
                    message: "Complete login in your browser, or paste the authorization code / redirect URL here:"
                        .to_string(),
                    placeholder: Some(redirect_uri.clone()),
                },
            });

            let (code, state) = tokio::select! {
                result = server.wait_for_code() => match result {
                    Some(result) => (Some(result.code), Some(result.state)),
                    None => (None, None),
                },
                manual_input = manual => {
                    cancel.cancel();
                    let input = manual_input?;
                    let (code, state) = parse_authorization_input(&input);
                    if let Some(state) = &state
                        && state != &pkce.verifier
                    {
                        return Err(AuthError("OAuth state mismatch".to_string()));
                    }
                    (code, Some(state.unwrap_or_else(|| pkce.verifier.clone())))
                }
            };

            let Some(code) = code else {
                return Err(AuthError("Missing authorization code".to_string()));
            };
            let Some(state) = state else {
                return Err(AuthError("Missing OAuth state".to_string()));
            };

            interaction.notify(AuthEvent::Progress {
                message: "Exchanging authorization code for tokens...".to_string(),
            });
            exchange_authorization_code(&code, &state, &pkce.verifier, &redirect_uri).await
        })
    }

    fn refresh<'a>(
        &'a self,
        credential: OAuthCredential,
        _signal: tokio_util::sync::CancellationToken,
    ) -> BoxFuture<'a, Result<OAuthCredential, AuthError>> {
        Box::pin(async move { refresh_anthropic_token(&credential.refresh).await })
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

pub fn anthropic_oauth() -> Arc<dyn OAuthAuth> {
    Arc::new(AnthropicOAuth)
}

pub fn urlencode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            b' ' => "+".to_string(),
            other => format!("%{other:02X}"),
        })
        .collect()
}
