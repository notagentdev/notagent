//! The MTPLX providers: address-derived key policy, activation through login,
//! and the session header the server's warm-prefix bank keys on.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use notagent_ai::api::openai_completions_compat::get_compat;
use notagent_ai::api::openai_completions_params::build_client_headers;
use notagent_ai::auth::types::{
    ApiKeyAuthInput, ApiKeyCredential, AuthContext, AuthError, AuthEvent, AuthPrompt,
    AuthPromptKind, BoxFuture, ProviderAuthInteraction,
};
use notagent_ai::models::Provider;
use notagent_ai::providers::mtplx::{MtplxProviderConfig, models_from_listing, mtplx_provider};
use notagent_ai::types::{Context, Message, ProviderEnv, UserContent, UserMessage};
use serde_json::json;

// ---------------------------------------------------------------------------
// Test doubles
// ---------------------------------------------------------------------------

struct FakeAuthContext {
    env: ProviderEnv,
}

impl FakeAuthContext {
    fn shared(env: &[(&str, &str)]) -> Arc<dyn AuthContext> {
        Arc::new(FakeAuthContext {
            env: env
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect(),
        })
    }
}

impl AuthContext for FakeAuthContext {
    fn env(&self, name: &str) -> BoxFuture<'_, Option<String>> {
        let value = self.env.get(name).cloned();
        Box::pin(async move { value })
    }

    fn file_exists(&self, _path: &str) -> BoxFuture<'_, bool> {
        Box::pin(async move { false })
    }
}

struct ScriptedInteraction {
    answers: Mutex<Vec<String>>,
    prompts: Mutex<Vec<AuthPromptKind>>,
}

impl ScriptedInteraction {
    fn new(answers: &[&str]) -> Arc<ScriptedInteraction> {
        Arc::new(ScriptedInteraction {
            answers: Mutex::new(answers.iter().rev().map(|a| a.to_string()).collect()),
            prompts: Mutex::new(Vec::new()),
        })
    }
}

impl notagent_ai::auth::types::AuthInteraction for ScriptedInteraction {
    fn signal(&self) -> Option<tokio_util::sync::CancellationToken> {
        None
    }

    fn prompt(&self, prompt: AuthPrompt) -> BoxFuture<'_, Result<String, AuthError>> {
        self.prompts.lock().expect("prompts").push(prompt.kind);
        let answer = self.answers.lock().expect("answers").pop();
        Box::pin(
            async move { answer.ok_or_else(|| AuthError("no scripted answer left".to_string())) },
        )
    }

    fn notify(&self, _event: AuthEvent) {}
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn local_config() -> MtplxProviderConfig {
    MtplxProviderConfig {
        id: "mtplx".to_string(),
        name: "MTPLX (local)".to_string(),
        base_url: "http://127.0.0.1:8000/v1".to_string(),
        client_hint: "notagent".to_string(),
    }
}

fn remote_config() -> MtplxProviderConfig {
    MtplxProviderConfig {
        base_url: "https://mtplx.example.com/v1".to_string(),
        name: "MTPLX (remote)".to_string(),
        ..local_config()
    }
}

async fn resolve(
    config: MtplxProviderConfig,
    env: &[(&str, &str)],
    credential: Option<ApiKeyCredential>,
) -> Option<String> {
    let provider = mtplx_provider(config);
    let auth = provider.auth();
    let api_key = auth.api_key.as_ref().expect("mtplx offers api-key auth");
    let ctx = FakeAuthContext::shared(env);
    api_key
        .resolve(ApiKeyAuthInput {
            ctx: ctx.as_ref(),
            credential,
            signal: tokio_util::sync::CancellationToken::new(),
        })
        .await
        .expect("resolve does not fail")
        .and_then(|result| result.auth.api_key)
}

async fn login(config: MtplxProviderConfig, answers: &[&str]) -> (Option<String>, usize) {
    let provider = mtplx_provider(config);
    let auth = provider.auth();
    let api_key = auth.api_key.as_ref().expect("mtplx offers api-key auth");
    let interaction = ScriptedInteraction::new(answers);
    let provider_interaction = ProviderAuthInteraction {
        interaction: interaction.as_ref(),
        signal: tokio_util::sync::CancellationToken::new(),
    };
    let credential = api_key
        .login(&provider_interaction)
        .expect("mtplx offers a login flow")
        .await
        .expect("login succeeds");
    let prompts = interaction.prompts.lock().expect("prompts").len();
    (credential.key, prompts)
}

// ---------------------------------------------------------------------------
// Address-derived key policy
// ---------------------------------------------------------------------------

#[test]
fn loopback_is_derived_from_the_address_not_a_flag() {
    for address in [
        "http://127.0.0.1:8000/v1",
        "http://localhost:8000/v1",
        "http://[::1]:8000/v1",
        "http://127.0.0.1/v1",
    ] {
        let config = MtplxProviderConfig {
            base_url: address.to_string(),
            ..local_config()
        };
        assert!(config.is_loopback(), "expected loopback: {address}");
    }
    for address in [
        "https://mtplx.example.com/v1",
        "http://192.168.1.4:8000/v1",
        "http://127.0.0.1.example.com/v1",
    ] {
        let config = MtplxProviderConfig {
            base_url: address.to_string(),
            ..local_config()
        };
        assert!(!config.is_loopback(), "expected remote: {address}");
    }
}

// ---------------------------------------------------------------------------
// Activation: resolve stays empty until login stores something
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_unactivated_local_instance_resolves_to_nothing() {
    // Not a detail: an empty resolution is what keeps the provider out of the
    // model list until someone activates it.
    assert_eq!(resolve(local_config(), &[], None).await, None);
}

