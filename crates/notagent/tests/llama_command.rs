//! The `/llama` command and its manager UI — the half of
//! `packages/coding-agent/src/extensions/llama/` that stayed with the app
//! workstream (`ui.ts`, `index.ts`), plus the first case of
//! `packages/coding-agent/test/llama-extension.test.ts` ("registers a native
//! provider and /llama command"), which `tests/llama_extension.rs` leaves out
//! because it drives the extension loader.
//!
//! The dialogs answer over a channel instead of resolving promises, so the
//! cases below install a dialog, feed keys to the view and read the answer.

#[path = "support/llama_server.rs"]
mod llama_server;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use notagent::core::keybindings::KeybindingsManager;
use notagent::core::llama::client::{LlamaModelInfo, LlamaProgress};
use notagent::core::llama::huggingface::HuggingFaceModel;
use notagent::modes::interactive::components::llama::{
    LlamaAnswer, LlamaManagerAction, LlamaView, ProgressState,
};
use notagent::modes::interactive::llama_command::LLAMA_COMMAND_DESCRIPTION;
use notagent::modes::interactive::theme::theme::init_theme;
use notagent::utils::ansi::strip_ansi;
use notagent_tui::keybindings::set_keybindings;
use notagent_tui::tui::{Component, Focusable};
use serde_json::json;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

/// The theme and the keybindings registry are process globals; the cases here
/// await the flow while holding the lock, so it is the async mutex.
async fn test_setup() -> tokio::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    let guard = LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    init_theme(Some("dark"), false);
    set_keybindings(KeybindingsManager::default().to_tui());
    guard
}

/// Down arrow — `tui.select.down`.
const DOWN: &str = "\x1b[B";
/// Enter — `tui.select.confirm`.
const ENTER: &str = "\r";
/// Escape — `tui.select.cancel`.
const ESCAPE: &str = "\x1b";

fn model(id: &str, status: &str) -> LlamaModelInfo {
    serde_json::from_value(json!({ "id": id, "status": { "value": status } })).expect("model")
}

fn view() -> (
    Rc<RefCell<LlamaView>>,
    UnboundedReceiver<(u64, LlamaAnswer)>,
) {
    let (answers, receiver) = unbounded_channel();
    let view = Rc::new(RefCell::new(LlamaView::new(answers, Rc::new(|| {}))));
    (view, receiver)
}

fn render(view: &Rc<RefCell<LlamaView>>) -> String {
    strip_ansi(&view.borrow_mut().render(80).join("\n"))
}

// ============================================================================
// The model list
// ============================================================================

#[tokio::test]
async fn the_model_list_puts_loaded_models_first_and_describes_their_state() {
    let _guard = test_setup().await;
    let (view, _answers) = view();
    let catalog = vec![
        model("zeta", "unloaded"),
        model("alpha", "unloaded"),
        serde_json::from_value(json!({
            "id": "loaded-one",
            "status": { "value": "loaded", "args": ["llama-server", "--ctx-size", "65536"] },
        }))
        .expect("model"),
        model("beta", "downloading"),
        model("napping", "sleeping"),
    ];
    view.borrow_mut()
        .show_models("http://127.0.0.1:8080", &catalog);
    let screen = render(&view);
    let rows: Vec<&str> = screen
        .lines()
        .filter(|line| {
            ["loaded-one", "napping", "alpha", "beta", "zeta"]
                .iter()
                .any(|id| line.contains(id))
        })
        .collect();
    assert!(
        rows[0].contains("loaded-one"),
        "the loaded model is first: {screen}"
    );
    assert!(
        rows[0].contains("loaded") && rows[0].contains("66k context"),
        "a loaded row carries its state and its context size: {screen}"
    );
    let order: Vec<&str> = rows[1..].to_vec();
    assert!(
        order[0].contains("alpha") && order[1].contains("beta") && order[2].contains("napping"),
        "the rest is sorted by id: {screen}"
    );
    assert!(
        order[1].contains("downloading"),
        "a downloading model shows its state: {screen}"
    );
    assert!(
        order[2].contains("loaded"),
        "a sleeping model counts as loaded: {screen}"
    );
    assert!(
        screen.contains("Download model…") && screen.contains("owner/repository[:quant]"),
        "the download row closes the list: {screen}"
    );
    assert!(
        screen.contains("http://127.0.0.1:8080"),
        "the server URL is the subtitle: {screen}"
    );
}

