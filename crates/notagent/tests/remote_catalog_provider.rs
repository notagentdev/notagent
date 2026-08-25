//! Port of `packages/coding-agent/test/remote-catalog-provider.test.ts` (241 LOC).

mod support;

use std::sync::Arc;

use notagent::config::VERSION;
use notagent::core::remote_catalog_provider::with_remote_catalog;
use notagent_ai::auth::types::{
    ApiKeyAuth, ApiKeyAuthInput, AuthError, AuthResult, BoxFuture, ProviderAuth,
};
use notagent_ai::models::{
    CreateModelsOptions, CreateProviderOptions, ModelsPublication, ModelsRefreshOptions, Provider,
    ProviderApis, RefreshModelsContext, create_models, create_provider,
};
use notagent_ai::models_store::{InMemoryModelsStore, ModelsStore};
use notagent_ai::types::{
    Context, Modality, Model, ModelCost, ProviderStreams, SimpleStreamOptions, StreamOptions,
};
use notagent_ai::utils::event_stream::AssistantMessageEventStream;
use support::{CannedResponse, TestServer};
use tokio_util::sync::CancellationToken;

struct UnusedStreams;

impl ProviderStreams for UnusedStreams {
    fn stream(
        &self,
        _model: &Model,
        _context: &Context,
        _options: Option<StreamOptions>,
    ) -> AssistantMessageEventStream {
        panic!("not used")
    }

    fn stream_simple(
        &self,
        _model: &Model,
        _context: &Context,
        _options: Option<SimpleStreamOptions>,
    ) -> AssistantMessageEventStream {
        panic!("not used")
    }
}

struct TestApiKeyAuth;

impl ApiKeyAuth for TestApiKeyAuth {
    fn name(&self) -> &str {
        "Test"
    }

    fn resolve<'a>(
        &'a self,
        _input: ApiKeyAuthInput<'a>,
    ) -> BoxFuture<'a, Result<Option<AuthResult>, AuthError>> {
        Box::pin(async { Ok(Some(AuthResult::default())) })
    }
}

fn model(id: &str) -> Model {
    Model {
        id: id.to_owned(),
        name: id.to_owned(),
        api: "openai-completions".to_owned(),
        provider: "test-provider".to_owned(),
        base_url: "https://example.test/v1".to_owned(),
        reasoning: false,
        thinking_level_map: None,
        input: vec![Modality::Text],
        cost: ModelCost::default(),
        context_window: 1000,
        max_tokens: 100,
        sampling_params: None,
        headers: None,
        compat: None,
    }
}

fn model_json(id: &str) -> String {
    serde_json::to_string(&model(id)).expect("model json")
}

fn test_provider(base_url: &str, local_generated_at: Option<i64>) -> Arc<dyn Provider> {
    with_remote_catalog(
        create_provider(CreateProviderOptions {
            id: "test-provider".to_owned(),
            name: None,
            base_url: None,
            headers: None,
            auth: ProviderAuth {
                api_key: Some(Arc::new(TestApiKeyAuth)),
                oauth: None,
            },
            models: vec![model("static")],
            fetch_models: None,
            filter_models: None,
            api: ProviderApis::Single(Arc::new(UnusedStreams)),
        }),
        Some(base_url),
        local_generated_at,
    )
}

struct RefreshOverrides {
    allow_network: bool,
    force: Option<bool>,
    signal: CancellationToken,
}

impl Default for RefreshOverrides {
    fn default() -> Self {
        RefreshOverrides {
            allow_network: true,
            force: None,
            signal: CancellationToken::new(),
        }
    }
}

async fn refresh_provider(
    provider: &Arc<dyn Provider>,
    store: &InMemoryModelsStore,
    overrides: RefreshOverrides,
) -> Result<(), String> {
    let stored = store.read(provider.id(), None).await.expect("read");
    let publish = make_publish(store.clone(), provider.id().to_owned());
    let Some(refresh) = provider.refresh_models(RefreshModelsContext {
        credential: Some(notagent_ai::auth::types::Credential::ApiKey(
            notagent_ai::auth::types::ApiKeyCredential::default(),
        )),
        stored,
        publish,
        allow_network: overrides.allow_network,
        force: overrides.force,
        signal: overrides.signal,
    }) else {
        return Ok(());
    };
    refresh.await
}

