//! 1:1 port of `packages/coding-agent/src/extensions/llama/index.ts` (228 LOC).
//!
//! The `/llama` command: it reads the router catalog of the configured
//! llama.cpp server, refreshes the provider from it, and loads, unloads or
//! downloads a model.
//!
//! Deviation (class 2): the file is the llama extension's factory in
//! TypeScript. With the extension system gone
//! (`plans/facts/extension-boundary.md` §2.3) `registerProvider` becomes the
//! native registration in `core/agent_session_services.rs` and
//! `registerCommand("llama", …)` becomes a command of the interactive mode's
//! own table. The `ctx.mode !== "tui"` guard goes with it: without the
//! extension command dispatcher there is nothing that would run `/llama` in
//! print or RPC mode, where the text stays an ordinary prompt.
//!
//! Deviation (class 1, structural): the dialogs of `ui.ts` answer over a
//! channel instead of resolving promises (see
//! [`super::components::llama`]); [`LlamaUi`] is the flow's side of it and
//! turns each answer back into an awaited value, so the control flow below
//! stays the one TypeScript has.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use notagent_ai::models::ModelsRefreshOptions;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use tokio_util::sync::CancellationToken;

use crate::core::llama::client::{
    LLAMA_STATUS_LOADED, LLAMA_STATUS_SLEEPING, LLAMA_STATUS_UNLOADED, LlamaClient, LlamaError,
    LlamaModelInfo, LlamaProgress, ProgressFn, format_bytes, normalize_llama_server_url,
};
use crate::core::llama::huggingface::{
    HuggingFaceClient, HuggingFaceGated, HuggingFaceModel, find_hugging_face_token_from_process_env,
};
use crate::core::llama::provider::{LLAMA_PROVIDER_ID, LlamaProviderController};
use crate::core::model_registry::ModelRegistry;
use crate::utils::abort::timeout_signal;

use super::components::llama::{LlamaAnswer, LlamaManagerAction, LlamaView, ProgressState};

/// `description` of the registered command.
pub const LLAMA_COMMAND_DESCRIPTION: &str = "Manage llama.cpp router models";

/// `ctx.ui.notify(message, type)`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyLevel {
    Info,
    Warning,
    Error,
}

/// The sink of `ctx.ui.notify`.
pub type Notify = Rc<dyn Fn(&str, NotifyLevel)>;

/// The awaited `search(query, signal)` of `ui.searchModels`.
type SearchFuture<'a> =
    std::pin::Pin<Box<dyn Future<Output = Result<Vec<HuggingFaceModel>, LlamaError>> + 'a>>;

/// `modelIsLoaded(model)`
fn model_is_loaded(model: &LlamaModelInfo) -> bool {
    model.status.value == LLAMA_STATUS_LOADED || model.status.value == LLAMA_STATUS_SLEEPING
}

/// `isConnectionError(error)`
///
/// Deviation (class 3): TypeScript matches the wording of `node:fetch`
/// (`fetch failed`, plus `timeout`/`network` for the messages a server or a
/// DNS layer produces). `reqwest` reports every unreachable host, refused
/// connection, DNS failure and exceeded deadline as
/// `error sending request for url (…)`, so that wording joins the list; the
/// three original substrings stay, because a server-sent message can carry
/// them.
fn is_connection_error(message: &str) -> bool {
    let message = message.to_lowercase();
    message.contains("fetch failed")
        || message.contains("timeout")
        || message.contains("network")
        || message.contains("error sending request")
}

/// `connectionErrorMessage(error)`
fn connection_error_message(message: &str) -> String {
    if is_connection_error(message) {
        return "Could not connect to the server.".to_owned();
    }
    message.to_owned()
}

/// `parseHuggingFaceModel(value)`
fn parse_hugging_face_model(value: &str) -> (String, Option<String>) {
    let start = value.find('/').map_or(0, |index| index + 1);
    match value[start..].find(':') {
        Some(offset) => {
            let colon = start + offset;
            (
                value[..colon].to_owned(),
                Some(value[colon + 1..].to_owned()),
            )
        }
        None => (value.to_owned(), None),
    }
}

