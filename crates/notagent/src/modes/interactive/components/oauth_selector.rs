//! 1:1 port of
//! `packages/coding-agent/src/modes/interactive/components/oauth-selector.ts` (206 LOC).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use notagent_ai::auth::types::{ApiKeyAuth, AuthCheck, AuthType, OAuthAuth};
use notagent_tui::components::input::Input;
use notagent_tui::components::spacer::Spacer;
use notagent_tui::components::truncated_text::TruncatedText;
use notagent_tui::fuzzy::fuzzy_filter;
use notagent_tui::keybindings::keybindings_match;
use notagent_tui::tui::{Component, ComponentRef, Container, Focusable, Line, component_ref};
use regex::Regex;

use crate::modes::interactive::theme::theme::{ThemeColor, theme};

use super::dynamic_border::DynamicBorder;

/// The auth method backing a provider entry; only its name is displayed.
#[derive(Clone)]
pub enum AuthSelectorMethod {
    /// `ApiKeyAuth`
    ApiKey(Arc<dyn ApiKeyAuth>),
    /// `OAuthAuth`
    OAuth(Arc<dyn OAuthAuth>),
}

impl AuthSelectorMethod {
    /// Display name of the method.
    pub fn name(&self) -> &str {
        match self {
            AuthSelectorMethod::ApiKey(auth) => auth.name(),
            AuthSelectorMethod::OAuth(auth) => auth.name(),
        }
    }
}

/// `AuthSelectorProvider`
#[derive(Clone)]
pub struct AuthSelectorProvider {
    /// Provider id, e.g. `anthropic`.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Which auth flow this entry offers.
    pub auth_type: AuthType,
    /// The method object, when the provider declares one.
    pub method: Option<AuthSelectorMethod>,
    /// Result of the auth check, when one ran.
    pub status: Option<AuthCheck>,
}

/// `"subscription"` for OAuth, `"API key"` otherwise.
pub fn format_auth_selector_provider_type(auth_type: AuthType) -> &'static str {
    if auth_type == AuthType::OAuth {
        "subscription"
    } else {
        "API key"
    }
}

/// The literal strings of the TypeScript `"oauth" | "api_key"` union, which the
/// filter text interpolates.
fn auth_type_text(auth_type: AuthType) -> &'static str {
    match auth_type {
        AuthType::OAuth => "oauth",
        AuthType::ApiKey => "api_key",
    }
}

/// Invoked with the provider id and auth type of the confirmed entry.
pub type AuthSelectCallback = Box<dyn FnMut(&str, AuthType)>;

/// Selector mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthSelectorMode {
    /// `/login`
    Login,
    /// `/logout`
    Logout,
}

/// Component that renders an auth provider selector
pub struct OAuthSelectorComponent {
    container: Container,
    search_input: Rc<RefCell<Input>>,
    /// Set by the input's `onSubmit`; the TypeScript callback closes over `this`.
    submitted: Rc<Cell<bool>>,

    // Focusable implementation - propagate to search input for IME cursor positioning
    focused: bool,

    list_container: Rc<RefCell<Container>>,
    all_providers: Vec<AuthSelectorProvider>,
    filtered_providers: Vec<AuthSelectorProvider>,
    selected_index: usize,
    mode: AuthSelectorMode,
    on_select_callback: AuthSelectCallback,
    on_cancel_callback: Box<dyn FnMut()>,
    show_auth_type_labels: bool,
}

