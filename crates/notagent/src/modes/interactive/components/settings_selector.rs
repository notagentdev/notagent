//! 1:1 port of
//! `packages/coding-agent/src/modes/interactive/components/settings-selector.ts` (881 LOC).
//!
//! The `/settings` dialog: one `SettingsList` over roughly thirty rows, three
//! submenus (warnings, thinking level, theme) and the two-mode theme submenu
//! with its own nested light/dark selects.
//!
//! Deviations:
//!   * Class 1: TypeScript hands every submenu a `done(value?)` continuation
//!     it may call from inside the input it is handling. A submenu cannot hold
//!     a callback into the list that owns it, so the ported `SettingsList`
//!     hands out a [`SubmenuDone`] slot instead and reads it the moment that
//!     input returns (`settings_list.rs`).
//!   * Class 1: for the same reason `ThemeSubmenu` cannot rebuild itself from
//!     inside a select callback — the callbacks record the requested change in
//!     a shared cell and the submenu applies it after the dispatch.
//!   * Class 1: the string unions of the settings become the crate enums, and
//!     the wire spelling comes from their serde renames (`wire`/`from_wire`),
//!     so a value can never drift from what `settings.json` stores.
//!   * Class 1: `SettingsCallbacks` is a struct of boxed closures shared as
//!     `Rc<RefCell<…>>` (TypeScript passes one object by reference); it has a
//!     `Default` with no-op callbacks, which is what the TS suite builds with
//!     its partial cast.

use std::cell::RefCell;
use std::rc::Rc;

use serde::Serialize;
use serde::de::DeserializeOwned;

use notagent_agent::types::ThinkingLevel;
use notagent_ai::types::Transport;
use notagent_tui::components::scroll_view::ScrollViewScrollbar;
use notagent_tui::components::select_list::{SelectItem, SelectList, SelectListLayoutOptions};
use notagent_tui::components::settings_list::{
    SettingItem, SettingsList, SettingsListOptions, SubmenuDone, SubmenuFactory,
};
use notagent_tui::components::spacer::Spacer;
use notagent_tui::components::text::Text;
use notagent_tui::terminal_image::get_capabilities;
use notagent_tui::tui::{Component, ComponentRef, Container, Line, component_ref};

use crate::core::http_dispatcher::{HTTP_IDLE_TIMEOUT_CHOICES, format_http_idle_timeout_ms};
use crate::core::settings_manager::{
    DefaultProjectTrust, DoubleEscapeAction, FullscreenExitOutput, MermaidRenderingMode, QueueMode,
    TreeFilterMode, TuiMode, WarningSettings,
};
use crate::modes::interactive::theme::theme::{
    TerminalTheme, ThemeColor, get_select_list_theme, get_settings_list_theme,
    parse_auto_theme_setting, theme,
};

use super::dynamic_border::DynamicBorder;
use super::keybinding_hints::key_display_text;

/// `SETTINGS_SUBMENU_SELECT_LIST_LAYOUT`.
fn settings_submenu_select_list_layout() -> SelectListLayoutOptions {
    SelectListLayoutOptions {
        min_primary_column_width: Some(12),
        max_primary_column_width: Some(32),
        truncate_primary: None,
    }
}

/// `THINKING_DESCRIPTIONS`.
fn thinking_description(level: ThinkingLevel) -> &'static str {
    match level {
        ThinkingLevel::Off => "No reasoning",
        ThinkingLevel::Minimal => "Very brief reasoning (~1k tokens)",
        ThinkingLevel::Low => "Light reasoning (~2k tokens)",
        ThinkingLevel::Medium => "Moderate reasoning (~8k tokens)",
        ThinkingLevel::High => "Deep reasoning (~16k tokens)",
        ThinkingLevel::Xhigh => "Extra-high reasoning (~32k tokens)",
        ThinkingLevel::Max => "Maximum reasoning",
    }
}

/// `DEFAULT_PROJECT_TRUST_LABELS`, in declaration order.
const DEFAULT_PROJECT_TRUST_LABELS: [(DefaultProjectTrust, &str); 3] = [
    (DefaultProjectTrust::Ask, "Ask"),
    (DefaultProjectTrust::Always, "Always trust"),
    (DefaultProjectTrust::Never, "Never trust"),
];

fn default_project_trust_label(value: DefaultProjectTrust) -> &'static str {
    DEFAULT_PROJECT_TRUST_LABELS
        .iter()
        .find(|(trust, _)| *trust == value)
        .map(|(_, label)| *label)
        .unwrap_or("Ask")
}

/// `DEFAULT_PROJECT_TRUST_BY_LABEL`.
fn default_project_trust_by_label(label: &str) -> Option<DefaultProjectTrust> {
    DEFAULT_PROJECT_TRUST_LABELS
        .iter()
        .find(|(_, item)| *item == label)
        .map(|(trust, _)| *trust)
}

/// The wire spelling of a settings value — its serde rename, which is what
/// `settings.json` carries and what the TypeScript string union spells out.
fn wire<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_default()
}

fn from_wire<T: DeserializeOwned>(value: &str) -> Option<T> {
    serde_json::from_value(serde_json::Value::String(value.to_string())).ok()
}

/// `ScrollViewScrollbar` carries no serde derive (it is a rendering option of
/// the tui crate), so its two conversions are spelled out.
fn scrollbar_wire(value: ScrollViewScrollbar) -> &'static str {
    match value {
        ScrollViewScrollbar::Auto => "auto",
        ScrollViewScrollbar::Always => "always",
        ScrollViewScrollbar::Hidden => "hidden",
    }
}

fn scrollbar_from_wire(value: &str) -> Option<ScrollViewScrollbar> {
    match value {
        "auto" => Some(ScrollViewScrollbar::Auto),
        "always" => Some(ScrollViewScrollbar::Always),
        "hidden" => Some(ScrollViewScrollbar::Hidden),
        _ => None,
    }
}

// ============================================================================
// Configuration and callbacks
// ============================================================================

/// `SettingsConfig`.
#[derive(Debug, Clone)]
pub struct SettingsConfig {
    pub auto_compact: bool,
    pub show_images: bool,
    pub image_width_cells: u64,
    pub auto_resize_images: bool,
    pub block_images: bool,
    pub enable_skill_commands: bool,
    pub steering_mode: QueueMode,
    pub follow_up_mode: QueueMode,
    pub transport: Transport,
    pub http_idle_timeout_ms: u64,
    pub thinking_level: ThinkingLevel,
    pub available_thinking_levels: Vec<ThinkingLevel>,
    pub current_theme: String,
    pub terminal_theme: TerminalTheme,
    pub available_themes: Vec<String>,
    pub hide_thinking_block: bool,
    pub mermaid_rendering_mode: MermaidRenderingMode,
    pub show_cache_miss_notices: bool,
    pub enable_install_telemetry: bool,
    pub double_escape_action: DoubleEscapeAction,
    pub tree_filter_mode: TreeFilterMode,
    pub show_hardware_cursor: bool,
    pub editor_padding_x: u64,
    pub output_pad: u64,
    pub autocomplete_max_visible: u64,
    pub quiet_startup: bool,
    pub default_project_trust: DefaultProjectTrust,
    pub clear_on_shrink: bool,
    pub show_terminal_progress: bool,
    /// The footer line with the working directory and git branch; off by
    /// default (user decision 2026-08-25).
    pub show_workspace_in_footer: bool,
    pub tui_mode: TuiMode,
    pub fullscreen_exit_output: FullscreenExitOutput,
    pub fullscreen_scrollbar: ScrollViewScrollbar,
    pub warnings: WarningSettings,
    /// Chat-block style (port addition, v0.1.9): true = badge (the default,
    /// user decision 2026-08-17), false = standard filled surface.
    pub block_style_badge: bool,
    /// Atomic file leases (port addition, v0.1.19): off by default, also
    /// reachable as `/leases on|off`.
    pub atomic_leases: bool,
    /// Bash filter (port addition, v0.1.20): off by default, also reachable as
    /// `/bash-filter on|off`.
    pub bash_filter: bool,
}

