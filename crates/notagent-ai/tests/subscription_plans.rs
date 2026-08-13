//! End-to-end cover for the two subscription paths: the Claude plan (Anthropic OAuth)
//! and the Codex plan (ChatGPT OAuth).
//!
//! Each one walks the whole way a real request takes — the stored OAuth credential is
//! resolved through `resolveProviderAuth`, the resulting token goes into the adapter, and
//! the outgoing HTTP request is inspected. The pieces are covered individually elsewhere;
//! what this file pins down is that they fit together.

use std::sync::{Arc, Mutex};

use notagent_ai::api::anthropic_params::AnthropicOptions;
use notagent_ai::api::openai_codex_responses::OpenAICodexResponsesOptions;
use notagent_ai::auth::oauth::anthropic::anthropic_oauth;
use notagent_ai::auth::oauth::openai_codex::openai_codex_oauth;
use notagent_ai::auth::resolve::{AuthProvider, now_ms};
use notagent_ai::auth::types::*;
use notagent_ai::auth::{InMemoryCredentialStore, resolve_provider_auth};
use notagent_ai::types::{
    AssistantMessageEvent, Context, Message, Model, ProviderRequestOptions, Transport, UserContent,
    UserMessage,
};
use notagent_ai::utils::fetch::{FetchBody, FetchError, FetchFn, FetchRequest, FetchResponse};
use serde_json::{Value, json};

/// An auth context that answers no environment lookups: the stored credential decides.
struct EmptyAuthContext;

impl AuthContext for EmptyAuthContext {
    fn env(&self, _name: &str) -> BoxFuture<'_, Option<String>> {
        Box::pin(async { None })
    }

    fn file_exists(&self, _path: &str) -> BoxFuture<'_, bool> {
        Box::pin(async { false })
    }
}

async fn store_oauth(provider: &str, credential: OAuthCredential) -> InMemoryCredentialStore {
    let store = InMemoryCredentialStore::new();
    store
        .modify(
            provider,
            Box::new(move |_existing| {
                Box::pin(async move { Ok(Some(Credential::OAuth(credential))) })
            }),
            None,
        )
        .await
        .expect("seed credential");
    store
}

/// Records the outgoing request and answers with a complete stream.
struct RecordingFetch {
    body: String,
    seen: Arc<Mutex<Option<FetchRequest>>>,
}

impl FetchFn for RecordingFetch {
    fn fetch(
        &self,
        request: FetchRequest,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<FetchResponse, FetchError>> + Send>,
    > {
        *self.seen.lock().expect("poisoned") = Some(request);
        let body = self.body.clone();
        Box::pin(async move {
            Ok(FetchResponse {
                status: 200,
                status_text: String::new(),
                headers: vec![("content-type".to_string(), "text/event-stream".to_string())],
                body: FetchBody::Bytes(body.into_bytes()),
            })
        })
    }
}

fn model(raw: Value) -> Model {
    serde_json::from_value(raw).expect("model")
}

fn context() -> Context {
    Context {
        messages: vec![Message::User(UserMessage {
            content: UserContent::Text("hi".to_string()),
            timestamp: 1,
        })],
        ..Context::default()
    }
}

fn header<'a>(request: &'a FetchRequest, name: &str) -> Option<&'a str> {
    request
        .headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

