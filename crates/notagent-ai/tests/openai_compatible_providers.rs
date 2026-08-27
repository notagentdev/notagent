//! The OpenAI-compatible providers: the two local runtimes and the instance
//! whose address the user names.
//!
//! The listing bodies here are the shapes the real servers return — LM Studio's
//! `/api/v0/models`, Ollama's `/api/tags` and `/api/show` — so a change to how
//! they are read is caught without either runtime installed.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use notagent_ai::auth::types::{
    ApiKeyAuthInput, ApiKeyCredential, AuthContext, AuthError, AuthEvent, AuthPrompt,
    AuthPromptKind, BoxFuture, Credential, ProviderAuthInteraction,
};
use notagent_ai::models::Provider;
use notagent_ai::providers::openai_compatible::{
    CUSTOM_PROVIDER_ID, CatalogSource, LMSTUDIO_BASE_URL, LMSTUDIO_PROVIDER_ID, OLLAMA_BASE_URL,
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
// A loopback server that answers by path
// ---------------------------------------------------------------------------

/// Answers each request from a path table until it is dropped, recording what
/// was asked for. A path missing from the table answers 404, which is how an
/// older build answers a route it does not have.
struct RouteServer {
    base_url: String,
    asked: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}

impl RouteServer {
    async fn start(routes: BTreeMap<&'static str, String>) -> RouteServer {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap_or_else(|error| panic!("binding a loopback port failed: {error}"));
        let address = listener
            .local_addr()
            .unwrap_or_else(|error| panic!("reading the bound port failed: {error}"));
        let asked: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&asked);
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let routes = routes.clone();
                let recorder = Arc::clone(&recorder);
                tokio::spawn(async move {
                    let mut buffer = vec![0_u8; 4096];
                    let Ok(read) = tokio::io::AsyncReadExt::read(&mut socket, &mut buffer).await
                    else {
                        return;
                    };
                    let request = String::from_utf8_lossy(&buffer[..read]).into_owned();
                    let line = request.lines().next().unwrap_or_default().to_owned();
                    let path = line
                        .split_whitespace()
                        .nth(1)
                        .unwrap_or_default()
                        .to_owned();
                    recorder.lock().expect("asked").push(line);
                    let response = match routes.get(path.as_str()) {
                        Some(body) => format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        ),
                        None => "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string(),
                    };
                    let _ =
                        tokio::io::AsyncWriteExt::write_all(&mut socket, response.as_bytes()).await;
                    let _ = tokio::io::AsyncWriteExt::flush(&mut socket).await;
                });
            }
        });
        RouteServer {
            base_url: format!("http://{address}/v1"),
            asked,
            task,
        }
    }

    fn asked(&self) -> Vec<String> {
        self.asked.lock().expect("asked").clone()
    }
}

impl Drop for RouteServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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

/// Drives `refresh_models` and reports what it made of the attempt.
async fn try_refresh(
    provider: &Arc<notagent_ai::models::BuiltProvider>,
    credential: Option<Credential>,
) -> Result<(), String> {
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
    refresh.await
}

/// Drives `refresh_models` and returns what the provider ended up offering.
async fn refresh(
    provider: &Arc<notagent_ai::models::BuiltProvider>,
    credential: Option<Credential>,
) -> Vec<String> {
    try_refresh(provider, credential)
        .await
        .expect("the refresh succeeds");
    provider
        .get_models()
        .into_iter()
        .map(|model| model.id)
        .collect()
}

fn instance_at(base_url: &str, catalog: CatalogSource) -> Arc<notagent_ai::models::BuiltProvider> {
    openai_compatible_provider(OpenAICompatibleConfig {
        id: "test-server".to_string(),
        name: "Test Server".to_string(),
        base_url: Some(base_url.to_string()),
        key_env_var: "TEST_API_KEY".to_string(),
        base_url_env_var: None,
        catalog,
    })
}