/// `publish` of `RefreshModelsContext`, writing straight through to the store.
#[allow(clippy::type_complexity)]
fn make_publish<'a>(
    store: InMemoryModelsStore,
    provider_id: String,
) -> Box<dyn Fn(ModelsPublication<'a>) -> BoxFuture<'a, bool> + Send + Sync + 'a> {
    Box::new(move |publication: ModelsPublication<'a>| {
        let store = store.clone();
        let provider_id = provider_id.clone();
        Box::pin(async move {
            match publication.persist {
                Some(None) => {
                    store.delete(&provider_id, None).await.expect("delete");
                }
                Some(Some(entry)) => {
                    store.write(&provider_id, entry, None).await.expect("write");
                }
                None => {}
            }
            if let Some(update) = publication.update {
                update();
            }
            true
        }) as BoxFuture<'a, bool>
    })
}

fn ids(models: &[Model]) -> Vec<String> {
    models.iter().map(|model| model.id.clone()).collect()
}

#[tokio::test]
async fn parses_keyed_catalogs_sends_version_headers_and_observes_the_refresh_ttl() {
    let server = TestServer::start(
        Vec::new(),
        CannedResponse::json(format!("{{\"dynamic\": {}}}", model_json("dynamic"))),
    )
    .await;
    let provider = test_provider(&server.base_url, None);
    let store = InMemoryModelsStore::new();

    refresh_provider(&provider, &store, RefreshOverrides::default())
        .await
        .expect("first refresh");
    refresh_provider(&provider, &store, RefreshOverrides::default())
        .await
        .expect("second refresh");
    refresh_provider(
        &provider,
        &store,
        RefreshOverrides {
            force: Some(true),
            ..RefreshOverrides::default()
        },
    )
    .await
    .expect("forced refresh");

    assert_eq!(ids(&provider.get_models()), vec!["static", "dynamic"]);
    assert_eq!(
        ids(&store
            .read("test-provider", None)
            .await
            .expect("read")
            .expect("entry")
            .models),
        vec!["dynamic"]
    );
    assert_eq!(server.request_count(), 2);
    let user_agent = server.requests()[0]
        .header("user-agent")
        .expect("user agent")
        .to_owned();
    assert!(user_agent.contains(&format!("notagent/{VERSION}")));
}

#[tokio::test]
async fn prefers_the_newer_of_the_generated_and_remote_catalogs() {
    let local_generated_at = 1_784_800_800_000_i64; // 2026-07-23T10:00:00.000Z
    let older = httpdate(local_generated_at - 60_000);
    let newer = httpdate(local_generated_at + 60_000);
    let server = TestServer::start(
        vec![
            CannedResponse::json(format!("{{\"old\": {}}}", model_json("old")))
                .with_header("last-modified", &older),
            CannedResponse::json(format!("{{\"newer\": {}}}", model_json("newer")))
                .with_header("last-modified", &newer),
        ],
        CannedResponse::status(500, "unexpected"),
    )
    .await;
    let provider = test_provider(&server.base_url, Some(local_generated_at));
    let store = InMemoryModelsStore::new();

    refresh_provider(&provider, &store, RefreshOverrides::default())
        .await
        .expect("first refresh");
    assert_eq!(ids(&provider.get_models()), vec!["static"]);

    refresh_provider(
        &provider,
        &store,
        RefreshOverrides {
            force: Some(true),
            ..RefreshOverrides::default()
        },
    )
    .await
    .expect("forced refresh");
    assert_eq!(ids(&provider.get_models()), vec!["static", "newer"]);
    assert_eq!(
        store
            .read("test-provider", None)
            .await
            .expect("read")
            .expect("entry")
            .last_modified,
        Some(local_generated_at + 60_000)
    );
}

