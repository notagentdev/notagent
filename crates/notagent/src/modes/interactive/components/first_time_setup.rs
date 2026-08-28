use notagent_tui::components::spacer::Spacer;
use notagent_tui::components::text::Text;
use notagent_tui::keybindings::keybindings_match;
use notagent_tui::tui::{Component, Container, Line, component_ref};

use crate::config::APP_NAME;
use crate::modes::interactive::theme::theme::{TerminalTheme, ThemeColor, theme};

use super::dynamic_border::DynamicBorder;
use super::keybinding_hints::{key_hint, raw_key_hint};

/// `FirstTimeSetupResult`
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FirstTimeSetupResult {
    /// The chosen theme.
    pub theme: TerminalTheme,
    /// Whether anonymous usage data may be shared.
    pub share_analytics: bool,
}

/// `FirstTimeSetupOptions`
pub struct FirstTimeSetupOptions {
    /// Theme detected from the terminal.
    pub detected_theme: TerminalTheme,
    /// Invoked while the theme selection moves so the caller can preview it.
    pub on_theme_preview: Box<dyn FnMut(TerminalTheme)>,
    /// Invoked once both steps are confirmed.
    pub on_submit: Box<dyn FnMut(FirstTimeSetupResult)>,
    /// Invoked when the user skips the setup.
    pub on_cancel: Box<dyn FnMut()>,
}

const THEME_OPTIONS: [(TerminalTheme, &str); 2] = [
    (TerminalTheme::Dark, "Dark"),
    (TerminalTheme::Light, "Light"),
];

const ANALYTICS_OPTIONS: [(bool, &str); 2] =
    [(true, "Share anonymous usage data"), (false, "Don't share")];

const SETUP_LOGO_LINES: [&str; 4] = ["██████", "██  ██", "████  ██", "██    ██"];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Theme,
    Analytics,
}

/// First-time setup dialog: theme choice and analytics opt-in.
pub struct FirstTimeSetupComponent {
    container: Container,
    step: Step,
    theme_index: usize,
    analytics_index: usize,
    options: FirstTimeSetupOptions,
}

impl FirstTimeSetupComponent {
    /// New dialog, positioned on the detected theme.
    pub fn new(options: FirstTimeSetupOptions) -> Self {
        // `Math.max(0, findIndex(...))` — an unknown theme falls back to 0.
        let theme_index = THEME_OPTIONS
            .iter()
            .position(|option| option.0 == options.detected_theme)
            .unwrap_or(0);
        let mut component = Self {
            container: Container::new(),
            step: Step::Theme,
            theme_index,
            analytics_index: 0,
            options,
        };
        component.update();
        component
    }