// ---------------------------------------------------------------------------
// Claude plan
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn a_claude_plan_credential_reaches_anthropic_as_a_claude_code_request() {
    // A Claude Pro/Max login stores an OAuth credential whose access token is the
    // `sk-ant-oat` token the Messages API accepts as a bearer.
    let auth = ProviderAuth {
        api_key: None,
        oauth: Some(anthropic_oauth()),
    };
    let store = store_oauth(
        "anthropic",
        OAuthCredential {
            access: "sk-ant-oat01-plan-token".to_string(),
            refresh: "refresh".to_string(),
            // Far enough out that no refresh is attempted.
            expires: now_ms() + 3_600_000,
            extra: Default::default(),
        },
    )
    .await;

    let resolved = resolve_provider_auth(
        AuthProvider {
            id: "anthropic",
            auth: &auth,
        },
        &store,
        Arc::new(EmptyAuthContext),
        None,
    )
    .await
    .expect("resolution")
    .expect("configured");
    let api_key = resolved.auth.api_key.expect("api key");
    assert_eq!(api_key, "sk-ant-oat01-plan-token");
    // The flow marks itself as subscription-backed, which is what the model list uses.
    assert!(auth.oauth.as_ref().expect("oauth").is_subscription());

    let seen = Arc::new(Mutex::new(None));
    let events = notagent_ai::api::anthropic_messages::stream(
        model(json!({
            "id": "claude-opus-4-5", "name": "Claude Opus 4.5", "api": "anthropic-messages",
            "provider": "anthropic", "baseUrl": "https://api.anthropic.com", "reasoning": true,
            "input": ["text"], "cost": { "input": 5, "output": 25, "cacheRead": 0.5, "cacheWrite": 6.25 },
            "contextWindow": 200000, "maxTokens": 64000,
        })),
        Context {
            system_prompt: Some("be nice".to_string()),
            tools: Some(vec![notagent_ai::types::Tool {
                name: "read".to_string(),
                description: "Reads a file".to_string(),
                parameters: json!({ "type": "object", "properties": {} }),
                constrained_sampling: None,
            }]),
            ..context()
        },
        ProviderRequestOptions {
            api_key: Some(api_key),
            fetch: Some(Arc::new(RecordingFetch {
                body: concat!(
                    "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"m\",\"usage\":{}}}\n\n",
                    "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n\n",
                    "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
                )
                .to_string(),
                seen: seen.clone(),
            })),
            ..ProviderRequestOptions::default()
        },
        AnthropicOptions::default(),
    );
    let mut last = None;
    while let Some(event) = events.next().await {
        last = Some(event);
    }
    assert!(matches!(last, Some(AssistantMessageEvent::Done { .. })));

    let seen = seen.lock().expect("poisoned");
    let request = seen.as_ref().expect("request");
    // An OAuth token is a bearer, never an x-api-key.
    assert_eq!(
        header(request, "authorization"),
        Some("Bearer sk-ant-oat01-plan-token")
    );
    assert_eq!(header(request, "x-api-key"), None);
    // The subscription path identifies as Claude Code.
    assert_eq!(header(request, "x-app"), Some("cli"));
    let user_agent = header(request, "user-agent").expect("user agent");
    assert!(user_agent.starts_with("claude-cli/"), "{user_agent}");
    let beta = header(request, "anthropic-beta").expect("beta");
    assert!(beta.contains("oauth-2025-04-20"), "{beta}");

    let body: Value = serde_json::from_slice(request.body.as_ref().expect("body")).expect("json");
    // The Claude Code system prompt is prepended, and the caller's prompt follows it.
    let system = body["system"].as_array().expect("system");
    assert!(
        system[0]["text"]
            .as_str()
            .expect("text")
            .contains("Claude Code"),
        "{}",
        system[0]["text"]
    );
    assert_eq!(system[1]["text"], "be nice");
    // Tool names are mapped onto the Claude Code spelling.
    assert_eq!(body["tools"][0]["name"], "Read");
}

// ---------------------------------------------------------------------------
// Codex plan
// ---------------------------------------------------------------------------

