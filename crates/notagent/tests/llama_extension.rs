//! Port of `packages/coding-agent/test/llama-extension.test.ts` (295 LOC), minus
//! the first case ("registers a native provider and /llama command"), which
//! exercises `extensions/llama/index.ts` and stays with the app workstream.
//!
//! `createServer` from `node:http` has no Rust counterpart, so the suites run
//! against a small loopback server (deviation class 3, as in `tests/support`).
//! Its only behavioural addition: the delayed SSE events of the load and
//! download cases wait until the watcher has subscribed, which removes the race
//! the 20 ms timer papers over in Node.

#[path = "support/llama_server.rs"]
mod llama_server;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use notagent::core::llama::client::{
    LlamaClient, LlamaModelInfo, LlamaProgress, format_bytes, llama_inference_url,
    normalize_llama_server_url,
};
use notagent::core::llama::huggingface::{
    HuggingFaceClient, HuggingFaceGated, HuggingFaceModel, HuggingFaceModelDetails,
    HuggingFaceQuantization, find_hugging_face_token,
};
use notagent::core::llama::provider::{LLAMA_PROVIDER_ID, create_llama_provider};
use notagent_ai::auth::types::{
    ApiKeyAuthInput, ApiKeyCredential, AuthContext, AuthError, AuthEvent, AuthInteraction,
    AuthPrompt, AuthResult, AuthType, BoxFuture, Credential, ModelAuth, ProviderAuthInteraction,
};
use notagent_ai::models::{ModelsPublication, Provider, RefreshModelsContext};
use notagent_ai::models_store::ModelsStoreEntry;
use notagent_ai::types::{Modality, ProviderEnv};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use llama_server::{Reply, TestHttpServer};

fn model_info(id: &str, status: &str) -> Value {
    json!({ "id": id, "status": { "value": status } })
}

/// `AuthContext` of the TS `emptyContext`.
struct EmptyAuthContext;

impl AuthContext for EmptyAuthContext {
    fn env(&self, _name: &str) -> BoxFuture<'_, Option<String>> {
        Box::pin(async { None })
    }

    fn file_exists(&self, _path: &str) -> BoxFuture<'_, bool> {
        Box::pin(async { false })
    }
}

/// `interaction.prompt` answering from a fixed script, as in the TS case.
struct ScriptedInteraction {
    answers: Mutex<std::collections::VecDeque<String>>,
    signal: CancellationToken,
}

impl AuthInteraction for ScriptedInteraction {
    fn signal(&self) -> Option<CancellationToken> {
        Some(self.signal.clone())
    }

    fn prompt(&self, _prompt: AuthPrompt) -> BoxFuture<'_, Result<String, AuthError>> {
        let answer = self
            .answers
            .lock()
            .expect("poisoned")
            .pop_front()
            .expect("scripted answer");
        Box::pin(async move { Ok(answer) })
    }

    fn notify(&self, _event: AuthEvent) {}
}

#[test]
fn normalizes_management_and_inference_urls() {
    assert_eq!(
        normalize_llama_server_url("http://127.0.0.1:8080/v1/").expect("normalized"),
        "http://127.0.0.1:8080"
    );
    assert_eq!(
        normalize_llama_server_url("https://example.com/prefix/v1").expect("normalized"),
        "https://example.com/prefix"
    );
    let error = normalize_llama_server_url("file:///tmp/llama").expect_err("rejected");
    assert!(error.0.contains("http or https"), "{}", error.0);
    assert_eq!(
        llama_inference_url("http://127.0.0.1:8080/v1/").expect("inference url"),
        "http://127.0.0.1:8080/v1"
    );
}

#[test]
fn exposes_only_loaded_models_with_router_metadata() {
    let controller = create_llama_provider();
    let catalog: Vec<LlamaModelInfo> = serde_json::from_value(json!([
        {
            "id": "loaded",
            "status": { "value": "loaded", "args": ["llama-server", "--n-gpu-layers", "999"] },
            "architecture": { "input_modalities": ["text", "image"] },
            "meta": { "n_ctx": 65536, "n_ctx_train": 131_072 },
        },
        { "id": "unloaded", "status": { "value": "unloaded" } },
        { "id": "loading", "status": { "value": "loading" } },
    ]))
    .expect("catalog");
    controller
        .set_catalog(&catalog, "http://localhost:8080")
        .expect("set catalog");

    let models = controller.provider.get_models();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id, "loaded");
    assert_eq!(models[0].base_url, "http://localhost:8080/v1");
    assert_eq!(models[0].context_window, 65_536);
    assert_eq!(models[0].max_tokens, 65_536);
    assert_eq!(models[0].input, vec![Modality::Text, Modality::Image]);
    assert_eq!(models[0].provider, LLAMA_PROVIDER_ID);
    assert_eq!(models[0].api, "openai-completions");
}