#[tokio::test]
async fn the_list_answers_with_the_chosen_model_the_download_row_and_the_cancellation() {
    let _guard = test_setup().await;
    let (view, mut answers) = view();
    let catalog = vec![model("alpha", "unloaded"), model("beta", "unloaded")];

    let seq = view.borrow_mut().show_models("http://server", &catalog);
    view.borrow_mut().handle_input(ENTER);
    assert_eq!(
        answers.recv().await,
        Some((
            seq,
            LlamaAnswer::Models(LlamaManagerAction::Model(Box::new(model(
                "alpha", "unloaded"
            ))))
        ))
    );

    let seq = view.borrow_mut().show_models("http://server", &catalog);
    view.borrow_mut().handle_input(DOWN);
    view.borrow_mut().handle_input(DOWN);
    view.borrow_mut().handle_input(ENTER);
    assert_eq!(
        answers.recv().await,
        Some((seq, LlamaAnswer::Models(LlamaManagerAction::Download)))
    );

    let seq = view.borrow_mut().show_models("http://server", &catalog);
    view.borrow_mut().handle_input(ESCAPE);
    assert_eq!(
        answers.recv().await,
        Some((seq, LlamaAnswer::Models(LlamaManagerAction::Close)))
    );
}

#[tokio::test]
async fn a_select_dialog_answers_with_the_option_or_with_nothing() {
    let _guard = test_setup().await;
    let (view, mut answers) = view();
    let options = vec!["Yes".to_owned(), "No".to_owned()];

    let seq = view.borrow_mut().select("Unload model?\nalpha", &options);
    let screen = render(&view);
    assert!(
        screen.contains("Unload model?") && screen.contains("alpha"),
        "the two title lines are shown: {screen}"
    );
    view.borrow_mut().handle_input(ENTER);
    assert_eq!(
        answers.recv().await,
        Some((seq, LlamaAnswer::Select(Some("Yes".to_owned()))))
    );

    let seq = view.borrow_mut().select("Unload model?\nalpha", &options);
    view.borrow_mut().handle_input(ESCAPE);
    assert_eq!(answers.recv().await, Some((seq, LlamaAnswer::Select(None))));
}

// ============================================================================
// Progress
// ============================================================================

#[tokio::test]
async fn the_progress_view_draws_the_bar_and_stops_on_escape() {
    let _guard = test_setup().await;
    let (view, mut answers) = view();
    let mut state = ProgressState {
        title: "Downloading model".to_owned(),
        model: "owner/repo:Q4_K_M".to_owned(),
        message: "Starting…".to_owned(),
        ratio: None,
        detail: None,
    };
    let seq = view.borrow_mut().progress(&state);
    let screen = render(&view);
    assert!(
        screen.contains("Downloading model")
            && screen.contains("owner/repo:Q4_K_M")
            && screen.contains("Starting…"),
        "title, model and message: {screen}"
    );
    assert!(!screen.contains('█'), "no bar without a ratio: {screen}");

    state.apply(LlamaProgress {
        message: "Downloading model".to_owned(),
        ratio: Some(0.25),
        detail: Some("1.00 KiB / 4.00 KiB".to_owned()),
    });
    view.borrow_mut().update_progress(&state);
    let screen = render(&view);
    assert!(
        screen.contains(&format!("{}{} 25%", "█".repeat(10), "─".repeat(30))),
        "a quarter of the forty cells is filled: {screen}"
    );
    assert!(
        screen.contains("1.00 KiB / 4.00 KiB"),
        "the detail line is below the bar: {screen}"
    );

    // A progress view that is showing keeps the same promise.
    assert_eq!(view.borrow_mut().progress(&state), seq);
    view.borrow_mut().handle_input(ESCAPE);
    assert_eq!(answers.recv().await, Some((seq, LlamaAnswer::ProgressStop)));

    // The promise resolves once; a second escape has nobody to answer.
    view.borrow_mut().handle_input(ESCAPE);
    assert!(answers.try_recv().is_err(), "no second answer");
}