/// `configuredClient(ctx)`
async fn configured_client(
    registry: &ModelRegistry,
    notify: &Notify,
) -> Result<Option<LlamaClient>, LlamaError> {
    let result = registry
        .get_provider_auth(LLAMA_PROVIDER_ID)
        .await
        .ok()
        .flatten();
    let Some(result) = result else {
        notify(
            &format!("Configure llama.cpp with /login {LLAMA_PROVIDER_ID}"),
            NotifyLevel::Warning,
        );
        return Ok(None);
    };
    let configured_url = result
        .env
        .as_ref()
        .and_then(|env| env.get("LLAMA_BASE_URL"))
        .cloned()
        .filter(|url| !url.is_empty());
    let server_url = normalize_llama_server_url(
        configured_url
            .as_deref()
            .unwrap_or(result.auth.base_url.as_deref().unwrap_or("")),
    )?;
    Ok(Some(LlamaClient::new(
        &server_url,
        result.auth.api_key.as_deref(),
    )?))
}

// ============================================================================
// The dialog side of the flow
// ============================================================================

/// The flow's half of [`LlamaView`]: it installs a dialog and awaits its answer.
pub struct LlamaUi {
    view: Rc<RefCell<LlamaView>>,
    answers: UnboundedReceiver<(u64, LlamaAnswer)>,
}

impl LlamaUi {
    /// The next answer of the dialog `seq`; answers of dialogs that have since
    /// been replaced are dropped, like the promises TypeScript lets go.
    async fn await_answer(&mut self, seq: u64) -> Option<LlamaAnswer> {
        loop {
            let (answered, answer) = self.answers.recv().await?;
            if answered == seq {
                return Some(answer);
            }
        }
    }

    /// `ui.showModels(serverUrl, models)`
    async fn show_models(
        &mut self,
        server_url: &str,
        models: &[LlamaModelInfo],
    ) -> LlamaManagerAction {
        let seq = self.view.borrow_mut().show_models(server_url, models);
        match self.await_answer(seq).await {
            Some(LlamaAnswer::Models(action)) => action,
            _ => LlamaManagerAction::Close,
        }
    }

    /// `ui.select(title, options)`
    async fn select(&mut self, title: &str, options: &[String]) -> Option<String> {
        let seq = self.view.borrow_mut().select(title, options);
        match self.await_answer(seq).await {
            Some(LlamaAnswer::Select(choice)) => choice,
            _ => None,
        }
    }

    /// `ui.confirm(title, message)`
    async fn confirm(&mut self, title: &str, message: &str) -> bool {
        self.select(
            &format!("{title}\n{message}"),
            &["Yes".to_owned(), "No".to_owned()],
        )
        .await
        .as_deref()
            == Some("Yes")
    }

    /// `ui.connectionError(serverUrl, message)` — `true` means retry.
    async fn connection_error(&mut self, server_url: &str, message: &str) -> bool {
        let choice = self
            .select(
                &format!("llama.cpp unavailable\n{server_url}\n\n{message}"),
                &["Retry".to_owned(), "Close".to_owned()],
            )
            .await;
        choice.as_deref() == Some("Retry")
    }

    /// `ui.showStatus(title, message)`
    fn show_status(&mut self, title: &str, message: &str) {
        self.view.borrow_mut().show_status(title, message);
    }

