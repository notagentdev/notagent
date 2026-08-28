use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use notagent_tui::keybindings::set_keybindings;
use notagent_tui::terminal::{ProcessTerminal, ProcessTerminalPump};
use notagent_tui::tui::{TuiStopOptions, component_ref, run_until};
use notagent_tui::tui_main_screen::TuiMainScreen;

use crate::config::{
    APP_NAME, CONFIG_DIR_NAME, PACKAGE_NAME, env_agent_dir, get_agent_dir, get_settings_path,
};
use crate::core::experimental::are_experimental_features_enabled;
use crate::core::keybindings::KeybindingsManager;
use crate::core::package_manager::{DefaultPackageManager, PackageManagerOptions};
use crate::core::resource_loader::ResolvedResource;
use crate::core::settings_manager::{SettingsManager, SettingsManagerCreateOptions};
use crate::modes::interactive::components::first_time_setup::{
    FirstTimeSetupComponent, FirstTimeSetupOptions, FirstTimeSetupResult,
};
use crate::modes::interactive::components::list_selector::ListSelectorComponent;
use crate::modes::interactive::theme::theme::{
    Theme, detect_terminal_background_from_env, detect_terminal_theme_for_auto, init_theme,
    load_theme_from_path, parse_auto_theme_setting, resolve_theme_setting, set_registered_themes,
    set_theme,
};

const OFFICIAL_PACKAGE_NAME: &str = "@notagent/coding-agent";
const OFFICIAL_APP_NAME: &str = "notagent";
const OFFICIAL_CONFIG_DIR_NAME: &str = ".notagent";

/// Whether this build is the official distribution rather than a fork.
/// Deviation (class 1): the arguments are explicit instead of read from the
/// module constants, because a Rust test cannot swap a `const` the way the
pub fn is_official_distribution(package_name: &str, app_name: &str, config_dir_name: &str) -> bool {
    package_name == OFFICIAL_PACKAGE_NAME
        && app_name == OFFICIAL_APP_NAME
        && config_dir_name == OFFICIAL_CONFIG_DIR_NAME
}

/// First-time setup runs when all of these hold:
/// - this is the official distribution (not a fork or rebrand)
/// - experimental features are on (`NOTAGENT_EXPERIMENTAL=1`)
/// - the default agent directory is in use (no override)
/// - setup never completed before (no `settings.json`)
pub fn should_run_first_time_setup(settings_path: Option<&Path>) -> bool {
    if !is_official_distribution(PACKAGE_NAME, APP_NAME, CONFIG_DIR_NAME) {
        return false;
    }
    if !are_experimental_features_enabled() {
        return false;
    }
    if std::env::var(env_agent_dir()).is_ok_and(|value| !value.is_empty()) {
        return false;
    }
    match settings_path {
        Some(settings_path) => !settings_path.exists(),
        None => !get_settings_path().exists(),
    }
}

// ============================================================================
// The dialogs
// ============================================================================

/// A startup dialog's screen together with the pump its loop drives.
/// A-20), and both halves travel together.
pub struct StartupTui {
    pub ui: TuiMainScreen,
    pub pump: ProcessTerminalPump,
}

/// `loadThemes(resources)`
fn load_themes(resources: &[ResolvedResource]) -> Vec<Theme> {
    let mut themes = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for resource in resources {
        if !resource.enabled {
            continue;
        }
        // A broken theme must not stop a startup prompt; the normal resource
        // loader reports theme diagnostics later in startup.
        let Ok(loaded_theme) = load_theme_from_path(Path::new(&resource.path), None) else {
            continue;
        };
        if let Some(name) = &loaded_theme.name {
            if seen.contains(name) {
                continue;
            }
            seen.push(name.clone());
        }
        themes.push(loaded_theme);
    }
    themes
}

