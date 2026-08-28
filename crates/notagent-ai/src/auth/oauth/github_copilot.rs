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

fn client_id() -> String {
    use base64::Engine;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode("SXYxLmI1MDdhMDhjODdlY2ZlOTg=")
        .expect("the embedded client id is valid base64");
    String::from_utf8(decoded).expect("the embedded client id is UTF-8")
}

/// `COPILOT_HEADERS`
pub const COPILOT_HEADERS: [(&str, &str); 4] = [
    ("User-Agent", "GitHubCopilotChat/0.35.0"),
    ("Editor-Version", "vscode/1.107.0"),
    ("Editor-Plugin-Version", "copilot-chat/0.35.0"),
    ("Copilot-Integration-Id", "vscode-chat"),
];
pub const COPILOT_API_VERSION: &str = "2026-06-01";

/// `normalizeDomain(input)` — returns the hostname.
pub fn normalize_domain(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    let without_scheme = trimmed
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(trimmed);
    let host = without_scheme.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit('@').next().unwrap_or(host);
    let host = host.split(':').next().unwrap_or(host);
    if host.is_empty() {
        None
    } else {
        Some(host.to_string())
    }
}

/// `getUrls(domain)`
pub struct CopilotUrls {
    pub device_code_url: String,
    pub access_token_url: String,
    pub copilot_token_url: String,
}

pub fn get_urls(domain: &str) -> CopilotUrls {
    CopilotUrls {
        device_code_url: format!("https://{domain}/login/device/code"),
        access_token_url: format!("https://{domain}/login/oauth/access_token"),
        copilot_token_url: format!("https://api.{domain}/copilot_internal/v2/token"),
    }
}

/// `getBaseUrlFromToken(token)` — `proxy-ep=proxy.x` becomes `https://api.x`.
pub fn base_url_from_token(token: &str) -> Option<String> {
    let start = token.find("proxy-ep=")? + "proxy-ep=".len();
    let rest = &token[start..];
    let proxy_host = rest.split(';').next().unwrap_or(rest);
    if proxy_host.is_empty() {
        return None;
    }
    let api_host = proxy_host
        .strip_prefix("proxy.")
        .map(|rest| format!("api.{rest}"))
        .unwrap_or_else(|| proxy_host.to_string());
    Some(format!("https://{api_host}"))
}

/// `getGitHubCopilotBaseUrl(token?, enterpriseDomain?)`
pub fn github_copilot_base_url(token: Option<&str>, enterprise_domain: Option<&str>) -> String {
    if let Some(token) = token
        && let Some(url) = base_url_from_token(token)
    {
        return url;
    }
    if let Some(enterprise_domain) = enterprise_domain.filter(|domain| !domain.is_empty()) {
        return format!("https://copilot-api.{enterprise_domain}");
    }
    "https://api.individual.githubcopilot.com".to_string()
}

/// `parseAvailableCopilotModelIds(raw, allowPolicyFallback)`
pub fn parse_available_copilot_model_ids(
    raw: &Value,
    allow_policy_fallback: bool,
) -> Result<Vec<String>, AuthError> {
    let Some(data) = raw.get("data").and_then(Value::as_array) else {
        return Err(AuthError("Invalid Copilot models response".to_string()));
    };

    let mut picker_ids = Vec::new();
    let mut policy_enabled_ids = Vec::new();
    for item in data {
        let Some(id) = item.get("id").and_then(Value::as_str) else {
            continue;
        };
        // Models without tool calls are unusable for the agent.
        if item
            .get("capabilities")
            .and_then(|capabilities| capabilities.get("supports"))
            .and_then(|supports| supports.get("tool_calls"))
            == Some(&Value::Bool(false))
        {
            continue;
        }
        let policy_state = item
            .get("policy")
            .and_then(|policy| policy.get("state"))
            .and_then(Value::as_str);
        if item.get("model_picker_enabled") == Some(&Value::Bool(true))
            && policy_state != Some("disabled")
        {
            picker_ids.push(id.to_string());
        }
        if policy_state == Some("enabled") {
            policy_enabled_ids.push(id.to_string());
        }
    }

    Ok(if !picker_ids.is_empty() || !allow_policy_fallback {
        picker_ids
    } else {
        policy_enabled_ids
    })
}

/// `DeviceCodeResponse`
#[derive(Debug, Clone)]
pub struct CopilotDeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub interval: Option<f64>,
    pub expires_in: f64,
}