fn codex_token(account_id: &str) -> String {
    use base64::Engine;
    let payload = json!({
        "https://api.openai.com/auth": { "chatgpt_account_id": account_id }
    });
    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::to_vec(&payload).expect("json"));
    format!("header.{encoded}.signature")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_codex_plan_credential_reaches_the_chatgpt_backend_with_its_account_id() {
    // A ChatGPT login stores an OAuth credential whose access token is a JWT carrying the
    // account id the Codex backend wants in a header.
    let auth = ProviderAuth {
        api_key: None,
        oauth: Some(openai_codex_oauth()),
    };
    let token = codex_token("acct_plan_1");
    let store = store_oauth(
        "openai-codex",
        OAuthCredential {
            access: token.clone(),
            refresh: "refresh".to_string(),
            expires: now_ms() + 3_600_000,
            extra: Default::default(),
        },
    )
    .await;

    let resolved = resolve_provider_auth(
        AuthProvider {
            id: "openai-codex",
            auth: &auth,
        },
        &store,
        Arc::new(EmptyAuthContext),
        None,
    )
    .await
    .expect("resolution")
    .expect("configured");
    let api_key = resolved.auth.api_key.expect("api key");
    assert_eq!(api_key, token);
    assert!(auth.oauth.as_ref().expect("oauth").is_subscription());

    let seen = Arc::new(Mutex::new(None));
    let events = notagent_ai::api::openai_codex_responses::stream(
        model(json!({
            "id": "gpt-5-codex", "name": "gpt-5-codex", "api": "openai-codex-responses",
            "provider": "openai-codex", "baseUrl": "https://chatgpt.com/backend-api",
            "reasoning": true, "input": ["text"],
            "cost": { "input": 1, "output": 2, "cacheRead": 0, "cacheWrite": 0 },
            "contextWindow": 128000, "maxTokens": 4096,
        })),
        Context {
            system_prompt: Some("be precise".to_string()),
            ..context()
        },
        ProviderRequestOptions {
            api_key: Some(api_key),
            max_retries: Some(0),
            fetch: Some(Arc::new(RecordingFetch {
                body: "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"status\":\"completed\"}}\n\ndata: [DONE]\n\n".to_string(),
                seen: seen.clone(),
            })),
            ..ProviderRequestOptions::default()
        },
        OpenAICodexResponsesOptions {
            // The WebSocket path needs a real socket; the SSE path is the fallback anyway.
            transport: Some(Transport::Sse),
            session_id: Some("plan-session".to_string()),
            ..OpenAICodexResponsesOptions::default()
        },
    );
    let mut last = None;
    while let Some(event) = events.next().await {
        last = Some(event);
    }
    assert!(
        matches!(last, Some(AssistantMessageEvent::Done { .. })),
        "{last:?}"
    );

    let seen = seen.lock().expect("poisoned");
    let request = seen.as_ref().expect("request");
    assert_eq!(
        request.url,
        "https://chatgpt.com/backend-api/codex/responses"
    );
    assert_eq!(
        header(request, "authorization"),
        Some(format!("Bearer {token}").as_str())
    );
    // The account id is read out of the JWT the login stored.
    assert_eq!(header(request, "chatgpt-account-id"), Some("acct_plan_1"));
    assert_eq!(header(request, "originator"), Some("notagent"));
    assert_eq!(
        header(request, "openai-beta"),
        Some("responses=experimental")
    );
    assert_eq!(header(request, "session-id"), Some("plan-session"));
    // The body is zstd-compressed, as the official Codex client does it.
    assert_eq!(header(request, "content-encoding"), Some("zstd"));
    let decoded =
        zstd::stream::decode_all(request.body.as_ref().expect("body").as_slice()).expect("zstd");
    let body: Value = serde_json::from_slice(&decoded).expect("json");
    assert_eq!(body["model"], "gpt-5-codex");
    assert_eq!(body["store"], false);
    // The system prompt travels as `instructions`, not as a message.
    assert_eq!(body["instructions"], "be precise");
    assert_eq!(body["prompt_cache_key"], "plan-session");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_expired_claude_plan_credential_is_refreshed_before_the_request() {
    // A credential past its expiry has to be refreshed by the flow before it is handed
    // to the adapter; without a reachable endpoint that refresh fails, and the failure
    // must surface rather than sending a stale token.
    let auth = ProviderAuth {
        api_key: None,
        oauth: Some(anthropic_oauth()),
    };
    let store = store_oauth(
        "anthropic",
        OAuthCredential {
            access: "sk-ant-oat01-expired".to_string(),
            refresh: "refresh".to_string(),
            expires: now_ms() - 1_000,
            extra: Default::default(),
        },
    )
    .await;

    let result = resolve_provider_auth(
        AuthProvider {
            id: "anthropic",
            auth: &auth,
        },
        &store,
        Arc::new(EmptyAuthContext),
        None,
    )
    .await;
    let error = result.expect_err("refresh must fail without a reachable endpoint");
    assert_eq!(error.code, notagent_ai::auth::ModelsErrorCode::OAuth);
}