    // Rebuild the whole dialog on every change so theme previews recolor all text.
    fn update(&mut self) {
        let theme_instance = theme();
        self.container.clear();
        self.container
            .add_child(component_ref(DynamicBorder::new(None)));
        self.container.add_child(component_ref(Spacer::new(1)));
        self.container.add_child(component_ref(Text::new(
            theme_instance.fg(ThemeColor::Accent, &SETUP_LOGO_LINES.join("\n")),
            1,
            0,
        )));
        self.container.add_child(component_ref(Spacer::new(1)));
        self.container.add_child(component_ref(Text::new(
            theme_instance.fg(
                ThemeColor::Accent,
                &theme_instance.bold(&format!("Welcome to {APP_NAME}, the minimal coding agent.")),
            ),
            1,
            0,
        )));
        self.container.add_child(component_ref(Spacer::new(1)));

        if self.step == Step::Theme {
            self.container.add_child(component_ref(Text::new(
                theme_instance.fg(ThemeColor::Text, "Pick a theme."),
                1,
                0,
            )));
            self.container.add_child(component_ref(Text::new(
                theme_instance.fg(
                    ThemeColor::Muted,
                    &format!(
                        "Detected system appearance: {}",
                        self.options.detected_theme.as_str()
                    ),
                ),
                1,
                0,
            )));
            self.container.add_child(component_ref(Spacer::new(1)));
            let labels: Vec<&str> = THEME_OPTIONS.iter().map(|option| option.1).collect();
            self.add_option_list(&labels, self.theme_index);
        } else {
            self.container.add_child(component_ref(Text::new(
                theme_instance.fg(ThemeColor::Text, "Opt-in to anonymous usage data sharing?"),
                1,
                0,
            )));
            self.container.add_child(component_ref(Text::new(
                theme_instance.fg(
                    ThemeColor::Muted,
                    "Opting in stores a tracking identifier in settings.json and enables anonymous\nusage analytics. This helps us to better debug, reproduce, and resolve issues\nand bugs within Notagent. You can observe what is shared using /privacy and make\nchanges anytime in settings.json.",
                ),
                1,
                0,
            )));
            self.container.add_child(component_ref(Spacer::new(1)));
            let labels: Vec<&str> = ANALYTICS_OPTIONS.iter().map(|option| option.1).collect();
            self.add_option_list(&labels, self.analytics_index);
        }

        self.container.add_child(component_ref(Spacer::new(1)));
        self.container.add_child(component_ref(Text::new(
            raw_key_hint("↑↓", "navigate")
                + "  "
                + &key_hint(
                    "tui.select.confirm",
                    if self.step == Step::Theme {
                        "continue"
                    } else {
                        "finish"
                    },
                )
                + "  "
                + &key_hint("tui.select.cancel", "skip setup"),
            1,
            0,
        )));
        self.container.add_child(component_ref(Spacer::new(1)));
        self.container
            .add_child(component_ref(DynamicBorder::new(None)));
    }

    fn add_option_list(&mut self, labels: &[&str], selected_index: usize) {
        let theme_instance = theme();
        for (index, label) in labels.iter().enumerate() {
            let is_selected = index == selected_index;
            let prefix = if is_selected {
                theme_instance.fg(ThemeColor::Accent, "→ ")
            } else {
                "  ".to_string()
            };
            let label = if is_selected {
                theme_instance.fg(ThemeColor::Accent, label)
            } else {
                theme_instance.fg(ThemeColor::Text, label)
            };
            self.container
                .add_child(component_ref(Text::new(format!("{prefix}{label}"), 1, 0)));
        }
    }

    fn move_selection(&mut self, delta: isize) {
        if self.step == Step::Theme {
            let next = clamp_index(self.theme_index, delta, THEME_OPTIONS.len());
            if next != self.theme_index {
                self.theme_index = next;
                (self.options.on_theme_preview)(THEME_OPTIONS[self.theme_index].0);
            }
        } else {
            self.analytics_index =
                clamp_index(self.analytics_index, delta, ANALYTICS_OPTIONS.len());
        }
        self.update();
    }
}

/// `Math.max(0, Math.min(len - 1, index + delta))`.
fn clamp_index(index: usize, delta: isize, len: usize) -> usize {
    let moved = index as isize + delta;
    moved.clamp(0, len as isize - 1) as usize
}

impl Component for FirstTimeSetupComponent {
    fn render(&mut self, width: usize) -> Vec<Line> {
        self.container.render(width)
    }

    fn handle_input(&mut self, key_data: &str) {
        if keybindings_match(key_data, "tui.select.up") || key_data == "k" {
            self.move_selection(-1);
        } else if keybindings_match(key_data, "tui.select.down") || key_data == "j" {
            self.move_selection(1);
        } else if keybindings_match(key_data, "tui.select.confirm") || key_data == "\n" {
            if self.step == Step::Theme {
                self.step = Step::Analytics;
                self.update();
            } else {
                (self.options.on_submit)(FirstTimeSetupResult {
                    theme: THEME_OPTIONS[self.theme_index].0,
                    share_analytics: ANALYTICS_OPTIONS[self.analytics_index].0,
                });
            }
        } else if keybindings_match(key_data, "tui.select.cancel") {
            (self.options.on_cancel)();
        }
    }

    fn invalidate(&mut self) {
        self.container.invalidate();
    }
}