#[tokio::test]
async fn revalidates_a_stored_catalog_with_its_etag_and_keeps_the_overlay_on_304() {
    let server = TestServer::start(
        vec![
            CannedResponse::json(format!("{{\"dynamic\": {}}}", model_json("dynamic")))
                .with_header("etag", "\"catalog-1\""),
            CannedResponse::status(304, "").with_header("etag", "\"catalog-1\""),
        ],
        CannedResponse::status(500, "unexpected"),
    )
    .await;
    let provider = test_provider(&server.base_url, None);
    let store = InMemoryModelsStore::new();

    refresh_provider(&provider, &store, RefreshOverrides::default())
        .await
        .expect("first refresh");
    assert!(server.requests()[0].header("if-none-match").is_none());
    let stored = store
        .read("test-provider", None)
        .await
        .expect("read")
        .expect("entry");
    assert_eq!(stored.etag.as_deref(), Some("\"catalog-1\""));
    let checked_at = stored.checked_at.unwrap_or_default();

    refresh_provider(
        &provider,
        &store,
        RefreshOverrides {
            force: Some(true),
            ..RefreshOverrides::default()
        },
    )
    .await
    .expect("forced refresh");

    assert_eq!(
        server.requests()[1].header("if-none-match"),
        Some("\"catalog-1\"")
    );
    assert_eq!(ids(&provider.get_models()), vec!["static", "dynamic"]);
    let stored = store
        .read("test-provider", None)
        .await
        .expect("read")
        .expect("entry");
    assert_eq!(ids(&stored.models), vec!["dynamic"]);
    assert_eq!(stored.etag.as_deref(), Some("\"catalog-1\""));
    assert!(stored.checked_at.unwrap_or_default() >= checked_at);
}

#[tokio::test]
async fn drops_a_stale_etag_when_the_overlay_becomes_unavailable() {
    let server = TestServer::start(
        vec![
            CannedResponse::json(format!("{{\"dynamic\": {}}}", model_json("dynamic")))
                .with_header("etag", "\"catalog-1\""),
            CannedResponse::status(501, "not implemented"),
        ],
        CannedResponse::status(500, "unexpected"),
    )
    .await;
    let provider = test_provider(&server.base_url, None);
    let store = InMemoryModelsStore::new();

    refresh_provider(&provider, &store, RefreshOverrides::default())
        .await
        .expect("first refresh");
    refresh_provider(
        &provider,
        &store,
        RefreshOverrides {
            force: Some(true),
            ..RefreshOverrides::default()
        },
    )
    .await
    .expect("forced refresh");

    assert_eq!(
        store
            .read("test-provider", None)
            .await
            .expect("read")
            .expect("entry")
            .etag,
        None
    );
}

#[tokio::test]
async fn keeps_the_etag_and_overlay_after_a_transient_failure() {
    let server = TestServer::start(
        vec![
            CannedResponse::json(format!("{{\"dynamic\": {}}}", model_json("dynamic")))
                .with_header("etag", "\"catalog-1\""),
            CannedResponse::status(429, "rate limited"),
            CannedResponse::status(429, "rate limited"),
            CannedResponse::status(429, "rate limited"),
            CannedResponse::status(304, "").with_header("etag", "\"catalog-1\""),
        ],
        CannedResponse::status(500, "unexpected"),
    )
    .await;
    let provider = test_provider(&server.base_url, None);
    let store = InMemoryModelsStore::new();

    refresh_provider(&provider, &store, RefreshOverrides::default())
        .await
        .expect("first refresh");
    let error = refresh_provider(
        &provider,
        &store,
        RefreshOverrides {
            force: Some(true),
            ..RefreshOverrides::default()
        },
    )
    .await
    .expect_err("429");
    assert!(error.contains("429"));

    let stored = store
        .read("test-provider", None)
        .await
        .expect("read")
        .expect("entry");
    assert_eq!(stored.etag.as_deref(), Some("\"catalog-1\""));
    assert_eq!(ids(&stored.models), vec!["dynamic"]);

    refresh_provider(
        &provider,
        &store,
        RefreshOverrides {
            force: Some(true),
            ..RefreshOverrides::default()
        },
    )
    .await
    .expect("revalidation");
    assert_eq!(
        server.requests()[4].header("if-none-match"),
        Some("\"catalog-1\"")
    );
    assert_eq!(ids(&provider.get_models()), vec!["static", "dynamic"]);
}