impl OAuthSelectorComponent {
    /// New selector over `providers`.
    pub fn new(
        mode: AuthSelectorMode,
        providers: Vec<AuthSelectorProvider>,
        on_select: AuthSelectCallback,
        on_cancel: Box<dyn FnMut()>,
        initial_search_input: Option<&str>,
    ) -> Self {
        let mut container = Container::new();

        // `new Set(providers.map((p) => p.authType)).size > 1`
        let first_auth_type = providers.first().map(|provider| provider.auth_type);
        let show_auth_type_labels = providers
            .iter()
            .any(|provider| Some(provider.auth_type) != first_auth_type);

        // Add top border
        container.add_child(component_ref(DynamicBorder::new(None)));
        container.add_child(component_ref(Spacer::new(1)));

        // Add title
        let title = if mode == AuthSelectorMode::Login {
            "Select provider to configure:"
        } else {
            "Select provider to logout:"
        };
        let theme_instance = theme();
        container.add_child(component_ref(TruncatedText::new(
            theme_instance.fg(ThemeColor::Accent, &theme_instance.bold(title)),
            1,
            0,
        )));
        container.add_child(component_ref(Spacer::new(1)));

        let search_input = Rc::new(RefCell::new(Input::new()));
        if let Some(initial) = initial_search_input
            && !initial.is_empty()
        {
            search_input.borrow_mut().set_value(initial);
        }
        let submitted = Rc::new(Cell::new(false));
        {
            let flag = Rc::clone(&submitted);
            search_input.borrow_mut().on_submit = Some(Box::new(move |_value| flag.set(true)));
        }
        container.add_child(Rc::clone(&search_input) as ComponentRef);
        container.add_child(component_ref(Spacer::new(1)));

        // Create list container
        let list_container = Rc::new(RefCell::new(Container::new()));
        container.add_child(Rc::clone(&list_container) as ComponentRef);

        container.add_child(component_ref(Spacer::new(1)));

        // Add bottom border
        container.add_child(component_ref(DynamicBorder::new(None)));

        let mut selector = Self {
            container,
            search_input,
            submitted,
            focused: false,
            list_container,
            filtered_providers: providers.clone(),
            all_providers: providers,
            selected_index: 0,
            mode,
            on_select_callback: on_select,
            on_cancel_callback: on_cancel,
            show_auth_type_labels,
        };

        // Initial render
        selector.filter_providers(initial_search_input.unwrap_or(""));
        selector
    }

    fn filter_providers(&mut self, query: &str) {
        self.filtered_providers = if query.is_empty() {
            self.all_providers.clone()
        } else {
            fuzzy_filter(&self.all_providers, query, |provider| {
                format!(
                    "{} {} {} {}",
                    provider.name,
                    provider.id,
                    auth_type_text(provider.auth_type),
                    provider
                        .method
                        .as_ref()
                        .map(AuthSelectorMethod::name)
                        .unwrap_or("")
                )
            })
        };
        self.selected_index = self
            .selected_index
            .min(self.filtered_providers.len().saturating_sub(1));
        self.update_list();
    }

    fn update_list(&mut self) {
        let theme_instance = theme();
        let mut list_container = self.list_container.borrow_mut();
        list_container.clear();

        let max_visible = 8usize;
        // `Math.max(0, Math.min(selected - 4, length - maxVisible))` — both
        // operands can be negative in TypeScript, which the clamp to 0 absorbs.
        let start_index = (self.selected_index as isize - (max_visible / 2) as isize)
            .min(self.filtered_providers.len() as isize - max_visible as isize)
            .max(0) as usize;
        let end_index = (start_index + max_visible).min(self.filtered_providers.len());

        for index in start_index..end_index {
            let Some(provider) = self.filtered_providers.get(index) else {
                continue;
            };

            let is_selected = index == self.selected_index;

            let status_indicator = format_status_indicator(provider);
            let auth_type_label = if self.show_auth_type_labels {
                theme_instance.fg(
                    ThemeColor::Muted,
                    &format!(
                        " [{}]",
                        format_auth_selector_provider_type(provider.auth_type)
                    ),
                )
            } else {
                String::new()
            };
            let line = if is_selected {
                let prefix = theme_instance.fg(ThemeColor::Accent, "→ ");
                let text = theme_instance.fg(ThemeColor::Accent, &provider.name);
                prefix + &text + &auth_type_label + &status_indicator
            } else {
                let text = format!("  {}", theme_instance.fg(ThemeColor::Text, &provider.name));
                text + &auth_type_label + &status_indicator
            };

            list_container.add_child(component_ref(TruncatedText::new(line, 1, 0)));
        }

        if start_index > 0 || end_index < self.filtered_providers.len() {
            let scroll_info = theme_instance.fg(
                ThemeColor::Muted,
                &format!(
                    "  ({}/{})",
                    self.selected_index + 1,
                    self.filtered_providers.len()
                ),
            );
            list_container.add_child(component_ref(TruncatedText::new(scroll_info, 1, 0)));
        }

        // Show "no providers" if empty
        if self.filtered_providers.is_empty() {
            let message = if self.all_providers.is_empty() {
                if self.mode == AuthSelectorMode::Login {
                    "No providers available"
                } else {
                    "No providers logged in. Use /login first."
                }
            } else {
                "No matching providers"
            };
            list_container.add_child(component_ref(TruncatedText::new(
                theme_instance.fg(ThemeColor::Muted, &format!("  {message}")),
                1,
                0,
            )));
        }
    }
}

