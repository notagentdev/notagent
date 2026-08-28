use std::cell::RefCell;
use std::rc::Rc;

use notagent_tui::keybindings::set_keybindings;
use notagent_tui::terminal::TerminalPump;
use notagent_tui::tui::TuiStopOptions;

use crate::cli::startup_ui::{StartupTui, create_startup_tui, start_startup_tui};
use crate::core::keybindings::KeybindingsManager;
use crate::core::session_manager::{SessionInfo, SessionManager};
use crate::core::settings_manager::SettingsManager;
use crate::modes::interactive::components::session_selector::{
    LoadRequest, SessionScope, SessionSelectorComponent, SessionSelectorOptions,
};

/// What the picker decided.
pub enum SessionChoice {
    /// The session file to open.
    Selected(String),
    /// Escape: no session was picked.
    Cancelled,
    Exit,
}

/// `selectSession(currentSessionsLoader, allSessionsLoader, settingsManager)`
pub async fn select_session(
    cwd: &str,
    session_dir: Option<&str>,
    settings_manager: &SettingsManager,
) -> SessionChoice {
    let mut tui = create_startup_tui(settings_manager).await;

    let keybindings = Rc::new(RefCell::new(KeybindingsManager::create(None)));
    set_keybindings(keybindings.borrow().to_tui());

    let (done_tx, mut done_rx) = tokio::sync::oneshot::channel::<SessionChoice>();
    let done_tx = Rc::new(RefCell::new(Some(done_tx)));
    let select_tx = Rc::clone(&done_tx);
    let cancel_tx = Rc::clone(&done_tx);
    let exit_tx = Rc::clone(&done_tx);

    let render_core = tui.ui.core().clone();
    let selector = Rc::new(RefCell::new(SessionSelectorComponent::new(
        Box::new(move |path: &str| {
            if let Some(sender) = select_tx.borrow_mut().take() {
                let _ = sender.send(SessionChoice::Selected(path.to_owned()));
            }
        }),
        Box::new(move || {
            if let Some(sender) = cancel_tx.borrow_mut().take() {
                let _ = sender.send(SessionChoice::Cancelled);
            }
        }),
        Box::new(move || {
            if let Some(sender) = exit_tx.borrow_mut().take() {
                let _ = sender.send(SessionChoice::Exit);
            }
        }),
        Rc::new(move || render_core.request_render()),
        SessionSelectorOptions {
            show_rename_hint: Some(false),
            keybindings: Some(Rc::clone(&keybindings)),
            ..SessionSelectorOptions::default()
        },
        None,
    )));

    let session_list = Rc::clone(selector.borrow().get_session_list());
    tui.ui.core().add_child(Rc::clone(&selector) as _);
    tui.ui.core().set_focus(Some(session_list as _));
    start_startup_tui(&mut tui, settings_manager).await;

    let choice = run_picker_loop(&mut tui, &selector, cwd, session_dir, &mut done_rx).await;
    tui.ui.stop(TuiStopOptions::default());
    choice
}

/// The render loop of the picker.
/// It is written out rather than delegated to `run_until` because the picker has
/// a third thing to do: run the session loads the component asks for. They are
/// awaited as their own branch so the list keeps rendering while a scope loads,
async fn run_picker_loop(
    tui: &mut StartupTui,
    selector: &Rc<RefCell<SessionSelectorComponent>>,
    cwd: &str,
    session_dir: Option<&str>,
    done_rx: &mut tokio::sync::oneshot::Receiver<SessionChoice>,
) -> SessionChoice {
    let core = tui.ui.core().clone();
    let mut running_load: Option<RunningLoad> = None;

    loop {
        if running_load.is_none()
            && let Some(request) = selector.borrow_mut().take_pending_load()
        {
            let load = load_scope(request.scope, cwd, session_dir);
            running_load = Some((request, load));
        }

        let load_pending = running_load.is_some();
        tokio::select! {
            biased;
            choice = &mut *done_rx => {
                return choice.unwrap_or(SessionChoice::Cancelled);
            }
            () = core.wait_until_render_due() => tui.ui.render_pending_frame(),
            _ = tui.pump.pump() => {}
            sessions = async {
                let (_, load) = running_load.as_mut().expect("guarded by load_pending");
                load.await
            }, if load_pending => {
                let (request, _) = running_load.take().expect("guarded by load_pending");
                selector.borrow_mut().apply_load_result(request, Ok(sessions));
            }
        }
    }
}

/// A load in flight: which request started it, and the work itself.
type RunningLoad = (
    LoadRequest,
    std::pin::Pin<Box<dyn Future<Output = Vec<SessionInfo>>>>,
);

/// The loader behind one scope: `SessionManager.list` or `listAll`.
fn load_scope(
    scope: SessionScope,
    cwd: &str,
    session_dir: Option<&str>,
) -> std::pin::Pin<Box<dyn Future<Output = Vec<SessionInfo>>>> {
    let cwd = cwd.to_owned();
    let session_dir = session_dir.map(str::to_owned);
    Box::pin(async move {
        match scope {
            SessionScope::Current => SessionManager::list(&cwd, session_dir.as_deref(), None).await,
            SessionScope::All => SessionManager::list_all(session_dir.as_deref(), None).await,
        }
    })
}
