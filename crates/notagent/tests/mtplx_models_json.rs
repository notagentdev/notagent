//! A remote MTPLX server is configured through `models.json` rather than through
//! code. These tests walk that route end to end — config text in, request
//! headers out — because every stage between the two can drop what the previous
//! one produced: the schema validator rejects unknown literals, the composer
//! rebuilds `compat` against the model's api and drops keys that api never
//! reads, and a model defined in `models.json` carries no `headers` field on
//! the model itself.

use notagent::core::model_config::ModelConfig;
use notagent::core::provider_composer::{compose_model_provider, resolve_configured_model_headers};
use notagent_ai::api::openai_completions_compat::get_compat;
use notagent_ai::api::openai_completions_params::build_client_headers;
use notagent_ai::types::{
    Context, Message, Model, SessionAffinityFormat, UserContent, UserMessage,
};

const PROVIDER_ID: &str = "mtplx-remote";

/// What a user writes to reach a remote MTPLX server.
fn models_json() -> String {
    format!(
        r#"{{
  "providers": {{
    "{PROVIDER_ID}": {{
      "name": "MTPLX (remote)",
      "baseUrl": "https://mtplx.example.com/v1",
      "api": "openai-completions",
      "apiKey": "{{env:MTPLX_REMOTE_KEY}}",
      "compat": {{
        "sendSessionAffinityHeaders": true,
        "sessionAffinityFormat": "mtplx"
      }},
      "models": [
        {{
          "id": "qwen3.5-4b-mtplx-optimized-speed",
          "name": "Qwen3.5 4B Optimized Speed",
          "contextWindow": 262144,
          "maxTokens": 262144,
          "headers": {{ "x-mtplx-client": "notagent" }}
        }}
      ]
    }}
  }}
}}"#
    )
}

fn config() -> ModelConfig {
    let config = ModelConfig::parse(&models_json(), "test-models.json");
    assert_eq!(
        config.get_error(),
        None,
        "the schema validator rejected the config"
    );
    config
}

fn composed_model() -> (ModelConfig, Model) {
    let config = config();
    let provider = compose_model_provider(PROVIDER_ID, None, &config)
        .unwrap_or_else(|error| panic!("composing the provider failed: {error}"));
    let model = provider
        .get_models()
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("the composed provider offers no model"));
    (config, model)
}

fn context() -> Context {
    Context {
        messages: vec![Message::User(UserMessage {
            content: UserContent::Text("hi".to_string()),
            timestamp: 1,
        })],
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// Stage 1: the schema validator
// ---------------------------------------------------------------------------

#[test]
fn the_validator_accepts_the_mtplx_session_format() {
    // The validator checks this key against a fixed list of literals, so an
    // unlisted value fails the whole file before any of it is deserialized.
    assert_eq!(config().get_error(), None);
}

#[test]
fn the_validator_still_rejects_a_format_that_does_not_exist() {
    let broken = models_json().replace("\"mtplx\"", "\"not-a-format\"");
    let config = ModelConfig::parse(&broken, "test-models.json");
    let error = config
        .get_error()
        .unwrap_or_else(|| panic!("an unknown format must not be accepted"));
    assert!(error.contains("sessionAffinityFormat"), "{error}");
}

// ---------------------------------------------------------------------------
// Stage 2: the composer
// ---------------------------------------------------------------------------

#[test]
fn the_composed_model_keeps_the_session_format() {
    // The composer merges provider-level compat into the model and reparses the
    // result against the model's api, dropping keys that api does not read.
    let (_, model) = composed_model();
    let compat = get_compat(&model);
    assert_eq!(
        compat.session_affinity_format,
        Some(SessionAffinityFormat::Mtplx)
    );
    assert!(compat.send_session_affinity_headers);
}

#[test]
fn the_composed_model_points_at_the_configured_server() {
    let (_, model) = composed_model();
    assert_eq!(model.id, "qwen3.5-4b-mtplx-optimized-speed");
    assert_eq!(model.provider, PROVIDER_ID);
    assert_eq!(model.base_url, "https://mtplx.example.com/v1");
    assert_eq!(model.api, "openai-completions");
    assert_eq!(model.context_window, 262_144);
    assert_eq!(model.max_tokens, 262_144);
}

// ---------------------------------------------------------------------------
// Stage 3: the headers that leave the process
// ---------------------------------------------------------------------------

#[test]
fn a_request_carries_the_mtplx_session_header() {
    let (_, model) = composed_model();
    let headers = build_client_headers(
        &model,
        &context(),
        None,
        Some("session-from-models-json"),
        &get_compat(&model),
    );
    assert_eq!(
        headers.get("x-mtplx-session-id"),
        Some(&Some("session-from-models-json".to_string()))
    );
    for unread in ["session_id", "x-client-request-id", "x-session-affinity"] {
        assert!(!headers.contains_key(unread), "unexpected header: {unread}");
    }
}

#[test]
fn the_client_header_survives_although_the_model_itself_carries_none() {
    // A model defined in models.json is built with no `headers` field, so the
    // client header cannot come from the model. It reaches a request through
    // the configured-headers route instead, and losing that would leave the
    // server classifying us by user-agent.
    let (config, model) = composed_model();
    assert!(
        model.headers.is_none(),
        "a models.json model is expected to carry no headers of its own"
    );
    let configured =
        resolve_configured_model_headers(&model, config.get_provider(PROVIDER_ID), None)
            .unwrap_or_else(|error| panic!("resolving configured headers failed: {error}"))
            .unwrap_or_else(|| panic!("no configured headers were resolved"));
    assert_eq!(
        configured.get("x-mtplx-client"),
        Some(&"notagent".to_string())
    );
}