// ============================================================================
// The Hugging Face search
// ============================================================================

#[tokio::test]
async fn the_search_waits_for_two_characters_debounces_and_filters_the_results() {
    let _guard = test_setup().await;
    let (view, mut answers) = view();
    let seq = view.borrow_mut().search_models();
    let search = view.borrow().search().expect("the search is showing");
    assert!(
        render(&view).contains("Type at least 2 characters"),
        "the empty search says what it needs: {}",
        render(&view)
    );

    view.borrow_mut().handle_input("q");
    assert!(
        search.borrow().search_deadline().is_none(),
        "one character does not schedule a request"
    );
    view.borrow_mut().handle_input("w");
    let deadline = search
        .borrow()
        .search_deadline()
        .expect("the second character schedules the request");
    assert!(
        deadline > Instant::now() + Duration::from_millis(400),
        "the request waits out the debounce"
    );
    assert!(
        render(&view).contains("Searching Hugging Face…"),
        "the status says a request is coming: {}",
        render(&view)
    );
    assert!(
        search.borrow_mut().take_due_search().is_none(),
        "nothing is due before the debounce elapses"
    );

    tokio::time::sleep(Duration::from_millis(520)).await;
    let (query, token) = search
        .borrow_mut()
        .take_due_search()
        .expect("the request is due");
    assert_eq!(query, "qw");
    assert!(!token.is_cancelled());

    search.borrow_mut().apply_search_result(
        "qw",
        Ok(vec![
            HuggingFaceModel {
                id: "owner/qwen-gguf".to_owned(),
                downloads: 1_500_000.0,
            },
            HuggingFaceModel {
                id: "owner/other".to_owned(),
                downloads: 10.0,
            },
            HuggingFaceModel {
                id: "owner/qwen-small".to_owned(),
                downloads: 900.0,
            },
        ]),
    );
    let screen = render(&view);
    assert!(
        screen.contains("owner/qwen-gguf") && screen.contains("1.5M downloads"),
        "the results carry their download counts: {screen}"
    );
    assert!(
        screen.contains("900 downloads"),
        "counts below a thousand stay plain: {screen}"
    );
    assert!(
        !screen.contains("owner/other"),
        "the query also filters the results it just got: {screen}"
    );

    // A cached query answers without scheduling a new request.
    view.borrow_mut().handle_input("\x7f");
    view.borrow_mut().handle_input("w");
    assert!(
        search.borrow().search_deadline().is_none(),
        "the cache answers the repeated query"
    );

    view.borrow_mut().handle_input(ENTER);
    assert_eq!(
        answers.recv().await,
        Some((seq, LlamaAnswer::Search(Some("owner/qwen-gguf".to_owned()))))
    );
}

#[tokio::test]
async fn an_exact_repository_wins_over_the_highlighted_result() {
    let _guard = test_setup().await;
    let (view, mut answers) = view();
    let seq = view.borrow_mut().search_models();
    let search = view.borrow().search().expect("the search is showing");
    view.borrow_mut().handle_input("owner/repo:Q4_K_M");
    search
        .borrow_mut()
        .apply_search_result("owner/repo:Q4_K_M", Ok(Vec::new()));
    view.borrow_mut().handle_input(ENTER);
    assert_eq!(
        answers.recv().await,
        Some((
            seq,
            LlamaAnswer::Search(Some("owner/repo:Q4_K_M".to_owned()))
        ))
    );
}

#[tokio::test]
async fn a_failed_search_shows_its_message_and_escape_goes_back() {
    let _guard = test_setup().await;
    let (view, mut answers) = view();
    let seq = view.borrow_mut().search_models();
    let search = view.borrow().search().expect("the search is showing");
    view.borrow_mut().handle_input("qw");
    tokio::time::sleep(Duration::from_millis(520)).await;
    let (query, _token) = search
        .borrow_mut()
        .take_due_search()
        .expect("the request is due");
    search
        .borrow_mut()
        .apply_search_result(&query, Err("Hugging Face returned HTTP 500".to_owned()));
    assert!(
        render(&view).contains("Hugging Face returned HTTP 500"),
        "the error takes the place of the status: {}",
        render(&view)
    );
    view.borrow_mut().handle_input(ESCAPE);
    assert_eq!(answers.recv().await, Some((seq, LlamaAnswer::Search(None))));
}