    /// `ui.searchModels(search)` — drives the debounce and the request of the
    /// search component while it waits for the answer.
    async fn search_models(&mut self, hugging_face: &HuggingFaceClient) -> Option<String> {
        let seq = self.view.borrow_mut().search_models();
        let component = self.view.borrow().search()?;
        let mut inflight: Option<(String, SearchFuture<'_>)> = None;
        loop {
            if inflight.is_none()
                && let Some((query, token)) = component.borrow_mut().take_due_search()
            {
                let owned = query.clone();
                inflight = Some((
                    query,
                    Box::pin(async move { hugging_face.search(&owned, Some(&token)).await }),
                ));
            }
            let deadline = component.borrow().search_deadline();
            enum Step {
                Answer(Option<(u64, LlamaAnswer)>),
                Searched(String, Result<Vec<HuggingFaceModel>, LlamaError>),
                Due,
            }
            let step = tokio::select! {
                answer = self.answers.recv() => Step::Answer(answer),
                result = async {
                    let (_, search) = inflight.as_mut().expect("guarded by is_some");
                    search.as_mut().await
                }, if inflight.is_some() => {
                    let (query, _) = inflight.take().expect("guarded by is_some");
                    Step::Searched(query, result)
                }
                () = async {
                    let deadline = deadline.expect("guarded by is_some");
                    tokio::time::sleep_until(deadline.into()).await;
                }, if deadline.is_some() => Step::Due,
            };
            match step {
                Step::Answer(None) => return None,
                Step::Answer(Some((answered, answer))) => {
                    if answered != seq {
                        continue;
                    }
                    return match answer {
                        LlamaAnswer::Search(selected) => selected,
                        _ => None,
                    };
                }
                Step::Searched(query, result) => {
                    component
                        .borrow_mut()
                        .apply_search_result(&query, result.map_err(|error| error.0));
                }
                Step::Due => {}
            }
        }
    }
}

// ============================================================================
// The command
// ============================================================================

/// Everything `/llama` needs beyond the UI.
pub struct LlamaCommand {
    pub client: LlamaClient,
    pub provider: Arc<LlamaProviderController>,
    pub registry: ModelRegistry,
    pub notify: Notify,
}

impl LlamaCommand {
    /// `syncCatalog(ctx, client, catalog?)`
    async fn sync_catalog(
        &self,
        catalog: Option<Vec<LlamaModelInfo>>,
    ) -> Result<Vec<LlamaModelInfo>, String> {
        let signal = timeout_signal(15_000);
        let current = match catalog {
            Some(catalog) => catalog,
            None => self
                .client
                .list(false, Some(&signal))
                .await
                .map_err(|error| error.0)?,
        };
        self.provider
            .set_catalog(&current, &self.client.server_url)
            .map_err(|error| error.0)?;
        let result = self
            .registry
            .refresh(Some(ModelsRefreshOptions {
                providers: Some(vec![LLAMA_PROVIDER_ID.to_owned()]),
                signal: Some(signal),
                ..ModelsRefreshOptions::default()
            }))
            .await;
        if result.aborted {
            return Err("Model catalog refresh timed out.".to_owned());
        }
        if let Some(error) = result.errors.get(LLAMA_PROVIDER_ID) {
            return Err(error.message.clone());
        }
        Ok(current)
    }

    /// `loadModel(ctx, ui, client, catalog, target)`
    async fn load_model(
        &self,
        ui: &mut LlamaUi,
        catalog: &[LlamaModelInfo],
        target: &LlamaModelInfo,
    ) -> Result<(), String> {
        let loaded: Vec<LlamaModelInfo> = catalog
            .iter()
            .filter(|model| model.id != target.id && model_is_loaded(model))
            .cloned()
            .collect();
        let mut replace = false;
        if !loaded.is_empty() {
            let title = format!(
                "{} model{} loaded",
                loaded.len(),
                if loaded.len() == 1 { " is" } else { "s are" }
            );
            let choice = ui
                .select(
                    &title,
                    &[
                        "Unload all and load".to_owned(),
                        "Keep loaded and load".to_owned(),
                        "Cancel".to_owned(),
                    ],
                )
                .await;
            match choice.as_deref() {
                None | Some("Cancel") => return Ok(()),
                Some(choice) => replace = choice == "Unload all and load",
            }
        }

        // Outside the `try` in TypeScript: a failure here is not restored.
        if replace {
            for model in &loaded {
                self.client
                    .unload_and_wait(&model.id, None)
                    .await
                    .map_err(|error| error.0)?;
            }
        }

        match self.load_model_body(ui, &loaded, replace, target).await {
            Ok(()) => Ok(()),
            Err(error) => {
                if replace {
                    // `catch { /* Preserve the original load error. */ }`
                    let _ = self.restore_loaded(&loaded).await;
                }
                Err(error)
            }
        }
    }

