//! Behaviour tests for the remaining `packages/ai` modules without a TS suite:
//! `utils/abort.ts` + `utils/abort-signals.ts`, `api/lazy.ts`, `auth/helpers.ts`,
//! `auth/types.ts`, `compat/extension-oauth-types.ts` and `oauth.ts`.
//!
//! These modules are cancellation and trait plumbing rather than value mappings,
//! so there is nothing a JSONL oracle could pin — the expectations are read off
//! the TypeScript source named in each test.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use notagent_ai::api::lazy::{create_setup_error_message, lazy_stream};
use notagent_ai::auth::helpers::EnvApiKeyAuth;
use notagent_ai::auth::types::{
    ApiKeyAuth, ApiKeyAuthInput, ApiKeyCredential, AuthContext, AuthError, AuthEvent,
    AuthInteraction, AuthPrompt, AuthPromptKind, BoxFuture, Credential, OAuthCredential,
    ProviderAuthInteraction,
};
use notagent_ai::types::{
    AssistantMessageEvent, ErrorReason, Modality, Model, ModelCost, StopReason,
};
use notagent_ai::utils::abort::{
    Aborted, combine_abort_signals, operation_signal, race_with_abort_signal,
};
use notagent_ai::utils::event_stream::create_assistant_message_event_stream;
use serde_json::json;
use tokio_util::sync::CancellationToken;

// --- utils/abort.ts --------------------------------------------------------

#[test]
fn operation_signal_creates_a_token_when_the_caller_passes_none() {
    // `operationSignal(signal)` — `signal ?? new AbortController().signal`.
    let provided = CancellationToken::new();
    let kept = operation_signal(Some(provided.clone()));
    provided.cancel();
    assert!(kept.is_cancelled(), "the caller's signal is passed through");

    let created = operation_signal(None);
    assert!(!created.is_cancelled());
    created.cancel();
    assert!(created.is_cancelled());
}

#[tokio::test]
async fn race_with_abort_signal_resolves_the_operation_when_nothing_aborts() {
    let signal = CancellationToken::new();
    let value = race_with_abort_signal(async { 7 }, &signal).await;
    assert_eq!(value, Ok(7));
}

#[tokio::test]
async fn race_with_abort_signal_gives_up_on_an_already_aborted_signal() {
    let signal = CancellationToken::new();
    signal.cancel();
    // TS rejects with the abort reason before ever touching the operation.
    let ran = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&ran);
    let value = race_with_abort_signal(
        async move {
            counted.fetch_add(1, Ordering::SeqCst);
            7
        },
        &signal,
    )
    .await;
    assert_eq!(value, Err(Aborted));
    assert_eq!(ran.load(Ordering::SeqCst), 0, "the operation never started");
}

#[tokio::test]
async fn race_with_abort_signal_gives_up_when_the_signal_fires_first() {
    let signal = CancellationToken::new();
    let firing = signal.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(5)).await;
        firing.cancel();
    });
    let value = race_with_abort_signal(
        async {
            tokio::time::sleep(Duration::from_secs(30)).await;
            7
        },
        &signal,
    )
    .await;
    assert_eq!(value, Err(Aborted));
}

// --- utils/abort-signals.ts ------------------------------------------------

#[tokio::test]
async fn combine_abort_signals_returns_nothing_without_a_signal() {
    let combined = combine_abort_signals(&[None, None]);
    assert!(combined.signal.is_none());
}

#[tokio::test]
async fn combine_abort_signals_passes_a_single_signal_through() {
    let only = CancellationToken::new();
    let combined = combine_abort_signals(&[None, Some(only.clone()), None]);
    let signal = combined.signal.as_ref().expect("the single signal").clone();
    only.cancel();
    assert!(signal.is_cancelled());
}

#[tokio::test]
async fn combine_abort_signals_fires_when_any_source_fires() {
    let first = CancellationToken::new();
    let second = CancellationToken::new();
    let combined = combine_abort_signals(&[Some(first.clone()), Some(second.clone())]);
    let signal = combined.signal.as_ref().expect("a derived signal").clone();
    assert!(!signal.is_cancelled());

    second.cancel();
    signal.cancelled().await;
    assert!(signal.is_cancelled());
}