/// `loadStartupThemes(settingsManager)`
async fn load_startup_themes(settings_manager: &SettingsManager) -> Vec<Theme> {
    let global_settings_manager = Arc::new(SettingsManager::in_memory(
        &settings_manager.get_global_settings(),
        SettingsManagerCreateOptions {
            project_trusted: Some(false),
        },
    ));
    let package_manager = DefaultPackageManager::new(PackageManagerOptions {
        cwd: crate::utils::paths::current_dir(),
        agent_dir: get_agent_dir().to_string_lossy().into_owned(),
        settings_manager: global_settings_manager,
        command_runner: None,
    });
    // `async () => "skip"` — a startup prompt never installs anything.
    let resolved_paths = package_manager
        .resolve(Some(Arc::new(|_| {
            Box::pin(async { crate::core::package_manager::MissingSourceAction::Skip })
        })))
        .await
        .unwrap_or_default();
    load_themes(&resolved_paths.themes)
}

/// `createStartupTui(settingsManager)`
pub async fn create_startup_tui(settings_manager: &SettingsManager) -> StartupTui {
    let _ = set_registered_themes(load_startup_themes(settings_manager).await);
    let terminal_theme = detect_terminal_background_from_env(None).theme;
    init_theme(
        Some(
            &resolve_theme_setting(
                settings_manager.get_theme_setting().as_deref(),
                terminal_theme,
            )
            .unwrap_or_else(|| terminal_theme.as_str().to_owned()),
        ),
        false,
    );
    set_keybindings(KeybindingsManager::create(None).to_tui());
    let (terminal, pump) = ProcessTerminal::new().into_shared();
    let ui = TuiMainScreen::with_options(
        Box::new(terminal),
        Some(settings_manager.get_show_hardware_cursor()),
        Some(get_agent_dir()),
    );
    ui.core()
        .set_clear_on_shrink(settings_manager.get_clear_on_shrink());
    StartupTui { ui, pump }
}

/// `startStartupTui(ui, settingsManager)`
/// render loop, so the dialog is already drawn and reacting while the terminal
pub(crate) async fn start_startup_tui(tui: &mut StartupTui, settings_manager: &SettingsManager) {
    tui.ui.start();
    let theme_setting = settings_manager.get_theme_setting();
    if theme_setting
        .as_deref()
        .is_some_and(|setting| parse_auto_theme_setting(Some(setting)).is_none())
    {
        return;
    }
    let core = tui.ui.core().clone();
    let detected = run_until(
        &mut tui.ui,
        &mut tui.pump,
        detect_terminal_theme_for_auto(&core, 100, None),
    )
    .await;
    set_theme(
        &resolve_theme_setting(theme_setting.as_deref(), detected)
            .unwrap_or_else(|| detected.as_str().to_owned()),
        false,
    );
    tui.ui.core().invalidate();
    tui.ui.core().request_render();
}

/// `clearStartupTui(ui)` — the cleared frame still has to go out, so the loop
/// keeps rendering during the 25 ms wait, as the Node event loop does.
async fn clear_startup_tui(tui: &mut StartupTui) {
    tui.ui.core().clear();
    tui.ui.core().request_render();
    run_until(
        &mut tui.ui,
        &mut tui.pump,
        tokio::time::sleep(Duration::from_millis(25)),
    )
    .await;
}

/// `showStartupSelector(settingsManager, title, options)`
/// caller maps itself — a generic parameter would have to travel through the
/// selector's boxed callbacks for no gain.
pub async fn show_startup_selector(
    settings_manager: &SettingsManager,
    title: &str,
    options: Vec<String>,
) -> Option<String> {
    let mut tui = create_startup_tui(settings_manager).await;
    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<Option<String>>();
    let done_tx = Rc::new(RefCell::new(Some(done_tx)));

    let select_tx = Rc::clone(&done_tx);
    let cancel_tx = Rc::clone(&done_tx);
    let selector = component_ref(ListSelectorComponent::new(
        title,
        options,
        Box::new(move |option| {
            if let Some(sender) = select_tx.borrow_mut().take() {
                let _ = sender.send(Some(option));
            }
        }),
        Box::new(move || {
            if let Some(sender) = cancel_tx.borrow_mut().take() {
                let _ = sender.send(None);
            }
        }),
        None,
    ));
    tui.ui.core().add_child(Rc::clone(&selector));
    tui.ui.core().set_focus(Some(selector));
    start_startup_tui(&mut tui, settings_manager).await;

    let result = run_until(&mut tui.ui, &mut tui.pump, done_rx)
        .await
        .unwrap_or(None);
    clear_startup_tui(&mut tui).await;
    tui.ui.stop(TuiStopOptions::default());
    result
}