impl Default for SettingsConfig {
    fn default() -> Self {
        Self {
            auto_compact: false,
            block_style_badge: true,
            atomic_leases: false,
            bash_filter: false,
            show_images: false,
            image_width_cells: 0,
            auto_resize_images: false,
            block_images: false,
            enable_skill_commands: false,
            steering_mode: QueueMode::All,
            follow_up_mode: QueueMode::All,
            transport: Transport::Auto,
            http_idle_timeout_ms: 0,
            thinking_level: ThinkingLevel::Off,
            available_thinking_levels: Vec::new(),
            current_theme: String::new(),
            terminal_theme: TerminalTheme::Dark,
            available_themes: Vec::new(),
            hide_thinking_block: false,
            mermaid_rendering_mode: MermaidRenderingMode::Off,
            show_cache_miss_notices: false,
            enable_install_telemetry: false,
            double_escape_action: DoubleEscapeAction::None,
            tree_filter_mode: TreeFilterMode::Default,
            show_hardware_cursor: false,
            editor_padding_x: 0,
            output_pad: 0,
            autocomplete_max_visible: 0,
            quiet_startup: false,
            default_project_trust: DefaultProjectTrust::Ask,
            clear_on_shrink: false,
            show_terminal_progress: false,
            show_workspace_in_footer: false,
            tui_mode: TuiMode::Regular,
            fullscreen_exit_output: FullscreenExitOutput::Transcript,
            fullscreen_scrollbar: ScrollViewScrollbar::Hidden,
            warnings: WarningSettings::default(),
        }
    }
}

/// `SettingsCallbacks`.
pub struct SettingsCallbacks {
    pub on_auto_compact_change: Box<dyn FnMut(bool)>,
    /// Chat-block style toggle (v0.1.9): true = badge.
    pub on_block_style_change: Box<dyn FnMut(bool)>,
    /// Atomic file leases toggle (v0.1.19).
    pub on_atomic_leases_change: Box<dyn FnMut(bool)>,
    /// Bash filter toggle (v0.1.20).
    pub on_bash_filter_change: Box<dyn FnMut(bool)>,
    pub on_show_images_change: Box<dyn FnMut(bool)>,
    pub on_image_width_cells_change: Box<dyn FnMut(u64)>,
    pub on_auto_resize_images_change: Box<dyn FnMut(bool)>,
    pub on_block_images_change: Box<dyn FnMut(bool)>,
    pub on_enable_skill_commands_change: Box<dyn FnMut(bool)>,
    pub on_steering_mode_change: Box<dyn FnMut(QueueMode)>,
    pub on_follow_up_mode_change: Box<dyn FnMut(QueueMode)>,
    pub on_transport_change: Box<dyn FnMut(Transport)>,
    pub on_http_idle_timeout_ms_change: Box<dyn FnMut(u64)>,
    pub on_thinking_level_change: Box<dyn FnMut(ThinkingLevel)>,
    pub on_theme_change: Box<dyn FnMut(&str)>,
    pub on_theme_preview: Option<ValueCallback>,
    pub on_hide_thinking_block_change: Box<dyn FnMut(bool)>,
    pub on_mermaid_rendering_mode_change: Box<dyn FnMut(MermaidRenderingMode)>,
    pub on_show_cache_miss_notices_change: Box<dyn FnMut(bool)>,
    pub on_enable_install_telemetry_change: Box<dyn FnMut(bool)>,
    pub on_double_escape_action_change: Box<dyn FnMut(DoubleEscapeAction)>,
    pub on_tree_filter_mode_change: Box<dyn FnMut(TreeFilterMode)>,
    pub on_show_hardware_cursor_change: Box<dyn FnMut(bool)>,
    pub on_editor_padding_x_change: Box<dyn FnMut(u64)>,
    pub on_output_pad_change: Box<dyn FnMut(u64)>,
    pub on_autocomplete_max_visible_change: Box<dyn FnMut(u64)>,
    pub on_quiet_startup_change: Box<dyn FnMut(bool)>,
    pub on_default_project_trust_change: Box<dyn FnMut(DefaultProjectTrust)>,
    pub on_clear_on_shrink_change: Box<dyn FnMut(bool)>,
    pub on_show_terminal_progress_change: Box<dyn FnMut(bool)>,
    pub on_show_workspace_in_footer_change: Box<dyn FnMut(bool)>,
    pub on_tui_mode_change: Box<dyn FnMut(TuiMode)>,
    pub on_fullscreen_exit_output_change: Box<dyn FnMut(FullscreenExitOutput)>,
    pub on_fullscreen_scrollbar_change: Box<dyn FnMut(ScrollViewScrollbar)>,
    pub on_warnings_change: Box<dyn FnMut(WarningSettings)>,
    pub on_cancel: Box<dyn FnMut()>,
}

impl Default for SettingsCallbacks {
    fn default() -> Self {
        Self {
            on_auto_compact_change: Box::new(|_| {}),
            on_block_style_change: Box::new(|_| {}),
            on_atomic_leases_change: Box::new(|_| {}),
            on_bash_filter_change: Box::new(|_| {}),
            on_show_images_change: Box::new(|_| {}),
            on_image_width_cells_change: Box::new(|_| {}),
            on_auto_resize_images_change: Box::new(|_| {}),
            on_block_images_change: Box::new(|_| {}),
            on_enable_skill_commands_change: Box::new(|_| {}),
            on_steering_mode_change: Box::new(|_| {}),
            on_follow_up_mode_change: Box::new(|_| {}),
            on_transport_change: Box::new(|_| {}),
            on_http_idle_timeout_ms_change: Box::new(|_| {}),
            on_thinking_level_change: Box::new(|_| {}),
            on_theme_change: Box::new(|_| {}),
            on_theme_preview: None,
            on_hide_thinking_block_change: Box::new(|_| {}),
            on_mermaid_rendering_mode_change: Box::new(|_| {}),
            on_show_cache_miss_notices_change: Box::new(|_| {}),
            on_enable_install_telemetry_change: Box::new(|_| {}),
            on_double_escape_action_change: Box::new(|_| {}),
            on_tree_filter_mode_change: Box::new(|_| {}),
            on_show_hardware_cursor_change: Box::new(|_| {}),
            on_editor_padding_x_change: Box::new(|_| {}),
            on_output_pad_change: Box::new(|_| {}),
            on_autocomplete_max_visible_change: Box::new(|_| {}),
            on_quiet_startup_change: Box::new(|_| {}),
            on_default_project_trust_change: Box::new(|_| {}),
            on_clear_on_shrink_change: Box::new(|_| {}),
            on_show_terminal_progress_change: Box::new(|_| {}),
            on_show_workspace_in_footer_change: Box::new(|_| {}),
            on_tui_mode_change: Box::new(|_| {}),
            on_fullscreen_exit_output_change: Box::new(|_| {}),
            on_fullscreen_scrollbar_change: Box::new(|_| {}),
            on_warnings_change: Box::new(|_| {}),
            on_cancel: Box::new(|| {}),
        }
    }
}

