use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use notagent_tui::tui::{ListenerId, TuiCore};
use notagent_tui::{RgbColor, TerminalColorScheme};

use crate::core::settings_manager::SettingsManager;

use super::theme::{
    TerminalAutoThemeDetector, TerminalBackgroundThemeDetector, TerminalQueryFuture, TerminalTheme,
    Theme, ThemeResult, detect_terminal_background_from_env, detect_terminal_background_theme,
    detect_terminal_theme_for_auto, init_theme, parse_auto_theme_setting, resolve_theme_setting,
    set_theme, set_theme_instance,
};

// typing). Rust needs the adapter to be spelled out; the two helpers below keep
// the inherent `TuiCore` methods unambiguous.
async fn query_background_color(ui: &TuiCore, timeout: Duration) -> Option<RgbColor> {
    ui.query_terminal_background_color(timeout).await
}

async fn query_color_scheme(ui: &TuiCore, timeout: Duration) -> Option<TerminalColorScheme> {
    ui.query_terminal_color_scheme(timeout).await
}

impl TerminalBackgroundThemeDetector for TuiCore {
    fn query_terminal_background_color(
        &self,
        timeout_ms: u64,
    ) -> TerminalQueryFuture<'_, Option<RgbColor>> {
        Box::pin(async move {
            Ok(query_background_color(self, Duration::from_millis(timeout_ms)).await)
        })
    }
}

impl TerminalAutoThemeDetector for TuiCore {
    fn query_terminal_color_scheme(
        &self,
        timeout_ms: u64,
    ) -> TerminalQueryFuture<'_, Option<TerminalTheme>> {
        Box::pin(async move {
            Ok(query_color_scheme(self, Duration::from_millis(timeout_ms))
                .await
                .map(TerminalTheme::from))
        })
    }
}

struct ControllerState {
    ui: TuiCore,
    settings_manager: Arc<SettingsManager>,
    show_error: Box<dyn Fn(&str)>,
    on_changed: Box<dyn Fn()>,
    terminal_theme: TerminalTheme,
    active_theme_name: Option<String>,
    auto_sync_enabled: bool,
    terminal_color_scheme_unsubscribe: Option<ListenerId>,
}

/// Drives theme selection for the interactive mode.
/// closures capture `this`.
#[derive(Clone)]
pub struct InteractiveThemeController(Rc<RefCell<ControllerState>>);

impl InteractiveThemeController {
    /// Detect the terminal theme, install the configured theme and subscribe to
    /// color-scheme notifications.
    pub fn new(
        ui: TuiCore,
        settings_manager: Arc<SettingsManager>,
        show_error: Box<dyn Fn(&str)>,
        on_changed: Box<dyn Fn()>,
    ) -> Self {
        let terminal_theme = detect_terminal_background_from_env(None).theme;
        let active_theme_name = resolve_theme_setting(
            settings_manager.get_theme_setting().as_deref(),
            terminal_theme,
        );
        let controller = Self(Rc::new(RefCell::new(ControllerState {
            ui,
            settings_manager,
            show_error,
            on_changed,
            terminal_theme,
            active_theme_name: active_theme_name.clone(),
            auto_sync_enabled: false,
            terminal_color_scheme_unsubscribe: None,
        })));
        init_theme(active_theme_name.as_deref(), true);
        controller.bind_terminal_color_scheme_listener();
        controller
    }

    /// Re-subscribe after the TUI was swapped (fullscreen toggle).
    pub fn rebind_tui(&self) {
        let (ui, previous, auto_sync_enabled) = {
            let state = self.0.borrow();
            (
                state.ui.clone(),
                state.terminal_color_scheme_unsubscribe,
                state.auto_sync_enabled,
            )
        };
        if let Some(id) = previous {
            ui.remove_terminal_color_scheme_listener(id);
        }
        self.bind_terminal_color_scheme_listener();
        ui.set_terminal_color_scheme_notifications(auto_sync_enabled);
    }

    /// Apply the theme configured in the settings, detecting the terminal
    /// background when the setting is absent.
    pub async fn apply_from_settings(&self) {
        let (ui, theme_setting) = {
            let state = self.0.borrow();
            (state.ui.clone(), state.settings_manager.get_theme_setting())
        };
        let auto_theme = parse_auto_theme_setting(theme_setting.as_deref());
        if let Some((light_theme, dark_theme)) = auto_theme {
            let terminal_theme = detect_terminal_theme_for_auto(&ui, 100, None).await;
            self.0.borrow_mut().terminal_theme = terminal_theme;
            self.set_auto_sync(true);
            self.apply_theme_name(
                if terminal_theme == TerminalTheme::Light {
                    &light_theme
                } else {
                    &dark_theme
                },
                true,
            );
            return;
        }

        self.set_auto_sync(false);
        if let Some(theme_setting) = theme_setting {
            self.apply_theme_name(&theme_setting, true);
            return;
        }

        let detection = detect_terminal_background_theme(&ui, 100, None).await;
        self.0.borrow_mut().terminal_theme = detection.theme;
        if !self
            .apply_theme_name(detection.theme.as_str(), false)
            .success
        {
            return;
        }
        if detection.confidence == super::theme::TerminalThemeConfidence::High {
            let settings_manager = Arc::clone(&self.0.borrow().settings_manager);
            settings_manager.set_theme(detection.theme.as_str());
            settings_manager.flush();
        }
    }