    /// The `try` block of `loadModel`.
    async fn load_model_body(
        &self,
        ui: &mut LlamaUi,
        loaded: &[LlamaModelInfo],
        replace: bool,
        target: &LlamaModelInfo,
    ) -> Result<(), String> {
        let outcome = self
            .run_with_progress(
                ui,
                ProgressOptions {
                    title: "Loading model".to_owned(),
                    model: target.id.clone(),
                    initial_message: "Starting…".to_owned(),
                    cancel_title: "Stop loading?".to_owned(),
                    cancel_message: target.id.clone(),
                },
                |progress, signal| {
                    let client = self.client.clone();
                    let model = target.id.clone();
                    Box::pin(
                        async move { client.load_and_wait(&model, progress, Some(&signal)).await },
                    )
                },
            )
            .await?;
        if outcome.is_none() {
            if replace {
                self.restore_loaded(loaded).await?;
            }
            return Ok(());
        }
        let refreshed = self.sync_catalog(None).await?;
        let loaded_model = refreshed.iter().find(|model| model.id == target.id);
        let message = if loaded_model.is_some_and(|model| model.status.value == LLAMA_STATUS_LOADED)
        {
            format!("Loaded {}", target.id)
        } else {
            format!("Load started for {}", target.id)
        };
        (self.notify)(&message, NotifyLevel::Info);
        Ok(())
    }

    /// `restoreLoaded()`
    async fn restore_loaded(&self, loaded: &[LlamaModelInfo]) -> Result<(), String> {
        (self.notify)("Restoring previously loaded models", NotifyLevel::Info);
        for model in loaded {
            self.client
                .load_and_wait(&model.id, Arc::new(|_| {}), None)
                .await
                .map_err(|error| error.0)?;
        }
        self.sync_catalog(None).await.map(|_| ())
    }

    /// `unloadModel(ctx, ui, client, model)`
    async fn unload_model(&self, ui: &mut LlamaUi, model: &LlamaModelInfo) -> Result<(), String> {
        if !ui.confirm("Unload model?", &model.id).await {
            return Ok(());
        }
        self.client
            .unload_and_wait(&model.id, None)
            .await
            .map_err(|error| error.0)?;
        self.sync_catalog(None).await?;
        (self.notify)(&format!("Unloaded {}", model.id), NotifyLevel::Info);
        Ok(())
    }