#[tokio::test]
async fn persists_and_restores_loaded_models_for_cache_only_startup_refreshes() {
    let server = TestHttpServer::start(|request| match request.path.as_str() {
        "/models" => Reply::json(json!({
            "data": [
                { "id": "loaded", "status": { "value": "loaded" }, "meta": { "n_ctx": 32768 } },
                { "id": "unloaded", "status": { "value": "unloaded" } },
            ]
        })),
        _ => Reply::status(404),
    })
    .await;

    let cached: Arc<Mutex<Option<ModelsStoreEntry>>> = Arc::new(Mutex::new(None));
    let credential = Credential::ApiKey(ApiKeyCredential {
        key: Some("local".to_owned()),
        env: Some(ProviderEnv::from([(
            "LLAMA_BASE_URL".to_owned(),
            server.base_url.clone(),
        )])),
    });

    let publish = |cached: Arc<Mutex<Option<ModelsStoreEntry>>>| {
        move |publication: ModelsPublication<'_>| -> BoxFuture<'_, bool> {
            if let Some(persist) = publication.persist {
                *cached.lock().expect("poisoned") = persist;
            }
            if let Some(update) = publication.update {
                update();
            }
            Box::pin(async { true })
        }
    };

    let first = create_llama_provider();
    // The guard has to be released before the call: a temporary inside the
    // struct literal would live until the end of the statement — across the
    // `.await` — and `publish` locks the same mutex.
    let stored = cached.lock().expect("poisoned").clone();
    first
        .provider
        .refresh_models(RefreshModelsContext {
            credential: Some(credential.clone()),
            stored,
            publish: Box::new(publish(Arc::clone(&cached))),
            allow_network: true,
            force: None,
            signal: CancellationToken::new(),
        })
        .expect("dynamic provider")
        .await
        .expect("refresh");
    assert_eq!(
        first
            .provider
            .get_models()
            .iter()
            .map(|model| model.id.clone())
            .collect::<Vec<_>>(),
        vec!["loaded".to_owned()]
    );
    assert_eq!(
        cached
            .lock()
            .expect("poisoned")
            .as_ref()
            .expect("persisted")
            .models
            .iter()
            .map(|model| model.id.clone())
            .collect::<Vec<_>>(),
        vec!["loaded".to_owned()]
    );

    let second = create_llama_provider();
    let stored = cached.lock().expect("poisoned").clone();
    second
        .provider
        .refresh_models(RefreshModelsContext {
            credential: Some(credential),
            stored,
            publish: Box::new(publish(Arc::clone(&cached))),
            allow_network: false,
            force: None,
            signal: CancellationToken::new(),
        })
        .expect("dynamic provider")
        .await
        .expect("refresh");
    let restored = second.provider.get_models();
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].id, "loaded");
    assert_eq!(restored[0].base_url, format!("{}/v1", server.base_url));
    assert_eq!(restored[0].context_window, 32_768);
}

#[tokio::test]
async fn stays_dormant_until_configured_and_stores_url_plus_optional_key() {
    let controller = create_llama_provider();
    let provider = Arc::clone(&controller.provider);
    let auth = provider.auth().api_key.as_ref().expect("api key auth");
    let context = EmptyAuthContext;
    let signal = CancellationToken::new();

    let check = auth
        .check(ApiKeyAuthInput {
            ctx: &context,
            credential: None,
            signal: signal.clone(),
        })
        .expect("check implemented")
        .await
        .expect("check");
    assert_eq!(check, None);
    let resolved = auth
        .resolve(ApiKeyAuthInput {
            ctx: &context,
            credential: None,
            signal: signal.clone(),
        })
        .await
        .expect("resolve");
    assert_eq!(resolved, None);

    let server = TestHttpServer::start(|_request| Reply::json(json!({ "data": [] }))).await;

    let interaction = ScriptedInteraction {
        answers: Mutex::new([server.base_url.clone(), "secret".to_owned()].into()),
        signal: signal.clone(),
    };
    let provider_interaction = ProviderAuthInteraction {
        interaction: &interaction,
        signal: signal.clone(),
    };
    let credential = auth
        .login(&provider_interaction)
        .expect("login implemented")
        .await
        .expect("login");
    let login_requests = server.requests();
    assert_eq!(login_requests.len(), 1);
    assert_eq!(login_requests[0].method, "GET");
    assert_eq!(login_requests[0].path, "/models");
    assert_eq!(
        login_requests[0].header("authorization").as_deref(),
        Some("Bearer secret")
    );
    assert_eq!(
        credential,
        ApiKeyCredential {
            key: Some("secret".to_owned()),
            env: Some(ProviderEnv::from([(
                "LLAMA_BASE_URL".to_owned(),
                server.base_url.clone(),
            )])),
        }
    );

    let resolved = auth
        .resolve(ApiKeyAuthInput {
            ctx: &context,
            credential: Some(credential),
            signal,
        })
        .await
        .expect("resolve");
    assert_eq!(
        resolved,
        Some(AuthResult {
            auth: ModelAuth {
                api_key: Some("secret".to_owned()),
                headers: None,
                base_url: Some(format!("{}/v1", server.base_url)),
            },
            env: Some(ProviderEnv::from([(
                "LLAMA_BASE_URL".to_owned(),
                server.base_url.clone(),
            )])),
            source: Some("stored credential".to_owned()),
        })
    );
    assert_eq!(
        auth.check(ApiKeyAuthInput {
            ctx: &context,
            credential: Some(ApiKeyCredential {
                key: None,
                env: Some(ProviderEnv::from([(
                    "LLAMA_BASE_URL".to_owned(),
                    server.base_url.clone(),
                )])),
            }),
            signal: CancellationToken::new(),
        })
        .expect("check implemented")
        .await
        .expect("check")
        .map(|check| (check.source, check.check_type)),
        Some((Some("stored credential".to_owned()), AuthType::ApiKey))
    );
}