    /// Switch to a named theme and stop following the terminal.
    pub fn set_theme_name(&self, theme_name: &str, show_error: bool) -> ThemeResult {
        self.set_auto_sync(false);
        self.apply_theme_name(theme_name, show_error)
    }

    /// Install a theme instance and stop following the terminal.
    pub fn set_theme_instance(&self, theme_instance: Theme) -> ThemeResult {
        self.set_auto_sync(false);
        set_theme_instance(theme_instance);
        self.0.borrow_mut().active_theme_name = Some("<in-memory>".to_string());
        self.notify_changed();
        ThemeResult {
            success: true,
            error: None,
        }
    }

    /// Preview a theme without persisting it.
    pub fn preview(&self, theme_setting_or_name: &str) {
        let (terminal_theme, active_theme_name, ui) = {
            let state = self.0.borrow();
            (
                state.terminal_theme,
                state.active_theme_name.clone(),
                state.ui.clone(),
            )
        };
        let theme_name = resolve_theme_setting(Some(theme_setting_or_name), terminal_theme)
            .or(active_theme_name);
        let Some(theme_name) = theme_name else {
            return;
        };
        if set_theme(&theme_name, true).success {
            ui.invalidate();
            ui.request_render();
        }
    }

    /// Stop following the terminal's color scheme.
    pub fn disable_auto_sync(&self) {
        self.set_auto_sync(false);
    }

    /// The terminal theme the controller last detected.
    pub fn get_terminal_theme(&self) -> TerminalTheme {
        self.0.borrow().terminal_theme
    }

    fn apply_theme_name(&self, theme_name: &str, show_error: bool) -> ThemeResult {
        let result = set_theme(theme_name, true);
        self.0.borrow_mut().active_theme_name = Some(if result.success {
            theme_name.to_string()
        } else {
            "dark".to_string()
        });
        self.notify_changed();
        if !result.success && show_error {
            let message = format!(
                "Failed to load theme \"{theme_name}\": {}\nFell back to dark theme.",
                result.error.clone().unwrap_or_default()
            );
            let state = self.0.borrow();
            (state.show_error)(&message);
        }
        result
    }

    fn notify_changed(&self) {
        let ui = self.0.borrow().ui.clone();
        ui.invalidate();
        let on_changed = Rc::clone(&self.0);
        let state = on_changed.borrow();
        (state.on_changed)();
    }

    fn set_auto_sync(&self, enabled: bool) {
        let ui = {
            let mut state = self.0.borrow_mut();
            if state.auto_sync_enabled == enabled {
                return;
            }
            state.auto_sync_enabled = enabled;
            state.ui.clone()
        };
        ui.set_terminal_color_scheme_notifications(enabled);
    }

    fn bind_terminal_color_scheme_listener(&self) {
        let ui = self.0.borrow().ui.clone();
        let weak = Rc::downgrade(&self.0);
        let id = ui.on_terminal_color_scheme_change(Box::new(move |terminal_theme| {
            if let Some(inner) = weak.upgrade() {
                InteractiveThemeController(inner).apply_terminal_theme(terminal_theme.into());
            }
        }));
        self.0.borrow_mut().terminal_color_scheme_unsubscribe = Some(id);
    }

    fn apply_terminal_theme(&self, terminal_theme: TerminalTheme) {
        let theme_setting = {
            let mut state = self.0.borrow_mut();
            if !state.auto_sync_enabled {
                return;
            }
            state.terminal_theme = terminal_theme;
            state.settings_manager.get_theme_setting()
        };
        let Some((light_theme, dark_theme)) = parse_auto_theme_setting(theme_setting.as_deref())
        else {
            self.set_auto_sync(false);
            return;
        };
        let theme_name = if terminal_theme == TerminalTheme::Light {
            light_theme
        } else {
            dark_theme
        };
        if Some(&theme_name) != self.0.borrow().active_theme_name.as_ref() {
            self.apply_theme_name(&theme_name, false);
        }
    }
}
