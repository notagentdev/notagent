use std::sync::Arc;
use std::sync::atomic::Ordering;

use notagent_ai::models::{CreateModelsOptions, create_models};
use notagent_ai::providers::faux::*;
use notagent_ai::types::*;
use serde_json::json;

fn context(text: &str) -> Context {
    Context {
        system_prompt: None,
        messages: vec![Message::User(UserMessage {
            content: UserContent::Text(text.to_string()),
            timestamp: 1,
        })],
        tools: None,
    }
}

async fn drain(
    stream: notagent_ai::utils::event_stream::AssistantMessageEventStream,
) -> (Vec<String>, AssistantMessage) {
    let mut names = Vec::new();
    while let Some(event) = stream.next().await {
        names.push(
            match event {
                AssistantMessageEvent::Start { .. } => "start",
                AssistantMessageEvent::TextStart { .. } => "text_start",
                AssistantMessageEvent::TextDelta { .. } => "text_delta",
                AssistantMessageEvent::TextEnd { .. } => "text_end",
                AssistantMessageEvent::ThinkingStart { .. } => "thinking_start",
                AssistantMessageEvent::ThinkingDelta { .. } => "thinking_delta",
                AssistantMessageEvent::ThinkingEnd { .. } => "thinking_end",
                AssistantMessageEvent::ToolcallStart { .. } => "toolcall_start",
                AssistantMessageEvent::ToolcallDelta { .. } => "toolcall_delta",
                AssistantMessageEvent::ToolcallEnd { .. } => "toolcall_end",
                AssistantMessageEvent::Done { .. } => "done",
                AssistantMessageEvent::Error { .. } => "error",
            }
            .to_string(),
        );
    }
    (names, stream.result().await)
}

#[tokio::test]
async fn streams_a_scripted_text_response_as_deltas() {
    let faux = faux_provider(FauxProviderOptions::default());
    faux.set_responses(vec![
        faux_assistant_message(
            vec![faux_text("Hello from the faux provider")],
            StopReason::Stop,
        )
        .into(),
    ]);

    let model = faux.get_model(None).expect("default model");
    let (names, message) = drain(faux.core.stream(&model, &context("hi"), None)).await;

    assert_eq!(names.first().map(String::as_str), Some("start"));
    assert_eq!(names.last().map(String::as_str), Some("done"));
    assert!(names.iter().filter(|name| *name == "text_delta").count() >= 1);
    assert_eq!(message.stop_reason, StopReason::Stop);
    let AssistantContent::Text(text) = &message.content[0] else {
        panic!("text")
    };
    assert_eq!(text.text, "Hello from the faux provider");
    assert_eq!(faux.state().call_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn streams_thinking_and_tool_calls() {
    let faux = faux_provider(FauxProviderOptions::default());
    faux.set_responses(vec![
        faux_assistant_message(
            vec![
                faux_thinking("considering"),
                faux_text("answer"),
                faux_tool_call("read", json!({"path": "a.txt"}), Some("call_1".to_string())),
            ],
            StopReason::ToolUse,
        )
        .into(),
    ]);

    let model = faux.get_model(None).expect("model");
    let (names, message) = drain(faux.core.stream(&model, &context("hi"), None)).await;

    assert!(names.contains(&"thinking_start".to_string()));
    assert!(names.contains(&"toolcall_start".to_string()));
    assert!(names.contains(&"toolcall_end".to_string()));
    assert_eq!(message.stop_reason, StopReason::ToolUse);
    let AssistantContent::ToolCall(tool_call) = &message.content[2] else {
        panic!("tool call")
    };
    assert_eq!(tool_call.id, "call_1");
    assert_eq!(tool_call.arguments.get("path"), Some(&json!("a.txt")));
}

#[tokio::test]
async fn an_empty_queue_produces_an_error_message() {
    let faux = faux_provider(FauxProviderOptions::default());
    let model = faux.get_model(None).expect("model");
    let (names, message) = drain(faux.core.stream(&model, &context("hi"), None)).await;

    assert_eq!(names, ["error"]);
    assert_eq!(message.stop_reason, StopReason::Error);
    assert_eq!(
        message.error_message.as_deref(),
        Some("No more faux responses queued")
    );
}

#[tokio::test]
async fn responses_are_consumed_in_order() {
    let faux = faux_provider(FauxProviderOptions::default());
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("first")], StopReason::Stop).into(),
        faux_assistant_message(vec![faux_text("second")], StopReason::Stop).into(),
    ]);
    assert_eq!(faux.pending_response_count(), 2);

    let model = faux.get_model(None).expect("model");
    let (_, first) = drain(faux.core.stream(&model, &context("a"), None)).await;
    let (_, second) = drain(faux.core.stream(&model, &context("b"), None)).await;

    let AssistantContent::Text(text) = &first.content[0] else {
        panic!("text")
    };
    assert_eq!(text.text, "first");
    let AssistantContent::Text(text) = &second.content[0] else {
        panic!("text")
    };
    assert_eq!(text.text, "second");
    assert_eq!(faux.pending_response_count(), 0);
}