#[tokio::test]
async fn searches_hugging_face_and_reads_quantizations_plus_access_requirements() {
    let server = TestHttpServer::start(|request| {
        if request.path.starts_with("/api/models?") {
            return Reply::json(json!([{ "id": "owner/model-GGUF", "downloads": 1200 }]));
        }
        if request.path == "/api/models/owner/model-GGUF?blobs=true" {
            return Reply::json(json!({
                "id": "owner/model-GGUF",
                "gated": "manual",
                "siblings": [
                    { "rfilename": "model-Q5_K_M.gguf", "size": 6000 },
                    { "rfilename": "model-Q4_K_M-00001-of-00002.gguf", "size": 2000 },
                    { "rfilename": "model-Q4_K_M-00002-of-00002.gguf", "size": 3000 },
                    { "rfilename": "mmproj-F16.gguf", "size": 1000 },
                ],
            }));
        }
        Reply::status(404)
    })
    .await;
    let client = HuggingFaceClient::new(Some("hf-secret"), Some(&server.base_url));

    assert_eq!(
        client.search("qwen coder", None).await.expect("search"),
        vec![HuggingFaceModel {
            id: "owner/model-GGUF".to_owned(),
            downloads: 1200.0,
        }]
    );
    assert_eq!(
        client
            .details("owner/model-GGUF", None)
            .await
            .expect("details"),
        HuggingFaceModelDetails {
            id: "owner/model-GGUF".to_owned(),
            gated: HuggingFaceGated::Manual,
            quantizations: vec![
                HuggingFaceQuantization {
                    name: "Q4_K_M".to_owned(),
                    size: Some(5000.0),
                },
                HuggingFaceQuantization {
                    name: "Q5_K_M".to_owned(),
                    size: Some(6000.0),
                },
            ],
        }
    );
    let requests = server.requests();
    assert!(
        requests
            .iter()
            .all(|request| request.header("authorization").as_deref() == Some("Bearer hf-secret")),
        "{requests:?}"
    );
    let search = requests
        .iter()
        .find(|request| request.path.starts_with("/api/models?"))
        .expect("search request");
    let query = search.query();
    assert_eq!(query.get("search").map(String::as_str), Some("qwen coder"));
    assert_eq!(query.get("filter").map(String::as_str), Some("gguf"));
    assert_eq!(query.get("sort").map(String::as_str), Some("downloads"));
    assert_eq!(query.get("direction").map(String::as_str), Some("-1"));
    assert_eq!(query.get("limit").map(String::as_str), Some("20"));
    assert!(
        requests
            .iter()
            .any(|request| request.path == "/api/models/owner/model-GGUF?blobs=true"),
        "{requests:?}"
    );

    assert_eq!(
        find_hugging_face_token(&BTreeMap::from([(
            "HF_TOKEN".to_owned(),
            " hf-secret ".to_owned(),
        )]))
        .await,
        Some("hf-secret".to_owned())
    );
}