#[tokio::test]
async fn combine_abort_signals_is_already_aborted_when_a_source_is() {
    let first = CancellationToken::new();
    first.cancel();
    let combined = combine_abort_signals(&[Some(first), Some(CancellationToken::new())]);
    assert!(
        combined
            .signal
            .as_ref()
            .expect("a derived signal")
            .is_cancelled()
    );
}

#[tokio::test]
async fn combine_abort_signals_cleanup_stops_the_forwarding() {
    let first = CancellationToken::new();
    let second = CancellationToken::new();
    let mut combined = combine_abort_signals(&[Some(first.clone()), Some(second.clone())]);
    let signal = combined.signal.as_ref().expect("a derived signal").clone();
    combined.cleanup();
    first.cancel();
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(
        !signal.is_cancelled(),
        "cleanup() removes the forwarding listeners"
    );
}

// --- api/lazy.ts -----------------------------------------------------------

fn test_model() -> Model {
    Model {
        id: "model-1".to_owned(),
        name: "Model One".to_owned(),
        api: "anthropic-messages".to_owned(),
        provider: "anthropic".to_owned(),
        base_url: "https://example.invalid".to_owned(),
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

#[test]
fn setup_error_messages_carry_the_model_identity() {
    // `createSetupErrorMessage(model, error)`.
    let message = create_setup_error_message(&test_model(), &"no api key", 99);
    assert_eq!(message.stop_reason, StopReason::Error);
    assert_eq!(message.error_message.as_deref(), Some("no api key"));
    assert_eq!(message.api, "anthropic-messages");
    assert_eq!(message.provider, "anthropic");
    assert_eq!(message.model, "model-1");
    assert_eq!(message.timestamp, 99);
    assert!(message.content.is_empty());
}

#[tokio::test]
async fn lazy_stream_forwards_the_inner_stream() {
    let inner = create_assistant_message_event_stream();
    let source = inner.clone();
    let stream = lazy_stream(test_model(), move || async move { Ok(source) });

    inner.push(AssistantMessageEvent::Start {
        partial: create_setup_error_message(&test_model(), &"", 1),
    });
    let mut final_message = create_setup_error_message(&test_model(), &"", 1);
    final_message.stop_reason = StopReason::Stop;
    final_message.error_message = None;
    inner.end(Some(final_message));

    let mut kinds = Vec::new();
    while let Some(event) = stream.next().await {
        kinds.push(match event {
            AssistantMessageEvent::Start { .. } => "start",
            AssistantMessageEvent::Error { .. } => "error",
            _ => "other",
        });
    }
    assert_eq!(kinds, vec!["start"]);
    assert_eq!(stream.result().await.stop_reason, StopReason::Stop);
}

#[tokio::test]
async fn lazy_stream_turns_a_setup_failure_into_an_error_event() {
    // The TS contract: nothing is thrown after `lazyStream` returned; a failed
    // setup terminates the stream instead.
    let stream = lazy_stream(test_model(), || async { Err("boom".to_owned()) });

    let mut saw_error = false;
    while let Some(event) = stream.next().await {
        if let AssistantMessageEvent::Error { reason, error } = event {
            assert_eq!(reason, ErrorReason::Error);
            assert_eq!(error.error_message.as_deref(), Some("boom"));
            saw_error = true;
        }
    }
    assert!(saw_error, "the setup failure reaches the consumer");
    let result = stream.result().await;
    assert_eq!(result.stop_reason, StopReason::Error);
    assert_eq!(result.error_message.as_deref(), Some("boom"));
}

// --- auth/helpers.ts -------------------------------------------------------

struct MapAuthContext(Vec<(String, String)>);

impl AuthContext for MapAuthContext {
    fn env(&self, name: &str) -> BoxFuture<'_, Option<String>> {
        let value = self
            .0
            .iter()
            .find(|(candidate, _)| candidate == name)
            .map(|(_, value)| value.clone());
        Box::pin(async move { value })
    }

    fn file_exists(&self, _path: &str) -> BoxFuture<'_, bool> {
        Box::pin(async { false })
    }
}

