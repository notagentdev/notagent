//! 1:1 port of `packages/coding-agent/src/cli/config-selector.ts` (56 LOC) —
//! the driver that shows workstream A's `ConfigSelectorComponent` for
//! `notagent config`.
//!
//! Like the startup dialogs it runs on the pump seam of interface request A-20:
//! `run_until` renders and pumps while the component waits for its close or
//! exit callback. Where TypeScript resolves a `Promise` from the callback and
//! lets `process.exit(0)` end the run on the exit path, the port sends on a
//! `oneshot` and reports the exit through the return value (deviation class 1,
//! the same shape the interactive mode uses for its own shutdown).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use notagent_tui::terminal::ProcessTerminal;
use notagent_tui::tui::{TuiStopOptions, run_until};
use notagent_tui::tui_main_screen::TuiMainScreen;

use crate::core::settings_manager::SettingsManager;
use crate::modes::interactive::components::config_selector::{
    ConfigSelectorComponent, ConfigWriteScope, ScopedResolvedPaths,
};
use crate::modes::interactive::theme::theme::{init_theme, stop_theme_watcher};

/// `ConfigSelectorOptions`
pub struct ConfigSelectorOptions<'a> {
    pub resolved_paths: ScopedResolvedPaths,
    pub settings_manager: Arc<SettingsManager>,
    pub cwd: &'a str,
    pub agent_dir: &'a str,
    pub write_scope: ConfigWriteScope,
    pub project_mode_available: bool,
}

/// `selectConfig(options)` — returns when the selector closed.
///
/// The `bool` reports which callback ended it: `true` for the exit path, which
/// TypeScript takes straight to `process.exit(0)`.
pub async fn select_config(options: ConfigSelectorOptions<'_>) -> bool {
    init_theme(options.settings_manager.get_theme().as_deref(), true);

    let (terminal, mut pump) = ProcessTerminal::new().into_shared();
    let mut ui = TuiMainScreen::with_options(
        Box::new(terminal),
        None,
        Some(std::path::PathBuf::from(options.agent_dir)),
    );

    let terminal_rows = ui.core().rows();
    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<bool>();
    let done_tx = Rc::new(RefCell::new(Some(done_tx)));
    let close_tx = Rc::clone(&done_tx);
    let exit_tx = Rc::clone(&done_tx);
    let core = ui.core().clone();

    let selector = Rc::new(RefCell::new(ConfigSelectorComponent::new(
        &options.resolved_paths,
        Arc::clone(&options.settings_manager),
        options.cwd,
        options.agent_dir,
        Box::new(move || {
            if let Some(sender) = close_tx.borrow_mut().take() {
                let _ = sender.send(false);
            }
        }),
        Box::new(move || {
            if let Some(sender) = exit_tx.borrow_mut().take() {
                let _ = sender.send(true);
            }
        }),
        Rc::new({
            let core = core.clone();
            move || core.request_render()
        }),
        Some(terminal_rows),
        options.write_scope,
        options.project_mode_available,
    )));
    // `getResourceList()` — the focus target.
    let focus = selector.borrow().resource_list();

    ui.core()
        .add_child(Rc::clone(&selector) as notagent_tui::tui::ComponentRef);
    ui.core().set_focus(Some(focus));
    ui.start();

    let exited = run_until(&mut ui, &mut pump, done_rx)
        .await
        .unwrap_or(false);
    ui.stop(TuiStopOptions::default());
    stop_theme_watcher();
    exited
}