/// Parses and validates the device-code response.
pub fn parse_device_code(body: &Map<String, Value>) -> Result<CopilotDeviceCode, AuthError> {
    let device_code = body.get("device_code").and_then(Value::as_str);
    let user_code = body.get("user_code").and_then(Value::as_str);
    let verification_uri = body.get("verification_uri").and_then(Value::as_str);
    let expires_in = body.get("expires_in").and_then(Value::as_f64);
    let (Some(device_code), Some(user_code), Some(verification_uri), Some(expires_in)) =
        (device_code, user_code, verification_uri, expires_in)
    else {
        return Err(AuthError("Invalid device code response fields".to_string()));
    };
    // The URI is opened in a browser, so only http(s) is trusted.
    if !verification_uri.starts_with("https://") && !verification_uri.starts_with("http://") {
        return Err(AuthError(
            "Untrusted verification_uri in device code response".to_string(),
        ));
    }
    Ok(CopilotDeviceCode {
        device_code: device_code.to_string(),
        user_code: user_code.to_string(),
        verification_uri: verification_uri.to_string(),
        interval: body.get("interval").and_then(Value::as_f64),
        expires_in,
    })
}

/// Classifies one access-token poll response.
pub fn classify_poll_response(body: &Map<String, Value>) -> DeviceCodePollResult<String> {
    if let Some(access_token) = body.get("access_token").and_then(Value::as_str)
        && !access_token.is_empty()
    {
        return DeviceCodePollResult::Complete {
            value: access_token.to_string(),
        };
    }
    match body.get("error").and_then(Value::as_str) {
        Some("authorization_pending") => DeviceCodePollResult::Pending,
        Some("slow_down") => DeviceCodePollResult::SlowDown {
            interval_seconds: body.get("interval").and_then(Value::as_f64),
        },
        Some(error) => {
            let description = body
                .get("error_description")
                .and_then(Value::as_str)
                .map(|description| format!(": {description}"))
                .unwrap_or_default();
            DeviceCodePollResult::Failed {
                message: format!("GitHub device flow failed: {error}{description}"),
            }
        }
        None => DeviceCodePollResult::Pending,
    }
}

async fn fetch_json(
    url: &str,
    bearer: Option<&str>,
    signal: &CancellationToken,
) -> Result<Value, AuthError> {
    let mut request = reqwest::Client::new()
        .get(url)
        .header("Accept", "application/json");
    for (name, value) in COPILOT_HEADERS {
        request = request.header(name, value);
    }
    request = request.header("X-GitHub-Api-Version", COPILOT_API_VERSION);
    if let Some(bearer) = bearer {
        request = request.header("Authorization", format!("Bearer {bearer}"));
    }

    let response = tokio::select! {
        response = request.send() => response.map_err(|error| AuthError(error.to_string()))?,
        _ = signal.cancelled() => return Err(AuthError("Login cancelled".to_string())),
    };
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|error| AuthError(error.to_string()))?;
    if !status.is_success() {
        return Err(AuthError(format!(
            "{} {}: {text}",
            status.as_u16(),
            status.canonical_reason().unwrap_or("")
        )));
    }
    serde_json::from_str(&text).map_err(|error| AuthError(error.to_string()))
}

/// `refreshGitHubCopilotAccessToken(refreshToken, enterpriseDomain, signal)`
pub async fn refresh_copilot_access_token(
    refresh_token: &str,
    enterprise_domain: Option<&str>,
    signal: &CancellationToken,
) -> Result<OAuthCredential, AuthError> {
    let domain = enterprise_domain
        .filter(|domain| !domain.is_empty())
        .unwrap_or("github.com");
    let urls = get_urls(domain);
    let raw = fetch_json(&urls.copilot_token_url, Some(refresh_token), signal).await?;

    let token = raw.get("token").and_then(Value::as_str);
    let expires_at = raw.get("expires_at").and_then(Value::as_f64);
    let (Some(token), Some(expires_at)) = (token, expires_at) else {
        return Err(AuthError(
            "Invalid Copilot token response fields".to_string(),
        ));
    };

    let mut extra = Map::new();
    if let Some(enterprise_domain) = enterprise_domain {
        extra.insert(
            "enterpriseUrl".to_string(),
            Value::String(enterprise_domain.to_string()),
        );
    }
    Ok(OAuthCredential {
        refresh: refresh_token.to_string(),
        access: token.to_string(),
        expires: (expires_at * 1000.0) as i64 - 5 * 60 * 1000,
        extra,
    })
}

/// `fetchAvailableGitHubCopilotModelIds(...)`
pub async fn fetch_available_model_ids(
    copilot_token: &str,
    enterprise_domain: Option<&str>,
    signal: &CancellationToken,
) -> Result<Vec<String>, AuthError> {
    let base_url = github_copilot_base_url(Some(copilot_token), enterprise_domain);
    // Individual accounts can report false for every picker flag despite enabled policies.
    let allow_policy_fallback = base_url == "https://api.individual.githubcopilot.com";
    let raw = fetch_json(&format!("{base_url}/models"), Some(copilot_token), signal).await?;
    parse_available_copilot_model_ids(&raw, allow_policy_fallback)
}