#[tokio::test]
async fn env_api_key_auth_prefers_a_stored_credential() {
    let auth = EnvApiKeyAuth::new("Anthropic API key", ["ANTHROPIC_API_KEY"]);
    let context = MapAuthContext(vec![(
        "ANTHROPIC_API_KEY".to_owned(),
        "from-env".to_owned(),
    )]);
    let signal = CancellationToken::new();
    let resolved = auth
        .resolve(ApiKeyAuthInput {
            ctx: &context,
            credential: Some(ApiKeyCredential {
                key: Some("stored".to_owned()),
                env: None,
            }),
            signal: signal.clone(),
        })
        .await
        .expect("resolves")
        .expect("a result");
    assert_eq!(resolved.auth.api_key.as_deref(), Some("stored"));
    assert_eq!(resolved.source.as_deref(), Some("stored credential"));
}

#[tokio::test]
async fn env_api_key_auth_falls_back_to_the_first_set_env_var() {
    let auth = EnvApiKeyAuth::new("Anthropic API key", ["MISSING_ONE", "SECOND_ONE"]);
    let context = MapAuthContext(vec![("SECOND_ONE".to_owned(), "from-env".to_owned())]);
    let signal = CancellationToken::new();
    let resolved = auth
        .resolve(ApiKeyAuthInput {
            ctx: &context,
            credential: None,
            signal: signal.clone(),
        })
        .await
        .expect("resolves")
        .expect("a result");
    assert_eq!(resolved.auth.api_key.as_deref(), Some("from-env"));
    assert_eq!(
        resolved.source.as_deref(),
        Some("SECOND_ONE"),
        "the first set env var wins"
    );
}

#[tokio::test]
async fn env_api_key_auth_resolves_to_nothing_without_a_key() {
    let auth = EnvApiKeyAuth::new("Anthropic API key", ["MISSING_ONE"]);
    let context = MapAuthContext(vec![]);
    let signal = CancellationToken::new();
    let resolved = auth
        .resolve(ApiKeyAuthInput {
            ctx: &context,
            credential: None,
            signal: signal.clone(),
        })
        .await
        .expect("resolves");
    assert!(resolved.is_none());
}

#[tokio::test]
async fn env_api_key_auth_gives_up_on_an_aborted_signal() {
    let auth = EnvApiKeyAuth::new("Anthropic API key", ["MISSING_ONE"]);
    let context = MapAuthContext(vec![]);
    let signal = CancellationToken::new();
    signal.cancel();
    let error = auth
        .resolve(ApiKeyAuthInput {
            ctx: &context,
            credential: None,
            signal: signal.clone(),
        })
        .await
        .expect_err("throwIfAborted");
    assert_eq!(error.0, "The operation was aborted");
}

/// Records the prompts an `ApiKeyAuth::login` issues.
struct RecordingInteraction {
    prompts: Arc<std::sync::Mutex<Vec<String>>>,
}

impl AuthInteraction for RecordingInteraction {
    fn signal(&self) -> Option<CancellationToken> {
        None
    }

    fn prompt(&self, prompt: AuthPrompt) -> BoxFuture<'_, Result<String, AuthError>> {
        if let AuthPromptKind::Secret { message, .. } = &prompt.kind {
            self.prompts
                .lock()
                .expect("prompts mutex")
                .push(message.clone());
        }
        Box::pin(async { Ok("typed-key".to_owned()) })
    }

    fn notify(&self, _event: AuthEvent) {}
}

#[tokio::test]
async fn env_api_key_auth_login_prompts_for_the_key() {
    let auth = EnvApiKeyAuth::new("Anthropic API key", ["ANTHROPIC_API_KEY"]);
    let signal = CancellationToken::new();
    let prompts = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let recording = RecordingInteraction {
        prompts: Arc::clone(&prompts),
    };
    let interaction = ProviderAuthInteraction {
        interaction: &recording,
        signal: signal.clone(),
    };

    let credential = auth
        .login(&interaction)
        .expect("api key auth offers a login")
        .await
        .expect("logs in");
    assert_eq!(credential.key.as_deref(), Some("typed-key"));
    assert_eq!(
        *prompts.lock().expect("prompts mutex"),
        vec!["Enter Anthropic API key".to_owned()]
    );
}

// --- auth/types.ts ---------------------------------------------------------

