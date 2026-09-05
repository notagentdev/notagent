use std::sync::{Arc, Mutex};

use notagent_agent::types::AgentMessage;
use notagent_ai::types::{AssistantContent, ImageContent, StopReason};

use crate::core::agent_session::{ListenerHandle, PromptOptions};
use crate::core::agent_session_runtime::AgentSessionRuntime;
use crate::core::output_guard::{
    flush_raw_stdout, wait_for_raw_stdout_backpressure, write_raw_stdout,
};
use crate::modes::json_event::to_json_event;
use crate::modes::rpc::jsonl::serialize_json_line;
use crate::utils::shell::kill_tracked_detached_children;

/// Output mode of [`run_print_mode`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrintOutputMode {
    Text,
    Json,
}

#[derive(Debug, Clone, Default)]
pub struct PrintModeOptions {
    /// `text` prints the final response, `json` prints every event.
    pub mode: Option<PrintOutputMode>,
    /// Prompts sent after `initial_message`.
    pub messages: Vec<String>,
    pub initial_message: Option<String>,
    pub initial_images: Option<Vec<ImageContent>>,
}

/// What the current session subscription consists of; replaced on every rebind.
#[derive(Default)]
struct Subscriptions {
    events: Option<ListenerHandle>,
    backpressure: Option<Box<dyn FnOnce() + Send>>,
}

impl Subscriptions {
    fn clear(&mut self) {
        self.events = None;
        if let Some(unsubscribe) = self.backpressure.take() {
            unsubscribe();
        }
    }
}

fn rebind(
    runtime_host: &AgentSessionRuntime,
    mode: PrintOutputMode,
    subscriptions: &Arc<Mutex<Subscriptions>>,
) {
    let session = runtime_host.session();
    let mut current = subscriptions.lock().expect("poisoned");
    current.clear();
    if mode == PrintOutputMode::Json {
        current.events = Some(session.subscribe(Arc::new(move |event| {
            write_raw_stdout(&serialize_json_line(&to_json_event(&event)));
        })));
        let unsubscribe = session.agent().subscribe(Arc::new(move |_event, _signal| {
            Box::pin(async move {
                wait_for_raw_stdout_backpressure().await;
            })
        }));
        current.backpressure = Some(Box::new(unsubscribe));
    } else {
        current.events = Some(session.subscribe(Arc::new(move |event| {
            if let crate::core::agent_session::AgentSessionEvent::PersistenceError {
                error_message,
            } = event
            {
                eprintln!("{error_message}");
            }
        })));
    }
}

/// Runs one non-interactive turn set and returns the process exit code.
pub async fn run_print_mode(
    runtime_host: Arc<AgentSessionRuntime>,
    options: PrintModeOptions,
) -> i32 {
    let mode = options.mode.unwrap_or(PrintOutputMode::Text);
    let subscriptions: Arc<Mutex<Subscriptions>> = Arc::new(Mutex::new(Subscriptions::default()));

    // SIGTERM and SIGHUP must leave no detached child behind; the exit codes are
    // the shell's convention for a process killed by that signal.
    let signal_host = Arc::clone(&runtime_host);
    let signal_task = tokio::spawn(async move {
        let Ok(mut terminate) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        else {
            return;
        };
        let Ok(mut hangup) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
        else {
            return;
        };
        let exit_code = tokio::select! {
            _ = terminate.recv() => 143,
            _ = hangup.recv() => 129,
        };
        kill_tracked_detached_children();
        signal_host.dispose().await;
        flush_raw_stdout().await;
        std::process::exit(exit_code);
    });

    // A `/new` or `/resume` inside a print run replaces the session; the
    // subscription has to follow it or the rest of the run would be silent.
    {
        let subscriptions = Arc::clone(&subscriptions);
        let rebind_host = Arc::clone(&runtime_host);
        runtime_host.set_rebind_session(Some(Arc::new(move |_session| {
            let subscriptions = Arc::clone(&subscriptions);
            let rebind_host = Arc::clone(&rebind_host);
            Box::pin(async move {
                rebind(&rebind_host, mode, &subscriptions);
            })
        })));
    }

    let exit_code = run(&runtime_host, mode, &options, &subscriptions).await;

    signal_task.abort();
    runtime_host.set_rebind_session(None);
    subscriptions.lock().expect("poisoned").clear();
    runtime_host.dispose().await;
    flush_raw_stdout().await;
    exit_code
}

async fn run(
    runtime_host: &Arc<AgentSessionRuntime>,
    mode: PrintOutputMode,
    options: &PrintModeOptions,
    subscriptions: &Arc<Mutex<Subscriptions>>,
) -> i32 {
    if mode == PrintOutputMode::Json {
        let header = runtime_host
            .session()
            .with_session_manager(|session_manager| session_manager.get_header().cloned());
        if let Some(header) = header {
            let mut ordered = serde_json::Map::new();
            ordered.insert("type".to_owned(), serde_json::Value::from("session"));
            if let Ok(serde_json::Value::Object(mut map)) = serde_json::to_value(&header) {
                ordered.append(&mut map);
            }
            write_raw_stdout(&serialize_json_line(&serde_json::Value::Object(ordered)));
        }
    }

    rebind(runtime_host, mode, subscriptions);

    if let Some(initial_message) = options.initial_message.as_deref() {
        let session = runtime_host.session();
        if let Err(error) = session
            .prompt(
                initial_message,
                PromptOptions {
                    images: options.initial_images.clone().unwrap_or_default(),
                    ..PromptOptions::default()
                },
            )
            .await
        {
            eprintln!("{error}");
            return 1;
        }
    }

    for message in &options.messages {
        let session = runtime_host.session();
        if let Err(error) = session.prompt(message, PromptOptions::default()).await {
            eprintln!("{error}");
            return 1;
        }
    }

    if mode == PrintOutputMode::Text {
        let session = runtime_host.session();
        let messages = session.state().messages;
        if let Some(AgentMessage::Assistant(assistant)) = messages.last() {
            if assistant.stop_reason == StopReason::Error
                || assistant.stop_reason == StopReason::Aborted
            {
                let reason = if assistant.stop_reason == StopReason::Error {
                    "error"
                } else {
                    "aborted"
                };
                let message = assistant
                    .error_message
                    .clone()
                    .filter(|text| !text.is_empty())
                    .unwrap_or_else(|| format!("Request {reason}"));
                eprintln!("{message}");
                return 1;
            }
            for content in &assistant.content {
                if let AssistantContent::Text(text) = content {
                    write_raw_stdout(&format!("{}\n", text.text));
                }
            }
        }
    }

    0
}
