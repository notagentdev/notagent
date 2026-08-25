//! The OpenAI-compatible providers: the two local runtimes and the instance
//! whose address the user names.

use std::sync::{Arc, Mutex};

use notagent_ai::auth::types::{
    ApiKeyAuthInput, ApiKeyCredential, AuthContext, AuthError, AuthEvent, AuthPrompt,
    AuthPromptKind, BoxFuture, Credential, ProviderAuthInteraction,
};
use notagent_ai::models::Provider;
use notagent_ai::providers::openai_compatible::{
    CUSTOM_PROVIDER_ID, LMSTUDIO_BASE_URL, LMSTUDIO_PROVIDER_ID, OLLAMA_BASE_URL,
    OLLAMA_PROVIDER_ID, OpenAICompatibleConfig, custom_openai_provider, lmstudio_provider,
    ollama_provider, openai_compatible_provider,
};
use notagent_ai::types::ProviderEnv;

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
// Helpers
// ---------------------------------------------------------------------------

/// Runs a provider's login and reports the credential plus what it asked.
async fn login(
    provider: &Arc<notagent_ai::models::BuiltProvider>,
    answers: &[&str],
) -> (ApiKeyCredential, Vec<AuthPromptKind>) {
    let auth = provider.auth();
    let api_key = auth.api_key.as_ref().expect("api-key auth");
    let interaction = ScriptedInteraction::new(answers);
    let provider_interaction = ProviderAuthInteraction {
        interaction: interaction.as_ref(),
        signal: tokio_util::sync::CancellationToken::new(),
    };
    let credential = api_key
        .login(&provider_interaction)
        .expect("a login flow")
        .await
        .expect("login succeeds");
    let prompts = interaction.prompts.lock().expect("prompts").clone();
    (credential, prompts)
}

async fn resolve(
    provider: &Arc<notagent_ai::models::BuiltProvider>,
    env: &[(&str, &str)],
    credential: Option<ApiKeyCredential>,
) -> Option<String> {
    let auth = provider.auth();
    let api_key = auth.api_key.as_ref().expect("api-key auth");
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

/// One-shot loopback server answering a single GET with `body`, plus the path
/// it was asked for.
async fn serve_once(body: &'static str) -> (String, tokio::task::JoinHandle<Option<String>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap_or_else(|error| panic!("binding a loopback port failed: {error}"));
    let address = listener
        .local_addr()
        .unwrap_or_else(|error| panic!("reading the bound port failed: {error}"));
    let handle = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.ok()?;
        let mut buffer = vec![0_u8; 2048];
        let read = tokio::io::AsyncReadExt::read(&mut socket, &mut buffer)
            .await
            .ok()?;
        let request = String::from_utf8_lossy(&buffer[..read]).into_owned();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = tokio::io::AsyncWriteExt::write_all(&mut socket, response.as_bytes()).await;
        let _ = tokio::io::AsyncWriteExt::flush(&mut socket).await;
        request.lines().next().map(str::to_owned)
    });
    (format!("http://{address}/v1"), handle)
}

/// Drives `refresh_models` and returns what the provider ended up offering.
async fn refresh(
    provider: &Arc<notagent_ai::models::BuiltProvider>,
    credential: Option<Credential>,
) -> Vec<String> {
    let context = notagent_ai::models::RefreshModelsContext {
        credential,
        stored: None,
        publish: Box::new(|publication: notagent_ai::models::ModelsPublication<'_>| {
            if let Some(update) = publication.update {
                update();
            }
            Box::pin(async move { true }) as BoxFuture<'_, bool>
        }),
        allow_network: true,
        force: Some(true),
        signal: tokio_util::sync::CancellationToken::new(),
    };
    let refresh = Provider::refresh_models(provider.as_ref(), context)
        .unwrap_or_else(|| panic!("the provider offers no refresh"));
    refresh.await.expect("the refresh succeeds");
    provider
        .get_models()
        .into_iter()
        .map(|model| model.id)
        .collect()
}

fn instance_at(base_url: &str) -> Arc<notagent_ai::models::BuiltProvider> {
    openai_compatible_provider(OpenAICompatibleConfig {
        id: "test-server".to_string(),
        name: "Test Server".to_string(),
        base_url: Some(base_url.to_string()),
        key_env_var: "TEST_API_KEY".to_string(),
        base_url_env_var: None,
    })
}

// ---------------------------------------------------------------------------
// The local runtimes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_local_runtimes_answer_on_their_documented_ports() {
    assert_eq!(ollama_provider().id(), OLLAMA_PROVIDER_ID);
    assert_eq!(ollama_provider().base_url(), Some(OLLAMA_BASE_URL));
    assert_eq!(lmstudio_provider().id(), LMSTUDIO_PROVIDER_ID);
    assert_eq!(lmstudio_provider().base_url(), Some(LMSTUDIO_BASE_URL));
}

#[tokio::test]
async fn activating_a_local_runtime_asks_for_nothing() {
    for provider in [ollama_provider(), lmstudio_provider()] {
        let (credential, prompts) = login(&provider, &[]).await;
        assert!(
            prompts.is_empty(),
            "a loopback server needs no secret: {prompts:?}"
        );
        assert!(credential.key.is_some(), "a placeholder still stands in");
        assert!(credential.env.is_none(), "a fixed address is not stored");
    }
}