fn routes(entries: &[(&'static str, &str)]) -> BTreeMap<&'static str, String> {
    entries
        .iter()
        .map(|(path, body)| (*path, (*body).to_string()))
        .collect()
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_local_runtimes_answer_on_their_documented_ports() {
    assert_eq!(ollama_provider().id(), OLLAMA_PROVIDER_ID);
    assert_eq!(ollama_provider().base_url(), Some(OLLAMA_BASE_URL));
    assert_eq!(lmstudio_provider().id(), LMSTUDIO_PROVIDER_ID);
    assert_eq!(lmstudio_provider().base_url(), Some(LMSTUDIO_BASE_URL));
}

#[tokio::test]
async fn activating_a_local_runtime_takes_an_empty_key() {
    // Both runtimes can be switched to requiring a token, so the question is
    // asked; an empty answer is the ordinary case and stands a placeholder in.
    for provider in [ollama_provider(), lmstudio_provider()] {
        let (credential, prompts) = login(&provider, &[""]).await;
        assert_eq!(prompts.len(), 1, "just the key: {prompts:?}");
        assert!(
            matches!(prompts.first(), Some(AuthPromptKind::Secret { message, .. })
                if message.contains("leave empty")),
            "the question says the key is optional here: {prompts:?}"
        );
        assert!(credential.key.is_some(), "a placeholder still stands in");
        assert!(credential.env.is_none(), "a fixed address is not stored");
    }
}

#[tokio::test]
async fn a_local_runtime_switched_to_requiring_a_token_takes_one() {
    // LM Studio has an authentication toggle in its server settings; a token
    // typed here is what the listing and every later request carry.
    let (credential, _) = login(&lmstudio_provider(), &["lms-secret"]).await;
    assert_eq!(credential.key.as_deref(), Some("lms-secret"));
}

#[tokio::test]
async fn an_unactivated_runtime_offers_nothing() {
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
async fn a_server_that_is_not_running_says_so() {
    // Port 1 on loopback refuses immediately, which is what a stopped runtime
    // looks like. The fetch runs only for a provider somebody activated, so
    // staying silent would leave the picker empty while the refresh claims
    // success — which is exactly the report that helps nobody.
    for catalog in [
        CatalogSource::OpenAI,
        CatalogSource::LmStudio,
        CatalogSource::Ollama,
    ] {
        let provider = instance_at("http://127.0.0.1:1/v1", catalog);
        let error = try_refresh(&provider, None)
            .await
            .expect_err("an unreachable address is reported");
        assert!(
            error.contains("not reachable") && error.contains("127.0.0.1:1"),
            "{error}"
        );
    }
}

// ---------------------------------------------------------------------------
// LM Studio
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lmstudio_reads_its_own_listing_beside_the_openai_one() {
    let server = RouteServer::start(routes(&[(
        "/api/v0/models",
        r#"{"object":"list","data":[
            {"id":"qwen3-coder-30b","object":"model","type":"llm","state":"not-loaded","max_context_length":262144},
            {"id":"text-embedding-nomic-embed-text-v1.5","object":"model","type":"embeddings","state":"not-loaded","max_context_length":2048},
            {"id":"qwen2-vl-7b","object":"model","type":"vlm","state":"loaded","max_context_length":131072,"loaded_context_length":8192}
        ]}"#,
    )]))
    .await;
    let provider = instance_at(&server.base_url, CatalogSource::LmStudio);

    let ids = refresh(&provider, None).await;
    assert_eq!(ids, ["qwen3-coder-30b", "qwen2-vl-7b"]);
    assert!(
        server
            .asked()
            .iter()
            .any(|line| line.contains("/api/v0/models")),
        "the native listing is what carries the context window: {:?}",
        server.asked()
    );

    let models = provider.get_models();
    assert_eq!(models[0].context_window, 262_144);
    // A loaded model reports what it was actually given, which is the number
    // that decides when compaction has to run.
    assert_eq!(models[1].context_window, 8_192);
    assert_eq!(models[0].base_url, server.base_url);
}

#[tokio::test]
async fn lmstudio_falls_back_to_the_openai_listing_on_an_older_build() {
    // No `/api/v0` route: the build predates it, so only the names are left.
    let server = RouteServer::start(routes(&[(
        "/v1/models",
        r#"{"object":"list","data":[{"id":"qwen3-coder-30b","object":"model"}]}"#,
    )]))
    .await;
    let provider = instance_at(&server.base_url, CatalogSource::LmStudio);

    let ids = refresh(&provider, None).await;
    assert_eq!(ids, ["qwen3-coder-30b"]);
    let asked = server.asked();
    assert!(
        asked.iter().any(|line| line.contains("/api/v0/models")),
        "the native listing is tried first: {asked:?}"
    );
    assert!(
        asked.iter().any(|line| line.contains("/v1/models")),
        "and the OpenAI one catches the fall: {asked:?}"
    );
    // Nothing said otherwise, so the conservative floor applies.
    assert_eq!(provider.get_models()[0].context_window, 32_768);
}

// ---------------------------------------------------------------------------
// Ollama
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ollama_asks_tags_for_the_names_and_show_for_the_rest() {
    let server = RouteServer::start(routes(&[
        (
            "/api/tags",
            r#"{"models":[
                {"name":"qwen3-coder:30b","model":"qwen3-coder:30b","size":18000000000,"details":{"family":"qwen3"}},
                {"name":"nomic-embed-text:latest","model":"nomic-embed-text:latest","details":{"family":"nomic-bert"}}
            ]}"#,
        ),
        (
            "/api/show",
            r#"{"capabilities":["completion","tools"],
                "model_info":{"general.architecture":"qwen3","qwen3.context_length":262144}}"#,
        ),
    ]))
    .await;
    let provider = instance_at(&server.base_url, CatalogSource::Ollama);

    let ids = refresh(&provider, None).await;
    // Both are asked about; this server answers the same details for each, so
    // both are chat targets here. What matters is that the names come from
    // `/api/tags` and the window from `/api/show`.
    assert_eq!(ids, ["qwen3-coder:30b", "nomic-embed-text:latest"]);
    let asked = server.asked();
    assert!(
        asked.iter().any(|line| line.contains("/api/tags")),
        "{asked:?}"
    );
    assert!(
        asked.iter().any(|line| line.contains("/api/show")),
        "{asked:?}"
    );
    assert_eq!(provider.get_models()[0].context_window, 262_144);
}