#[tokio::test]
async fn a_newer_catalog_request_bypasses_a_stalled_older_request() {
    let server = TestServer::start(
        vec![
            CannedResponse::json(format!("{{\"older\": {}}}", model_json("older"))).held(),
            CannedResponse::json(format!("{{\"newer\": {}}}", model_json("newer"))),
        ],
        CannedResponse::status(500, "unexpected"),
    )
    .await;
    let provider = test_provider(&server.base_url, None);
    let store = InMemoryModelsStore::new();
    let models = create_models(Some(CreateModelsOptions {
        credentials: None,
        models_store: Some(Arc::new(store.clone())),
        auth_context: None,
    }));
    models.set_provider(Arc::clone(&provider));

    let first_models = Arc::clone(&models);
    let first = tokio::spawn(async move {
        first_models
            .refresh(Some(ModelsRefreshOptions {
                providers: Some(vec!["test-provider".to_owned()]),
                force: Some(true),
                ..ModelsRefreshOptions::default()
            }))
            .await
    });
    while server.request_count() == 0 {
        tokio::task::yield_now().await;
    }
    models
        .refresh(Some(ModelsRefreshOptions {
            providers: Some(vec!["test-provider".to_owned()]),
            force: Some(true),
            ..ModelsRefreshOptions::default()
        }))
        .await;
    assert_eq!(ids(&provider.get_models()), vec!["static", "newer"]);

    server.release();
    let _ = first.await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    assert_eq!(ids(&provider.get_models()), vec!["static", "newer"]);
    assert_eq!(
        ids(&store
            .read("test-provider", None)
            .await
            .expect("read")
            .expect("entry")
            .models),
        vec!["newer"]
    );
}

#[tokio::test]
async fn treats_unimplemented_catalog_routes_as_an_unavailable_overlay() {
    let server =
        TestServer::start(Vec::new(), CannedResponse::status(501, "not implemented")).await;
    let provider = test_provider(&server.base_url, None);
    let store = InMemoryModelsStore::new();

    refresh_provider(&provider, &store, RefreshOverrides::default())
        .await
        .expect("refresh");
    assert_eq!(ids(&provider.get_models()), vec!["static"]);
    let stored = store
        .read("test-provider", None)
        .await
        .expect("read")
        .expect("entry");
    assert!(stored.models.is_empty());
    assert!(stored.checked_at.is_some());
}

#[tokio::test]
async fn treats_an_html_answer_as_an_unavailable_overlay() {
    // A parked domain or captive portal answers every path with 200 and an
    // HTML page; that is the host saying "no catalog here", not an error the
    // picker should show on every open.
    let server = TestServer::start(
        Vec::new(),
        CannedResponse {
            status: 200,
            headers: vec![("content-type".to_owned(), "text/html".to_owned())],
            body: "<!DOCTYPE html><html><body>Parked</body></html>".to_owned(),
            hold: false,
        },
    )
    .await;
    let provider = test_provider(&server.base_url, None);
    let store = InMemoryModelsStore::new();

    refresh_provider(&provider, &store, RefreshOverrides::default())
        .await
        .expect("refresh reports no error");
    assert_eq!(ids(&provider.get_models()), vec!["static"]);
    let stored = store
        .read("test-provider", None)
        .await
        .expect("read")
        .expect("entry");
    assert!(stored.models.is_empty());
    assert!(stored.checked_at.is_some());
}

/// `new Date(ms).toUTCString()`
fn httpdate(millis: i64) -> String {
    chrono::DateTime::from_timestamp_millis(millis)
        .expect("timestamp")
        .format("%a, %d %b %Y %H:%M:%S GMT")
        .to_string()
}