#[tokio::test]
async fn an_unactivated_runtime_offers_nothing() {
    // The whole point of the gate: an unused local runtime stays out of the
    // model picker instead of showing an empty provider.
    assert_eq!(resolve(&ollama_provider(), &[], None).await, None);
    assert_eq!(
        resolve(
            &ollama_provider(),
            &[],
            Some(ApiKeyCredential {
                key: Some("stored".to_string()),
                env: None
            })
        )
        .await
        .as_deref(),
        Some("stored")
    );
    assert_eq!(
        resolve(&ollama_provider(), &[("OLLAMA_API_KEY", "ambient")], None)
            .await
            .as_deref(),
        Some("ambient"),
        "an ambient key activates a fixed address"
    );
}

#[tokio::test]
async fn a_server_that_is_not_running_offers_no_models() {
    // Port 1 on loopback refuses immediately, which is what a stopped runtime
    // looks like. It must not surface as an error.
    let provider = instance_at("http://127.0.0.1:1/v1");
    assert!(refresh(&provider, None).await.is_empty());
}

#[tokio::test]
async fn the_served_models_come_from_the_server() {
    let (base_url, server) = serve_once(
        r#"{"object":"list","data":[
            {"id":"qwen3-coder:30b"},
            {"id":"nomic-embed-text","type":"embedding"},
            {"id":"gpt-oss-20b","max_context_length":131072}
        ]}"#,
    )
    .await;
    let provider = instance_at(&base_url);

    let ids = refresh(&provider, None).await;
    assert_eq!(ids, ["qwen3-coder:30b", "gpt-oss-20b"]);

    let request = server.await.expect("the server task").expect("a request");
    assert!(request.contains("/v1/models"), "{request}");

    let models = provider.get_models();
    assert_eq!(models[0].base_url, base_url);
    assert_eq!(models[0].provider, "test-server");
    assert_eq!(models[0].api, "openai-completions");
    // A local runtime bills nothing, and the window is the conservative floor
    // until the server reports its own.
    assert_eq!(models[0].cost.input, 0.0);
    assert_eq!(models[0].context_window, 32_768);
    assert_eq!(models[1].context_window, 131_072);
}

// ---------------------------------------------------------------------------
// The custom instance
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_custom_instance_asks_for_its_address_and_keeps_it() {
    let provider = custom_openai_provider();
    assert_eq!(provider.id(), CUSTOM_PROVIDER_ID);
    assert_eq!(
        provider.base_url(),
        None,
        "there is no address until someone names one"
    );

    let (credential, prompts) =
        login(&provider, &["https://models.example.com/v1", "secret"]).await;
    assert!(
        matches!(prompts.first(), Some(AuthPromptKind::Text { .. })),
        "the address is asked for first: {prompts:?}"
    );
    assert!(
        matches!(prompts.get(1), Some(AuthPromptKind::Secret { .. })),
        "a remote address is asked for a key: {prompts:?}"
    );
    assert_eq!(credential.key.as_deref(), Some("secret"));
    assert_eq!(
        credential
            .env
            .as_ref()
            .and_then(|env| env.get("baseUrl"))
            .map(String::as_str),
        Some("https://models.example.com/v1"),
        "the address travels with the credential, so /logout takes it away too"
    );
}

#[tokio::test]
async fn a_custom_instance_on_loopback_is_not_asked_for_a_key() {
    let (credential, prompts) = login(&custom_openai_provider(), &["localhost:8080"]).await;
    assert_eq!(prompts.len(), 1, "only the address: {prompts:?}");
    assert!(credential.key.is_some(), "a placeholder still stands in");
    assert_eq!(
        credential
            .env
            .as_ref()
            .and_then(|env| env.get("baseUrl"))
            .map(String::as_str),
        Some("http://localhost:8080/v1"),
        "a bare authority is completed rather than rejected"
    );
}

#[tokio::test]
async fn a_custom_instance_without_an_address_offers_nothing() {
    let provider = custom_openai_provider();
    // A key alone cannot activate it: there is nothing for the key to
    // authorize.
    assert_eq!(
        resolve(&provider, &[("CUSTOM_API_KEY", "ambient")], None).await,
        None
    );
    assert_eq!(
        resolve(
            &provider,
            &[
                ("CUSTOM_API_KEY", "ambient"),
                ("CUSTOM_BASE_URL", "https://models.example.com/v1"),
            ],
            None
        )
        .await
        .as_deref(),
        Some("ambient"),
        "an address in the environment activates it without a login"
    );
    // Nothing to ask, so no error either.
    assert!(refresh(&provider, None).await.is_empty());
}

#[tokio::test]
async fn a_custom_instance_asks_the_server_its_credential_names() {
    let (base_url, server) = serve_once(r#"{"data":[{"id":"local-model"}]}"#).await;
    let provider = custom_openai_provider();
    let mut env = ProviderEnv::new();
    env.insert("baseUrl".to_string(), base_url.clone());

    let ids = refresh(
        &provider,
        Some(Credential::ApiKey(ApiKeyCredential {
            key: Some("secret".to_string()),
            env: Some(env),
        })),
    )
    .await;

    assert_eq!(ids, ["local-model"]);
    let request = server.await.expect("the server task").expect("a request");
    assert!(request.contains("/v1/models"), "{request}");
    assert_eq!(provider.get_models()[0].base_url, base_url);
}