    /// `downloadModel(ctx, ui, client)`
    async fn download_model(&self, ui: &mut LlamaUi) -> Result<(), String> {
        let hugging_face = HuggingFaceClient::new(
            find_hugging_face_token_from_process_env().await.as_deref(),
            None,
        );
        let Some(selected) = ui.search_models(&hugging_face).await else {
            return Ok(());
        };
        let (repository, quantization) = parse_hugging_face_model(&selected);
        ui.show_status("Loading model details", &repository);
        let details = hugging_face
            .details(&repository, None)
            .await
            .map_err(|error| error.0)?;
        if details.gated != HuggingFaceGated::No {
            let approval = if details.gated == HuggingFaceGated::Manual {
                "Manual approval is required"
            } else {
                "Accept the access terms"
            };
            let choice = ui
                .select(
                    &format!(
                        "Hugging Face access required\n{}\n\n{approval} at:\nhttps://huggingface.co/{}\n\nThe llama.cpp server needs HF_TOKEN with access.",
                        details.id, details.id
                    ),
                    &["Continue".to_owned(), "Back".to_owned()],
                )
                .await;
            if choice.as_deref() != Some("Continue") {
                return Ok(());
            }
        }
        let mut quantization = quantization;
        if quantization.is_none() && !details.quantizations.is_empty() {
            let options: Vec<String> = details
                .quantizations
                .iter()
                .map(|entry| {
                    let detail = [
                        entry.size.map(format_bytes),
                        (entry.name == "Q4_K_M").then(|| "recommended".to_owned()),
                    ]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" · ");
                    if detail.is_empty() {
                        entry.name.clone()
                    } else {
                        format!("{} · {detail}", entry.name)
                    }
                })
                .collect();
            let Some(choice) = ui
                .select(&format!("Select quantization\n{}", details.id), &options)
                .await
            else {
                return Ok(());
            };
            quantization = options
                .iter()
                .position(|option| *option == choice)
                .and_then(|index| details.quantizations.get(index))
                .map(|entry| entry.name.clone());
            if quantization.is_none() {
                return Ok(());
            }
        }
        let model = match &quantization {
            Some(quantization) => format!("{}:{quantization}", details.id),
            None => details.id.clone(),
        };
        let outcome = self
            .run_with_progress(
                ui,
                ProgressOptions {
                    title: "Downloading model".to_owned(),
                    model: model.clone(),
                    initial_message: "Starting…".to_owned(),
                    cancel_title: "Stop download?".to_owned(),
                    cancel_message: model.clone(),
                },
                |progress, signal| {
                    let client = self.client.clone();
                    let model = model.clone();
                    Box::pin(async move {
                        client
                            .download_and_wait(&model, progress, Some(&signal))
                            .await
                    })
                },
            )
            .await?;
        let Some(catalog) = outcome else {
            return Ok(());
        };
        self.sync_catalog(Some(catalog)).await?;
        (self.notify)(&format!("Downloaded {model}"), NotifyLevel::Info);
        Ok(())
    }

    /// `runWithProgress(ui, options)` — `Ok(None)` is `{ cancelled: true }`.
    ///
    /// Deviation (class 1): the run is a task, not a future awaited in place.
    /// A JavaScript promise keeps running while the confirmation dialog is up
    /// and `completed` may flip in the meantime; a Rust future would be
    /// suspended for exactly that time, so the run is spawned and
    /// `JoinHandle::is_finished` takes the role of `completed`.
    async fn run_with_progress<T, F>(
        &self,
        ui: &mut LlamaUi,
        options: ProgressOptions,
        run: F,
    ) -> Result<Option<T>, String>
    where
        T: Send + 'static,
        F: FnOnce(
            ProgressFn,
            CancellationToken,
        ) -> std::pin::Pin<Box<dyn Future<Output = Result<T, LlamaError>> + Send>>,
    {
        let controller = CancellationToken::new();
        let mut state = ProgressState {
            title: options.title,
            model: options.model.clone(),
            message: options.initial_message,
            ratio: None,
            detail: None,
        };
        let (progress_tx, mut progress_rx) = unbounded_channel::<LlamaProgress>();
        let on_progress: ProgressFn = Arc::new(move |progress| {
            let _ = progress_tx.send(progress);
        });
        let mut settled = tokio::spawn(run(on_progress, controller.clone()));
        let mut progress_open = true;
        let mut completed: Option<Result<T, LlamaError>> = None;

        while completed.is_none() {
            let seq = ui.view.borrow_mut().progress(&state);
            enum Step<T> {
                Settled(Result<T, LlamaError>),
                Progress(Option<LlamaProgress>),
                Answer(Option<(u64, LlamaAnswer)>),
            }
            let step = tokio::select! {
                result = &mut settled => match result {
                    Ok(result) => Step::Settled(result),
                    Err(error) => Step::Settled(Err(LlamaError(error.to_string()))),
                },
                progress = progress_rx.recv(), if progress_open => Step::Progress(progress),
                answer = ui.answers.recv() => Step::Answer(answer),
            };
            match step {
                Step::Settled(result) => completed = Some(result),
                Step::Progress(None) => progress_open = false,
                Step::Progress(Some(progress)) => {
                    state.apply(progress);
                    ui.view.borrow_mut().update_progress(&state);
                }
                Step::Answer(None) => break,
                Step::Answer(Some((answered, answer))) => {
                    if answered != seq || answer != LlamaAnswer::ProgressStop {
                        continue;
                    }
                    let stop = ui
                        .confirm(&options.cancel_title, &options.cancel_message)
                        .await;
                    if !stop || settled.is_finished() {
                        continue;
                    }
                    // `try { await options.cancel(); } finally { controller.abort(); }`
                    let cancel = self.client.unload(&options.model, None).await;
                    controller.cancel();
                    let _ = (&mut settled).await;
                    cancel.map_err(|error| error.0)?;
                    return Ok(None);
                }
            }
        }

        match completed {
            Some(Ok(value)) => Ok(Some(value)),
            Some(Err(error)) => Err(error.0),
            None => Ok(None),
        }
    }
}