#[test]
fn oauth_credentials_keep_unknown_fields_like_the_typescript_index_signature() {
    // `OAuthCredential` carries `[key: string]: unknown` in TS; the port keeps the
    // extra fields so a stored credential survives a round trip unchanged.
    let stored = json!({
        "type": "oauth",
        "refresh": "r",
        "access": "a",
        "expires": 1_700_000_000_000i64,
        "accountId": "acct-1",
        "scopes": ["a", "b"],
    });
    let credential: Credential = serde_json::from_value(stored.clone()).expect("deserializes");
    let Credential::OAuth(oauth) = &credential else {
        panic!("expected an oauth credential");
    };
    assert_eq!(oauth.refresh, "r");
    assert_eq!(oauth.access, "a");
    assert_eq!(oauth.expires, 1_700_000_000_000);
    assert_eq!(oauth.extra["accountId"], json!("acct-1"));
    assert_eq!(
        serde_json::to_value(&credential).expect("serializes"),
        stored,
        "a stored credential round trips byte for byte"
    );
}

#[test]
fn api_key_credentials_omit_absent_fields() {
    let credential = Credential::ApiKey(ApiKeyCredential {
        key: Some("k".to_owned()),
        env: None,
    });
    assert_eq!(
        serde_json::to_value(&credential).expect("serializes"),
        json!({ "type": "api_key", "key": "k" }),
        "optional fields are omitted, never serialized as null"
    );
}

#[test]
fn oauth_credentials_serialize_without_extra_fields() {
    let credential = Credential::OAuth(OAuthCredential {
        refresh: "r".to_owned(),
        access: "a".to_owned(),
        expires: 1,
        extra: serde_json::Map::new(),
    });
    assert_eq!(
        serde_json::to_value(&credential).expect("serializes"),
        json!({ "type": "oauth", "refresh": "r", "access": "a", "expires": 1 })
    );
}

// --- compat/extension-oauth-types.ts and oauth.ts --------------------------

/// The extension callback surface, with only the two required callbacks filled in.
struct MinimalCallbacks {
    selected: Option<String>,
}

impl notagent_ai::OAuthLoginCallbacks for MinimalCallbacks {
    fn on_auth(&self, _info: notagent_ai::OAuthAuthInfo) {}

    fn on_device_code(&self, _info: notagent_ai::OAuthDeviceCodeInfo) {}

    fn on_prompt(
        &self,
        _prompt: notagent_ai::OAuthPrompt,
    ) -> BoxFuture<'_, Result<String, AuthError>> {
        Box::pin(async { Ok("typed".to_owned()) })
    }

    fn on_select(
        &self,
        _prompt: notagent_ai::OAuthSelectPrompt,
    ) -> BoxFuture<'_, Result<Option<String>, AuthError>> {
        let selected = self.selected.clone();
        Box::pin(async move { Ok(selected) })
    }
}

#[tokio::test]
async fn extension_oauth_callbacks_make_the_optional_hooks_defaults() {
    // `oauth.ts` is a type-only re-export of these declarations. Deviation class 1:
    // the optional TS callbacks become default trait methods, and `onSelect`
    // returns `Option<String>` instead of `string | undefined`.
    use notagent_ai::{OAuthLoginCallbacks, OAuthSelectOption, OAuthSelectPrompt};

    let callbacks = MinimalCallbacks {
        selected: Some("one".to_owned()),
    };
    assert!(
        callbacks.on_manual_code_input().is_none(),
        "onManualCodeInput is optional"
    );
    assert!(callbacks.signal().is_none(), "signal is optional");
    callbacks.on_progress("still working".to_owned());

    let prompt = OAuthSelectPrompt {
        message: "Pick one".to_owned(),
        options: vec![OAuthSelectOption {
            id: "one".to_owned(),
            label: "One".to_owned(),
        }],
    };
    assert_eq!(
        callbacks.on_select(prompt.clone()).await.expect("selects"),
        Some("one".to_owned())
    );

    let cancelling = MinimalCallbacks { selected: None };
    assert_eq!(
        cancelling.on_select(prompt).await.expect("cancels"),
        None,
        "a cancelled selection is None, not an error"
    );
}