#[tokio::test]
async fn a_stored_credential_activates_the_local_instance() {
    let credential = ApiKeyCredential {
        key: Some("mtplx-local".to_string()),
        env: None,
    };
    assert_eq!(
        resolve(local_config(), &[], Some(credential)).await,
        Some("mtplx-local".to_string())
    );
}

#[tokio::test]
async fn the_environment_activates_without_a_login() {
    assert_eq!(
        resolve(local_config(), &[("MTPLX_API_KEY", "from-env")], None).await,
        Some("from-env".to_string())
    );
}

#[tokio::test]
async fn local_login_stores_a_placeholder_without_prompting() {
    let (key, prompts) = login(local_config(), &[]).await;
    assert_eq!(key, Some("mtplx-local".to_string()));
    assert_eq!(prompts, 0, "loopback login must not ask for a secret");
}

#[tokio::test]
async fn remote_login_asks_for_a_real_key() {
    let (key, prompts) = login(remote_config(), &["sk-remote"]).await;
    assert_eq!(key, Some("sk-remote".to_string()));
    assert_eq!(prompts, 1);
}

#[tokio::test]
async fn an_unactivated_remote_instance_resolves_to_nothing() {
    assert_eq!(resolve(remote_config(), &[], None).await, None);
}

// ---------------------------------------------------------------------------
// The headers the server actually reads
// ---------------------------------------------------------------------------

fn context() -> Context {
    Context {
        messages: vec![Message::User(UserMessage {
            content: UserContent::Text("hi".to_string()),
            timestamp: 1,
        })],
        ..Default::default()
    }
}

/// One model, as the server would report it.
fn served_model() -> notagent_ai::types::Model {
    let body = json!({
        "object": "list",
        "data": [{
            "id": "mtplx-qwen35-4b-optimized-speed",
            "object": "model",
            "owned_by": "mtplx",
            "capability": "chat",
            "context_length": 262_144u64,
        }],
    });
    models_from_listing(&local_config(), &body)
        .expect("the listing parses")
        .into_iter()
        .next()
        .expect("the listing yields a model")
}

fn headers_for(session_id: Option<&str>) -> BTreeMap<String, Option<String>> {
    let model = served_model();
    // `get_compat`, not `detect_compat`: the latter is auto-detection only and
    // never reads `model.compat`, which is where our format lives. The stream
    // path uses `get_compat` too.
    let compat = get_compat(&model);
    build_client_headers(&model, &context(), None, session_id, &compat)
}

#[test]
fn a_request_carries_the_session_header_the_prefix_bank_keys_on() {
    let headers = headers_for(Some("session-42"));
    assert_eq!(
        headers.get("x-mtplx-session-id"),
        Some(&Some("session-42".to_string()))
    );
}

#[test]
fn the_openai_affinity_headers_stay_off_this_provider() {
    // The shared tail would add three headers the server never reads.
    let headers = headers_for(Some("session-42"));
    for unread in ["session_id", "x-client-request-id", "x-session-affinity"] {
        assert!(!headers.contains_key(unread), "unexpected header: {unread}");
    }
}

#[test]
fn without_a_session_there_is_no_session_header() {
    let headers = headers_for(None);
    assert!(!headers.contains_key("x-mtplx-session-id"));
}

#[test]
fn every_request_declares_which_client_it_is() {
    let headers = headers_for(Some("session-42"));
    assert_eq!(
        headers.get("x-mtplx-client"),
        Some(&Some("notagent".to_string()))
    );
}

#[test]
fn models_speak_chat_completions_not_responses() {
    let model = served_model();
    assert_eq!(model.api, "openai-completions");
    assert_eq!(model.base_url, "http://127.0.0.1:8000/v1");
}

// ---------------------------------------------------------------------------
// The model list comes from the server, not from a static guess
// ---------------------------------------------------------------------------

#[test]
fn without_a_reachable_server_the_provider_offers_nothing() {
    // A static list would claim a model the server may not be serving; the
    // failure would surface as a 400 on the first request instead of an empty
    // picker.
    let provider = mtplx_provider(local_config());
    assert!(provider.get_models().is_empty());
}

#[test]
fn the_served_model_keeps_the_id_the_server_reports() {
    // Model ids are model-specific, so a 4B server and a 27B server do not
    // answer to the same name.
    assert_eq!(served_model().id, "mtplx-qwen35-4b-optimized-speed");
}

#[test]
fn the_reported_window_wins_over_the_default() {
    assert_eq!(served_model().context_window, 262_144);
    // No output ceiling is reported; inventing a smaller one would truncate
    // generations the server was willing to produce.
    assert_eq!(served_model().max_tokens, 262_144);
}

#[test]
fn retrieval_models_are_never_offered_as_chat_targets() {
    let body = json!({
        "data": [
            {"id": "an-embedder", "capability": "embedding"},
            {"id": "a-reranker", "capability": "rerank"},
            {"id": "mtplx-chat", "capability": "chat"},
        ],
    });
    let models = models_from_listing(&local_config(), &body).expect("parses");
    let ids: Vec<&str> = models.iter().map(|model| model.id.as_str()).collect();
    assert_eq!(ids, vec!["mtplx-chat"]);
}

#[test]
fn an_entry_without_a_capability_is_taken_as_chat() {
    // Older servers omit the field; dropping those entries would leave the
    // picker empty against a perfectly good server.
    let body = json!({ "data": [{ "id": "mtplx" }] });
    let models = models_from_listing(&local_config(), &body).expect("parses");
    assert_eq!(models.len(), 1);
}

#[test]
fn a_body_without_a_data_array_is_a_fault_not_an_empty_list() {
    assert!(models_from_listing(&local_config(), &json!({ "oops": true })).is_err());
}

#[test]
fn wire_ids_become_readable_names() {
    assert_eq!(served_model().name, "Qwen35 4b Optimized Speed");
}