/// The options of `runWithProgress` that are plain data.
struct ProgressOptions {
    title: String,
    model: String,
    initial_message: String,
    cancel_title: String,
    cancel_message: String,
}

/// The view and the flow half of the manager, both fresh.
///
/// `showLlamaUi(ctx, run)` in TypeScript: the caller mounts the returned view
/// through the mode's own `ctx.ui.custom` equivalent and drives
/// [`run_llama_command`] to completion.
pub fn create_llama_ui(request_render: Rc<dyn Fn()>) -> (Rc<RefCell<LlamaView>>, LlamaUi) {
    let (answers_tx, answers_rx) = unbounded_channel();
    let view = Rc::new(RefCell::new(LlamaView::new(answers_tx, request_render)));
    let ui = LlamaUi {
        view: Rc::clone(&view),
        answers: answers_rx,
    };
    (view, ui)
}

/// The `handler` of the registered `llama` command, minus the client setup.
///
/// The body of `showLlamaUi(ctx, async (ui) => { … })`.
pub async fn run_llama_command(command: &LlamaCommand, ui: &mut LlamaUi) -> Result<(), String> {
    let server_url = command.client.server_url.clone();

    let Some(mut catalog) = read_catalog(command, ui, &server_url).await else {
        return Ok(());
    };
    loop {
        let action = ui.show_models(&server_url, &catalog).await;
        if action == LlamaManagerAction::Close {
            return Ok(());
        }
        let mut action_error: Option<String> = None;
        let outcome = match &action {
            LlamaManagerAction::Download => command.download_model(ui).await,
            LlamaManagerAction::Model(model) if model_is_loaded(model) => {
                command.unload_model(ui, model).await
            }
            LlamaManagerAction::Model(model) if model.status.value == LLAMA_STATUS_UNLOADED => {
                command.load_model(ui, &catalog, model).await
            }
            LlamaManagerAction::Model(model) => {
                (command.notify)(
                    &format!("{} is {}", model.id, model.status.value),
                    NotifyLevel::Warning,
                );
                Ok(())
            }
            LlamaManagerAction::Close => Ok(()),
        };
        if let Err(error) = outcome {
            action_error = Some(error);
        }
        let Some(refreshed) = read_catalog(command, ui, &server_url).await else {
            return Ok(());
        };
        catalog = refreshed;
        if let Some(error) = action_error
            && !is_connection_error(&error)
        {
            (command.notify)(&error, NotifyLevel::Error);
        }
    }
}

/// `readCatalog()` — retries until the catalog arrives or the user closes.
async fn read_catalog(
    command: &LlamaCommand,
    ui: &mut LlamaUi,
    server_url: &str,
) -> Option<Vec<LlamaModelInfo>> {
    loop {
        match command.sync_catalog(None).await {
            Ok(catalog) => return Some(catalog),
            Err(error) => {
                if !ui
                    .connection_error(server_url, &connection_error_message(&error))
                    .await
                {
                    return None;
                }
            }
        }
    }
}

/// `configuredClient(ctx)` plus the client the command runs against.
pub async fn llama_client_for_command(
    registry: &ModelRegistry,
    notify: &Notify,
) -> Result<Option<LlamaClient>, String> {
    configured_client(registry, notify)
        .await
        .map_err(|error| error.0)
}