#[tokio::test]
async fn the_view_hands_the_focus_to_the_dialog_that_takes_input() {
    let _guard = test_setup().await;
    let (view, _answers) = view();
    view.borrow_mut().set_focused(true);
    view.borrow_mut().search_models();
    let search = view.borrow().search().expect("the search is showing");
    assert!(
        search.borrow().focused(),
        "the search input follows the view's focus"
    );
    view.borrow_mut()
        .show_status("Loading model details", "owner/repo");
    assert!(
        !search.borrow().focused(),
        "the replaced dialog loses the focus"
    );
    assert!(
        render(&view).contains("Loading model details") && render(&view).contains("owner/repo"),
        "the status view shows title and message: {}",
        render(&view)
    );
}

#[test]
fn the_command_keeps_the_description_of_the_typescript_registration() {
    assert_eq!(LLAMA_COMMAND_DESCRIPTION, "Manage llama.cpp router models");
}

// ============================================================================
// The flow of `index.ts` against a llama.cpp server
// ============================================================================

use std::sync::Mutex;

use llama_server::{Reply, TestHttpServer};
use notagent::core::auth_storage::AuthStorage;
use notagent::core::llama::client::LlamaClient;
use notagent::core::llama::provider::{LLAMA_PROVIDER_ID, create_llama_provider};
use notagent::core::model_registry::ModelRegistry;
use notagent::core::model_runtime::{CreateModelRuntimeOptions, ModelRuntime};
use notagent::core::models_store::InMemoryCodingAgentModelsStore;
use notagent::modes::interactive::llama_command::{
    LlamaCommand, Notify, NotifyLevel, create_llama_ui, run_llama_command,
};
use notagent_ai::models::Provider;

async fn local<F: std::future::Future>(body: F) -> F::Output {
    tokio::task::LocalSet::new().run_until(body).await
}

/// What `main.ts` builds for the llama provider, reduced to one provider: the
/// credential with its `LLAMA_BASE_URL`, and the natively registered provider.
async fn llama_command(
    base_url: &str,
    notes: Arc<Mutex<Vec<(String, NotifyLevel)>>>,
) -> LlamaCommand {
    let stored = json!({
        "llama.cpp": { "type": "api_key", "key": "local", "env": { "LLAMA_BASE_URL": base_url } }
    })
    .as_object()
    .cloned()
    .expect("credentials");
    let runtime = ModelRuntime::create(CreateModelRuntimeOptions {
        credentials: Some(Arc::new(AuthStorage::in_memory(stored))),
        models_path: Some(None),
        models_store: Some(Arc::new(InMemoryCodingAgentModelsStore::new())),
        refresh_on_create: Some(false),
        ..CreateModelRuntimeOptions::default()
    })
    .await
    .expect("model runtime");
    let provider = Arc::new(create_llama_provider());
    runtime
        .register_native_provider(Arc::clone(&provider.provider) as Arc<dyn Provider>)
        .expect("register the llama provider");
    let notify: Notify = Rc::new(move |message: &str, level: NotifyLevel| {
        notes
            .lock()
            .expect("poisoned")
            .push((message.to_owned(), level));
    });
    LlamaCommand {
        client: LlamaClient::new(base_url, Some("local")).expect("client"),
        provider,
        registry: ModelRegistry::new(runtime),
        notify,
    }
}