/// `showFirstTimeSetup(settingsManager)` — the theme and analytics questions,
/// persisted when they are answered.
pub async fn show_first_time_setup(settings_manager: &SettingsManager) {
    let mut tui = create_startup_tui(settings_manager).await;
    tui.ui.start();

    let core = tui.ui.core().clone();
    let detected_theme = run_until(
        &mut tui.ui,
        &mut tui.pump,
        detect_terminal_theme_for_auto(&core, 100, None),
    )
    .await;
    set_theme(detected_theme.as_str(), false);

    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<Option<FirstTimeSetupResult>>();
    let done_tx = Rc::new(RefCell::new(Some(done_tx)));
    let submit_tx = Rc::clone(&done_tx);
    let cancel_tx = Rc::clone(&done_tx);
    let preview_core = tui.ui.core().clone();
    let component = component_ref(FirstTimeSetupComponent::new(FirstTimeSetupOptions {
        detected_theme,
        on_theme_preview: Box::new(move |theme_name| {
            set_theme(theme_name.as_str(), false);
            preview_core.request_render();
        }),
        on_submit: Box::new(move |result| {
            if let Some(sender) = submit_tx.borrow_mut().take() {
                let _ = sender.send(Some(result));
            }
        }),
        on_cancel: Box::new(move || {
            if let Some(sender) = cancel_tx.borrow_mut().take() {
                let _ = sender.send(None);
            }
        }),
    }));
    tui.ui.core().add_child(Rc::clone(&component));
    tui.ui.core().set_focus(Some(component));
    tui.ui.core().request_render();

    let result = run_until(&mut tui.ui, &mut tui.pump, done_rx)
        .await
        .unwrap_or(None);
    if let Some(result) = result {
        settings_manager.set_theme(result.theme.as_str());
        settings_manager.set_enable_analytics(result.share_analytics);
        settings_manager.flush();
    }
    clear_startup_tui(&mut tui).await;
    tui.ui.stop(TuiStopOptions::default());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::source_info::{PathMetadata, SourceOrigin, SourceScope};

    fn resource(path: &str, enabled: bool) -> ResolvedResource {
        ResolvedResource {
            path: path.to_owned(),
            metadata: PathMetadata {
                source: "test-source".to_owned(),
                scope: SourceScope::User,
                origin: SourceOrigin::TopLevel,
                base_dir: None,
            },
            enabled,
        }
    }

    #[test]
    fn load_themes_skips_disabled_broken_and_duplicate_files() {
        let dir = tempfile::tempdir().expect("temp dir");
        let write = |name: &str, contents: &str| {
            let path = dir.path().join(name);
            std::fs::write(&path, contents).expect("write");
            path.to_string_lossy().into_owned()
        };
        let named = |name: &str| {
            let mut theme_json: serde_json::Value =
                serde_json::from_str(include_str!("../modes/interactive/theme/dark.json"))
                    .expect("dark.json parses");
            theme_json["name"] = serde_json::json!(name);
            theme_json.to_string()
        };
        let first = write("one.json", &named("source-one"));
        let duplicate = write("two.json", &named("source-one"));
        let broken = write("broken.json", "{ not json");
        let disabled = write("disabled.json", &named("source-off"));

        let themes = load_themes(&[
            resource(&first, true),
            resource(&duplicate, true),
            resource(&broken, true),
            resource(&disabled, false),
        ]);

        let names: Vec<Option<String>> = themes.into_iter().map(|theme| theme.name).collect();
        assert_eq!(names, vec![Some("source-one".to_owned())]);
    }
}