#[tokio::test]
async fn loads_with_sse_progress_and_waits_for_the_loaded_catalog_state() {
    let status = Arc::new(Mutex::new("unloaded".to_owned()));
    let server = {
        let status = Arc::clone(&status);
        TestHttpServer::start(move |request| {
            match (request.method.as_str(), request.path.as_str()) {
                (_, "/models/sse") => Reply::Sse,
                ("POST", "/models/load") => {
                    *status.lock().expect("poisoned") = "loading".to_owned();
                    let status = Arc::clone(&status);
                    Reply::json(json!({ "success": true })).then_after_subscriber(Box::new(
                        move |send| {
                            send(json!({
                                "model": "test-model",
                                "event": "status_change",
                                "data": {
                                    "status": "loading",
                                    "progress": {
                                        "stages": ["text_model", "mmproj_model"],
                                        "current": "text_model",
                                        "value": 0.5,
                                    },
                                },
                            }));
                            *status.lock().expect("poisoned") = "loaded".to_owned();
                            send(json!({
                                "model": "test-model",
                                "event": "status_change",
                                "data": { "status": "loaded" },
                            }));
                        },
                    ))
                }
                (_, "/models") => Reply::json(json!({
                    "data": [model_info("test-model", &status.lock().expect("poisoned"))]
                })),
                _ => Reply::status(404),
            }
        })
        .await
    };

    let progress: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&progress);
    let model = LlamaClient::new(&server.base_url, None)
        .expect("client")
        .load_and_wait(
            "test-model",
            Arc::new(move |entry: LlamaProgress| {
                sink.lock().expect("poisoned").push(entry.message);
            }),
            None,
        )
        .await
        .expect("load");
    assert_eq!(model.status.value, "loaded");
    assert!(
        progress
            .lock()
            .expect("poisoned")
            .contains(&"Loading text model".to_owned()),
        "{:?}",
        progress.lock().expect("poisoned")
    );
}

#[tokio::test]
async fn downloads_with_byte_progress_and_returns_the_refreshed_catalog() {
    let status = Arc::new(Mutex::new("missing".to_owned()));
    let server = {
        let status = Arc::clone(&status);
        TestHttpServer::start(move |request| {
            if request.path == "/models/sse" {
                return Reply::Sse;
            }
            if request.method == "POST" && request.path == "/models" {
                *status.lock().expect("poisoned") = "downloading".to_owned();
                let status = Arc::clone(&status);
                return Reply::json(json!({ "success": true })).then_after_subscriber(Box::new(
                    move |send| {
                        send(json!({
                            "model": "owner/repo:Q4_K_M",
                            "event": "download_progress",
                            "data": {
                                "progress": {
                                    "https://example/model.gguf": { "done": 512, "total": 1024 }
                                }
                            },
                        }));
                        *status.lock().expect("poisoned") = "unloaded".to_owned();
                        send(json!({
                            "model": "owner/repo:Q4_K_M",
                            "event": "download_finished",
                            "data": {},
                        }));
                    },
                ));
            }
            if request.path.starts_with("/models") {
                let status = status.lock().expect("poisoned").clone();
                return Reply::json(json!({
                    "data": if status == "missing" {
                        Vec::new()
                    } else {
                        vec![model_info("owner/repo:Q4_K_M", &status)]
                    }
                }));
            }
            Reply::status(404)
        })
        .await
    };

    let progress: Arc<Mutex<Vec<LlamaProgress>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&progress);
    let models = LlamaClient::new(&server.base_url, None)
        .expect("client")
        .download_and_wait(
            "owner/repo:Q4_K_M",
            Arc::new(move |entry: LlamaProgress| {
                sink.lock().expect("poisoned").push(entry);
            }),
            None,
        )
        .await
        .expect("download");
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id, "owner/repo:Q4_K_M");
    assert_eq!(models[0].status.value, "unloaded");
    assert!(
        progress.lock().expect("poisoned").contains(&LlamaProgress {
            message: "Downloading model".to_owned(),
            ratio: Some(0.5),
            detail: Some("512 B / 1.00 KiB".to_owned()),
        }),
        "{:?}",
        progress.lock().expect("poisoned")
    );
}

/// Extra coverage for the byte formatter behind the `detail` line: TS reads it
/// off `Number.prototype.toFixed`, which rounds ties away from zero.
#[test]
fn formats_bytes_like_the_typescript_helper() {
    assert_eq!(format_bytes(0.0), "0 B");
    assert_eq!(format_bytes(1023.0), "1023 B");
    assert_eq!(format_bytes(1024.0), "1.00 KiB");
    assert_eq!(format_bytes(1152.0), "1.13 KiB");
    assert_eq!(format_bytes(10_240.0), "10.0 KiB");
    assert_eq!(format_bytes(1_048_576.0), "1.00 MiB");
    assert_eq!(format_bytes(1_073_741_824.0), "1.00 GiB");
    assert_eq!(format_bytes(1_099_511_627_776.0), "1.00 TiB");
    // TiB is the last unit; larger values keep growing in it.
    assert_eq!(format_bytes(1_125_899_906_842_624.0), "1024.0 TiB");
}