#[tokio::test]
async fn ollama_keeps_a_model_whose_details_cannot_be_read() {
    // `/api/tags` answers, `/api/show` does not: a name that chats is still
    // usable, only its window is then the conservative floor.
    let server = RouteServer::start(routes(&[(
        "/api/tags",
        r#"{"models":[{"name":"qwen3-coder:30b","model":"qwen3-coder:30b"}]}"#,
    )]))
    .await;
    let provider = instance_at(&server.base_url, CatalogSource::Ollama);

    assert_eq!(refresh(&provider, None).await, ["qwen3-coder:30b"]);
    assert_eq!(provider.get_models()[0].context_window, 32_768);
}

#[tokio::test]
async fn ollama_falls_back_to_the_openai_listing_without_tags() {
    let server = RouteServer::start(routes(&[(
        "/v1/models",
        r#"{"object":"list","data":[{"id":"qwen3-coder:30b","object":"model"}]}"#,
    )]))
    .await;
    let provider = instance_at(&server.base_url, CatalogSource::Ollama);

    assert_eq!(refresh(&provider, None).await, ["qwen3-coder:30b"]);
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
async fn a_custom_instance_on_loopback_may_skip_the_key() {
    let (credential, prompts) = login(&custom_openai_provider(), &["localhost:8080", ""]).await;
    assert_eq!(prompts.len(), 2, "the address and the key: {prompts:?}");
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
    assert!(refresh(&provider, None).await.is_empty());
}

#[tokio::test]
async fn a_custom_instance_asks_the_server_its_credential_names() {
    let server = RouteServer::start(routes(&[(
        "/v1/models",
        r#"{"data":[{"id":"local-model"}]}"#,
    )]))
    .await;
    let provider = custom_openai_provider();
    let mut env = ProviderEnv::new();
    env.insert("baseUrl".to_string(), server.base_url.clone());

    let ids = refresh(
        &provider,
        Some(Credential::ApiKey(ApiKeyCredential {
            key: Some("secret".to_string()),
            env: Some(env),
        })),
    )
    .await;

    assert_eq!(ids, ["local-model"]);
    assert!(
        server
            .asked()
            .iter()
            .any(|line| line.contains("/v1/models")),
        "{:?}",
        server.asked()
    );
    assert_eq!(provider.get_models()[0].base_url, server.base_url);
}