/// A callback receiving the value under the cursor.
pub type ValueCallback = Box<dyn FnMut(&str)>;

/// The `done(value?)` continuation a submenu is handed by its list.
type DoneSlot = SubmenuDone;

// ============================================================================
// Warning submenu
// ============================================================================

/// `WarningSettingsSubmenu`.
struct WarningSettingsSubmenu {
    settings_list: SettingsList,
}

impl WarningSettingsSubmenu {
    fn new(
        warnings: &WarningSettings,
        mut on_change: Box<dyn FnMut(WarningSettings)>,
        on_cancel: Box<dyn FnMut()>,
    ) -> Self {
        let state = Rc::new(RefCell::new(warnings.clone()));

        let items = vec![SettingItem {
            id: "anthropic-extra-usage".to_string(),
            label: "Anthropic extra usage".to_string(),
            description: Some(
                "Warn when Anthropic subscription auth may use paid extra usage".to_string(),
            ),
            current_value: if state.borrow().anthropic_extra_usage.unwrap_or(true) {
                "true".to_string()
            } else {
                "false".to_string()
            },
            values: Some(vec!["true".to_string(), "false".to_string()]),
            submenu: None,
        }];

        let max_visible = items.len().min(10);
        let change_state = Rc::clone(&state);
        let settings_list = SettingsList::new(
            items,
            max_visible,
            get_settings_list_theme(),
            Box::new(move |id, new_value| {
                if id == "anthropic-extra-usage" {
                    change_state.borrow_mut().anthropic_extra_usage = Some(new_value == "true");
                    on_change(change_state.borrow().clone());
                }
            }),
            on_cancel,
            SettingsListOptions::default(),
        );

        Self { settings_list }
    }
}

impl Component for WarningSettingsSubmenu {
    fn render(&mut self, width: usize) -> Vec<Line> {
        self.settings_list.render(width)
    }

    fn invalidate(&mut self) {
        self.settings_list.invalidate();
    }

    fn handle_input(&mut self, data: &str) {
        self.settings_list.handle_input(data);
    }
}

// ============================================================================
// Select submenu
// ============================================================================

/// `SelectSubmenu`.
struct SelectSubmenu {
    container: Container,
    select_list: Rc<RefCell<SelectList>>,
}

impl SelectSubmenu {
    fn new(
        title: &str,
        description: &str,
        options: Vec<SelectItem>,
        current_value: &str,
        mut on_select: Box<dyn FnMut(&str)>,
        on_cancel: Box<dyn FnMut()>,
        on_selection_change: Option<ValueCallback>,
    ) -> Self {
        let theme = theme();
        let mut container = Container::new();

        // Title
        container.add_child(component_ref(Text::new(
            theme.bold(&theme.fg(ThemeColor::Accent, title)),
            0,
            0,
        )));

        // Description
        if !description.is_empty() {
            container.add_child(component_ref(Spacer::new(1)));
            container.add_child(component_ref(Text::new(
                theme.fg(ThemeColor::Muted, description),
                0,
                0,
            )));
        }

        // Spacer
        container.add_child(component_ref(Spacer::new(1)));

        // Select list
        let max_visible = options.len().min(10);
        let current_index = options
            .iter()
            .position(|option| option.value == current_value);
        let mut select_list = SelectList::new(
            options,
            max_visible,
            get_select_list_theme(),
            settings_submenu_select_list_layout(),
        );

        // Pre-select current value
        if let Some(index) = current_index {
            select_list.set_selected_index(index);
        }

        select_list.on_select = Some(Box::new(move |item| on_select(&item.value)));
        select_list.on_cancel = Some(on_cancel);
        if let Some(mut on_selection_change) = on_selection_change {
            select_list.on_selection_change =
                Some(Box::new(move |item| on_selection_change(&item.value)));
        }

        let select_list = Rc::new(RefCell::new(select_list));
        container.add_child(Rc::clone(&select_list) as ComponentRef);

        // Hint
        container.add_child(component_ref(Spacer::new(1)));
        container.add_child(component_ref(Text::new(
            theme.fg(ThemeColor::Dim, "  Enter to select · Esc to go back"),
            0,
            0,
        )));

        Self {
            container,
            select_list,
        }
    }
}

impl Component for SelectSubmenu {
    fn render(&mut self, width: usize) -> Vec<Line> {
        self.container.render(width)
    }

    fn invalidate(&mut self) {
        self.container.invalidate();
    }

    fn handle_input(&mut self, data: &str) {
        self.select_list.borrow_mut().handle_input(data);
    }
}

// ============================================================================
// Theme submenu
// ============================================================================

fn theme_items(available_themes: &[String]) -> Vec<SelectItem> {
    available_themes
        .iter()
        .map(|name| SelectItem {
            value: name.clone(),
            label: name.clone(),
            description: None,
        })
        .collect()
}

const AUTOMATIC_THEME_VALUE: &str = "/";

fn single_mode_theme_items(available_themes: &[String]) -> Vec<SelectItem> {
    let mut items = vec![SelectItem {
        value: AUTOMATIC_THEME_VALUE.to_string(),
        label: "Automatic".to_string(),
        description: Some("Use separate themes for light and dark terminal appearance".to_string()),
    }];
    items.extend(theme_items(available_themes));
    items
}

fn preferred_theme(available_themes: &[String], preferred: Option<&str>, fallback: &str) -> String {
    if let Some(preferred) = preferred
        && available_themes.iter().any(|name| name == preferred)
    {
        return preferred.to_string();
    }
    if available_themes.iter().any(|name| name == fallback) {
        return fallback.to_string();
    }
    available_themes
        .first()
        .cloned()
        .unwrap_or_else(|| fallback.to_string())
}