#[tokio::test]
async fn a_response_factory_sees_the_request_context() {
    let faux = faux_provider(FauxProviderOptions::default());
    faux.set_responses(vec![FauxResponseStep::Factory(Arc::new(
        |context, model| {
            let echoed = match context.messages.first() {
                Some(Message::User(user)) => match &user.content {
                    UserContent::Text(text) => text.clone(),
                    _ => String::new(),
                },
                _ => String::new(),
            };
            faux_assistant_message(
                vec![faux_text(format!("{}:{echoed}", model.id))],
                StopReason::Stop,
            )
        },
    ))]);

    let model = faux.get_model(None).expect("model");
    let (_, message) = drain(faux.core.stream(&model, &context("ping"), None)).await;
    let AssistantContent::Text(text) = &message.content[0] else {
        panic!("text")
    };
    assert_eq!(text.text, "faux-1:ping");
}

#[tokio::test]
async fn usage_is_estimated_from_the_serialized_context() {
    let faux = faux_provider(FauxProviderOptions::default());
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("12345678")], StopReason::Stop).into(),
    ]);

    let model = faux.get_model(None).expect("model");
    let (_, message) = drain(faux.core.stream(&model, &context("hello"), None)).await;

    // "user:hello" is 10 characters -> 3 tokens; the answer is 8 characters -> 2 tokens.
    assert_eq!(message.usage.input, 3);
    assert_eq!(message.usage.output, 2);
    assert_eq!(message.usage.total_tokens, Some(5));
}

#[tokio::test]
async fn a_session_id_enables_prompt_cache_accounting() {
    let faux = faux_provider(FauxProviderOptions::default());
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("one")], StopReason::Stop).into(),
        faux_assistant_message(vec![faux_text("two")], StopReason::Stop).into(),
    ]);
    let model = faux.get_model(None).expect("model");
    let options = |session: &str| SimpleStreamOptions {
        base: StreamOptions {
            session_id: Some(session.to_string()),
            ..Default::default()
        },
        ..Default::default()
    };

    let first_context = context("hello");
    let (_, first) = drain(
        faux.core
            .stream(&model, &first_context, Some(options("s1"))),
    )
    .await;
    assert_eq!(first.usage.cache_read, 0, "nothing cached yet");
    assert!(
        first.usage.cache_write > 0,
        "the whole prompt is written to the cache"
    );

    // A prompt that extends the previous one reads the shared prefix from the cache.
    let mut second_context = first_context.clone();
    second_context.messages.push(Message::User(UserMessage {
        content: UserContent::Text("more".to_string()),
        timestamp: 2,
    }));
    let (_, second) = drain(
        faux.core
            .stream(&model, &second_context, Some(options("s1"))),
    )
    .await;
    assert!(
        second.usage.cache_read > 0,
        "the common prefix is a cache read"
    );
    assert!(second.usage.cache_write > 0, "the new suffix is written");
}