/// `copilotEnterpriseDomain(credential)`
pub fn copilot_enterprise_domain(credential: &OAuthCredential) -> Option<String> {
    let enterprise_url = credential
        .extra
        .get("enterpriseUrl")
        .and_then(Value::as_str)?;
    normalize_domain(enterprise_url)
}

/// `githubCopilotOAuth`
pub struct GitHubCopilotOAuth;

impl OAuthAuth for GitHubCopilotOAuth {
    fn name(&self) -> &str {
        "GitHub Copilot"
    }

    fn is_subscription(&self) -> bool {
        true
    }

    fn login<'a>(
        &'a self,
        interaction: &'a ProviderAuthInteraction<'a>,
    ) -> BoxFuture<'a, Result<OAuthCredential, AuthError>> {
        Box::pin(async move {
            let domain = "github.com";
            let urls = get_urls(domain);
            let response = post_form(
                &urls.device_code_url,
                &[
                    ("client_id", client_id()),
                    ("scope", "read:user".to_string()),
                ],
                &interaction.signal,
                &[("User-Agent", "GitHubCopilotChat/0.35.0".to_string())],
            )
            .await?;
            let device = parse_device_code(&response.body)?;

            interaction.notify(AuthEvent::DeviceCode {
                user_code: device.user_code.clone(),
                verification_uri: device.verification_uri.clone(),
                interval_seconds: device.interval.map(|interval| interval as u64),
                expires_in_seconds: Some(device.expires_in as u64),
            });

            let github_token = poll_device_code_flow(
                DeviceCodePollOptions {
                    interval_seconds: device.interval,
                    expires_in_seconds: Some(device.expires_in),
                    wait_before_first_poll: true,
                    signal: interaction.signal.clone(),
                },
                || async {
                    let response = post_form(
                        &urls.access_token_url,
                        &[
                            ("client_id", client_id()),
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
                        Ok(response) => classify_poll_response(&response.body),
                        Err(error) => DeviceCodePollResult::Failed {
                            message: error.to_string(),
                        },
                    }
                },
            )
            .await
            .map_err(|error| AuthError(error.to_string()))?;

            let mut credential =
                refresh_copilot_access_token(&github_token, None, &interaction.signal).await?;
            interaction.notify(AuthEvent::Progress {
                message: "Enabling models...".to_string(),
            });
            let available =
                fetch_available_model_ids(&credential.access, None, &interaction.signal).await?;
            credential.extra.insert(
                "availableModelIds".to_string(),
                Value::Array(available.into_iter().map(Value::String).collect()),
            );
            Ok(credential)
        })
    }

    fn refresh<'a>(
        &'a self,
        credential: OAuthCredential,
        signal: CancellationToken,
    ) -> BoxFuture<'a, Result<OAuthCredential, AuthError>> {
        Box::pin(async move {
            let enterprise_domain = copilot_enterprise_domain(&credential);
            let mut refreshed = refresh_copilot_access_token(
                &credential.refresh,
                enterprise_domain.as_deref(),
                &signal,
            )
            .await?;
            let available =
                fetch_available_model_ids(&refreshed.access, enterprise_domain.as_deref(), &signal)
                    .await?;
            refreshed.extra.insert(
                "availableModelIds".to_string(),
                Value::Array(available.into_iter().map(Value::String).collect()),
            );
            Ok(refreshed)
        })
    }

    fn to_auth<'a>(
        &'a self,
        credential: OAuthCredential,
    ) -> BoxFuture<'a, Result<ModelAuth, AuthError>> {
        Box::pin(async move {
            // The proxy endpoint is credential specific and derived per request.
            let enterprise_domain = copilot_enterprise_domain(&credential);
            Ok(ModelAuth {
                api_key: Some(credential.access.clone()),
                base_url: Some(github_copilot_base_url(
                    Some(&credential.access),
                    enterprise_domain.as_deref(),
                )),
                headers: None,
            })
        })
    }
}

pub fn github_copilot_oauth() -> Arc<dyn OAuthAuth> {
    Arc::new(GitHubCopilotOAuth)
}

/// Only used by the provider factory: the credential's model allow-list.
pub fn available_model_ids(credential: &OAuthCredential) -> Option<Vec<String>> {
    let ids = credential.extra.get("availableModelIds")?.as_array()?;
    Some(
        ids.iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
    )
}

/// Header map for Copilot requests.
pub fn copilot_headers() -> BTreeMap<String, String> {
    COPILOT_HEADERS
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect()
}