fn default_automatic_themes(
    current_theme_setting: &str,
    available_themes: &[String],
) -> (String, String) {
    if let Some(auto_theme) = parse_auto_theme_setting(Some(current_theme_setting)) {
        return auto_theme;
    }

    let current_fixed_theme = if current_theme_setting.contains('/') {
        None
    } else {
        Some(current_theme_setting)
    };
    let theme_name = preferred_theme(available_themes, current_fixed_theme, "dark");
    (theme_name.clone(), theme_name)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ThemeMode {
    Single,
    Automatic,
}

/// The theme selection while the submenu is open. The select callbacks share
/// it, because they run while the submenu itself is borrowed.
struct ThemeState {
    mode: ThemeMode,
    single_theme: String,
    light_theme: String,
    dark_theme: String,
    terminal_theme: TerminalTheme,
}

impl ThemeState {
    fn theme_setting(&self) -> String {
        match self.mode {
            ThemeMode::Automatic => self.automatic_theme_setting(),
            ThemeMode::Single => self.single_theme.clone(),
        }
    }

    fn active_automatic_theme(&self) -> String {
        if self.terminal_theme == TerminalTheme::Light {
            self.light_theme.clone()
        } else {
            self.dark_theme.clone()
        }
    }

    fn automatic_theme_setting(&self) -> String {
        format!("{}/{}", self.light_theme, self.dark_theme)
    }
}

/// What a select callback asked the submenu to do next.
enum ThemeMenuChange {
    ShowSingle,
    ShowAutomatic,
    Done(Option<String>),
}

/// `ThemeSubmenu`.
struct ThemeSubmenu {
    content: Option<ComponentRef>,
    input_component: Option<ComponentRef>,
    /// The settings list of the automatic menu, when that menu is open.
    automatic_list: Option<Rc<RefCell<SettingsList>>>,
    pending: Rc<RefCell<Option<ThemeMenuChange>>>,
    state: Rc<RefCell<ThemeState>>,
    callbacks: Rc<RefCell<SettingsCallbacks>>,
    available_themes: Vec<String>,
    original_theme_setting: String,
    /// `done` of the theme submenu itself.
    done: DoneSlot,
}

impl ThemeSubmenu {
    fn new(
        current_theme_setting: &str,
        terminal_theme: TerminalTheme,
        available_themes: Vec<String>,
        callbacks: Rc<RefCell<SettingsCallbacks>>,
        done: DoneSlot,
    ) -> Self {
        let auto_theme = parse_auto_theme_setting(Some(current_theme_setting));
        let automatic_themes = default_automatic_themes(current_theme_setting, &available_themes);
        let fixed_theme = if auto_theme.is_some() || current_theme_setting.contains('/') {
            None
        } else {
            Some(current_theme_setting.to_string())
        };
        let mode = if auto_theme.is_some() {
            ThemeMode::Automatic
        } else {
            ThemeMode::Single
        };
        let state = ThemeState {
            mode,
            single_theme: String::new(),
            light_theme: automatic_themes.0,
            dark_theme: automatic_themes.1,
            terminal_theme,
        };
        let single_preference = fixed_theme
            .clone()
            .or_else(|| auto_theme.is_some().then(|| state.active_automatic_theme()));
        let mut state = state;
        state.single_theme =
            preferred_theme(&available_themes, single_preference.as_deref(), "dark");

        let mut submenu = Self {
            content: None,
            input_component: None,
            automatic_list: None,
            pending: Rc::new(RefCell::new(None)),
            state: Rc::new(RefCell::new(state)),
            callbacks,
            available_themes,
            original_theme_setting: current_theme_setting.to_string(),
            done,
        };

        if mode == ThemeMode::Automatic {
            submenu.show_automatic_menu();
        } else {
            submenu.show_single_menu();
        }
        submenu
    }

    fn preview(callbacks: &Rc<RefCell<SettingsCallbacks>>, value: &str) {
        if let Some(preview) = callbacks.borrow_mut().on_theme_preview.as_mut() {
            preview(value);
        }
    }

    fn set_content(&mut self, render_component: ComponentRef, input_component: ComponentRef) {
        self.content = Some(render_component);
        self.input_component = Some(input_component);
    }

    fn show_single_menu(&mut self) {
        self.state.borrow_mut().mode = ThemeMode::Single;
        self.automatic_list = None;

        let select_state = Rc::clone(&self.state);
        let select_callbacks = Rc::clone(&self.callbacks);
        let select_pending = Rc::clone(&self.pending);
        let cancel_pending = Rc::clone(&self.pending);
        let change_state = Rc::clone(&self.state);
        let change_callbacks = Rc::clone(&self.callbacks);

        let menu = SelectSubmenu::new(
            "Theme",
            "Select a theme, or choose Automatic to follow terminal appearance.",
            single_mode_theme_items(&self.available_themes),
            &self.state.borrow().single_theme.clone(),
            Box::new(move |value| {
                if value == AUTOMATIC_THEME_VALUE {
                    select_state.borrow_mut().mode = ThemeMode::Automatic;
                    let setting = select_state.borrow().theme_setting();
                    Self::preview(&select_callbacks, &setting);
                    *select_pending.borrow_mut() = Some(ThemeMenuChange::ShowAutomatic);
                    return;
                }

                select_state.borrow_mut().single_theme = value.to_string();
                *select_pending.borrow_mut() = Some(ThemeMenuChange::Done(Some(value.to_string())));
            }),
            Box::new(move || {
                *cancel_pending.borrow_mut() = Some(ThemeMenuChange::Done(None));
            }),
            Some(Box::new(move |value| {
                let preview_value = if value == AUTOMATIC_THEME_VALUE {
                    change_state.borrow().automatic_theme_setting()
                } else {
                    value.to_string()
                };
                Self::preview(&change_callbacks, &preview_value);
            })),
        );
        let menu = component_ref(menu);
        self.set_content(Rc::clone(&menu), menu);
    }

    fn show_automatic_menu(&mut self) {
        self.state.borrow_mut().mode = ThemeMode::Automatic;

        let theme_instance = theme();
        let mut content = Container::new();
        content.add_child(component_ref(Text::new(
            theme_instance.bold(&theme_instance.fg(ThemeColor::Accent, "Automatic Theme")),
            0,
            0,
        )));
        content.add_child(component_ref(Spacer::new(1)));
        content.add_child(component_ref(Text::new(
            theme_instance.fg(
                ThemeColor::Muted,
                "Choose themes for terminal light and dark appearance.",
            ),
            0,
            0,
        )));
        content.add_child(component_ref(Text::new(
            theme_instance.fg(
                ThemeColor::Muted,
                "Light/dark detection requires terminal support.",
            ),
            0,
            0,
        )));
        content.add_child(component_ref(Spacer::new(1)));

        let items = vec![
            SettingItem {
                id: "light-theme".to_string(),
                label: "Light theme".to_string(),
                description: Some(
                    "Theme to use in automatic mode when the terminal is light".to_string(),
                ),
                current_value: self.state.borrow().light_theme.clone(),
                values: None,
                submenu: Some(self.theme_select_factory(
                    "Light Theme",
                    "Select the theme to use for light terminal appearance",
                    true,
                )),
            },
            SettingItem {
                id: "dark-theme".to_string(),
                label: "Dark theme".to_string(),
                description: Some(
                    "Theme to use in automatic mode when the terminal is dark".to_string(),
                ),
                current_value: self.state.borrow().dark_theme.clone(),
                values: None,
                submenu: Some(self.theme_select_factory(
                    "Dark Theme",
                    "Select the theme to use for dark terminal appearance",
                    false,
                )),
            },
            SettingItem {
                id: "apply".to_string(),
                label: "Apply".to_string(),
                description: Some("Save and go back".to_string()),
                current_value: "save and go back".to_string(),
                values: Some(vec!["save and go back".to_string()]),
                submenu: None,
            },
            SettingItem {
                id: "single-mode".to_string(),
                label: "Change mode".to_string(),
                description: Some("Switch to one theme for light and dark".to_string()),
                current_value: "switch to single theme".to_string(),
                values: Some(vec!["switch to single theme".to_string()]),
                submenu: None,
            },
        ];

        let max_visible = items.len().min(10);
        let change_state = Rc::clone(&self.state);
        let change_callbacks = Rc::clone(&self.callbacks);
        let change_pending = Rc::clone(&self.pending);
        let cancel_pending = Rc::clone(&self.pending);
        let settings_list = SettingsList::new(
            items,
            max_visible,
            get_settings_list_theme(),
            Box::new(move |id, _new_value| match id {
                "single-mode" => {
                    let single = change_state.borrow().active_automatic_theme();
                    {
                        let mut state = change_state.borrow_mut();
                        state.mode = ThemeMode::Single;
                        state.single_theme = single.clone();
                    }
                    Self::preview(&change_callbacks, &single);
                    *change_pending.borrow_mut() = Some(ThemeMenuChange::ShowSingle);
                }
                "apply" => {
                    let setting = change_state.borrow().automatic_theme_setting();
                    *change_pending.borrow_mut() = Some(ThemeMenuChange::Done(Some(setting)));
                }
                _ => {}
            }),
            Box::new(move || {
                *cancel_pending.borrow_mut() = Some(ThemeMenuChange::Done(None));
            }),
            SettingsListOptions::default(),
        );

        let settings_list = Rc::new(RefCell::new(settings_list));
        content.add_child(Rc::clone(&settings_list) as ComponentRef);
        self.automatic_list = Some(Rc::clone(&settings_list));
        self.set_content(
            component_ref(content),
            Rc::clone(&settings_list) as ComponentRef,
        );
    }

    /// `createThemeSelect` — the light/dark selects of the automatic menu.
    fn theme_select_factory(&self, title: &str, description: &str, light: bool) -> SubmenuFactory {
        let title = title.to_string();
        let description = description.to_string();
        let available_themes = self.available_themes.clone();
        let state = Rc::clone(&self.state);
        let callbacks = Rc::clone(&self.callbacks);

        Rc::new(move |current_value, done| {
            let select_state = Rc::clone(&state);
            let select_callbacks = Rc::clone(&callbacks);
            let select_done = Rc::clone(&done);
            let cancel_state = Rc::clone(&state);
            let cancel_callbacks = Rc::clone(&callbacks);
            let cancel_done = done;
            let preview_callbacks = Rc::clone(&callbacks);

            component_ref(SelectSubmenu::new(
                &title,
                &description,
                theme_items(&available_themes),
                current_value,
                Box::new(move |value| {
                    if light {
                        select_state.borrow_mut().light_theme = value.to_string();
                    } else {
                        select_state.borrow_mut().dark_theme = value.to_string();
                    }
                    let setting = select_state.borrow().theme_setting();
                    ThemeSubmenu::preview(&select_callbacks, &setting);
                    *select_done.borrow_mut() = Some(Some(value.to_string()));
                }),
                Box::new(move || {
                    let setting = cancel_state.borrow().theme_setting();
                    ThemeSubmenu::preview(&cancel_callbacks, &setting);
                    *cancel_done.borrow_mut() = Some(None);
                }),
                Some(Box::new(move |value| {
                    ThemeSubmenu::preview(&preview_callbacks, value);
                })),
            ))
        })
    }

    fn cancel(&mut self) {
        let original = self.original_theme_setting.clone();
        Self::preview(&self.callbacks, &original);
        *self.done.borrow_mut() = Some(None);
    }

    /// Apply what a callback recorded while the submenu was borrowed.
    fn drain_pending(&mut self) {
        let pending = self.pending.borrow_mut().take();
        match pending {
            Some(ThemeMenuChange::ShowSingle) => self.show_single_menu(),
            Some(ThemeMenuChange::ShowAutomatic) => self.show_automatic_menu(),
            Some(ThemeMenuChange::Done(None)) => self.cancel(),
            Some(ThemeMenuChange::Done(Some(setting))) => {
                *self.done.borrow_mut() = Some(Some(setting));
            }
            None => {}
        }
    }
}

impl Component for ThemeSubmenu {
    fn render(&mut self, width: usize) -> Vec<Line> {
        match self.content.as_ref() {
            Some(content) => content.borrow_mut().render(width),
            None => Vec::new(),
        }
    }

    fn invalidate(&mut self) {
        if let Some(content) = self.content.as_ref() {
            content.borrow_mut().invalidate();
        }
    }

    fn handle_input(&mut self, data: &str) {
        if let Some(input_component) = self.input_component.clone() {
            input_component.borrow_mut().handle_input(data);
        }
        self.drain_pending();
    }
}

// ============================================================================
// Main component
// ============================================================================

fn bool_value(value: bool) -> String {
    if value { "true" } else { "false" }.to_string()
}

/// `SettingsSelectorComponent`.
pub struct SettingsSelectorComponent {
    container: Container,
    settings_list: Rc<RefCell<SettingsList>>,
}

impl SettingsSelectorComponent {
    pub fn new(config: SettingsConfig, callbacks: SettingsCallbacks) -> Self {
        let callbacks = Rc::new(RefCell::new(callbacks));

        let supports_images = get_capabilities().images.is_some();
        let follow_up_key = key_display_text("app.message.followUp");
        let current_warnings = Rc::new(RefCell::new(config.warnings.clone()));

        let mut items: Vec<SettingItem> = vec![
            SettingItem {
                id: "autocompact".to_string(),
                label: "Auto-compact".to_string(),
                description: Some(
                    "Automatically compact context when it gets too large".to_string(),
                ),
                current_value: bool_value(config.auto_compact),
                values: Some(vec!["true".to_string(), "false".to_string()]),
                submenu: None,
            },
            SettingItem {
                id: "block-style".to_string(),
                label: "Block style".to_string(),
                description: Some(
                    "How chat blocks render: 'badge' leads with a state badge, 'standard' fills the block background".to_string(),
                ),
                current_value: if config.block_style_badge {
                    "badge".to_string()
                } else {
                    "standard".to_string()
                },
                values: Some(vec!["badge".to_string(), "standard".to_string()]),
                submenu: None,
            },
            SettingItem {
                id: "steering-mode".to_string(),
                label: "Steering mode".to_string(),
                description: Some(
                    "Enter while streaming queues steering messages. 'one-at-a-time': deliver one, wait for response. 'all': deliver all at once."
                        .to_string(),
                ),
                current_value: wire(&config.steering_mode),
                values: Some(vec!["one-at-a-time".to_string(), "all".to_string()]),
                submenu: None,
            },
            SettingItem {
                id: "follow-up-mode".to_string(),
                label: "Follow-up mode".to_string(),
                description: Some(format!(
                    "{follow_up_key} queues follow-up messages until agent stops. 'one-at-a-time': deliver one, wait for response. 'all': deliver all at once."
                )),
                current_value: wire(&config.follow_up_mode),
                values: Some(vec!["one-at-a-time".to_string(), "all".to_string()]),
                submenu: None,
            },
            SettingItem {
                id: "transport".to_string(),
                label: "Transport".to_string(),
                description: Some(
                    "Preferred transport for providers that support multiple transports"
                        .to_string(),
                ),
                current_value: wire(&config.transport),
                values: Some(vec![
                    "sse".to_string(),
                    "websocket".to_string(),
                    "websocket-cached".to_string(),
                    "auto".to_string(),
                ]),
                submenu: None,
            },
            SettingItem {
                id: "http-idle-timeout".to_string(),
                label: "HTTP idle timeout".to_string(),
                description: Some(
                    "Maximum idle gap while waiting for HTTP headers or body chunks. Disable for local models that pause longer than five minutes."
                        .to_string(),
                ),
                current_value: format_http_idle_timeout_ms(config.http_idle_timeout_ms),
                values: Some(
                    HTTP_IDLE_TIMEOUT_CHOICES
                        .iter()
                        .map(|choice| choice.label.to_string())
                        .collect(),
                ),
                submenu: None,
            },
            SettingItem {
                id: "hide-thinking".to_string(),
                label: "Hide thinking".to_string(),
                description: Some("Hide thinking blocks in assistant responses".to_string()),
                current_value: bool_value(config.hide_thinking_block),
                values: Some(vec!["true".to_string(), "false".to_string()]),
                submenu: None,
            },
            SettingItem {
                id: "mermaid-rendering".to_string(),
                label: "Mermaid diagrams".to_string(),
                description: Some("Render Mermaid code blocks as Unicode diagrams".to_string()),
                current_value: wire(&config.mermaid_rendering_mode),
                values: Some(vec![
                    "off".to_string(),
                    "final".to_string(),
                    "streaming".to_string(),
                ]),
                submenu: None,
            },
            SettingItem {
                id: "cache-miss-notices".to_string(),
                label: "Cache miss notices".to_string(),
                description: Some(
                    "Show transcript notices for significant prompt-cache misses".to_string(),
                ),
                current_value: bool_value(config.show_cache_miss_notices),
                values: Some(vec!["true".to_string(), "false".to_string()]),
                submenu: None,
            },
            SettingItem {
                id: "atomic-leases".to_string(),
                label: "Atomic leases".to_string(),
                description: Some(
                    "Reserve a file before write, edit or patch_minified changes it, so several agents in one workspace cannot overwrite each other. Also reachable as /leases on|off."
                        .to_string(),
                ),
                current_value: bool_value(config.atomic_leases),
                values: Some(vec!["true".to_string(), "false".to_string()]),
                submenu: None,
            },
            SettingItem {
                id: "bash-filter".to_string(),
                label: "Bash filter".to_string(),
                description: Some(
                    "Compact the output of supported bash commands before it enters the context. The raw output stays reachable. Also reachable as /bash-filter on|off."
                        .to_string(),
                ),
                current_value: bool_value(config.bash_filter),
                values: Some(vec!["true".to_string(), "false".to_string()]),
                submenu: None,
            },
            SettingItem {
                id: "quiet-startup".to_string(),
                label: "Quiet startup".to_string(),
                description: Some("Disable verbose printing at startup".to_string()),
                current_value: bool_value(config.quiet_startup),
                values: Some(vec!["true".to_string(), "false".to_string()]),
                submenu: None,
            },
            SettingItem {
                id: "install-telemetry".to_string(),
                label: "Install telemetry".to_string(),
                description: Some(
                    "Send an anonymous version/update ping after changelog-detected updates"
                        .to_string(),
                ),
                current_value: bool_value(config.enable_install_telemetry),
                values: Some(vec!["true".to_string(), "false".to_string()]),
                submenu: None,
            },
            SettingItem {
                id: "default-project-trust".to_string(),
                label: "Default project trust".to_string(),
                description: Some(
                    "Fallback behavior when no extension or saved trust decision decides project trust"
                        .to_string(),
                ),
                current_value: default_project_trust_label(config.default_project_trust)
                    .to_string(),
                values: Some(
                    DEFAULT_PROJECT_TRUST_LABELS
                        .iter()
                        .map(|(_, label)| (*label).to_string())
                        .collect(),
                ),
                submenu: None,
            },
            SettingItem {
                id: "double-escape-action".to_string(),
                label: "Double-escape action".to_string(),
                description: Some("Action when pressing Escape twice with empty editor".to_string()),
                current_value: wire(&config.double_escape_action),
                values: Some(vec![
                    "tree".to_string(),
                    "fork".to_string(),
                    "none".to_string(),
                ]),
                submenu: None,
            },
            SettingItem {
                id: "tree-filter-mode".to_string(),
                label: "Tree filter mode".to_string(),
                description: Some("Default filter when opening /tree".to_string()),
                current_value: wire(&config.tree_filter_mode),
                values: Some(vec![
                    "default".to_string(),
                    "no-tools".to_string(),
                    "user-only".to_string(),
                    "labeled-only".to_string(),
                    "all".to_string(),
                ]),
                submenu: None,
            },
            SettingItem {
                id: "warnings".to_string(),
                label: "Warnings".to_string(),
                description: Some("Enable or disable individual warnings".to_string()),
                current_value: "configure".to_string(),
                values: None,
                submenu: Some({
                    let warnings = Rc::clone(&current_warnings);
                    let callbacks = Rc::clone(&callbacks);
                    Rc::new(move |_current_value: &str, done: SubmenuDone| {
                        let change_warnings = Rc::clone(&warnings);
                        let change_callbacks = Rc::clone(&callbacks);
                        let cancel_done = done;
                        component_ref(WarningSettingsSubmenu::new(
                            &warnings.borrow().clone(),
                            Box::new(move |updated| {
                                *change_warnings.borrow_mut() = updated.clone();
                                (change_callbacks.borrow_mut().on_warnings_change)(updated);
                            }),
                            Box::new(move || {
                                *cancel_done.borrow_mut() = Some(None);
                            }),
                        ))
                    }) as SubmenuFactory
                }),
            },
            SettingItem {
                id: "thinking".to_string(),
                label: "Thinking level".to_string(),
                description: Some("Reasoning depth for thinking-capable models".to_string()),
                current_value: wire(&config.thinking_level),
                values: None,
                submenu: Some({
                    let levels = config.available_thinking_levels.clone();
                    let callbacks = Rc::clone(&callbacks);
                    Rc::new(move |current_value: &str, done: SubmenuDone| {
                        let select_callbacks = Rc::clone(&callbacks);
                        let select_done = Rc::clone(&done);
                        let cancel_done = done;
                        component_ref(SelectSubmenu::new(
                            "Thinking Level",
                            "Select reasoning depth for thinking-capable models",
                            levels
                                .iter()
                                .map(|level| SelectItem {
                                    value: wire(level),
                                    label: wire(level),
                                    description: Some(thinking_description(*level).to_string()),
                                })
                                .collect(),
                            current_value,
                            Box::new(move |value| {
                                if let Some(level) = from_wire::<ThinkingLevel>(value) {
                                    (select_callbacks.borrow_mut().on_thinking_level_change)(level);
                                }
                                *select_done.borrow_mut() = Some(Some(value.to_string()));
                            }),
                            Box::new(move || {
                                *cancel_done.borrow_mut() = Some(None);
                            }),
                            None,
                        ))
                    }) as SubmenuFactory
                }),
            },
            SettingItem {
                id: "tui-mode".to_string(),
                label: "TUI mode".to_string(),
                description: Some("Interface layout; fullscreen mode is experimental".to_string()),
                current_value: wire(&config.tui_mode),
                values: Some(vec!["regular".to_string(), "fullscreen".to_string()]),
                submenu: None,
            },
            SettingItem {
                id: "fullscreen-exit-output".to_string(),
                label: "Fullscreen exit output".to_string(),
                description: Some(
                    "Print the transcript or only a session resume hint when exiting fullscreen mode"
                        .to_string(),
                ),
                current_value: wire(&config.fullscreen_exit_output),
                values: Some(vec!["transcript".to_string(), "resume-hint".to_string()]),
                submenu: None,
            },
            SettingItem {
                id: "fullscreen-scrollbar".to_string(),
                label: "Fullscreen scrollbar".to_string(),
                description: Some(
                    "Scrollbar behavior in fullscreen mode; has no effect in regular mode"
                        .to_string(),
                ),
                current_value: scrollbar_wire(config.fullscreen_scrollbar).to_string(),
                values: Some(vec![
                    "auto".to_string(),
                    "always".to_string(),
                    "hidden".to_string(),
                ]),
                submenu: None,
            },
            SettingItem {
                id: "theme".to_string(),
                label: "Theme".to_string(),
                description: Some("Color theme for the interface".to_string()),
                current_value: config.current_theme.clone(),
                values: None,
                submenu: Some({
                    let terminal_theme = config.terminal_theme;
                    let available_themes = config.available_themes.clone();
                    let callbacks = Rc::clone(&callbacks);
                    Rc::new(move |current_value: &str, done: SubmenuDone| {
                        component_ref(ThemeSubmenu::new(
                            current_value,
                            terminal_theme,
                            available_themes.clone(),
                            Rc::clone(&callbacks),
                            done,
                        ))
                    }) as SubmenuFactory
                }),
            },
        ];

        // Only show image toggle if terminal supports it
        if supports_images {
            // Insert after autocompact
            items.insert(
                1,
                SettingItem {
                    id: "show-images".to_string(),
                    label: "Show images".to_string(),
                    description: Some("Render images inline in terminal".to_string()),
                    current_value: bool_value(config.show_images),
                    values: Some(vec!["true".to_string(), "false".to_string()]),
                    submenu: None,
                },
            );
            items.insert(
                2,
                SettingItem {
                    id: "image-width-cells".to_string(),
                    label: "Image width".to_string(),
                    description: Some("Preferred inline image width in terminal cells".to_string()),
                    current_value: config.image_width_cells.to_string(),
                    values: Some(vec!["60".to_string(), "80".to_string(), "120".to_string()]),
                    submenu: None,
                },
            );
        }

        // Image auto-resize toggle (always available, affects both attached and read images)
        items.insert(
            if supports_images { 3 } else { 1 },
            SettingItem {
                id: "auto-resize-images".to_string(),
                label: "Auto-resize images".to_string(),
                description: Some(
                    "Resize large images to 2000x2000 max for better model compatibility"
                        .to_string(),
                ),
                current_value: bool_value(config.auto_resize_images),
                values: Some(vec!["true".to_string(), "false".to_string()]),
                submenu: None,
            },
        );

        // Block images toggle (always available, insert after auto-resize-images)
        insert_after(
            &mut items,
            "auto-resize-images",
            SettingItem {
                id: "block-images".to_string(),
                label: "Block images".to_string(),
                description: Some("Prevent images from being sent to LLM providers".to_string()),
                current_value: bool_value(config.block_images),
                values: Some(vec!["true".to_string(), "false".to_string()]),
                submenu: None,
            },
        );

        // Skill commands toggle (insert after block-images)
        insert_after(
            &mut items,
            "block-images",
            SettingItem {
                id: "skill-commands".to_string(),
                label: "Skill commands".to_string(),
                description: Some("Register skills as /skill:name commands".to_string()),
                current_value: bool_value(config.enable_skill_commands),
                values: Some(vec!["true".to_string(), "false".to_string()]),
                submenu: None,
            },
        );

        // Hardware cursor toggle (insert after skill-commands)
        insert_after(
            &mut items,
            "skill-commands",
            SettingItem {
                id: "show-hardware-cursor".to_string(),
                label: "Show hardware cursor".to_string(),
                description: Some(
                    "Show the terminal cursor while still positioning it for IME support"
                        .to_string(),
                ),
                current_value: bool_value(config.show_hardware_cursor),
                values: Some(vec!["true".to_string(), "false".to_string()]),
                submenu: None,
            },
        );

        // Editor padding toggle (insert after show-hardware-cursor)
        insert_after(
            &mut items,
            "show-hardware-cursor",
            SettingItem {
                id: "editor-padding".to_string(),
                label: "Editor padding".to_string(),
                description: Some("Horizontal padding for input editor (0-3)".to_string()),
                current_value: config.editor_padding_x.to_string(),
                values: Some(vec![
                    "0".to_string(),
                    "1".to_string(),
                    "2".to_string(),
                    "3".to_string(),
                ]),
                submenu: None,
            },
        );

        // Output padding toggle (insert after editor-padding)
        insert_after(
            &mut items,
            "editor-padding",
            SettingItem {
                id: "output-padding".to_string(),
                label: "Output padding".to_string(),
                description: Some(
                    "Horizontal padding for user messages, assistant messages, and thinking"
                        .to_string(),
                ),
                current_value: config.output_pad.to_string(),
                values: Some(vec!["0".to_string(), "1".to_string()]),
                submenu: None,
            },
        );

        // Autocomplete max visible toggle (insert after output-padding)
        insert_after(
            &mut items,
            "output-padding",
            SettingItem {
                id: "autocomplete-max-visible".to_string(),
                label: "Autocomplete max items".to_string(),
                description: Some("Max visible items in autocomplete dropdown (3-20)".to_string()),
                current_value: config.autocomplete_max_visible.to_string(),
                values: Some(vec![
                    "3".to_string(),
                    "5".to_string(),
                    "7".to_string(),
                    "10".to_string(),
                    "15".to_string(),
                    "20".to_string(),
                ]),
                submenu: None,
            },
        );

        // Clear on shrink toggle (insert after autocomplete-max-visible)
        insert_after(
            &mut items,
            "autocomplete-max-visible",
            SettingItem {
                id: "clear-on-shrink".to_string(),
                label: "Clear on shrink".to_string(),
                description: Some(
                    "Clear empty rows when content shrinks (may cause flicker)".to_string(),
                ),
                current_value: bool_value(config.clear_on_shrink),
                values: Some(vec!["true".to_string(), "false".to_string()]),
                submenu: None,
            },
        );

        // Terminal progress toggle (insert after clear-on-shrink)
        insert_after(
            &mut items,
            "clear-on-shrink",
            SettingItem {
                id: "terminal-progress".to_string(),
                label: "Terminal progress".to_string(),
                description: Some(
                    "Show OSC 9;4 progress indicators in the terminal tab bar".to_string(),
                ),
                current_value: bool_value(config.show_terminal_progress),
                values: Some(vec!["true".to_string(), "false".to_string()]),
                submenu: None,
            },
        );

        // Workspace footer line toggle (insert after terminal-progress)
        insert_after(
            &mut items,
            "terminal-progress",
            SettingItem {
                id: "workspace-in-footer".to_string(),
                label: "Workspace in footer".to_string(),
                description: Some(
                    "Show the working directory and git branch under the input".to_string(),
                ),
                current_value: bool_value(config.show_workspace_in_footer),
                values: Some(vec!["true".to_string(), "false".to_string()]),
                submenu: None,
            },
        );

        // Add borders
        let mut container = Container::new();
        container.add_child(component_ref(DynamicBorder::new(None)));

        let change_callbacks = Rc::clone(&callbacks);
        let cancel_callbacks = Rc::clone(&callbacks);
        let settings_list = Rc::new(RefCell::new(SettingsList::new(
            items,
            10,
            get_settings_list_theme(),
            Box::new(move |id, new_value| {
                let mut callbacks = change_callbacks.borrow_mut();
                match id {
                    "autocompact" => (callbacks.on_auto_compact_change)(new_value == "true"),
                    "block-style" => (callbacks.on_block_style_change)(new_value == "badge"),
                    "atomic-leases" => (callbacks.on_atomic_leases_change)(new_value == "true"),
                    "bash-filter" => (callbacks.on_bash_filter_change)(new_value == "true"),
                    "show-images" => (callbacks.on_show_images_change)(new_value == "true"),
                    "image-width-cells" => {
                        (callbacks.on_image_width_cells_change)(parse_int(new_value))
                    }
                    "auto-resize-images" => {
                        (callbacks.on_auto_resize_images_change)(new_value == "true")
                    }
                    "block-images" => (callbacks.on_block_images_change)(new_value == "true"),
                    "skill-commands" => {
                        (callbacks.on_enable_skill_commands_change)(new_value == "true")
                    }
                    "steering-mode" => {
                        if let Some(mode) = from_wire::<QueueMode>(new_value) {
                            (callbacks.on_steering_mode_change)(mode);
                        }
                    }
                    "follow-up-mode" => {
                        if let Some(mode) = from_wire::<QueueMode>(new_value) {
                            (callbacks.on_follow_up_mode_change)(mode);
                        }
                    }
                    "transport" => {
                        if let Some(transport) = from_wire::<Transport>(new_value) {
                            (callbacks.on_transport_change)(transport);
                        }
                    }
                    "http-idle-timeout" => {
                        if let Some(choice) = HTTP_IDLE_TIMEOUT_CHOICES
                            .iter()
                            .find(|item| item.label == new_value)
                        {
                            (callbacks.on_http_idle_timeout_ms_change)(choice.timeout_ms);
                        }
                    }
                    "hide-thinking" => {
                        (callbacks.on_hide_thinking_block_change)(new_value == "true")
                    }
                    "mermaid-rendering" => {
                        if let Some(mode) = from_wire::<MermaidRenderingMode>(new_value) {
                            (callbacks.on_mermaid_rendering_mode_change)(mode);
                        }
                    }
                    "cache-miss-notices" => {
                        (callbacks.on_show_cache_miss_notices_change)(new_value == "true")
                    }
                    "quiet-startup" => (callbacks.on_quiet_startup_change)(new_value == "true"),
                    "install-telemetry" => {
                        (callbacks.on_enable_install_telemetry_change)(new_value == "true")
                    }
                    "default-project-trust" => {
                        if let Some(trust) = default_project_trust_by_label(new_value) {
                            (callbacks.on_default_project_trust_change)(trust);
                        }
                    }
                    "double-escape-action" => {
                        if let Some(action) = from_wire::<DoubleEscapeAction>(new_value) {
                            (callbacks.on_double_escape_action_change)(action);
                        }
                    }
                    "tree-filter-mode" => {
                        if let Some(mode) = from_wire::<TreeFilterMode>(new_value) {
                            (callbacks.on_tree_filter_mode_change)(mode);
                        }
                    }
                    "show-hardware-cursor" => {
                        (callbacks.on_show_hardware_cursor_change)(new_value == "true")
                    }
                    "editor-padding" => {
                        (callbacks.on_editor_padding_x_change)(parse_int(new_value))
                    }
                    "output-padding" => {
                        (callbacks.on_output_pad_change)(if new_value == "0" { 0 } else { 1 })
                    }
                    "autocomplete-max-visible" => {
                        (callbacks.on_autocomplete_max_visible_change)(parse_int(new_value))
                    }
                    "clear-on-shrink" => (callbacks.on_clear_on_shrink_change)(new_value == "true"),
                    "terminal-progress" => {
                        (callbacks.on_show_terminal_progress_change)(new_value == "true")
                    }
                    "workspace-in-footer" => {
                        (callbacks.on_show_workspace_in_footer_change)(new_value == "true")
                    }
                    "tui-mode" => {
                        if let Some(mode) = from_wire::<TuiMode>(new_value) {
                            (callbacks.on_tui_mode_change)(mode);
                        }
                    }
                    "fullscreen-exit-output" => {
                        if let Some(output) = from_wire::<FullscreenExitOutput>(new_value) {
                            (callbacks.on_fullscreen_exit_output_change)(output);
                        }
                    }
                    "fullscreen-scrollbar" => {
                        if let Some(scrollbar) = scrollbar_from_wire(new_value) {
                            (callbacks.on_fullscreen_scrollbar_change)(scrollbar);
                        }
                    }
                    "theme" => (callbacks.on_theme_change)(new_value),
                    _ => {}
                }
            }),
            Box::new(move || (cancel_callbacks.borrow_mut().on_cancel)()),
            SettingsListOptions {
                enable_search: true,
            },
        )));

        container.add_child(Rc::clone(&settings_list) as ComponentRef);
        container.add_child(component_ref(DynamicBorder::new(None)));

        Self {
            container,
            settings_list,
        }
    }

    /// `getSettingsList()`.
    pub fn settings_list(&self) -> Rc<RefCell<SettingsList>> {
        Rc::clone(&self.settings_list)
    }
}

/// `items.splice(items.findIndex(…) + 1, 0, item)`.
fn insert_after(items: &mut Vec<SettingItem>, id: &str, item: SettingItem) {
    let index = items.iter().position(|entry| entry.id == id);
    match index {
        Some(index) => items.insert(index + 1, item),
        // `findIndex` returning -1 makes `splice(0, 0, …)` prepend.
        None => items.insert(0, item),
    }
}

/// `parseInt(value, 10)` for the numeric rows, all of which carry digits only.
fn parse_int(value: &str) -> u64 {
    value.parse().unwrap_or_default()
}

impl Component for SettingsSelectorComponent {
    fn render(&mut self, width: usize) -> Vec<Line> {
        self.container.render(width)
    }

    fn invalidate(&mut self) {
        self.container.invalidate();
    }

    fn handle_input(&mut self, data: &str) {
        self.settings_list.borrow_mut().handle_input(data);
    }
}