#[tokio::test]
async fn an_aborted_request_ends_as_an_aborted_message() {
    let faux = faux_provider(FauxProviderOptions::default());
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("never delivered")], StopReason::Stop).into(),
    ]);
    let model = faux.get_model(None).expect("model");

    let signal = tokio_util::sync::CancellationToken::new();
    signal.cancel();
    let options = SimpleStreamOptions {
        base: StreamOptions {
            base: ProviderRequestOptions {
                signal: Some(signal),
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    };

    let (names, message) = drain(faux.core.stream(&model, &context("hi"), Some(options))).await;
    assert_eq!(names, ["error"]);
    assert_eq!(message.stop_reason, StopReason::Aborted);
    assert_eq!(
        message.error_message.as_deref(),
        Some("Request was aborted")
    );
}

#[tokio::test]
async fn deferred_requests_return_a_handle_and_resolve_on_fetch() {
    let faux = faux_provider(FauxProviderOptions {
        deferred_pending_fetches: Some(1),
        ..Default::default()
    });
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("deferred answer")], StopReason::Stop).into(),
    ]);
    let model = faux.get_model(None).expect("model");

    let options = SimpleStreamOptions {
        deferred: Some(DeferredRequest::Enabled(true)),
        ..Default::default()
    };
    let (_, submitted) = drain(faux.core.stream(&model, &context("hi"), Some(options))).await;
    assert_eq!(submitted.stop_reason, StopReason::Deferred);
    let handle = submitted.deferred.clone().expect("handle");

    // The first fetch is still pending and returns the handle again.
    let (_, pending) = drain(faux.core.fetch_deferred(&model, &handle, None)).await;
    assert_eq!(pending.stop_reason, StopReason::Deferred);

    // The second fetch resolves the scripted response.
    let (_, resolved) = drain(faux.core.fetch_deferred(&model, &handle, None)).await;
    assert_eq!(resolved.stop_reason, StopReason::Stop);
    let AssistantContent::Text(text) = &resolved.content[0] else {
        panic!("text")
    };
    assert_eq!(text.text, "deferred answer");
    assert_eq!(faux.state().deferred_fetch_count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn fetching_an_unknown_or_cancelled_handle_fails() {
    let faux = faux_provider(FauxProviderOptions::default());
    let model = faux.get_model(None).expect("model");
    let unknown = DeferredHandle {
        provider: model.provider.clone(),
        model_id: model.id.clone(),
        api: model.api.clone(),
        id: "missing".to_string(),
        expires_at: None,
        poll_after_ms: None,
        data: None,
    };
    let (_, message) = drain(faux.core.fetch_deferred(&model, &unknown, None)).await;
    assert_eq!(message.stop_reason, StopReason::Error);
    assert!(
        message
            .error_message
            .as_deref()
            .unwrap()
            .contains("Unknown faux deferred response")
    );

    // Cancelling marks the entry and records the handle.
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("x")], StopReason::Stop).into(),
    ]);
    let options = SimpleStreamOptions {
        deferred: Some(DeferredRequest::Enabled(true)),
        ..Default::default()
    };
    let (_, submitted) = drain(faux.core.stream(&model, &context("hi"), Some(options))).await;
    let handle = submitted.deferred.clone().expect("handle");
    faux.core.cancel_deferred(&handle);

    let (_, message) = drain(faux.core.fetch_deferred(&model, &handle, None)).await;
    assert!(
        message
            .error_message
            .as_deref()
            .unwrap()
            .contains("was cancelled")
    );
    assert_eq!(faux.state().cancelled_deferred.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn the_provider_plugs_into_a_models_collection() {
    let faux = faux_provider(FauxProviderOptions::default());
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("through models")], StopReason::Stop).into(),
    ]);

    let models = create_models(Some(CreateModelsOptions::default()));
    models.set_provider(faux.provider.clone());

    let model = faux.get_model(None).expect("model");
    // The faux auth always resolves, so no credential setup is needed.
    let message = models.complete_simple(model, context("hi"), None).await;
    assert_eq!(message.stop_reason, StopReason::Stop);
    let AssistantContent::Text(text) = &message.content[0] else {
        panic!("text")
    };
    assert_eq!(text.text, "through models");
}

#[tokio::test]
async fn custom_model_definitions_are_exposed() {
    let faux = faux_provider(FauxProviderOptions {
        models: Some(vec![
            FauxModelDefinition {
                reasoning: Some(true),
                ..FauxModelDefinition::new("alpha")
            },
            FauxModelDefinition::new("beta"),
        ]),
        ..Default::default()
    });

    assert_eq!(faux.models().len(), 2);
    assert_eq!(
        faux.get_model(None).map(|model| model.id),
        Some("alpha".to_string())
    );
    assert!(faux.get_model(Some("alpha")).expect("alpha").reasoning);
    assert!(faux.get_model(Some("missing")).is_none());
}