/// Run the flow until the screen shows `needle`, then send `keys` to it.
///
/// The needle has to be text only the awaited dialog shows: consecutive dialogs
/// of the manager share model ids and titles, and a key that reaches the
/// previous dialog answers a sequence number the flow no longer waits for —
/// which would park the flow instead of failing the case.
async fn answer(view: &Rc<RefCell<LlamaView>>, needle: &str, keys: &[&str]) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if render(view).contains(needle) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "{needle:?} never appeared; screen was:\n{}",
            render(view)
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    for key in keys {
        view.borrow_mut().handle_input(key);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Only the model list offers the download row, so it is the needle that tells
/// the list apart from the dialogs the flow shows in between.
const MODEL_LIST: &str = "Download model…";

/// A whole case never waits longer than this. Every wait below has its own
/// deadline; this is the backstop for the flow itself, so a case can only fail,
/// never park a workspace run.
const CASE_TIMEOUT: Duration = Duration::from_secs(60);

async fn within_case<F: Future>(body: F) -> F::Output {
    tokio::time::timeout(CASE_TIMEOUT, body)
        .await
        .expect("the case finished inside its timeout")
}

#[tokio::test(flavor = "current_thread")]
async fn the_manager_loads_a_model_refreshes_the_catalog_and_reports_it() {
    local(within_case(async {
        let _guard = test_setup().await;
        let status = Arc::new(Mutex::new("unloaded".to_owned()));
        let server = {
            let status = Arc::clone(&status);
            TestHttpServer::start(move |request| {
                let path = request
                    .path
                    .split('?')
                    .next()
                    .unwrap_or_default()
                    .to_owned();
                match (request.method.as_str(), path.as_str()) {
                    (_, "/models/sse") => Reply::Sse,
                    ("POST", "/models/load") => {
                        *status.lock().expect("poisoned") = "loaded".to_owned();
                        Reply::json(json!({ "success": true }))
                    }
                    ("GET", "/models") => Reply::json(json!({
                        "data": [
                            {
                                "id": "test-model",
                                "status": { "value": *status.lock().expect("poisoned") },
                                "meta": { "n_ctx": 32768 },
                            },
                        ]
                    })),
                    _ => Reply::status(404),
                }
            })
            .await
        };

        let notes = Arc::new(Mutex::new(Vec::new()));
        let command = llama_command(&server.base_url, Arc::clone(&notes)).await;
        let (view, mut ui) = create_llama_ui(Rc::new(|| {}));
        let flow = tokio::task::spawn_local(async move {
            let outcome = run_llama_command(&command, &mut ui).await;
            (outcome, command_models(&command))
        });

        // Pick the model. The list is back once the load finished, which is the
        // download row showing again — until then the progress view is up.
        answer(&view, MODEL_LIST, &[ENTER]).await;
        answer(&view, MODEL_LIST, &[ESCAPE]).await;
        let (outcome, models) = flow.await.expect("the flow finished");
        assert_eq!(outcome, Ok(()));
        assert_eq!(
            *notes.lock().expect("poisoned"),
            vec![("Loaded test-model".to_owned(), NotifyLevel::Info)]
        );
        assert_eq!(
            models,
            vec![(
                "test-model".to_owned(),
                format!("{}/v1", server.base_url),
                32768_u64
            )],
            "the loaded model reached the provider catalog"
        );
    }))
    .await;
}

/// The provider's published catalog, as `/model` would see it.
fn command_models(command: &LlamaCommand) -> Vec<(String, String, u64)> {
    command
        .registry
        .get_provider(LLAMA_PROVIDER_ID)
        .expect("the llama provider is registered")
        .get_models()
        .into_iter()
        .map(|model| (model.id, model.base_url, model.context_window))
        .collect()
}

#[tokio::test(flavor = "current_thread")]
async fn a_server_that_is_not_there_reports_the_connection_and_closes() {
    local(within_case(async {
        let _guard = test_setup().await;
        let notes = Arc::new(Mutex::new(Vec::new()));
        // Port 1 on loopback: nothing listens, every request fails at once.
        let command = llama_command("http://127.0.0.1:1", Arc::clone(&notes)).await;
        let (view, mut ui) = create_llama_ui(Rc::new(|| {}));
        let flow =
            tokio::task::spawn_local(async move { run_llama_command(&command, &mut ui).await });

        answer(&view, "llama.cpp unavailable", &[]).await;
        let screen = render(&view);
        assert!(
            screen.contains("Could not connect to the server."),
            "a connection failure is reported as one: {screen}"
        );
        assert!(
            screen.contains("Retry") && screen.contains("Close"),
            "both answers are offered: {screen}"
        );
        // Close is the second row.
        view.borrow_mut().handle_input(DOWN);
        view.borrow_mut().handle_input(ENTER);
        assert_eq!(flow.await.expect("the flow finished"), Ok(()));
        assert!(
            notes.lock().expect("poisoned").is_empty(),
            "a connection error is not repeated as a notification"
        );
    }))
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn retry_reads_the_catalog_again_after_a_server_error() {
    local(within_case(async {
        let _guard = test_setup().await;
        let attempts = Arc::new(Mutex::new(0_usize));
        let server = {
            let attempts = Arc::clone(&attempts);
            TestHttpServer::start(move |request| {
                let path = request
                    .path
                    .split('?')
                    .next()
                    .unwrap_or_default()
                    .to_owned();
                match path.as_str() {
                    "/models/sse" => Reply::Sse,
                    "/models" => {
                        let mut attempts = attempts.lock().expect("poisoned");
                        *attempts += 1;
                        if *attempts == 1 {
                            return Reply::status(503);
                        }
                        Reply::json(json!({
                            "data": [{ "id": "test-model", "status": { "value": "unloaded" } }]
                        }))
                    }
                    _ => Reply::status(404),
                }
            })
            .await
        };

        let notes = Arc::new(Mutex::new(Vec::new()));
        let command = llama_command(&server.base_url, Arc::clone(&notes)).await;
        let (view, mut ui) = create_llama_ui(Rc::new(|| {}));
        let flow =
            tokio::task::spawn_local(async move { run_llama_command(&command, &mut ui).await });

        // A server error is not a connection error: its message is shown as is.
        answer(&view, "llama.cpp unavailable", &[]).await;
        assert!(
            render(&view).contains("llama.cpp returned HTTP 503"),
            "the server's own message survives: {}",
            render(&view)
        );
        // Retry is the first row; the second read succeeds and the list opens.
        view.borrow_mut().handle_input(ENTER);
        answer(&view, MODEL_LIST, &[ESCAPE]).await;
        assert_eq!(flow.await.expect("the flow finished"), Ok(()));
    }))
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn unloading_asks_first_and_only_unloads_after_a_yes() {
    local(within_case(async {
        let _guard = test_setup().await;
        let unloads = Arc::new(Mutex::new(0_usize));
        let server = {
            let unloads = Arc::clone(&unloads);
            TestHttpServer::start(move |request| {
                let path = request
                    .path
                    .split('?')
                    .next()
                    .unwrap_or_default()
                    .to_owned();
                match (request.method.as_str(), path.as_str()) {
                    (_, "/models/sse") => Reply::Sse,
                    ("POST", "/models/unload") => {
                        *unloads.lock().expect("poisoned") += 1;
                        Reply::json(json!({ "success": true }))
                    }
                    ("GET", "/models") => Reply::json(json!({
                        "data": [{
                            "id": "test-model",
                            "status": {
                                "value": if *unloads.lock().expect("poisoned") == 0 {
                                    "loaded"
                                } else {
                                    "unloaded"
                                }
                            },
                        }]
                    })),
                    _ => Reply::status(404),
                }
            })
            .await
        };

        let notes = Arc::new(Mutex::new(Vec::new()));
        let command = llama_command(&server.base_url, Arc::clone(&notes)).await;
        let (view, mut ui) = create_llama_ui(Rc::new(|| {}));
        let flow =
            tokio::task::spawn_local(async move { run_llama_command(&command, &mut ui).await });

        // Pick the loaded model, then decline the unload.
        answer(&view, MODEL_LIST, &[ENTER]).await;
        answer(&view, "Unload model?", &[DOWN, ENTER]).await;
        answer(&view, MODEL_LIST, &[]).await;
        assert_eq!(*unloads.lock().expect("poisoned"), 0, "No means no");

        // Pick it again and confirm.
        answer(&view, MODEL_LIST, &[ENTER]).await;
        answer(&view, "Unload model?", &[ENTER]).await;
        answer(&view, MODEL_LIST, &[ESCAPE]).await;
        assert_eq!(flow.await.expect("the flow finished"), Ok(()));
        assert_eq!(*unloads.lock().expect("poisoned"), 1);
        assert_eq!(
            *notes.lock().expect("poisoned"),
            vec![("Unloaded test-model".to_owned(), NotifyLevel::Info)]
        );
    }))
    .await;
}