fn format_status_indicator(provider: &AuthSelectorProvider) -> String {
    let theme_instance = theme();
    let Some(status) = provider.status.as_ref() else {
        return theme_instance.fg(ThemeColor::Muted, " • unconfigured");
    };
    if status.check_type != provider.auth_type {
        let label = if status.check_type == AuthType::OAuth {
            "subscription configured"
        } else {
            "API key configured"
        };
        return theme_instance.fg(ThemeColor::Muted, " • ")
            + &theme_instance.fg(ThemeColor::Warning, label);
    }
    let source = status.source.as_deref().unwrap_or("");
    if source.is_empty() || source == "OAuth" || source == "stored credential" {
        return theme_instance.fg(ThemeColor::Success, " ✓ configured");
    }
    let source = if env_var_list_pattern().is_match(source) {
        format!("env: {source}")
    } else {
        source.to_string()
    };
    theme_instance.fg(ThemeColor::Success, &format!(" ✓ {source}"))
}

/// `/^[A-Z][A-Z0-9_]*(?:, [A-Z][A-Z0-9_]*)*$/`
fn env_var_list_pattern() -> &'static Regex {
    static PATTERN: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    PATTERN.get_or_init(|| Regex::new(r"^[A-Z][A-Z0-9_]*(?:, [A-Z][A-Z0-9_]*)*$").unwrap())
}

impl Component for OAuthSelectorComponent {
    fn render(&mut self, width: usize) -> Vec<Line> {
        self.container.render(width)
    }

    fn handle_input(&mut self, key_data: &str) {
        // Up arrow
        if keybindings_match(key_data, "tui.select.up") {
            if self.filtered_providers.is_empty() {
                return;
            }
            self.selected_index = self.selected_index.saturating_sub(1);
            self.update_list();
        }
        // Down arrow
        else if keybindings_match(key_data, "tui.select.down") {
            if self.filtered_providers.is_empty() {
                return;
            }
            self.selected_index = (self.selected_index + 1).min(self.filtered_providers.len() - 1);
            self.update_list();
        }
        // Enter
        else if keybindings_match(key_data, "tui.select.confirm") {
            if let Some(provider) = self.filtered_providers.get(self.selected_index) {
                let (id, auth_type) = (provider.id.clone(), provider.auth_type);
                (self.on_select_callback)(&id, auth_type);
            }
        }
        // Escape or Ctrl+C
        else if keybindings_match(key_data, "tui.select.cancel") {
            (self.on_cancel_callback)();
        }
        // Pass everything else to search input
        else {
            self.search_input.borrow_mut().handle_input(key_data);
            // `searchInput.onSubmit` fires inside `handleInput`, before the
            // filter below runs.
            if self.submitted.replace(false)
                && let Some(provider) = self.filtered_providers.get(self.selected_index)
            {
                let (id, auth_type) = (provider.id.clone(), provider.auth_type);
                (self.on_select_callback)(&id, auth_type);
            }
            let value = self.search_input.borrow().get_value().to_string();
            self.filter_providers(&value);
        }
    }

    fn invalidate(&mut self) {
        self.container.invalidate();
    }

    fn as_focusable(&mut self) -> Option<&mut dyn Focusable> {
        Some(self)
    }
}

impl Focusable for OAuthSelectorComponent {
    fn focused(&self) -> bool {
        self.focused
    }

    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
        self.search_input.borrow_mut().set_focused(focused);
    }
}
